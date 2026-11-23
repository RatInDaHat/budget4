//! Undo/redo: each edit is recorded as the records it changed (as they were before), and undoing
//! puts them back as a new local edit, so the result syncs like any other change.

use budget4_core::budget::{account_balances, compute_budget};
use budget4_core::create::create_budget;
use budget4_core::editor::{AccountInput, Editor, SplitInput, TransactionInput};
use budget4_core::importing::{assign_ynab_ids, BankTxn, ImportOptions};
use budget4_core::json::{Obj, ObjExt};
use budget4_core::package::{BudgetPackage, OpenOptions};
use budget4_core::store::{Store, UndoStep};

fn opts(guid: Option<&str>) -> OpenOptions {
    OpenOptions { device_guid: guid.map(str::to_string), backup_on_first_save: false, ..OpenOptions::new("Laptop", "test") }
}

fn category(store: &Store, master: &str, sub: &str) -> String {
    let m = store.collection("masterCategories").into_iter().find(|m| m.str_of("name") == Some(master)).unwrap();
    store.children(m.id(), "subCategories").into_iter().find(|s| s.str_of("name") == Some(sub)).unwrap().id().to_string()
}

fn checking(store: &mut Store) -> String {
    let input = AccountInput {
        name: "Checking".into(),
        account_type: "Checking".into(),
        on_budget: true,
        starting_balance: 1000.0,
        starting_date: Some("2026-10-01".into()),
        note: None,
    };
    Editor::new(store, 2).create_account(&input).unwrap()
}

fn txn(account: &str, date: &str, amount: f64) -> TransactionInput {
    TransactionInput { account_id: account.into(), date: date.into(), amount, ..Default::default() }
}

/// Runs one edit under the journal and returns its undo step.
fn edit<T>(store: &mut Store, f: impl FnOnce(&mut Editor) -> T) -> (T, UndoStep) {
    store.begin_journal();
    let out = f(&mut Editor::new(store, 2));
    let step = store.end_journal().unwrap();
    assert!(step.complete, "edit wasn't fully recorded");
    (out, step)
}

/// Undoes a step and returns the step that redoes it.
fn undo(store: &mut Store, step: &UndoStep) -> UndoStep {
    store.begin_journal();
    let r = store.restore(step);
    let back = store.end_journal().unwrap();
    r.unwrap();
    assert!(back.complete);
    back
}

/// A record's content, ignoring version stamps.
fn content(store: &Store, id: &str) -> serde_json::Value {
    let mut v = store.materialize(id);
    strip_versions(&mut v);
    v
}

fn strip_versions(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(o) => {
            o.remove("entityVersion");
            o.remove("isResolvedConflict");
            o.values_mut().for_each(strip_versions);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(strip_versions),
        _ => {}
    }
}

fn working(store: &Store, account: &str) -> f64 {
    account_balances(store, 2)[account].working
}

fn live(store: &Store, id: &str) -> bool {
    store.get(id).is_some_and(|o| !o.tomb())
}

#[test]
fn add_edit_delete_undo_and_redo() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Undo", "en_US", opts(None)).unwrap();
    let store = &mut pkg.store;
    let groceries = category(store, "Everyday Expenses", "Groceries");
    let acct = checking(store);

    // Add (with a brand-new payee), then undo: the transaction and the payee go away.
    let input = TransactionInput { payee_name: Some("Corner Store".into()), category_id: Some(groceries.clone()), ..txn(&acct, "2026-10-05", -40.0) };
    let (id, step) = edit(store, |ed| ed.save_transaction(&input).unwrap());
    let payee = store.get(&id).unwrap().str_of("payeeId").unwrap().to_string();
    let added = content(store, &id);
    assert_eq!(working(store, &acct), 960.0);
    let redo = undo(store, &step);
    assert!(!live(store, &id) && !live(store, &payee));
    assert_eq!(working(store, &acct), 1000.0);
    // Redo brings back exactly what was added.
    let again = undo(store, &redo);
    assert!(live(store, &id) && live(store, &payee));
    assert_eq!(content(store, &id), added);
    assert_eq!(working(store, &acct), 960.0);
    drop(again);

    // Edit fields, then undo back to the original.
    let (_, step) = edit(store, |ed| {
        ed.save_transaction(&TransactionInput { id: Some(id.clone()), memo: Some("milk".into()), ..txn(&acct, "2026-10-07", -55.5) }).unwrap()
    });
    assert_eq!(store.get(&id).unwrap().f("amount"), -55.5);
    undo(store, &step);
    assert_eq!(content(store, &id), added);

    // Delete, then undo.
    let (_, step) = edit(store, |ed| ed.delete_transactions(std::slice::from_ref(&id)));
    assert!(!live(store, &id));
    undo(store, &step);
    assert!(live(store, &id));
    assert_eq!(working(store, &acct), 960.0);
}

