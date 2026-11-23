//! Budget math checked against a "… as of YYYY-MM-DD …-Budget.csv" export made by YNAB 4 itself,
//! using the first .y4backup taken after the export. Months before the export can't have changed in
//! between, so every row there must match exactly.

mod common;

use std::collections::HashMap;
use std::fs;
use std::io::Read;

use budget4_core::budget::compute_budget;
use budget4_core::json::{parse, ObjExt};
use budget4_core::store::Store;

fn parse_csv_line(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut q) = (Vec::new(), String::new(), false);
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if q {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    cur.push('"');
                    it.next();
                } else {
                    q = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            q = true;
        } else if c == ',' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

fn money(s: &str) -> f64 {
    let v: f64 = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect::<String>().parse().unwrap_or(0.0);
    if s.contains('-') {
        -v
    } else {
        v
    }
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

#[test]
fn matches_ynab_export_before_export_date() {
    let budgets = common::test_budgets();
    require_testdata!(budgets);
    let Some(csv) = fs::read_dir(common::testdata()).unwrap().flatten().map(|e| e.path()).find(|p| {
        let n = p.file_name().unwrap().to_string_lossy();
        n.contains(" as of ") && n.ends_with("-Budget.csv")
    }) else {
        eprintln!("skipped: no YNAB Budget.csv export in test data");
        return;
    };
    let name = csv.file_name().unwrap().to_string_lossy().to_string();
    let export_date = &name[name.find(" as of ").unwrap() + 7..][..10];

    let budget = &budgets[0];
    let mut backups: Vec<String> = fs::read_dir(budget)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".y4backup") && &n[7..17] >= export_date)
        .collect();
    backups.sort_by(|a, b| a[7..17].cmp(&b[7..17]));
    let backup = backups.first().expect("a backup taken after the export");
    let mut zip = zip::ZipArchive::new(fs::File::open(budget.join(backup)).unwrap()).unwrap();
    let entry_name = zip.file_names().find(|n| n.ends_with(".ynab4")).unwrap().to_string();
    let mut text = String::new();
    zip.by_name(&entry_name).unwrap().read_to_string(&mut text).unwrap();
    let store = Store::from_root(parse(&text).unwrap(), "Z");
    let model = compute_budget(&store, None, 2);

    let mut by_name: HashMap<String, String> = HashMap::new();
    for node in &model.tree {
        let m = store.get(&node.master_id).unwrap();
        for s in &node.sub_ids {
            by_name.insert(format!("{}|{}", m.str_of("name").unwrap_or(""), store.get(s).unwrap().str_of("name").unwrap_or("")), s.clone());
        }
    }

    let content = fs::read_to_string(&csv).unwrap();
    let mut lines = content.trim_start_matches('\u{feff}').lines().filter(|l| !l.is_empty());
    let header = parse_csv_line(lines.next().unwrap());
    let col = |n: &str| header.iter().position(|h| h == n).unwrap();
    let (c_month, c_master, c_sub, c_bud, c_out, c_bal) =
        (col("Month"), col("Master Category"), col("Sub Category"), col("Budgeted"), col("Outflows"), col("Category Balance"));
    let cutoff = &export_date[..7];
    let (mut compared, mut mismatches) = (0, Vec::new());
    for line in lines {
        let r = parse_csv_line(line);
        let mut parts = r[c_month].split(' ');
        let (mname, year) = (parts.next().unwrap(), parts.next().unwrap());
        let month = format!("{year}-{:02}", MONTHS.iter().position(|m| *m == mname).unwrap() + 1);
        if month.as_str() >= cutoff {
            continue;
        }
        let Some(id) = by_name.get(&format!("{}|{}", r[c_master], r[c_sub])) else { continue };
        compared += 1;
        let close = |a: f64, b: f64| (a - b).abs() < 0.005;
        let ok = model.by_month.get(&month).and_then(|mm| mm.categories.get(id)).is_some_and(|c| {
            close(c.budgeted, money(&r[c_bud])) && close(c.spent, money(&r[c_out])) && close(c.balance, money(&r[c_bal]))
        });
        if !ok {
            mismatches.push(format!("{month} {id}"));
        }
    }
    eprintln!("compared {compared} rows");
    assert!(compared > 1000);
    mismatches.truncate(10);
    assert!(mismatches.is_empty(), "{mismatches:?}");
}
