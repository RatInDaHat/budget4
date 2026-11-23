//! Budget4 desktop app: the Rust core behind a Tauri window.
//!
//! The UI talks to one command, `api(method, path, body)`, which answers the same routes the
//! earlier HTTP server did. A background thread runs YNAB 4's 5-second sync cycle and tells the UI
//! about outside changes through the "budget-event" event.

mod config;
mod discover;
mod import;
mod lock;
mod session;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use budget4_core::create::{create_budget_with, Starter};
use budget4_core::editor::{AccountInput, AccountUpdate, TransactionInput};
use budget4_core::package::OpenOptions;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use config::AppConfig;
use session::{Session, APP_VERSION};

pub struct AppState {
    config: AppConfig,
    session: Option<Session>,
    /// Set at startup when a PIN is configured; nothing but the lock routes answers until it's cleared.
    locked: bool,
    throttle: lock::Throttle,
    /// A budget folder given on the command line, opened once the PIN is entered.
    pending_open: Option<String>,
}

const LOCKED: &str = "Budget4 is locked";

/// PIN routes, answered even while locked.
fn route_lock(st: &mut AppState, method: &str, seg: &[&str], body: &Value) -> Option<Result<Value, String>> {
    let pin = || str_field(body, "pin").map(str::to_string);
    let current_ok = |st: &mut AppState| -> Result<(), String> {
        let Some(hash) = st.config.pin_hash.clone() else { return Ok(()) };
        st.throttle.check()?;
        if lock::verify_pin(body.get("current").and_then(Value::as_str).unwrap_or(""), &hash) {
            st.throttle.succeeded();
            Ok(())
        } else {
            st.throttle.failed();
            Err("The current PIN isn't right".into())
        }
    };
    Some(match (method, seg) {
        ("GET", ["lock"]) => Ok(json!({"enabled": st.config.pin_hash.is_some(), "locked": st.locked})),
        ("POST", ["lock", "unlock"]) => (|| {
            let Some(hash) = st.config.pin_hash.clone() else {
                st.locked = false;
                return Ok(ok());
            };
            st.throttle.check()?;
            if !lock::verify_pin(&pin()?, &hash) {
                st.throttle.failed();
                st.throttle.check()?;
                return Err("Wrong PIN".into());
            }
            st.throttle.succeeded();
            st.locked = false;
            if let Some(folder) = st.pending_open.take() {
                if let Err(e) = open_session(st, &folder) {
                    eprintln!("Couldn't open {folder}: {e}");
                }
            }
            Ok(ok())
        })(),
        ("POST", ["lock", "set"]) if !st.locked => (|| {
            let new = lock::hash_pin(&pin()?)?;
            current_ok(st)?;
            st.config.pin_hash = Some(new);
            st.config.save();
            Ok(ok())
        })(),
        ("POST", ["lock", "remove"]) if !st.locked => (|| {
            current_ok(st)?;
            st.config.pin_hash = None;
            st.config.save();
            Ok(ok())
        })(),
        _ => return None,
    })
}

type Shared = Arc<Mutex<AppState>>;

fn emit(app: &tauri::AppHandle, event: &str) {
    let _ = app.emit("budget-event", event);
}

fn arg<T: for<'de> Deserialize<'de>>(body: &Value) -> Result<T, String> {
    serde_json::from_value(body.clone()).map_err(|e| format!("Bad request: {e}"))
}

fn str_field<'a>(body: &'a Value, key: &str) -> Result<&'a str, String> {
    body.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing {key}"))
}

fn ids(body: &Value) -> Result<Vec<String>, String> {
    arg::<Vec<String>>(body.get("ids").unwrap_or(&Value::Null))
}

fn ok() -> Value {
    json!({"ok": true})
}

fn open_session(st: &mut AppState, folder: &str) -> Result<Value, String> {
    if let Some(mut s) = st.session.take() {
        s.close();
    }
    let path = PathBuf::from(folder);
    if !path.join("Budget.ymeta").is_file() {
        return Err("That folder is not a YNAB 4 budget (no Budget.ymeta)".into());
    }
    let c = &st.config;
    let mut s = Session::open(&path, &c.device_guid, &c.friendly_name, c.enter_scheduled_transactions, c.backup_on_first_save)?;
    let state = s.state();
    st.session = Some(s);
    st.config.remember(folder);
    Ok(state)
}

