//! Editing, budget creation and two-device sync on throwaway budgets.

mod common;

use std::fs;
use std::path::Path;

use budget4_core::budget::{account_balances, compute_budget};
use budget4_core::create::{create_budget, create_budget_with, Starter, BUSINESS_CATEGORIES, PERSONAL_CATEGORIES};
use budget4_core::editor::{AccountInput, AccountUpdate, Editor, SplitInput, TransactionInput};
use budget4_core::json::{parse, ObjExt};
use budget4_core::package::{import_diffs, parse_diff_filename, read_json, BudgetPackage, DiffFileRef, OpenOptions};
use budget4_core::schema::{builtin, pre_ynab_debt_category_id};
use budget4_core::store::Store;
use budget4_core::vv::VersionVector;

fn opts(name: &str, guid: Option<&str>) -> OpenOptions {
    OpenOptions { device_guid: guid.map(str::to_string), backup_on_first_save: false, ..OpenOptions::new(name, "test") }
}

fn category(store: &Store, master: &str, sub: &str) -> String {
    let m = store.collection("masterCategories").into_iter().find(|m| m.str_of("name") == Some(master)).unwrap();
    store.children(m.id(), "subCategories").into_iter().find(|s| s.str_of("name") == Some(sub)).unwrap().id().to_string()
}

fn account(name: &str, on_budget: bool, balance: f64, date: &str) -> AccountInput {
    AccountInput {
        name: name.into(),
        account_type: if on_budget { "Checking".into() } else { "OtherLiability".into() },
        on_budget,
        starting_balance: balance,
        starting_date: Some(date.into()),
        note: None,
    }
}

fn txn(account: &str, date: &str, amount: f64) -> TransactionInput {
    TransactionInput { account_id: account.into(), date: date.into(), amount, ..Default::default() }
}

#[test]
fn new_budget_starter_categories() {
    let user_groups = |pkg: &BudgetPackage| -> Vec<(String, Vec<String>)> {
        let mut masters: Vec<_> = pkg.store.collection("masterCategories").into_iter().filter(|m| !m.id().starts_with("MasterCategory/__")).collect();
        masters.sort_by(|a, b| a.f("sortableIndex").total_cmp(&b.f("sortableIndex")));
        masters
            .iter()
            .map(|m| {
                let mut subs = pkg.store.children(m.id(), "subCategories");
                subs.sort_by(|a, b| a.f("sortableIndex").total_cmp(&b.f("sortableIndex")));
                (m.str_of("name").unwrap().to_string(), subs.iter().map(|s| s.str_of("name").unwrap().to_string()).collect())
            })
            .collect()
    };
    let expect = |set: &[(&str, &[&str])]| -> Vec<(String, Vec<String>)> {
        set.iter().map(|(m, subs)| (m.to_string(), subs.iter().map(|s| s.to_string()).collect())).collect()
    };
    let budget_type = |pkg: &BudgetPackage| pkg.store.budget_meta().and_then(|m| m.str_of("budgetType")).map(str::to_string);

    for (starter, set, kind) in [
        (Starter::Personal, PERSONAL_CATEGORIES, "Personal"),
        (Starter::Business, BUSINESS_CATEGORIES, "Business"),
        (Starter::None, &[][..], "Personal"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let pkg = create_budget_with(dir.path(), "Starter", "en_US", starter, opts("Laptop", None)).unwrap();
        assert_eq!(user_groups(&pkg), expect(set), "{starter:?}");
        assert_eq!(budget_type(&pkg).as_deref(), Some(kind));
        // Reopening reads back the same groups, in order.
        let again = BudgetPackage::open(&pkg.folder, opts("Laptop", Some(&pkg.device.device_guid))).unwrap();
        assert_eq!(user_groups(&again), expect(set));
    }
    assert_eq!(PERSONAL_CATEGORIES.len(), 6);
    assert_eq!(PERSONAL_CATEGORIES.iter().map(|(_, s)| s.len()).sum::<usize>(), 29);
    assert_eq!(BUSINESS_CATEGORIES.iter().map(|(_, s)| s.len()).sum::<usize>(), 20);
}

#[test]
fn new_budget_has_ynab_layout_and_reopens_as_same_device() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = create_budget(dir.path(), "Test Budget", "en_US", opts("Laptop", None)).unwrap();
    assert_eq!(pkg.device.short_device_id, "A");
    assert!(pkg.folder.join("Budget.ymeta").is_file());
    assert!(pkg.data_folder.join("devices").join("A.ydevice").is_file());
    assert!(pkg.full_file_path().is_file());
    let ids: Vec<&str> = pkg.store.collection("masterCategories").iter().map(|m| m.id()).collect();
    assert!(ids.contains(&builtin::HIDDEN_MASTER) && ids.contains(&builtin::INCOME_MASTER));
    let again = BudgetPackage::open(&pkg.folder, opts("Laptop", Some(&pkg.device.device_guid))).unwrap();
    assert_eq!(again.device.short_device_id, "A");
    assert_eq!(again.store.len(), pkg.store.len());
}

