//! Tests that need a real YNAB 4 budget read it from BUDGET4_TESTDATA (a folder holding one or more
//! *.ynab4 packages; default: a `ynab4-testdata` folder next to the repository) and skip when it
//! isn't there. Never point it at a live budget: some tests write into copies of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

pub fn testdata() -> PathBuf {
    match std::env::var_os("BUDGET4_TESTDATA") {
        Some(p) => PathBuf::from(p),
        // crates/budget4-core -> the folder holding the repository
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../ynab4-testdata"),
    }
}

pub fn test_budgets() -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(testdata()) else { return vec![] };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "ynab4")).collect();
    v.sort();
    v
}

pub fn find_files(dir: &Path, pred: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(find_files(&p, pred));
        } else if pred(&e.file_name().to_string_lossy()) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Skips the calling test (prints why) when there's no real data.
#[macro_export]
macro_rules! require_testdata {
    ($v:expr) => {
        if $v.is_empty() {
            eprintln!("skipped: no test data in {}", common::testdata().display());
            return;
        }
    };
}