/// Routes one UI request. Returns (response, edited) — edits get published immediately.
fn route(st: &mut AppState, method: &str, path: &str, body: &Value) -> Result<(Value, bool), String> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let q = |k: &str| query.split('&').filter_map(|p| p.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| decode(v));
    let seg: Vec<String> = path.trim_start_matches("/api/").split('/').map(decode).collect();
    let seg: Vec<&str> = seg.iter().map(String::as_str).collect();

    if let Some(r) = route_lock(st, method, &seg, body) {
        return r.map(|v| (v, false));
    }
    if st.locked {
        return Err(LOCKED.into());
    }

    // Routes that don't need an open budget.
    match (method, seg.as_slice()) {
        ("GET", ["app"]) => {
            let c = &st.config;
            return Ok((
                json!({
                    "version": APP_VERSION,
                    "open": st.session.as_ref().map(|s| s.pkg.folder.display().to_string()),
                    "recent": c.recent_budgets.iter().filter(|f| PathBuf::from(f).is_dir()).collect::<Vec<_>>(),
                    "budgetFolders": discover::budget_folders().iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                    "friendlyName": c.friendly_name,
                    "enterScheduledTransactions": c.enter_scheduled_transactions,
                    "backupOnFirstSave": c.backup_on_first_save,
                }),
                false,
            ))
        }
        ("GET", ["budgets"]) => return Ok((serde_json::to_value(discover::list_budgets(&st.config.recent_budgets)).unwrap(), false)),
        ("POST", ["open"]) => return Ok((open_session(st, str_field(body, "folder")?)?, false)),
        ("POST", ["create"]) => {
            let parent = PathBuf::from(str_field(body, "parent")?);
            if !parent.is_dir() {
                return Err("Choose an existing folder to create the budget in".into());
            }
            let name = str_field(body, "name")?;
            let locale = body.get("currencyLocale").and_then(Value::as_str).unwrap_or("en_US");
            let opts = OpenOptions {
                device_guid: Some(st.config.device_guid.clone()),
                ..OpenOptions::new(&st.config.friendly_name, APP_VERSION)
            };
            let starter = body.get("starter").and_then(Value::as_str).and_then(Starter::parse).unwrap_or_default();
            let pkg = create_budget_with(&parent, name, locale, starter, opts).map_err(|e| e.to_string())?;
            return Ok((open_session(st, &pkg.folder.display().to_string())?, false));
        }
        ("POST", ["close"]) => {
            if let Some(mut s) = st.session.take() {
                s.close();
            }
            return Ok((ok(), false));
        }
        ("POST", ["settings"]) => {
            if let Some(n) = body.get("friendlyName").and_then(Value::as_str).filter(|n| !n.trim().is_empty()) {
                st.config.friendly_name = n.trim().to_string();
            }
            if let Some(b) = body.get("enterScheduledTransactions").and_then(Value::as_bool) {
                st.config.enter_scheduled_transactions = b;
            }
            if let Some(b) = body.get("backupOnFirstSave").and_then(Value::as_bool) {
                st.config.backup_on_first_save = b;
            }
            st.config.save();
            return Ok((ok(), false));
        }
        _ => {}
    }

    if seg.first() == Some(&"import") {
        let AppState { config, session, .. } = st;
        let s = session.as_mut().ok_or("No budget is open")?;
        return import::route(config, s, method, &seg, body);
    }

    let s = st.session.as_mut().ok_or("No budget is open")?;
    let e = |r: budget4_core::Result<()>| r.map_err(|x| x.to_string());
    let edited = |v: Value| Ok((v, true));
    match (method, seg.as_slice()) {
        // ---------------------------------------------------------------- views
        ("GET", ["state"]) => Ok((s.state(), false)),
        ("GET", ["months", from]) => {
            let count = q("count").and_then(|c| c.parse().ok()).unwrap_or(1).clamp(1, 12);
            Ok((s.months(from, count), false))
        }
        ("GET", ["transactions"]) => Ok((s.transactions(q("account").as_deref().filter(|a| !a.is_empty())), false)),
        ("GET", ["scheduled"]) => Ok((s.scheduled(), false)),
        ("GET", ["log"]) => {
            let l = s.log.lock().map(|l| l.iter().rev().take(200).rev().cloned().collect::<Vec<_>>()).unwrap_or_default();
            Ok((json!(l), false))
        }
        ("POST", ["sync"]) => {
            let r = s.sync()?;
            Ok((json!({"imported": r.diffs_imported, "blocked": r.diffs_blocked, "conflicts": r.conflicts.len(), "published": r.published.is_some()}), false))
        }
        ("POST", ["save"]) => {
            s.save_full()?;
            Ok((ok(), false))
        }
        ("POST", ["backup"]) => {
            let f = s.pkg.write_backup().map_err(|x| x.to_string())?;
            Ok((json!({"file": f.display().to_string()}), false))
        }

        // ---------------------------------------------------------------- edits
        ("POST", ["transactions"]) => edited(json!(s.editor().save_transaction(&arg::<TransactionInput>(body)?).map_err(|x| x.to_string())?)),
        ("POST", ["transactions", "delete"]) => {
            let ids = ids(body)?;
            s.editor().delete_transactions(&ids);
            edited(ok())
        }
        ("POST", ["transactions", "cleared"]) => {
            let ids = ids(body)?;
            match body.get("cleared").and_then(Value::as_str) {
                Some(c) => e(s.editor().set_cleared(&ids, c))?,
                None => e(s.editor().toggle_cleared(&ids))?,
            }
            edited(ok())
        }
        ("POST", ["transactions", "unmatch"]) => {
            e(s.editor().unmatch(str_field(body, "id")?))?;
            edited(ok())
        }
        ("POST", ["transactions", "approve"]) => {
            let ids = ids(body)?;
            e(s.editor().approve(&ids))?;
            edited(ok())
        }
        ("POST", ["accounts"]) => edited(json!(s.editor().create_account(&arg::<AccountInput>(body)?).map_err(|x| x.to_string())?)),
        ("PATCH", ["accounts", id]) => {
            e(s.editor().update_account(id, &arg::<AccountUpdate>(body)?))?;
            edited(ok())
        }
        ("POST", ["accounts", id, "move"]) => {
            let before = body.get("beforeId").and_then(Value::as_str).map(str::to_string);
            e(s.editor().move_account(id, before.as_deref()))?;
            edited(ok())
        }
        ("POST", ["accounts", id, "reconcile"]) => {
            let balance = body.get("balance").and_then(Value::as_f64).ok_or("Missing balance")?;
            let date = body.get("date").and_then(Value::as_str).map(str::to_string).unwrap_or_else(budget4_core::dates::today);
            let adj = s.editor().reconcile(id, balance, &date).map_err(|x| x.to_string())?;
            edited(json!({"adjustment": adj}))
        }
        ("POST", ["accounts", id, "delete"]) => {
            e(s.editor().delete_account(id))?;
            edited(ok())
        }
        ("POST", ["budget"]) => {
            let amount = body.get("budgeted").and_then(Value::as_f64).ok_or("Missing budgeted")?;
            e(s.editor().set_budgeted(str_field(body, "month")?, str_field(body, "categoryId")?, amount))?;
            edited(ok())
        }
        ("POST", ["budget", "overspending"]) => {
            let h = body.get("handling").and_then(Value::as_str);
            e(s.editor().set_overspending_handling(str_field(body, "month")?, str_field(body, "categoryId")?, h))?;
            edited(ok())
        }
        ("POST", ["budget", "note"]) => {
            let month = str_field(body, "month")?;
            let note = body.get("note").and_then(Value::as_str);
            match body.get("categoryId").and_then(Value::as_str) {
                Some(c) => e(s.editor().set_monthly_category_note(month, c, note))?,
                None => e(s.editor().set_month_note(month, note))?,
            }
            edited(ok())
        }
        ("POST", ["categories", "master"]) => edited(json!(s.editor().create_master_category(str_field(body, "name")?).map_err(|x| x.to_string())?)),
        ("POST", ["categories"]) => {
            edited(json!(s.editor().create_category(str_field(body, "masterId")?, str_field(body, "name")?).map_err(|x| x.to_string())?))
        }
        ("PATCH", ["categories", id]) => {
            if let Some(n) = body.get("name").and_then(Value::as_str) {
                e(s.editor().rename_category(id, n))?;
            }
            if body.get("note").is_some() {
                e(s.editor().set_category_note(id, body.get("note").and_then(Value::as_str)))?;
            }
            edited(ok())
        }
        ("POST", ["categories", id, "hide"]) => {
            e(s.editor().hide_category(id))?;
            edited(ok())
        }
        ("POST", ["categories", id, "unhide"]) => {
            e(s.editor().unhide_category(id))?;
            edited(ok())
        }
        ("POST", ["categories", id, "move"]) => {
            let before = body.get("beforeId").and_then(Value::as_str).map(str::to_string);
            match body.get("masterId").and_then(Value::as_str) {
                Some(m) => e(s.editor().move_category(id, m, before.as_deref()))?,
                None => e(s.editor().move_master_category(id, before.as_deref()))?,
            }
            edited(ok())
        }
        ("POST", ["categories", id, "delete"]) => {
            if body.get("master").and_then(Value::as_bool) == Some(true) {
                e(s.editor().delete_master_category(id))?;
            } else {
                e(s.editor().delete_category(id, body.get("reassignTo").and_then(Value::as_str)))?;
            }
            edited(ok())
        }
        ("PATCH", ["payees", id]) => {
            e(s.editor().rename_payee(id, str_field(body, "name")?))?;
            edited(ok())
        }
        ("POST", ["payees", "merge"]) => {
            let from: Vec<String> = arg(body.get("fromIds").unwrap_or(&Value::Null))?;
            e(s.editor().merge_payees(&from, str_field(body, "intoId")?))?;
            edited(ok())
        }
        ("POST", ["scheduled"]) => edited(json!(s.editor().save_scheduled(&arg::<TransactionInput>(body)?).map_err(|x| x.to_string())?)),
        ("POST", ["scheduled", id, "delete"]) => {
            s.editor().delete_scheduled(id);
            edited(ok())
        }
        ("POST", ["scheduled", id, "enter"]) => edited(json!(s.editor().enter_scheduled_now(id).map_err(|x| x.to_string())?)),
        ("POST", ["scheduled", id, "skip"]) => {
            s.editor().skip_scheduled(id);
            edited(ok())
        }
        ("POST", ["undo"]) => edited(json!({"label": s.undo_redo(false)?})),
        ("POST", ["redo"]) => edited(json!({"label": s.undo_redo(true)?})),
        _ => Err(format!("Not found: {method} {path}")),
    }
}

