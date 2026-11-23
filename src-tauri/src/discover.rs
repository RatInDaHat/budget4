//! Finds YNAB 4 budgets in Dropbox (the folder YNAB 4 itself uses).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use budget4_core::package::budget_name_from_folder;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct BudgetListing {
    pub name: String,
    pub folder: String,
    pub modified: f64,
}

fn read_json(p: &Path) -> Option<Value> {
    let t = fs::read_to_string(p).ok()?;
    serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()
}

/// Dropbox roots from Dropbox's own info.json, plus ~/Dropbox.
pub fn dropbox_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut candidates = Vec::new();
    for var in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(v) = std::env::var_os(var) {
            candidates.push(PathBuf::from(v).join("Dropbox").join("info.json"));
        }
    }
    if let Some(h) = dirs::home_dir() {
        candidates.push(h.join(".dropbox").join("info.json"));
    }
    for c in candidates {
        if let Some(Value::Object(info)) = read_json(&c) {
            for v in info.values() {
                if let Some(p) = v.get("path").and_then(Value::as_str) {
                    let p = PathBuf::from(p);
                    if p.is_dir() && !roots.contains(&p) {
                        roots.push(p);
                    }
                }
            }
        }
    }
    if let Some(h) = dirs::home_dir() {
        let p = h.join("Dropbox");
        if p.is_dir() && !roots.contains(&p) {
            roots.push(p);
        }
    }
    roots
}

/// Folders YNAB 4 keeps budgets in (".ynabSettings.yroot" → relativeDefaultBudgetsFolder, default "YNAB").
pub fn budget_folders() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in dropbox_roots() {
        let mut rel = "YNAB".to_string();
        if let Some(y) = read_json(&root.join(".ynabSettings.yroot")) {
            if let Some(r) = y.get("relativeDefaultBudgetsFolder").and_then(Value::as_str) {
                rel = r.to_string();
            }
            for k in y.get("relativeKnownBudgets").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if let Some(parent) = root.join(k).parent() {
                    if parent.is_dir() && !out.iter().any(|x| x == parent) {
                        out.push(parent.to_path_buf());
                    }
                }
            }
        }
        let p = root.join(rel);
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

pub fn list_budgets(extra: &[String]) -> Vec<BudgetListing> {
    let mut seen: BTreeMap<String, BudgetListing> = BTreeMap::new();
    let mut add = |folder: PathBuf| {
        let is_budget = folder.extension().is_some_and(|e| e.eq_ignore_ascii_case("ynab4")) && folder.join("Budget.ymeta").is_file();
        if !is_budget {
            return;
        }
        let modified = fs::metadata(folder.join("Budget.ymeta"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        let key = folder.display().to_string().to_lowercase();
        seen.insert(key, BudgetListing { name: budget_name_from_folder(&folder), folder: folder.display().to_string(), modified });
    };
    for parent in budget_folders() {
        if let Ok(rd) = fs::read_dir(&parent) {
            for e in rd.flatten() {
                add(e.path());
            }
        }
    }
    for f in extra {
        add(PathBuf::from(f));
    }
    let mut v: Vec<BudgetListing> = seen.into_values().collect();
    v.sort_by(|a, b| b.modified.total_cmp(&a.modified));
    v
}
