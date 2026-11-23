//! One open YNAB 4 budget package (PackageManagerDropbox + DropboxSyncAdapter + VOToSyncDiffBridge).
//!
//! Joins the budget as its own sync device: reads the most complete Budget.yfull, imports every diff
//! it hasn't seen, publishes its own changes as .ydiff files and keeps its own Budget.yfull.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::json::{parse, Obj, ObjExt};
use crate::schema::CURRENT_DATA_VERSION;
use crate::serialize::{as3_date_string, serialize_diff, serialize_full, serialize_tabbed, DiffFile};
use crate::store::{ConflictRecord, Store};
use crate::vv::VersionVector;
use crate::{Error, Result};

pub const DEVICE_FORMAT_VERSION: &str = "1.2";
const DIFF_EXT: &str = ".ydiff";

/// A .ydevice file (com.ynab.entityFramework.sync.SyncDevice), in YNAB's key order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecord {
    #[serde(rename = "formatVersion", default)]
    pub format_version: Option<String>,
    #[serde(rename = "deviceGUID")]
    pub device_guid: String,
    #[serde(rename = "shortDeviceId")]
    pub short_device_id: String,
    #[serde(rename = "YNABVersion", default)]
    pub ynab_version: Option<String>,
    #[serde(rename = "deviceType", default)]
    pub device_type: Option<String>,
    #[serde(rename = "friendlyName", default)]
    pub friendly_name: Option<String>,
    #[serde(rename = "hasFullKnowledge", default)]
    pub has_full_knowledge: bool,
    #[serde(default)]
    pub knowledge: Option<String>,
    #[serde(rename = "knowledgeInFullBudgetFile", default)]
    pub knowledge_in_full_budget_file: Option<String>,
    #[serde(rename = "lastDataVersionFullyKnown", default)]
    pub last_data_version_fully_known: Option<String>,
    #[serde(rename = "highestDataVersionImported", default)]
    pub highest_data_version_imported: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiffFileRef {
    pub path: PathBuf,
    pub device: String,
    pub start: VersionVector,
    pub end: VersionVector,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    pub diffs_imported: usize,
    pub diffs_blocked: usize,
    pub created: usize,
    pub updated: usize,
    pub conflicts: Vec<ConflictRecord>,
    pub highest_data_version: Option<String>,
    pub published: Option<String>,
}

pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct OpenOptions {
    /// This machine's device GUID; a new device is registered if absent or unknown here.
    pub device_guid: Option<String>,
    pub friendly_name: String,
    pub app_version: String,
    /// Write a .y4backup before the first save of the session.
    pub backup_on_first_save: bool,
    pub log: Option<Logger>,
}

impl OpenOptions {
    pub fn new(friendly_name: &str, app_version: &str) -> Self {
        OpenOptions {
            device_guid: None,
            friendly_name: friendly_name.into(),
            app_version: app_version.into(),
            backup_on_first_save: true,
            log: None,
        }
    }
}

pub fn read_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    parse(&text).map_err(|e| Error::Msg(format!("{}: {e}", path.display())))
}

/// Version comparison "4.2" >= "4.1".
pub fn version_gte(a: Option<&str>, b: Option<&str>) -> bool {
    let pa: Vec<i64> = a.unwrap_or("0").split('.').map(|x| x.parse().unwrap_or(0)).collect();
    let pb: Vec<i64> = b.unwrap_or("0").split('.').map(|x| x.parse().unwrap_or(0)).collect();
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    true
}

/// "My Budget~1A2B3C4D.ynab4" -> "My Budget"
pub fn budget_name_from_folder(folder: &Path) -> String {
    let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let name = name.strip_suffix(".ynab4").or_else(|| name.strip_suffix(".YNAB4")).unwrap_or(&name).to_string();
    match name.rfind('~') {
        Some(i) if name.len() - i == 9 && name[i + 1..].chars().all(|c| c.is_ascii_hexdigit()) => name[..i].to_string(),
        _ => name,
    }
}

/// Parses "<startVV>_<endElement>.ydiff"; the end version is start ∪ end element.
pub fn parse_diff_filename(name: &str) -> Option<(VersionVector, VersionVector)> {
    let stem = name.strip_suffix(DIFF_EXT)?;
    let mut parts = stem.split('_');
    let (a, b) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let start = VersionVector::parse(Some(a));
    let mut end = VersionVector::parse(Some(b));
    if start.devices().count() == 0 || end.devices().count() == 0 {
        return None;
    }
    end.incorporate(&start);
    Some((start, end))
}