/// What an edit is called in the Undo menu.
fn edit_label(method: &str, path: &str, body: &Value) -> String {
    let path = path.split('?').next().unwrap_or("");
    let seg: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    let has = |k: &str| body.get(k).is_some_and(|v| !v.is_null());
    let count = |k: &str| body.get(k).and_then(Value::as_array).map_or(1, Vec::len);
    let s = |one: &str, many: &str, n: usize| if n == 1 { one.to_string() } else { format!("{many} ({n})") };
    match (method, seg.as_slice()) {
        ("POST", ["transactions"]) => if has("id") { "edit transaction" } else { "add transaction" }.into(),
        ("POST", ["transactions", "delete"]) => s("delete transaction", "delete transactions", count("ids")),
        ("POST", ["transactions", "cleared"]) => s("change cleared", "change cleared", count("ids")),
        ("POST", ["transactions", "approve"]) => s("approve transaction", "approve transactions", count("ids")),
        ("POST", ["transactions", "unmatch"]) => "unmatch".into(),
        ("POST", ["import", "commit"]) => "import".into(),
        ("POST", ["accounts"]) => "add account".into(),
        ("PATCH", ["accounts", _]) => "edit account".into(),
        ("POST", ["accounts", _, "move"]) => "move account".into(),
        ("POST", ["accounts", _, "reconcile"]) => "reconcile".into(),
        ("POST", ["accounts", _, "delete"]) => "delete account".into(),
        ("POST", ["budget"]) => "budget amount".into(),
        ("POST", ["budget", "overspending"]) => "overspending setting".into(),
        ("POST", ["budget", "note"]) => "note".into(),
        ("POST", ["categories", "master"]) => "add category group".into(),
        ("POST", ["categories"]) => "add category".into(),
        ("PATCH", ["categories", _]) => if has("name") { "rename category" } else { "category note" }.into(),
        ("POST", ["categories", _, "hide"]) => "hide category".into(),
        ("POST", ["categories", _, "unhide"]) => "unhide category".into(),
        ("POST", ["categories", _, "move"]) => "move category".into(),
        ("POST", ["categories", _, "delete"]) => "delete category".into(),
        ("PATCH", ["payees", _]) => "rename payee".into(),
        ("POST", ["payees", "merge"]) => "merge payees".into(),
        ("POST", ["scheduled"]) => if has("id") { "edit scheduled transaction" } else { "add scheduled transaction" }.into(),
        ("POST", ["scheduled", _, "delete"]) => "delete scheduled transaction".into(),
        ("POST", ["scheduled", _, "enter"]) => "enter scheduled transaction".into(),
        ("POST", ["scheduled", _, "skip"]) => "skip scheduled transaction".into(),
        _ => "edit".into(),
    }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tauri::command]