#[test]
fn splits_and_transfers_come_back_whole() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Undo", "en_US", opts(None)).unwrap();
    let store = &mut pkg.store;
    let groceries = category(store, "Everyday Expenses", "Groceries");
    let fuel = category(store, "Everyday Expenses", "Fuel");
    let acct = checking(store);
    let savings = checking(store);

    let split = |id: Option<&str>, a: f64, c: &str| SplitInput { id: id.map(str::to_string), amount: a, category_id: Some(c.into()), memo: None, transfer_account_id: None };
    let (id, _) = edit(store, |ed| {
        ed.save_transaction(&TransactionInput { splits: Some(vec![split(None, -30.0, &groceries), split(None, -20.0, &fuel)]), ..txn(&acct, "2026-10-05", -50.0) })
            .unwrap()
    });
    let before = content(store, &id);
    let subs: Vec<String> = store.children(&id, "subTransactions").iter().map(|s| s.id().to_string()).collect();
    assert_eq!(subs.len(), 2);

    // Replace the splits with one line: the old sub-transactions are removed outright.
    let (_, step) = edit(store, |ed| {
        ed.save_transaction(&TransactionInput { id: Some(id.clone()), category_id: Some(groceries.clone()), ..txn(&acct, "2026-10-05", -50.0) }).unwrap()
    });
    assert!(store.children(&id, "subTransactions").iter().all(|s| s.tomb()) || store.children(&id, "subTransactions").is_empty());
    undo(store, &step);
    assert_eq!(content(store, &id), before);
    let back: Vec<String> = store.children(&id, "subTransactions").iter().map(|s| s.id().to_string()).collect();
    assert_eq!(back, subs);
    // Restored children carry fresh versions (so other devices take them).
    let parent_v = store.get(&id).unwrap().str_of("entityVersion").unwrap().to_string();
    for s in &back {
        assert!(store.get(s).unwrap().str_of("entityVersion").unwrap().starts_with("A-"));
        assert_eq!(store.parent(s), Some(id.as_str()));
    }
    assert!(parent_v.starts_with("A-"));

    // A transfer edit changes both sides; undo puts both back.
    let (tr, _) = edit(store, |ed| ed.save_transaction(&TransactionInput { transfer_account_id: Some(savings.clone()), ..txn(&acct, "2026-10-06", -100.0) }).unwrap());
    let other = store.get(&tr).unwrap().str_of("transferTransactionId").unwrap().to_string();
    let (a, b) = (content(store, &tr), content(store, &other));
    let (_, step) = edit(store, |ed| {
        ed.save_transaction(&TransactionInput { id: Some(tr.clone()), transfer_account_id: Some(savings.clone()), ..txn(&acct, "2026-10-09", -250.0) }).unwrap()
    });
    assert_eq!(store.get(&other).unwrap().f("amount"), 250.0);
    undo(store, &step);
    assert_eq!((content(store, &tr), content(store, &other)), (a, b));
}

#[test]
fn budget_and_category_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Undo", "en_US", opts(None)).unwrap();
    let store = &mut pkg.store;
    let groceries = category(store, "Everyday Expenses", "Groceries");
    let budgeted = |s: &Store| compute_budget(s, Some("2026-12"), 2).by_month["2026-11"].categories[&groceries].budgeted;

    // First budget in a month creates its record; undo removes it; budgeting again revives it.
    let (_, step) = edit(store, |ed| ed.set_budgeted("2026-11", &groceries, 300.0).unwrap());
    assert_eq!(budgeted(store), 300.0);
    let redo = undo(store, &step);
    assert_eq!(budgeted(store), 0.0);
    undo(store, &redo);
    assert_eq!(budgeted(store), 300.0);
    let (_, step) = edit(store, |ed| ed.set_budgeted("2026-11", &groceries, 425.0).unwrap());
    undo(store, &step);
    assert_eq!(budgeted(store), 300.0);

    // Hiding renames and moves the category; undo restores both.
    let before = content(store, &groceries);
    let master = store.get(&groceries).unwrap().str_of("masterCategoryId").unwrap().to_string();
    let (_, step) = edit(store, |ed| ed.hide_category(&groceries).unwrap());
    assert_ne!(store.parent(&groceries), Some(master.as_str()));
    undo(store, &step);
    assert_eq!(content(store, &groceries), before);
    assert_eq!(store.parent(&groceries), Some(master.as_str()));
    assert!(store.children(&master, "subCategories").iter().any(|c| c.id() == groceries));
}