pub fn diff_filename(start: &VersionVector, end: &VersionVector, device: &str) -> String {
    format!("{start}_{device}-{}{DIFF_EXT}", end.get(device))
}

/// Writes via a temp file + rename so readers never see half a file.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("{}.budget4tmp", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    fs::write(&tmp, data).map_err(|e| Error::io(&tmp, e))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(Error::io(path, e));
    }
    Ok(())
}

pub fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

/// Imports diffs in chain order: skip if knowledge ⊇ end, import if knowledge ⊇ start, retry otherwise.
pub fn import_diffs(store: &mut Store, refs: &[DiffFileRef], processed: Option<&mut HashSet<PathBuf>>) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let mut done: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<&DiffFileRef> = refs.iter().collect();
    let mut progressed = true;
    while progressed && !pending.is_empty() {
        progressed = false;
        let mut next = Vec::new();
        for f in pending {
            if store.knowledge.dominates(&f.end) {
                done.push(f.path.clone());
                continue;
            }
            if !store.knowledge.dominates(&f.start) {
                next.push(f);
                continue;
            }
            let diff = read_json(&f.path)?;
            let data_version = diff.get("dataVersion").and_then(Value::as_str);
            if data_version.is_some() && !version_gte(Some(CURRENT_DATA_VERSION), data_version) {
                return Err(Error::Msg(format!("The version of the data in {} is too high", f.path.display())));
            }
            let end = match diff.get("endVersion").and_then(Value::as_str) {
                Some(e) if !e.is_empty() => VersionVector::parse(Some(e)),
                _ => f.end.clone(),
            };
            let items: Vec<Obj> = match diff.get("items") {
                Some(Value::Array(a)) => a.iter().filter_map(|i| i.as_object().cloned()).collect(),
                _ => Vec::new(),
            };
            let r = store.apply_diff_items(items, &end).map_err(Error::Msg)?;
            report.diffs_imported += 1;
            report.created += r.created;
            report.updated += r.updated;
            report.conflicts.extend(r.conflicts);
            if let Some(dv) = data_version {
                if report.highest_data_version.as_deref().is_none_or(|h| !version_gte(Some(h), Some(dv))) {
                    report.highest_data_version = Some(dv.to_string());
                }
            }
            done.push(f.path.clone());
            progressed = true;
        }
        pending = next;
    }
    report.diffs_blocked = pending.len();
    if let Some(p) = processed {
        p.extend(done);
    }
    Ok(report)
}

pub struct BudgetPackage {
    pub folder: PathBuf,
    pub data_folder: PathBuf,
    pub store: Store,
    pub device: DeviceRecord,
    pub loaded_from: PathBuf,
    pub open_report: SyncReport,
    last_published: i64,
    last_full_save_counter: i64,
    full_file_dirty: bool,
    processed: HashSet<PathBuf>,
    backup_written: bool,
    opts: OpenOptions,
}

impl BudgetPackage {
    fn log(&self, msg: &str) {
        if let Some(l) = &self.opts.log {
            l(msg);
        }
    }

    pub fn read_meta(folder: &Path) -> Result<Obj> {
        match read_json(&folder.join("Budget.ymeta"))? {
            Value::Object(o) => Ok(o),
            _ => Err(Error::Msg("Budget.ymeta is not an object".into())),
        }
    }