#[test]
fn transfers_splits_budgets_and_balances() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Edits", "en_US", opts("Laptop", None)).unwrap();
    let groceries = category(&pkg.store, "Everyday Expenses", "Groceries");
    let car = category(&pkg.store, "Debt", "Car Payment");
    let mut ed = Editor::new(&mut pkg.store, 2);
    let checking = ed.create_account(&account("Checking", true, 1000.0, "2026-10-01")).unwrap();
    let savings = ed.create_account(&account("Savings", true, 500.0, "2026-10-01")).unwrap();
    let loan = ed.create_account(&account("Car Loan", false, -8000.0, "2026-10-01")).unwrap();

    // On-budget → on-budget transfer: no category either side; counterpart id is "<id>_T_0".
    let tr = ed.save_transaction(&TransactionInput { transfer_account_id: Some(savings.clone()), ..txn(&checking, "2026-10-05", -200.0) }).unwrap();
    let other = ed.store.get(&tr).unwrap().str_of("transferTransactionId").unwrap().to_string();
    assert_eq!(other, format!("{tr}_T_0"));
    {
        let o = ed.store.get(&other).unwrap();
        assert_eq!(o.str_of("accountId"), Some(savings.as_str()));
        assert_eq!(o.f("amount"), 200.0);
        assert!(o.is_null("categoryId") && ed.store.get(&tr).unwrap().is_null("categoryId"));
    }
    // Editing one side updates the other.
    ed.save_transaction(&TransactionInput { id: Some(tr.clone()), transfer_account_id: Some(savings.clone()), ..txn(&checking, "2026-10-06", -250.0) }).unwrap();
    assert_eq!(ed.store.get(&other).unwrap().f("amount"), 250.0);
    assert_eq!(ed.store.get(&other).unwrap().str_of("date"), Some("2026-10-06"));

    // On-budget → off-budget keeps its category.
    let pay = ed
        .save_transaction(&TransactionInput { transfer_account_id: Some(loan.clone()), category_id: Some(car.clone()), ..txn(&checking, "2026-10-07", -300.0) })
        .unwrap();
    assert_eq!(ed.store.get(&pay).unwrap().str_of("categoryId"), Some(car.as_str()));
    let pay_other = ed.store.get(&pay).unwrap().str_of("transferTransactionId").unwrap().to_string();
    assert!(ed.store.get(&pay_other).unwrap().is_null("categoryId"));

    // A split with a transfer line.
    let split = ed
        .save_transaction(&TransactionInput {
            payee_name: Some("Market".into()),
            splits: Some(vec![
                SplitInput { amount: -100.0, category_id: Some(groceries.clone()), ..Default::default() },
                SplitInput { amount: -50.0, transfer_account_id: Some(savings.clone()), ..Default::default() },
            ]),
            ..txn(&checking, "2026-10-08", -150.0)
        })
        .unwrap();
    assert_eq!(ed.store.get(&split).unwrap().str_of("categoryId"), Some(builtin::SPLIT));
    let lines = ed.store.children(&split, "subTransactions");
    assert_eq!(lines.len(), 2);
    let line_far = lines.iter().find(|l| !l.is_null("targetAccountId")).unwrap().str_of("transferTransactionId").unwrap().to_string();
    assert_eq!(ed.store.get(&line_far).unwrap().f("amount"), 50.0);

    ed.set_budgeted("2026-10", &groceries, 400.0).unwrap();
    ed.set_budgeted("2026-10", &car, 300.0).unwrap();
    let model = compute_budget(ed.store, Some("2026-12"), 2);
    let oct = &model.by_month["2026-10"];
    assert_eq!(oct.summary.supplemental_income, 1500.0);
    assert_eq!(oct.summary.total_budgeted, 700.0);
    assert_eq!(oct.summary.available, 800.0);
    assert_eq!(oct.categories[&groceries].balance, 300.0);
    assert_eq!(oct.categories[&car].balance, 0.0);
    assert_eq!(model.by_month["2026-11"].categories[&groceries].balance, 300.0);

    let bal = account_balances(ed.store, 2);
    assert_eq!(bal[&checking].working, 1000.0 - 250.0 - 300.0 - 150.0);
    assert_eq!(bal[&savings].working, 500.0 + 250.0 + 50.0);
    assert_eq!(bal[&loan].working, -8000.0 + 300.0);

    ed.delete_transactions(&[tr]);
    assert!(ed.store.get(&other).unwrap().tomb());

    // The edited budget still writes a loadable file.
    pkg.save_full(true).unwrap();
    let reloaded = Store::from_root(read_json(&pkg.full_file_path()).unwrap(), "A");
    assert_eq!(reloaded.len(), pkg.store.len());
}

