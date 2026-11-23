//! One open budget: sync bookkeeping, the autosave rhythm, and the JSON views the UI reads.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use budget4_core::budget::{account_balances, compute_budget, register_rows, BudgetModel};
use budget4_core::dates::{add_months, month_key, today};
use budget4_core::editor::Editor;
use budget4_core::importing::Statement;
use budget4_core::json::{Obj, ObjExt};
use budget4_core::package::{budget_name_from_folder, BudgetPackage, OpenOptions, SyncReport};
use budget4_core::schema::{builtin, monthly_budget_id};
use budget4_core::store::UndoStep;
use serde::Serialize;
use serde_json::{json, Value};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// YNAB 4 writes the full file at most every 10 minutes (it syncs every 5 s).
const FULL_SAVE_INTERVAL_MS: i64 = 10 * 60_000;

/// How many edits Undo can walk back.
const UNDO_LIMIT: usize = 100;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub last_sync: Option<i64>,
    pub last_full_save: Option<i64>,
    pub last_import: Option<i64>,
    pub unpublished: bool,
    pub unsaved_full_file: bool,
    pub blocked_diffs: usize,
    pub last_error: Option<String>,
    pub conflicts_resolved: usize,
}

fn currency_for(locale: &str) -> &'static str {
    match locale {
        "en_CA" | "fr_CA" => "CAD",
        "en_GB" => "GBP",
        "en_AU" => "AUD",
        "en_NZ" => "NZD",
        "en_IE" | "de_DE" | "fr_FR" | "es_ES" | "it_IT" | "nl_NL" | "de_AT" | "fi_FI" | "pt_PT" => "EUR",
        "de_CH" => "CHF",
        "ja_JP" => "JPY",
        "sv_SE" => "SEK",
        "nb_NO" => "NOK",
        "da_DK" => "DKK",
        "pl_PL" => "PLN",
        "pt_BR" => "BRL",
        "es_MX" => "MXN",
        "en_ZA" => "ZAR",
        "en_IN" => "INR",
        "zh_CN" => "CNY",
        "ko_KR" => "KRW",
        "en_SG" => "SGD",
        "zh_HK" => "HKD",
        "ru_RU" => "RUB",
        "cs_CZ" => "CZK",
        "hu_HU" => "HUF",
        _ => "USD",
    }
}

fn decimals_for(currency: &str) -> u32 {
    match currency {
        "JPY" | "KRW" | "HUF" | "CLP" | "ISK" | "VND" => 0,
        _ => 2,
    }
}

/// A bank file read for import, kept between preview and commit.
pub enum PendingFile {
    Ofx { name: String, statements: Vec<Statement> },
    Csv { name: String, rows: Vec<Vec<String>> },
}

pub struct Session {
    pub pkg: BudgetPackage,
    pub pending_import: Option<PendingFile>,
    pub currency: String,
    pub decimals: u32,
    pub status: SyncStatus,
    pub log: Arc<Mutex<Vec<String>>>,
    undo: Vec<(String, UndoStep)>,
    redo: Vec<(String, UndoStep)>,
    enter_scheduled: bool,
    last_day: String,
    last_full_save_at: i64,
    model: Option<(u64, BudgetModel)>,
}

impl Session {
    pub fn open(folder: &Path, device_guid: &str, friendly_name: &str, enter_scheduled: bool, backup: bool) -> Result<Self, String> {
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let opts = OpenOptions {
            device_guid: Some(device_guid.to_string()),
            friendly_name: friendly_name.to_string(),
            app_version: APP_VERSION.to_string(),
            backup_on_first_save: backup,
            log: Some(Arc::new(move |m: &str| push_log(&sink, m))),
        };
        let pkg = BudgetPackage::open(folder, opts).map_err(|e| e.to_string())?;
        let locale = pkg.store.budget_meta().and_then(|m| m.str_of("currencyLocale")).unwrap_or("en_US").to_string();
        let currency = pkg
            .store
            .budget_meta()
            .and_then(|m| m.str_of("currencyISOSymbol"))
            .map(str::to_string)
            .unwrap_or_else(|| currency_for(&locale).to_string());
        let decimals = decimals_for(&currency);
        let status = SyncStatus {
            blocked_diffs: pkg.open_report.diffs_blocked,
            conflicts_resolved: pkg.open_report.conflicts.len(),
            ..Default::default()
        };
        let mut s = Session {
            pkg,
            pending_import: None,
            currency,
            decimals,
            status,
            log,
            undo: Vec::new(),
            redo: Vec::new(),
            enter_scheduled,
            last_day: today(),
            last_full_save_at: now_ms(),
            model: None,
        };
        s.ensure_data_is_consistent_with_current_date();
        Ok(s)
    }