#[test]
fn undoing_an_import_brings_back_your_entered_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Undo", "en_US", opts(None)).unwrap();
    let store = &mut pkg.store;
    let acct = checking(store);
    let (entered, _) = edit(store, |ed| ed.save_transaction(&TransactionInput { payee_name: Some("Landlord".into()), ..txn(&acct, "2026-10-03", -900.0) }).unwrap());
    let original = content(store, &entered);

    let mut bank = vec![
        BankTxn { date: "2026-10-04".into(), amount: -900.0, payee: Some("ACH LANDLORD LLC".into()), fitid: Some("F1".into()), ..Default::default() },
        BankTxn { date: "2026-10-05".into(), amount: -12.5, payee: Some("COFFEE".into()), fitid: Some("F2".into()), ..Default::default() },
    ];
    assign_ynab_ids(&mut bank, 2);
    let o = ImportOptions { account_id: acct.clone(), import_memos: true, flip_payee_memo: false, include_before_start: false };
    let (sum, step) = edit(store, |ed| ed.import_bank_transactions(&bank, &o).unwrap());
    assert_eq!((sum.imported, sum.matched), (1, 1));
    assert!(!live(store, &entered));
    let balance_after = working(store, &acct);

    let redo = undo(store, &step);
    assert_eq!(content(store, &entered), original);
    assert_eq!(working(store, &acct), 100.0);
    let imported: Vec<&Obj> = store.collection("transactions").into_iter().filter(|t| !t.tomb() && t.str_of("YNABID").is_some()).collect();
    assert!(imported.is_empty(), "bank transactions should be gone");

    undo(store, &redo);
    assert!(!live(store, &entered));
    assert_eq!(working(store, &acct), balance_after);
}

#[test]
fn refuses_when_changed_since_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Undo", "en_US", opts(None)).unwrap();
    let store = &mut pkg.store;
    let acct = checking(store);
    let (id, _) = edit(store, |ed| ed.save_transaction(&txn(&acct, "2026-10-05", -40.0)).unwrap());
    let (_, step) = edit(store, |ed| ed.save_transaction(&TransactionInput { id: Some(id.clone()), ..txn(&acct, "2026-10-05", -45.0) }).unwrap());
    // Changed again outside the recorded edit (as a change from another device would be).
    Editor::new(store, 2).save_transaction(&TransactionInput { id: Some(id.clone()), ..txn(&acct, "2026-10-05", -47.0) }).unwrap();
    let before = content(store, &id);
    store.begin_journal();
    assert!(store.restore(&step).is_err());
    assert!(store.end_journal().unwrap().is_empty());
    assert_eq!(content(store, &id), before);
}

#[test]
fn undo_syncs_as_a_normal_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Undo", "en_US", opts(None)).unwrap();
    let acct = checking(&mut pkg.store);
    let (id, _) = edit(&mut pkg.store, |ed| ed.save_transaction(&txn(&acct, "2026-10-05", -40.0)).unwrap());
    pkg.publish().unwrap();
    let (_, step) = edit(&mut pkg.store, |ed| ed.delete_transactions(std::slice::from_ref(&id)));
    pkg.publish().unwrap();
    undo(&mut pkg.store, &step);
    pkg.publish().unwrap();

    // A second device that reads only the change files sees the transaction back.
    let mut other = BudgetPackage::open(&pkg.folder, opts(None)).unwrap();
    other.sync().unwrap();
    assert!(live(&other.store, &id));
    assert_eq!(other.store.get(&id).unwrap().f("amount"), -40.0);
    // And so does this device after a reopen.
    pkg.save_full(false).unwrap();
    let again = BudgetPackage::open(&pkg.folder, opts(Some(&pkg.device.device_guid))).unwrap();
    assert!(live(&again.store, &id));
}