    pub fn open(folder: &Path, opts: OpenOptions) -> Result<Self> {
        let log = |m: &str| {
            if let Some(l) = &opts.log {
                l(m)
            }
        };
        let meta = Self::read_meta(folder)?;
        if !version_gte(Some("2"), meta.str_of("formatVersion")) {
            return Err(Error::Msg("Budget.ymeta is from a newer version of YNAB".into()));
        }
        let rel = meta.str_of("relativeDataFolderName").ok_or_else(|| Error::Msg("relativeDataFolderName missing".into()))?;
        let data_folder = folder.join(rel);
        if !data_folder.is_dir() {
            return Err(Error::Msg(format!("The data folder does not exist: {}", data_folder.display())));
        }

        let devices = Self::read_devices(&data_folder)?;
        let device = match opts.device_guid.as_deref().and_then(|g| devices.iter().find(|d| d.device_guid == g)) {
            Some(d) => {
                log(&format!("Found existing device {}", d.short_device_id));
                d.clone()
            }
            None => {
                let d = Self::register_device(&data_folder, &devices, &opts)?;
                log(&format!("Registered as new device {} ({})", d.short_device_id, d.device_guid));
                d
            }
        };

        let mut candidates: Vec<(VersionVector, PathBuf)> = devices
            .iter()
            .filter(|d| d.has_full_knowledge && d.knowledge_in_full_budget_file.is_some())
            .filter(|d| {
                d.highest_data_version_imported.is_none()
                    || version_gte(d.last_data_version_fully_known.as_deref(), d.highest_data_version_imported.as_deref())
            })
            .map(|d| (VersionVector::parse(d.knowledge_in_full_budget_file.as_deref()), data_folder.join(&d.device_guid).join("Budget.yfull")))
            .filter(|(_, f)| f.is_file())
            .collect();
        candidates.sort_by(|a, b| VersionVector::compare_for_sort(&b.0, &a.0));
        if candidates.is_empty() {
            return Err(Error::Msg("Could not open this budget. No suitable device to open was found.".into()));
        }

        let mut errors = Vec::new();
        for (_, file) in candidates {
            let loaded = (|| -> Result<Store> {
                let root = read_json(&file)?;
                let dv = root.get("fileMetaData").and_then(|m| m.get("budgetDataVersion")).and_then(Value::as_str);
                if dv.is_some() && !version_gte(Some(CURRENT_DATA_VERSION), dv) {
                    return Err(Error::Msg(format!("Data version {} is too new", dv.unwrap_or(""))));
                }
                Ok(Store::from_root(root, &device.short_device_id))
            })();
            match loaded {
                Ok(store) => {
                    log(&format!("Loaded {} (knowledge {})", file.display(), store.knowledge));
                    let counter = store.knowledge.get(&device.short_device_id);
                    let mut pkg = BudgetPackage {
                        folder: folder.to_path_buf(),
                        data_folder: data_folder.clone(),
                        store,
                        device: device.clone(),
                        loaded_from: file.clone(),
                        open_report: SyncReport::default(),
                        last_published: counter,
                        last_full_save_counter: counter,
                        full_file_dirty: false,
                        processed: HashSet::new(),
                        backup_written: false,
                        opts: opts.clone(),
                    };
                    pkg.open_report = pkg.check_for_updates()?;
                    pkg.device.knowledge = Some(pkg.store.knowledge.to_string());
                    pkg.device.last_data_version_fully_known = Some(CURRENT_DATA_VERSION.into());
                    pkg.write_device()?;
                    // A device with no full file of its own writes one straight away.
                    if !pkg.full_file_path().is_file() {
                        pkg.save_full(true)?;
                    }
                    return Ok(pkg);
                }
                Err(e) => {
                    log(&format!("Couldn't read {}: {e}", file.display()));
                    errors.push(format!("{}: {e}", file.display()));
                }
            }
        }
        Err(Error::Msg(format!("Could not load after {} tries:\n{}", errors.len(), errors.join("\n"))))
    }

    // ------------------------------------------------------------------ devices