    pub fn note(&self, msg: &str) {
        push_log(&self.log, msg);
    }

    pub fn editor(&mut self) -> Editor<'_> {
        Editor::new(&mut self.pkg.store, self.decimals)
    }

    /// What YNAB does at load and each new day: months -13..+13 exist, due schedules are entered.
    fn ensure_data_is_consistent_with_current_date(&mut self) {
        let t = today();
        let enter = self.enter_scheduled;
        let mut ed = self.editor();
        let _ = ed.ensure_months(&t);
        if enter {
            match ed.enter_due_scheduled(&t) {
                Ok(v) if !v.is_empty() => self.note(&format!("Entered {} scheduled transaction(s)", v.len())),
                Err(e) => self.note(&format!("Couldn't enter scheduled transactions: {e}")),
                _ => {}
            }
        }
    }

    /// One autosave/sync cycle. Returns true when outside changes arrived.
    pub fn tick(&mut self) -> bool {
        let t = today();
        let mut changed = false;
        if t != self.last_day {
            self.last_day = t;
            self.ensure_data_is_consistent_with_current_date();
            changed = true;
        }
        match self.sync() {
            Ok(r) => changed |= r.diffs_imported > 0,
            Err(e) => {
                self.status.last_error = Some(e.clone());
                self.note(&format!("Sync error: {e}"));
            }
        }
        if self.pkg.has_unsaved_full_file() && now_ms() - self.last_full_save_at > FULL_SAVE_INTERVAL_MS {
            if let Err(e) = self.save_full() {
                self.status.last_error = Some(e);
            }
        }
        changed
    }

    pub fn sync(&mut self) -> Result<SyncReport, String> {
        let r = self.pkg.sync().map_err(|e| e.to_string())?;
        self.status.last_sync = Some(now_ms());
        self.status.blocked_diffs = r.diffs_blocked;
        self.status.last_error = None;
        if r.diffs_imported > 0 {
            self.status.last_import = Some(now_ms());
            self.status.conflicts_resolved += r.conflicts.len();
            self.note(&format!("Imported {} change file(s) from other devices", r.diffs_imported));
        }
        self.refresh_flags();
        Ok(r)
    }

    pub fn save_full(&mut self) -> Result<(), String> {
        self.pkg.save_full(false).map_err(|e| e.to_string())?;
        self.last_full_save_at = now_ms();
        self.status.last_full_save = Some(now_ms());
        self.refresh_flags();
        Ok(())
    }

    fn refresh_flags(&mut self) {
        self.status.unpublished = self.pkg.has_unpublished_changes();
        self.status.unsaved_full_file = self.pkg.has_unsaved_full_file();
    }

    /// After each edit: publish right away so other devices see it within seconds.
    pub fn after_edit(&mut self) {
        if let Err(e) = self.pkg.publish() {
            self.status.last_error = Some(e.to_string());
        }
        self.refresh_flags();
    }

    // ------------------------------------------------------------------ undo

    /// Starts recording a UI edit so it can be undone.
    pub fn begin_edit(&mut self) {
        self.pkg.store.begin_journal();
    }

    /// Finishes recording; an edit that changed something becomes the next Undo.
    pub fn end_edit(&mut self, label: &str) {
        let Some(step) = self.pkg.store.end_journal() else { return };
        if step.is_empty() {
            return;
        }
        self.redo.clear();
        if step.complete {
            self.undo.push((label.to_string(), step));
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
    }

    /// Undoes (or redoes) the latest edit. Returns what it was ("add transaction", …).
    pub fn undo_redo(&mut self, redo: bool) -> Result<String, String> {
        let (word, from) = if redo { ("redo", &mut self.redo) } else { ("undo", &mut self.undo) };
        let (label, step) = from.pop().ok_or(format!("Nothing to {word}"))?;
        self.pkg.store.begin_journal();
        let r = self.pkg.store.restore(&step);
        let back = self.pkg.store.end_journal().unwrap_or_default();
        if let Err(e) = r {
            // That step can't ever apply now; it's gone, so older ones stay reachable.
            return Err(format!("Can't {word} \u{201c}{label}\u{201d}: {e}"));
        }
        if back.complete && !back.is_empty() {
            let to = if redo { &mut self.undo } else { &mut self.redo };
            to.push((label.clone(), back));
        }
        self.note(&format!("{} {label}", if redo { "Redid" } else { "Undid" }));
        Ok(label)
    }

    pub fn close(&mut self) {
        if let Err(e) = self.pkg.save_full(false) {
            self.note(&format!("Save on close failed: {e}"));
        }
    }

    // ------------------------------------------------------------------ views

    fn model(&mut self) -> &BudgetModel {
        let rev = self.pkg.store.revision;
        if self.model.as_ref().is_none_or(|(r, _)| *r != rev) {
            let through = add_months(month_key(&today()), 13);
            self.model = Some((rev, compute_budget(&self.pkg.store, Some(&through), self.decimals)));
        }
        &self.model.as_ref().unwrap().1
    }

    pub fn state(&mut self) -> Value {
        let decimals = self.decimals;
        let tree = self.model().tree.clone();
        let (first, last) = {
            let m = self.model();
            (m.months.first().cloned(), m.months.last().cloned())
        };
        let store = &self.pkg.store;
        let balances = account_balances(store, decimals);
        let mut unapproved: HashMap<&str, usize> = HashMap::new();
        for t in store.collection("transactions") {
            if !t.tomb() && t.get("accepted") == Some(&Value::Bool(false)) {
                *unapproved.entry(t.str_of("accountId").unwrap_or("")).or_default() += 1;
            }
        }
        let mut accounts: Vec<&Obj> = store.collection("accounts").into_iter().filter(|a| !a.tomb()).collect();
        accounts.sort_by(|a, b| a.f("sortableIndex").total_cmp(&b.f("sortableIndex")));
        let others: Vec<Value> = self
            .pkg
            .other_devices()
            .into_iter()
            .map(|d| json!({"id": d.short_device_id, "name": d.friendly_name, "version": d.ynab_version, "knowledge": d.knowledge}))
            .collect();
        json!({
            "budget": {
                "name": budget_name_from_folder(&self.pkg.folder),
                "folder": self.pkg.folder.display().to_string(),
                "device": {"id": self.pkg.device.short_device_id, "name": self.pkg.device.friendly_name},
                "otherDevices": others,
                "currency": self.currency,
                "locale": store.budget_meta().and_then(|m| m.str_of("currencyLocale")).unwrap_or("en_US").replace('_', "-"),
                "decimalDigits": decimals,
            },
            "accounts": accounts.iter().map(|a| json!({
                "id": a.id(),
                "name": a.str_of("accountName").unwrap_or(""),
                "type": a.str_of("accountType").unwrap_or(""),
                "onBudget": a.b("onBudget"),
                "hidden": a.b("hidden"),
                "note": a.get("note").cloned().unwrap_or(Value::Null),
                "lastReconciledDate": a.get("lastReconciledDate").cloned().unwrap_or(Value::Null),
                "lastReconciledBalance": a.f("lastReconciledBalance"),
                "balances": balances.get(a.id()).copied().unwrap_or_default(),
                "unapproved": unapproved.get(a.id()).copied().unwrap_or(0),
            })).collect::<Vec<_>>(),
            "masters": tree.iter().filter_map(|n| {
                let m = store.get(&n.master_id)?;
                Some(json!({
                    "id": n.master_id,
                    "name": m.str_of("name").unwrap_or(""),
                    "type": m.str_of("type").unwrap_or("OUTFLOW"),
                    "builtIn": m.get("deleteable") == Some(&Value::Bool(false)),
                    "note": m.get("note").cloned().unwrap_or(Value::Null),
                    "subs": n.sub_ids.iter().filter_map(|s| store.get(s)).map(|s| json!({
                        "id": s.id(), "name": s.str_of("name").unwrap_or(""), "note": s.get("note").cloned().unwrap_or(Value::Null)
                    })).collect::<Vec<_>>(),
                }))
            }).collect::<Vec<_>>(),
            "payees": store.collection("payees").into_iter().filter(|p| !p.tomb()).map(|p| json!({
                "id": p.id(),
                "name": p.str_of("name").unwrap_or(""),
                "transferAccountId": p.get("targetAccountId").cloned().unwrap_or(Value::Null),
                "autoFillCategoryId": p.get("autoFillCategoryId").cloned().unwrap_or(Value::Null),
                "autoFillMemo": p.get("autoFillMemo").cloned().unwrap_or(Value::Null),
                "autoFillAmount": p.f("autoFillAmount"),
                "enabled": p.get("enabled") != Some(&Value::Bool(false)),
            })).collect::<Vec<_>>(),
            "months": {"first": first, "last": last, "current": month_key(&today())},
            "sync": self.status,
            "undo": {"undo": self.undo.last().map(|u| &u.0), "redo": self.redo.last().map(|u| &u.0)},
        })
    }

    pub fn months(&mut self, from: &str, count: i32) -> Value {
        let notes: Vec<Value> = (0..count)
            .map(|i| {
                let m = add_months(from, i);
                self.pkg.store.get(&monthly_budget_id(&m)).and_then(|o| o.get("note")).cloned().unwrap_or(Value::Null)
            })
            .collect();
        let model = self.model();
        let mut out = Vec::new();
        for i in 0..count {
            let m = add_months(from, i);
            let Some(mm) = model.by_month.get(&m) else { continue };
            let mut v = serde_json::to_value(&mm.summary).unwrap();
            let o = v.as_object_mut().unwrap();
            o.insert("note".into(), notes[i as usize].clone());
            o.insert("categories".into(), serde_json::to_value(&mm.categories).unwrap());
            o.insert(
                "masters".into(),
                Value::Object(mm.masters.iter().map(|(k, x)| (k.clone(), json!({"budgeted": x.budgeted, "spent": x.spent, "balance": x.balance}))).collect()),
            );
            out.push(v);
        }
        Value::Array(out)
    }

    pub fn transactions(&self, account: Option<&str>) -> Value {
        let store = &self.pkg.store;
        Value::Array(register_rows(store, account, self.decimals).into_iter().map(|(t, rb)| txn_json(&self.pkg, t, Some(rb))).collect())
    }

    pub fn scheduled(&self) -> Value {
        let store = &self.pkg.store;
        let mut list: Vec<&Obj> = store.collection("scheduledTransactions").into_iter().filter(|s| !s.tomb()).collect();
        list.sort_by(|a, b| a.str_of("date").cmp(&b.str_of("date")));
        Value::Array(
            list.into_iter()
                .map(|s| {
                    let mut v = txn_json(&self.pkg, s, None);
                    let o = v.as_object_mut().unwrap();
                    o.insert("frequency".into(), json!(s.str_of("frequency").unwrap_or("Monthly")));
                    o.insert("twiceAMonthStartDay".into(), json!(s.f("twiceAMonthStartDay")));
                    v
                })
                .collect(),
        )
    }
}

