//! Bank file import routes: load a file, preview what importing it into an account would do (run on
//! a copy of the budget), then commit. OFX account choices are saved in the budget the way YNAB 4
//! saves them (account mappings, synced); CSV column layouts are saved per account in config.json.

use std::path::Path;

use base64::Engine;
use budget4_core::editor::Editor;
use budget4_core::importing::{
    csv_transactions, decode_text, guess_csv_mapping, parse_ofx, read_csv, valid_date_orders, BankTxn, CsvMapping, ImportOptions,
};
use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::session::{PendingFile, Session};

const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

fn preferred_date_order(locale: &str) -> &'static str {
    let l = locale.replace('-', "_");
    if l == "en_US" || l == "en_PH" || l == "es_US" {
        "MDY"
    } else if ["ja", "zh", "ko", "hu", "sv", "lt"].iter().any(|p| l.starts_with(p)) || l == "en_CA" || l == "fr_CA" {
        "YMD"
    } else {
        "DMY"
    }
}

fn locale(s: &Session) -> String {
    use budget4_core::json::ObjExt;
    s.pkg.store.budget_meta().and_then(|m| m.str_of("currencyLocale")).unwrap_or("en_US").to_string()
}

fn options(body: &Value) -> Result<ImportOptions, String> {
    serde_json::from_value(body.clone()).map_err(|e| format!("Bad request: {e}"))
}

fn is_ofx(path: &Path, text: &str) -> bool {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("ofx" | "qfx" | "qbo") => true,
        Some("csv" | "tsv") => false,
        _ => text.to_ascii_uppercase().contains("<OFX"),
    }
}

fn date_range(txns: &[BankTxn]) -> (Option<&str>, Option<&str>) {
    (txns.iter().map(|t| t.date.as_str()).min(), txns.iter().map(|t| t.date.as_str()).max())
}

fn load(config: &AppConfig, s: &mut Session, body: &Value) -> Result<Value, String> {
    let path = body.get("path").and_then(Value::as_str).ok_or("Missing path")?;
    let p = Path::new(path);
    let len = std::fs::metadata(p).map_err(|e| format!("Can't read {path}: {e}"))?.len();
    if len > MAX_FILE_BYTES {
        return Err("That file is too large to be a bank export".into());
    }
    let bytes = std::fs::read(p).map_err(|e| format!("Can't read {path}: {e}"))?;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    load_bytes(config, s, name, &bytes, body)
}

/// A bank file sent by the UI as its contents (dropped onto the window, where no path is known).
fn upload(config: &AppConfig, s: &mut Session, body: &Value) -> Result<Value, String> {
    let name = body.get("name").and_then(Value::as_str).ok_or("Missing name")?.to_string();
    let data = body.get("data").and_then(Value::as_str).ok_or("Missing data")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| "That file didn't arrive intact. Try again.".to_string())?;
    load_bytes(config, s, name, &bytes, body)
}

fn load_bytes(config: &AppConfig, s: &mut Session, name: String, bytes: &[u8], body: &Value) -> Result<Value, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("That file is too large to be a bank export".into());
    }
    let text = decode_text(bytes);
    let p = Path::new(&name);
    if is_ofx(p, &text) {
        let statements = parse_ofx(&text, s.decimals).map_err(|e| e.to_string())?;
        if statements.is_empty() {
            return Err("That file has no transactions in it".into());
        }
        let ed = Editor::new(&mut s.pkg.store, s.decimals);
        let list: Vec<Value> = statements
            .iter()
            .enumerate()
            .map(|(i, st)| {
                let (from, to) = date_range(&st.txns);
                let mapping = st.bank_account_id.as_deref().and_then(|a| ed.find_account_mapping(st.fid.as_deref(), a));
                json!({
                    "index": i,
                    "kind": st.kind,
                    "accountType": st.account_type,
                    "last4": st.last4(),
                    "count": st.txns.len(),
                    "from": from,
                    "to": to,
                    "mapping": mapping,
                })
            })
            .collect();
        s.pending_import = Some(PendingFile::Ofx { name: name.clone(), statements });
        Ok(json!({"kind": "ofx", "name": name, "statements": list}))
    } else {
        let rows = read_csv(&text);
        if rows.is_empty() {
            return Err("That file is empty".into());
        }
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        let saved = body
            .get("accountId")
            .and_then(Value::as_str)
            .and_then(|a| config.csv_mappings.get(a))
            .filter(|m| [m.date, m.payee, m.memo, m.amount, m.outflow, m.inflow, m.check_number].iter().flatten().all(|c| *c < columns))
            .cloned();
        let mapping = saved.unwrap_or_else(|| guess_csv_mapping(&rows, preferred_date_order(&locale(s))));
        let orders = valid_date_orders(&rows, &mapping);
        let sample: Vec<&Vec<String>> = rows.iter().take(12).collect();
        let out = json!({"kind": "csv", "name": name, "columns": columns, "rowCount": rows.len(), "sample": sample, "mapping": mapping, "dateOrders": orders});
        s.pending_import = Some(PendingFile::Csv { name, rows });
        Ok(out)
    }
}