#[test]
fn scheduled_transactions_enter_with_ynab_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Sched", "en_US", opts("Laptop", None)).unwrap();
    let mut ed = Editor::new(&mut pkg.store, 2);
    let a = ed.create_account(&account("Checking", true, 0.0, "2026-08-01")).unwrap();
    let s = ed
        .save_scheduled(&TransactionInput { payee_name: Some("Gym".into()), frequency: Some("Monthly".into()), ..txn(&a, "2026-08-15", -50.0) })
        .unwrap();
    let entered = ed.enter_due_scheduled("2026-10-20").unwrap();
    let dates: Vec<&str> = entered.iter().map(|id| ed.store.get(id).unwrap().str_of("date").unwrap()).collect();
    assert_eq!(dates, ["2026-08-15", "2026-09-15", "2026-10-15"]);
    assert_eq!(entered[0], format!("{s}_2026-08-15_0"));
    assert!(!ed.store.get(&entered[0]).unwrap().b("accepted"));
    assert_eq!(ed.store.get(&s).unwrap().str_of("date"), Some("2026-11-15"));
}

#[test]
fn two_devices_sync_and_resolve_concurrent_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_budget(dir.path(), "Shared", "en_US", opts("Laptop", None)).unwrap();
    let acct = Editor::new(&mut a.store, 2).create_account(&account("Checking", true, 100.0, "2026-10-01")).unwrap();
    a.save_full(false).unwrap();

    let mut b = BudgetPackage::open(&a.folder, opts("Desktop", None)).unwrap();
    assert_eq!(b.device.short_device_id, "B");
    assert!(b.store.get(&acct).is_some());
    let t = Editor::new(&mut b.store, 2)
        .save_transaction(&TransactionInput { payee_name: Some("Coffee".into()), ..txn(&acct, "2026-10-02", -20.0) })
        .unwrap();
    b.save_full(false).unwrap();
    let diff_path = fs::read_dir(b.my_device_folder()).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "ydiff")).unwrap();
    let diff = parse(&fs::read_to_string(&diff_path).unwrap()).unwrap();
    assert_eq!(diff["shortDeviceId"], "B");
    let mut types: Vec<&str> = diff["items"].as_array().unwrap().iter().map(|i| i["entityType"].as_str().unwrap()).collect();
    types.sort();
    assert_eq!(types, ["payee", "transaction"]);

    assert_eq!(a.sync().unwrap().diffs_imported, 1);
    assert_eq!(a.store.get(&t).unwrap().f("amount"), -20.0);

    // Both edit the same transaction before syncing: device A (lower letter) wins.
    let payee = a.store.get(&t).unwrap().str_of("payeeId").map(str::to_string);
    for (pkg, amount) in [(&mut a, -21.0), (&mut b, -22.0)] {
        Editor::new(&mut pkg.store, 2)
            .save_transaction(&TransactionInput { id: Some(t.clone()), payee_id: payee.clone(), ..txn(&acct, "2026-10-02", amount) })
            .unwrap();
        pkg.publish().unwrap();
    }
    let conflicts = b.sync().unwrap().conflicts.len() + a.sync().unwrap().conflicts.len();
    assert!(conflicts > 0);
    a.sync().unwrap();
    b.sync().unwrap();
    assert_eq!(a.store.get(&t).unwrap().f("amount"), -21.0);
    assert_eq!(b.store.get(&t).unwrap().f("amount"), -21.0);
    assert_eq!(a.store.knowledge.to_string(), b.store.knowledge.to_string());
}