fn push_log(log: &Arc<Mutex<Vec<String>>>, msg: &str) {
    if let Ok(mut l) = log.lock() {
        l.push(format!("{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
        let n = l.len();
        if n > 500 {
            l.drain(0..n - 500);
        }
    }
}

fn txn_json(pkg: &BudgetPackage, t: &Obj, running: Option<f64>) -> Value {
    let g = |k: &str| t.get(k).cloned().unwrap_or(Value::Null);
    let splits: Vec<Value> = pkg
        .store
        .children(t.id(), "subTransactions")
        .into_iter()
        .filter(|s| !s.tomb())
        .map(|s| {
            json!({
                "id": s.id(), "amount": s.f("amount"),
                "categoryId": s.get("categoryId").cloned().unwrap_or(Value::Null),
                "memo": s.get("memo").cloned().unwrap_or(Value::Null),
                "targetAccountId": s.get("targetAccountId").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    // A pending match: your entered transaction and what the bank sent.
    let kids = pkg.store.children(t.id(), "matchedTransactions");
    let is_bank = |c: &&Obj| c.str_of("YNABID").is_some_and(|y| !y.is_empty()) || c.str_of("FITID").is_some_and(|f| !f.is_empty());
    let matched = match (kids.iter().find(|c| !is_bank(c)), kids.iter().find(|c| is_bank(c))) {
        (Some(u), b) => json!({
            "date": u.get("date").cloned().unwrap_or(Value::Null),
            "payeeId": u.get("payeeId").cloned().unwrap_or(Value::Null),
            "memo": u.get("memo").cloned().unwrap_or(Value::Null),
            "bankDate": b.and_then(|b| b.get("date")).cloned().unwrap_or(Value::Null),
            "bankPayee": b.and_then(|b| b.get("importedPayee")).cloned().unwrap_or(Value::Null),
        }),
        _ => Value::Null,
    };
    json!({
        "id": t.id(),
        "accountId": g("accountId"),
        "date": g("date"),
        "amount": t.f("amount"),
        "payeeId": g("payeeId"),
        "categoryId": g("categoryId"),
        "memo": g("memo"),
        "cleared": t.str_of("cleared").unwrap_or("Uncleared"),
        "accepted": t.get("accepted") != Some(&Value::Bool(false)),
        "flag": g("flag"),
        "checkNumber": g("checkNumber"),
        "targetAccountId": g("targetAccountId"),
        "transferTransactionId": g("transferTransactionId"),
        "importedPayee": g("importedPayee"),
        "source": g("source"),
        "matched": matched,
        "isSplit": t.str_of("categoryId") == Some(builtin::SPLIT),
        "splits": splits,
        "runningBalance": running,
    })
}