fn api(app: tauri::AppHandle, state: tauri::State<'_, Shared>, method: String, path: String, body: Option<Value>) -> Result<Value, String> {
    let body = body.unwrap_or(Value::Null);
    let mut st = state.lock().map_err(|_| "App state is unavailable")?;
    let was_open = st.session.is_some();
    // Every change made through the UI (except undo/redo themselves) becomes one Undo step.
    let record = method != "GET" && !matches!(path.as_str(), "/api/undo" | "/api/redo") && !path.starts_with("/api/lock");
    if record {
        if let Some(s) = st.session.as_mut() {
            s.begin_edit();
        }
    }
    let result = route(&mut st, &method, &path, &body);
    if record {
        if let Some(s) = st.session.as_mut() {
            s.end_edit(&edit_label(&method, &path, &body));
        }
    }
    let (out, edited) = result?;
    if edited {
        if let Some(s) = st.session.as_mut() {
            s.after_edit();
        }
    }
    if was_open && st.session.is_none() {
        emit(&app, "closed");
    }
    Ok(out)
}

pub fn run() {
    let config = AppConfig::load();
    let locked = config.pin_hash.is_some();
    let shared: Shared = Arc::new(Mutex::new(AppState { config, session: None, locked, throttle: Default::default(), pending_open: None }));
    let initial = std::env::args().nth(1).filter(|a| PathBuf::from(a).join("Budget.ymeta").is_file());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![api])
        .setup(move |app| {
            if let Some(folder) = initial {
                if let Ok(mut st) = shared.lock() {
                    if st.locked {
                        st.pending_open = Some(folder);
                    } else if let Err(e) = open_session(&mut st, &folder) {
                        eprintln!("Couldn't open {folder}: {e}");
                    }
                }
            }
            // YNAB 4's autosave/sync rhythm.
            let handle = app.handle().clone();
            let state = shared.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(5));
                let changed = match state.lock() {
                    Ok(mut st) => st.session.as_mut().map(|s| s.tick()),
                    Err(_) => break,
                };
                match changed {
                    Some(true) => emit(&handle, "changed"),
                    Some(false) => emit(&handle, "status"),
                    None => {}
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Budget4")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit = event {
                if let Ok(mut st) = app.state::<Shared>().lock() {
                    if let Some(mut s) = st.session.take() {
                        s.close();
                    }
                }
            }
        });
}