fn starting_category(store: &Store, account_id: &str) -> Option<String> {
    let t = store.collection("transactions").into_iter().find(|t| t.str_of("accountId") == Some(account_id)).unwrap();
    t.str_of("categoryId").map(str::to_string)
}

#[test]
fn credit_cards_get_a_pre_ynab_debt_category() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Cards", "en_US", opts("Laptop", None)).unwrap();
    let mut ed = Editor::new(&mut pkg.store, 2);
    let card = |name: &str, balance: f64| AccountInput {
        name: name.into(),
        account_type: "CreditCard".into(),
        on_budget: true,
        starting_balance: balance,
        starting_date: Some("2026-10-01".into()),
        note: None,
    };
    let owed = ed.create_account(&card("Visa", -800.0)).unwrap();
    let credit = ed.create_account(&card("Store: Card", 50.0)).unwrap();

    let master = ed.store.get(builtin::PRE_YNAB_DEBT_MASTER).unwrap();
    assert_eq!(master.str_of("name"), Some("Pre-YNAB Debt"));
    let owed_sub = pre_ynab_debt_category_id(&owed);
    assert_eq!(ed.store.get(&owed_sub).unwrap().str_of("name"), Some("Visa"));
    assert_eq!(ed.store.get(&owed_sub).unwrap().str_of("masterCategoryId"), Some(builtin::PRE_YNAB_DEBT_MASTER));

    // Debt it started with is categorized to the debt; a positive starting balance is still income.
    assert_eq!(starting_category(&ed.store, &owed).as_deref(), Some(owed_sub.as_str()));
    assert_eq!(starting_category(&ed.store, &credit).as_deref(), Some(builtin::IMMEDIATE_INCOME));
    // The category is there for a card in credit too, ready for later debt.
    assert!(ed.store.get(&pre_ynab_debt_category_id(&credit)).is_some());

    // Renaming the card renames its debt category; colons become dashes, as YNAB does.
    ed.update_account(&owed, &AccountUpdate { name: Some("Visa: Gold".into()), ..Default::default() }).unwrap();
    assert_eq!(ed.store.get(&owed_sub).unwrap().str_of("name"), Some("Visa- Gold"));

    // Deleting the card retires its debt category too.
    ed.delete_account(&owed).unwrap();
    assert!(ed.store.get(&owed_sub).unwrap().tomb());
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn joins_a_real_budget_and_publishes_diffs_its_devices_can_merge() {
    let budgets = common::test_budgets();
    require_testdata!(budgets);
    let src = &budgets[0];
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join(src.file_name().unwrap());
    let meta = BudgetPackage::read_meta(src).unwrap();
    let rel = meta.str_of("relativeDataFolderName").unwrap().to_string();
    fs::create_dir_all(&folder).unwrap();
    fs::copy(src.join("Budget.ymeta"), folder.join("Budget.ymeta")).unwrap();
    copy_dir(&src.join(&rel), &folder.join(&rel));

    let mut pkg = BudgetPackage::open(&folder, opts("Test", None)).unwrap();
    let others = pkg.other_devices();
    assert!(others.iter().all(|d| d.short_device_id != pkg.device.short_device_id));
    assert_eq!(pkg.open_report.diffs_blocked, 0);
    let dev = read_json(&pkg.data_folder.join("devices").join(format!("{}.ydevice", pkg.device.short_device_id))).unwrap();
    assert_eq!(dev["hasFullKnowledge"], true);
    assert_eq!(dev["knowledgeInFullBudgetFile"].as_str(), Some(pkg.store.knowledge.to_string().as_str()));

    let model = compute_budget(&pkg.store, None, 2);
    let acct = pkg.store.collection("accounts").into_iter().find(|a| !a.tomb() && a.b("onBudget") && !a.b("hidden")).unwrap().id().to_string();
    let cat = model.tree.iter().find(|n| n.master_id != builtin::HIDDEN_MASTER && !n.sub_ids.is_empty()).unwrap().sub_ids[0].clone();
    let today = budget4_core::dates::today();
    let mut ed = Editor::new(&mut pkg.store, 2);
    let t = ed
        .save_transaction(&TransactionInput {
            payee_name: Some("Budget4 test payee".into()),
            category_id: Some(cat.clone()),
            memo: Some("test".into()),
            ..txn(&acct, &today, -12.34)
        })
        .unwrap();
    ed.set_budgeted(&today[..7], &cat, 123.45).unwrap();
    pkg.save_full(false).unwrap();

    // The device with the most knowledge loads its own full file and imports our diffs.
    let mut best = others.clone();
    best.sort_by(|x, y| VersionVector::compare_for_sort(&VersionVector::parse(y.knowledge_in_full_budget_file.as_deref()), &VersionVector::parse(x.knowledge_in_full_budget_file.as_deref())));
    let best = &best[0];
    let mut theirs = Store::from_root(read_json(&pkg.data_folder.join(&best.device_guid).join("Budget.yfull")).unwrap(), &best.short_device_id);
    let refs: Vec<DiffFileRef> = fs::read_dir(pkg.my_device_folder())
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let (start, end) = parse_diff_filename(&e.file_name().to_string_lossy())?;
            Some(DiffFileRef { path: e.path(), device: pkg.device.short_device_id.clone(), start, end })
        })
        .collect();
    assert!(!refs.is_empty());
    let r = import_diffs(&mut theirs, &refs, None).unwrap();
    assert_eq!(r.diffs_blocked, 0);
    assert!(r.conflicts.is_empty());
    assert_eq!(theirs.get(&t).unwrap().f("amount"), -12.34);
    assert_eq!(theirs.get(&t).unwrap().str_of("memo"), Some("test"));
    assert_eq!(theirs.get(&format!("MCB/{}/{cat}", &today[..7])).unwrap().f("budgeted"), 123.45);

    let again = BudgetPackage::open(&folder, opts("Test", Some(&pkg.device.device_guid))).unwrap();
    assert_eq!(again.device.short_device_id, pkg.device.short_device_id);
    assert_eq!(again.loaded_from, pkg.full_file_path());
    assert_eq!(again.store.get(&t).unwrap().f("amount"), -12.34);
}
