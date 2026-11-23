//! Format and sync engine checked against a real budget's history.

mod common;

use std::fs;
use std::path::Path;

use budget4_core::json::{parse, ObjExt};
use budget4_core::package::{import_diffs, parse_diff_filename, read_json, BudgetPackage, DeviceRecord, DiffFileRef};
use budget4_core::serialize::{serialize_full, serialize_full_value};
use budget4_core::store::Store;
use budget4_core::vv::VersionVector;
use serde_json::Value;

#[test]
fn every_full_file_round_trips_byte_for_byte() {
    let budgets = common::test_budgets();
    require_testdata!(budgets);
    let files: Vec<_> = budgets.iter().flat_map(|b| common::find_files(b, &|n| n == "Budget.yfull")).collect();
    assert!(!files.is_empty());
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        let value = parse(&text).unwrap();
        // Through plain JSON and through the flattened store.
        assert_same(&serialize_full_value(&value), &text, &f);
        assert_same(&serialize_full(&Store::from_root(value, "Z")), &text, &f);
    }
}

fn assert_same(ours: &str, theirs: &str, f: &Path) {
    if ours != theirs {
        let i = ours.bytes().zip(theirs.bytes()).take_while(|(a, b)| a == b).count();
        let lo = i.saturating_sub(120);
        panic!(
            "{} differs at byte {i}\nours:   {:?}\ntheirs: {:?}",
            f.display(),
            &ours[lo..(i + 80).min(ours.len())],
            &theirs[lo..(i + 80).min(theirs.len())]
        );
    }
}

/// An entity as Budget.yfull would hold it, with keys sorted for comparison.
fn normalized(store: &Store, id: &str, drop_version: bool) -> String {
    let v = store.materialize(id);
    let text = serialize_full_value(&Value::Array(vec![v]));
    let mut v = parse(&text).unwrap();
    if drop_version {
        if let Some(o) = v[0].as_object_mut() {
            o.remove("entityVersion");
            o.remove("isResolvedConflict");
        }
    }
    sort_keys(&v[0]).to_string()
}

fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let mut m = serde_json::Map::new();
            for k in keys {
                m.insert(k.clone(), sort_keys(&o[k]));
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.iter().map(sort_keys).collect()),
        x => x.clone(),
    }
}

/// For each pair of device snapshots (older, newer) whose diff chain is still on disk, the older
/// snapshot plus the diffs must reproduce the newer one entity for entity.
#[test]
fn older_snapshot_plus_diffs_reproduces_newer_snapshot() {
    let budgets = common::test_budgets();
    require_testdata!(budgets);
    let mut pairs_checked = 0;
    for budget in &budgets {
        let data_folders: Vec<_> = fs::read_dir(budget)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("devices").is_dir() && p.file_name().unwrap().to_string_lossy().starts_with("data"))
            .collect();
        for df in data_folders {
            let devices: Vec<DeviceRecord> =
                BudgetPackage::read_devices(&df).unwrap().into_iter().filter(|d| df.join(&d.device_guid).join("Budget.yfull").is_file()).collect();
            let mut diffs: Vec<DiffFileRef> = Vec::new();
            for d in &devices {
                for e in fs::read_dir(df.join(&d.device_guid)).unwrap().flatten() {
                    if let Some((start, end)) = parse_diff_filename(&e.file_name().to_string_lossy()) {
                        diffs.push(DiffFileRef { path: e.path(), device: d.short_device_id.clone(), start, end });
                    }
                }
            }
            diffs.sort_by(|a, b| VersionVector::compare_for_sort(&a.end, &b.end));
            let chain_complete = |from: &VersionVector, to: &VersionVector| {
                let mut k = from.clone();
                let mut progressed = true;
                while progressed {
                    progressed = false;
                    for f in &diffs {
                        if to.dominates(&f.end) && !k.dominates(&f.end) && k.dominates(&f.start) {
                            k.incorporate(&f.end);
                            progressed = true;
                        }
                    }
                }
                k.dominates(to)
            };
            for older in &devices {
                for newer in &devices {
                    let ko = VersionVector::parse(older.knowledge_in_full_budget_file.as_deref());
                    let kn = VersionVector::parse(newer.knowledge_in_full_budget_file.as_deref());
                    if older.device_guid == newer.device_guid || !kn.strictly_dominates(&ko) || !chain_complete(&ko, &kn) {
                        continue;
                    }
                    let label = format!("{}: {} -> {}", df.display(), older.short_device_id, newer.short_device_id);
                    let mut store = Store::from_root(read_json(&df.join(&older.device_guid).join("Budget.yfull")).unwrap(), "Z");
                    let new_root = read_json(&df.join(&newer.device_guid).join("Budget.yfull")).unwrap();
                    let target = VersionVector::parse(new_root["fileMetaData"]["currentKnowledge"].as_str());
                    let refs: Vec<DiffFileRef> = diffs.iter().filter(|f| target.dominates(&f.end)).cloned().collect();
                    let report = import_diffs(&mut store, &refs, None).unwrap();
                    assert_eq!(report.diffs_blocked, 0, "{label}");
                    // Replaying several devices' history can surface conflicts they already resolved
                    // between themselves; the replica ("Z") resolves them too with its own stamps.
                    let z_stamped = store.knowledge.get("Z") > 0;
                    store.knowledge.set("Z", 0);
                    assert!(store.knowledge.equals(&target), "{label}: knowledge {} vs {}", store.knowledge, target);

                    let expected = Store::from_root(new_root, "Z");
                    let mut mismatches = Vec::new();
                    for e in expected.all() {
                        let id = e.id();
                        match store.get(id) {
                            None => mismatches.push(format!("missing {} {id}", e.etype())),
                            Some(ours) => {
                                let drop = z_stamped && ours.str_of("entityVersion").is_some_and(|v| v.starts_with("Z-"));
                                if normalized(&store, id, drop) != normalized(&expected, id, drop) {
                                    mismatches.push(format!("differs {} {id}", e.etype()));
                                }
                            }
                        }
                    }
                    for e in store.all() {
                        if expected.get(e.id()).is_none() {
                            mismatches.push(format!("extra {} {}", e.etype(), e.id()));
                        }
                    }
                    mismatches.truncate(10);
                    assert!(mismatches.is_empty(), "{label}: {mismatches:?}");
                    assert_eq!(store.len(), expected.len(), "{label}");
                    eprintln!("ok {label} ({} diffs, {} conflicts)", report.diffs_imported, report.conflicts.len());
                    pairs_checked += 1;
                }
            }
        }
    }
    assert!(pairs_checked >= 1);
}