/// The transactions to import from the pending file, plus problems (CSV rows that didn't read)
/// and, for OFX, the statement they came from.
fn pending_txns(s: &Session, body: &Value) -> Result<(Vec<BankTxn>, Vec<String>, Value), String> {
    match s.pending_import.as_ref().ok_or("Choose a file to import first")? {
        PendingFile::Ofx { statements, .. } => {
            let i = body.get("statement").and_then(Value::as_u64).unwrap_or(0) as usize;
            let st = statements.get(i).ok_or("No such statement in the file")?;
            Ok((st.txns.clone(), vec![], json!(i)))
        }
        PendingFile::Csv { rows, .. } => {
            let m: CsvMapping = serde_json::from_value(body.get("mapping").cloned().unwrap_or(Value::Null)).map_err(|e| format!("Bad column mapping: {e}"))?;
            if m.date.is_none() || (m.amount.is_none() && m.outflow.is_none() && m.inflow.is_none()) {
                return Err("Choose the date column and the amount column(s)".into());
            }
            let (txns, problems) = csv_transactions(rows, &m, s.decimals);
            Ok((txns, problems, Value::Null))
        }
    }
}

fn preview(s: &mut Session, body: &Value) -> Result<Value, String> {
    let o = options(body)?;
    let (txns, problems, _) = pending_txns(s, body)?;
    let mut copy = s.pkg.store.clone();
    let summary = Editor::new(&mut copy, s.decimals).import_bank_transactions(&txns, &o).map_err(|e| e.to_string())?;
    let mut out = serde_json::to_value(&summary).unwrap();
    out["problems"] = json!(problems);
    if let (Some(PendingFile::Csv { rows, .. }), Some(m)) = (&s.pending_import, body.get("mapping")) {
        if let Ok(m) = serde_json::from_value::<CsvMapping>(m.clone()) {
            out["dateOrders"] = json!(valid_date_orders(rows, &m));
        }
    }
    Ok(out)
}

fn commit(config: &mut AppConfig, s: &mut Session, body: &Value) -> Result<Value, String> {
    let o = options(body)?;
    let skip = body.get("skip").and_then(Value::as_bool).unwrap_or(false);
    let (txns, problems, statement) = pending_txns(s, body)?;
    let decimals = s.decimals;
    let mut ed = Editor::new(&mut s.pkg.store, decimals);
    // Remember the choice the way YNAB 4 does, so either app picks the same account next time.
    let remembered = match &s.pending_import {
        Some(PendingFile::Ofx { statements, .. }) => {
            let st = &statements[statement.as_u64().unwrap_or(0) as usize];
            match st.bank_account_id.as_deref() {
                Some(acct) => {
                    ed.save_account_mapping(st.fid.as_deref(), acct, Some(&o.account_id).filter(|_| !skip).map(String::as_str), o.flip_payee_memo, o.import_memos, skip)
                        .map_err(|e| e.to_string())?;
                    true
                }
                None => false,
            }
        }
        _ => false,
    };
    if skip {
        return Ok(json!({"skipped": true, "remembered": remembered}));
    }
    let summary = ed.import_bank_transactions(&txns, &o).map_err(|e| e.to_string())?;
    if let Some(Ok(m)) = body.get("mapping").map(|m| serde_json::from_value::<CsvMapping>(m.clone())) {
        config.csv_mappings.insert(o.account_id.clone(), m);
        config.save();
    }
    let name = match &s.pending_import {
        Some(PendingFile::Ofx { name, .. } | PendingFile::Csv { name, .. }) => name.clone(),
        None => String::new(),
    };
    s.note(&format!(
        "Imported {name}: {} new, {} matched, {} already imported{}",
        summary.imported,
        summary.matched,
        summary.duplicates,
        if summary.before_start > 0 { format!(", {} before the account's start left out", summary.before_start) } else { String::new() }
    ));
    let mut out = serde_json::to_value(&summary).unwrap();
    out.as_object_mut().unwrap().remove("rows");
    out["problems"] = json!(problems.len());
    out["remembered"] = json!(remembered);
    Ok(out)
}

/// Routes /api/import/*. Returns (response, edited).
pub fn route(config: &mut AppConfig, s: &mut Session, method: &str, seg: &[&str], body: &Value) -> Result<(Value, bool), String> {
    match (method, seg) {
        ("POST", ["import", "load"]) => Ok((load(config, s, body)?, false)),
        ("POST", ["import", "upload"]) => Ok((upload(config, s, body)?, false)),
        ("POST", ["import", "preview"]) => Ok((preview(s, body)?, false)),
        ("POST", ["import", "commit"]) => Ok((commit(config, s, body)?, true)),
        ("POST", ["import", "cancel"]) => {
            s.pending_import = None;
            Ok((json!({"ok": true}), false))
        }
        _ => Err(format!("Not found: {method} /api/{}", seg.join("/"))),
    }
}
