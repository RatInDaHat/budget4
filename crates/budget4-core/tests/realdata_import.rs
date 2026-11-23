//! Import ids checked against transactions YNAB 4 imported into a real budget: for every imported
//! transaction that was never matched or edited, `YNAB:<amount>:<date>:<n>` must be exactly what
//! Budget4 would assign, and its entityId `Transaction/<YNABID>` (plus `_<n>` on collisions).

mod common;

use std::collections::HashMap;

use budget4_core::importing::format_amount_for_id;
use budget4_core::json::{parse, ObjExt};
use budget4_core::package::BudgetPackage;

#[test]
fn import_ids_match_what_ynab4_wrote() {
    let budgets = common::test_budgets();
    require_testdata!(budgets);
    for folder in budgets {
        let meta = BudgetPackage::read_meta(&folder).unwrap();
        let data = folder.join(meta.str_of("relativeDataFolderName").unwrap());
        let fulls = common::find_files(&data, &|n| n == "Budget.yfull");
        let newest = fulls.iter().max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).unwrap();
        let root = parse(&std::fs::read_to_string(newest).unwrap()).unwrap();
        let (mut checked, mut same, mut ids_ok) = (0usize, 0usize, 0usize);
        let mut shapes: HashMap<String, usize> = HashMap::new();
        for t in root["transactions"].as_array().unwrap() {
            let Some(y) = t["YNABID"].as_str() else { continue };
            // Matched (or approved-after-match) transactions keep your date, so skip them.
            if t.get("matchedTransactions").is_some() || t["source"] != "Imported" {
                continue;
            }
            let (amount, date) = (t["amount"].as_f64().unwrap_or(0.0), t["date"].as_str().unwrap_or(""));
            checked += 1;
            let parts: Vec<&str> = y.splitn(4, ':').collect();
            if parts.len() == 4 && parts[1] == format_amount_for_id(amount, 2) && parts[2] == date {
                same += 1;
            } else {
                *shapes.entry(y.chars().map(|c| if c.is_ascii_digit() { '9' } else { c }).collect()).or_default() += 1;
            }
            let id = t["entityId"].as_str().unwrap_or("");
            if id == format!("Transaction/{y}") || id.starts_with(&format!("Transaction/{y}_")) {
                ids_ok += 1;
            }
        }
        eprintln!("{}: {checked} imported, {same} ids reproduced, {ids_ok} entity ids follow the id", folder.display());
        if checked == 0 {
            continue;
        }
        // Edited-after-import transactions (amount or date changed by hand) can't reproduce.
        assert!(same as f64 >= checked as f64 * 0.97, "only {same}/{checked} reproduced; unmatched shapes: {shapes:?}");
        assert!(ids_ok as f64 >= checked as f64 * 0.90, "only {ids_ok}/{checked} entity ids follow YNABID");
    }
}