    pub fn read_devices(data_folder: &Path) -> Result<Vec<DeviceRecord>> {
        let dir = data_folder.join("devices");
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&dir) else { return Ok(out) };
        let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for p in names {
            if p.extension().and_then(|e| e.to_str()) != Some("ydevice") {
                continue;
            }
            let Ok(v) = read_json(&p) else { continue };
            let Ok(d) = serde_json::from_value::<DeviceRecord>(v) else { continue };
            let Some(fv) = d.format_version.as_deref() else { continue };
            if !version_gte(Some(DEVICE_FORMAT_VERSION), Some(fv)) {
                return Err(Error::Msg(format!("Device {} has format version {fv}, newer than supported", p.display())));
            }
            if !d.device_guid.is_empty() && !d.short_device_id.is_empty() {
                out.push(d);
            }
        }
        Ok(out)
    }

    fn register_device(data_folder: &Path, devices: &[DeviceRecord], opts: &OpenOptions) -> Result<DeviceRecord> {
        let used: HashSet<&str> = devices.iter().map(|d| d.short_device_id.as_str()).collect();
        let id = (b'A'..=b'Z')
            .map(|c| (c as char).to_string())
            .find(|c| !used.contains(c.as_str()))
            .ok_or_else(|| Error::Msg("Could not find an available short device ID".into()))?;
        let guid = opts.device_guid.clone().unwrap_or_else(new_guid);
        let device = DeviceRecord {
            format_version: Some(DEVICE_FORMAT_VERSION.into()),
            device_guid: guid.clone(),
            short_device_id: id.clone(),
            ynab_version: Some(format!("Budget4 v{}", opts.app_version)),
            device_type: Some(format!("Desktop (Budget4), OS:{}", std::env::consts::OS)),
            friendly_name: Some(opts.friendly_name.clone()),
            has_full_knowledge: false,
            knowledge: None,
            knowledge_in_full_budget_file: None,
            last_data_version_fully_known: None,
            highest_data_version_imported: None,
        };
        fs::create_dir_all(data_folder.join("devices")).map_err(|e| Error::io(data_folder, e))?;
        let folder = data_folder.join(&guid);
        if folder.is_dir() && fs::read_dir(&folder).map(|mut d| d.next().is_some()).unwrap_or(false) {
            let aside = data_folder.join(format!("{guid}-old-{}", chrono::Local::now().timestamp()));
            fs::rename(&folder, &aside).map_err(|e| Error::io(&folder, e))?;
        }
        fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
        let v = serde_json::to_value(&device).map_err(|e| Error::Msg(e.to_string()))?;
        write_atomic(&data_folder.join("devices").join(format!("{id}.ydevice")), serialize_tabbed(&v).as_bytes())?;
        Ok(device)
    }

    fn device_file(&self) -> PathBuf {
        self.data_folder.join("devices").join(format!("{}.ydevice", self.device.short_device_id))
    }

    fn write_device(&mut self) -> Result<()> {
        let path = self.device_file();
        if path.is_file() {
            if let Ok(on_disk) = read_json(&path).and_then(|v| serde_json::from_value::<DeviceRecord>(v).map_err(|e| Error::Msg(e.to_string()))) {
                if on_disk.device_guid != self.device.device_guid {
                    return Err(Error::Msg(format!(
                        "Device file {} now belongs to another device ({}); refusing to overwrite",
                        path.display(),
                        on_disk.device_guid
                    )));
                }
                if let (Some(before), Some(now)) = (on_disk.knowledge.as_deref(), self.device.knowledge.as_deref()) {
                    if !VersionVector::parse(Some(now)).dominates(&VersionVector::parse(Some(before))) {
                        return Err(Error::Msg(format!("Refusing to write device knowledge that regresses ({before} -> {now})")));
                    }
                }
            }
        }
        self.device.format_version = Some(DEVICE_FORMAT_VERSION.into());
        self.device.ynab_version = Some(format!("Budget4 v{}", self.opts.app_version));
        self.device.has_full_knowledge = self.device.knowledge_in_full_budget_file.is_some();
        let v = serde_json::to_value(&self.device).map_err(|e| Error::Msg(e.to_string()))?;
        write_atomic(&path, serialize_tabbed(&v).as_bytes())
    }

    pub fn my_device_folder(&self) -> PathBuf {
        self.data_folder.join(&self.device.device_guid)
    }

    pub fn full_file_path(&self) -> PathBuf {
        self.my_device_folder().join("Budget.yfull")
    }

    pub fn other_devices(&self) -> Vec<DeviceRecord> {
        Self::read_devices(&self.data_folder)
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.device_guid != self.device.device_guid)
            .collect()
    }

    // ------------------------------------------------------------------ incoming

    /// Every .ydiff in the folders of registered devices.
    pub fn list_diff_files(&self) -> Vec<DiffFileRef> {
        let mut out = Vec::new();
        for d in Self::read_devices(&self.data_folder).unwrap_or_default() {
            let Ok(entries) = fs::read_dir(self.data_folder.join(&d.device_guid)) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some((start, end)) = parse_diff_filename(&name) {
                    out.push(DiffFileRef { path: e.path(), device: d.short_device_id.clone(), start, end });
                }
            }
        }
        out.sort_by(|a, b| VersionVector::compare_for_sort(&a.end, &b.end));
        out
    }

    pub fn check_for_updates(&mut self) -> Result<SyncReport> {
        let pending: Vec<DiffFileRef> = self.list_diff_files().into_iter().filter(|f| !self.processed.contains(&f.path)).collect();
        let mut processed = std::mem::take(&mut self.processed);
        let report = import_diffs(&mut self.store, &pending, Some(&mut processed));
        self.processed = processed;
        let report = report?;
        if report.diffs_imported > 0 {
            if let Some(dv) = &report.highest_data_version {
                if !version_gte(self.device.highest_data_version_imported.as_deref(), Some(dv)) || self.device.highest_data_version_imported.is_none() {
                    self.device.highest_data_version_imported = Some(dv.clone());
                }
            }
            self.device.knowledge = Some(self.store.knowledge.to_string());
            self.full_file_dirty = true;
            self.log(&format!("Imported {} diffs; knowledge now {}", report.diffs_imported, self.store.knowledge));
        }
        if report.diffs_blocked > 0 {
            self.log(&format!("{} diffs can't be imported yet (broken diff chain?)", report.diffs_blocked));
        }
        Ok(report)
    }

    // ------------------------------------------------------------------ outgoing

    pub fn has_unpublished_changes(&self) -> bool {
        self.store.knowledge.get(&self.device.short_device_id) != self.last_published
    }

    pub fn has_unsaved_full_file(&self) -> bool {
        self.store.knowledge.get(&self.device.short_device_id) != self.last_full_save_counter || self.full_file_dirty
    }

    /// Writes this device's changes since the last publish as a .ydiff.
    pub fn publish(&mut self) -> Result<Option<PathBuf>> {
        let me = self.device.short_device_id.clone();
        let current = self.store.knowledge.get(&me);
        if current == self.last_published {
            return Ok(None);
        }
        let ids = self.store.changes_since(self.last_published);
        let mut start = self.store.knowledge.clone();
        start.set(&me, self.last_published);
        let end = self.store.knowledge.clone();
        let diff = DiffFile {
            data_version: CURRENT_DATA_VERSION.into(),
            device_guid: self.device.device_guid.clone(),
            short_device_id: me.clone(),
            start_version: start.to_string(),
            end_version: end.to_string(),
            publish_time: as3_date_string(chrono::Local::now()),
            items: ids.iter().map(|i| self.store.materialize(i)).collect(),
        };
        let folder = self.my_device_folder();
        fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
        let path = folder.join(diff_filename(&start, &end, &me));
        write_atomic(&path, serialize_diff(&diff).as_bytes())?;
        self.processed.insert(path.clone());
        self.last_published = current;
        self.device.knowledge = Some(end.to_string());
        self.write_device()?;
        self.log(&format!("Published {} items -> {}", ids.len(), path.file_name().unwrap_or_default().to_string_lossy()));
        Ok(Some(path))
    }

    /// Publishes, then writes Budget.yfull and updates the device record.
    pub fn save_full(&mut self, force: bool) -> Result<()> {
        self.publish()?;
        if !force && !self.has_unsaved_full_file() {
            return Ok(());
        }
        if self.opts.backup_on_first_save && !self.backup_written {
            self.write_backup()?;
        }
        self.store.sync_file_meta();
        let text = serialize_full(&self.store);
        let folder = self.my_device_folder();
        fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
        write_atomic(&self.full_file_path(), text.as_bytes())?;
        let k = self.store.knowledge.to_string();
        self.device.knowledge = Some(k.clone());
        self.device.knowledge_in_full_budget_file = Some(k.clone());
        self.device.last_data_version_fully_known = Some(CURRENT_DATA_VERSION.into());
        self.write_device()?;
        self.last_full_save_counter = self.store.knowledge.get(&self.device.short_device_id);
        self.full_file_dirty = false;
        self.log(&format!("Saved full file ({:.1} MB) at {k}", text.len() as f64 / 1e6));
        Ok(())
    }

    /// check + publish (SyncManager.sync).
    pub fn sync(&mut self) -> Result<SyncReport> {
        let mut report = self.check_for_updates()?;
        report.published = self.publish()?.map(|p| p.display().to_string());
        Ok(report)
    }

    /// Writes Backup_<timestamp>_<shortId>_<deviceGUID>.y4backup (YNAB's format: a zip holding
    /// "<knowledge>.ynab4"), so YNAB 4 can restore it too.
    pub fn write_backup(&mut self) -> Result<PathBuf> {
        let text = if self.loaded_from.is_file() {
            fs::read_to_string(&self.loaded_from).map_err(|e| Error::io(&self.loaded_from, e))?
        } else {
            serialize_full(&self.store)
        };
        let knowledge = parse(&text)
            .ok()
            .and_then(|v| v.get("fileMetaData")?.get("currentKnowledge")?.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            z.start_file(format!("{knowledge}.ynab4"), options).map_err(|e| Error::Msg(e.to_string()))?;
            z.write_all(text.as_bytes()).map_err(|e| Error::Msg(e.to_string()))?;
            z.finish().map_err(|e| Error::Msg(e.to_string()))?;
        }
        let stamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S");
        let path = self.folder.join(format!("Backup_{stamp}_{}_{}.y4backup", self.device.short_device_id, self.device.device_guid));
        write_atomic(&path, buf.get_ref())?;
        self.backup_written = true;
        self.log(&format!("Wrote backup {}", path.file_name().unwrap_or_default().to_string_lossy()));
        Ok(path)
    }
}
