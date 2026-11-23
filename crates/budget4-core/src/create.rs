//! Creates a new budget in YNAB 4's own layout (YNAB 4 can open it).

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::json::num;
use crate::package::{new_guid, write_atomic, BudgetPackage, DeviceRecord, OpenOptions, DEVICE_FORMAT_VERSION};
use crate::schema::{blank_entity, builtin, CURRENT_DATA_VERSION};
use crate::serialize::{serialize_full, serialize_tabbed};
use crate::store::Store;
use crate::{Error, Result};

pub const DEFAULT_CATEGORIES: &[(&str, &[&str])] = &[
    ("Giving", &["Tithing", "Charitable"]),
    ("Monthly Bills", &["Rent/Mortgage", "Phone", "Internet", "Cable TV", "Electricity", "Water", "Natural Gas/Propane/Oil"]),
    ("Everyday Expenses", &["Groceries", "Fuel", "Spending Money", "Restaurants", "Medical/Dental", "Clothing", "Household Goods"]),
    (
        "Rainy Day Funds",
        &[
            "Emergency Fund", "Car Maintenance", "Home Maintenance", "Car Insurance", "Life Insurance", "Health Insurance",
            "Birthdays", "Christmas", "Renter's/Home Insurance", "Retirement",
        ],
    ),
    ("Savings Goals", &["Car Replacement", "Vacation"]),
    ("Debt", &["Car Payment", "Student Loan Payment", "Personal Loan Payment"]),
];

fn hex8() -> String {
    new_guid().replace('-', "")[..8].to_string()
}

fn spread(n: usize) -> Vec<f64> {
    let step = (2147483647.0 / (n as f64 + 1.0)).floor();
    (1..=n).map(|i| step * i as f64).collect()
}

/// Creates "<name>~<hex>.ynab4" in `parent`, registers this machine as device A and opens it.
pub fn create_budget(parent: &Path, name: &str, currency_locale: &str, opts: OpenOptions) -> Result<BudgetPackage> {
    let clean: String = name.trim().chars().map(|c| if r#"\/:*?"<>|~"#.contains(c) { '_' } else { c }).collect();
    let clean = if clean.is_empty() { "My Budget".to_string() } else { clean };
    let folder = parent.join(format!("{clean}~{}.ynab4", hex8()));
    if folder.exists() {
        return Err(Error::Msg(format!("{} already exists", folder.display())));
    }
    let data_name = format!("data1~{}", hex8());
    let guid = opts.device_guid.clone().unwrap_or_else(new_guid);
    let data = folder.join(&data_name);
    fs::create_dir_all(data.join("devices")).map_err(|e| Error::io(&data, e))?;
    fs::create_dir_all(data.join(&guid)).map_err(|e| Error::io(&data, e))?;

    let mut meta = blank_entity("budgetMetaData", &new_guid());
    meta.insert("budgetType".into(), json!("Personal"));
    meta.insert("currencyLocale".into(), json!(currency_locale));
    meta.insert("dateLocale".into(), json!(currency_locale));
    meta.insert("strictBudget".into(), json!("TRUE"));
    let root = json!({
        "scheduledTransactions": [],
        "accountMappings": [],
        "budgetMetaData": Value::Object(meta),
        "accounts": [],
        "masterCategories": [],
        "payees": [],
        "monthlyBudgets": [],
        "transactions": [],
        "fileMetaData": { "entityType": "fileMetaData", "budgetDataVersion": CURRENT_DATA_VERSION, "currentKnowledge": "" }
    });
    let mut store = Store::from_root(root, "A");
    if let Some(id) = store.budget_meta().map(|m| crate::json::ObjExt::id(m).to_string()) {
        store.touch(&id, false);
    }

    for (id, n, t, idx) in [
        (builtin::HIDDEN_MASTER, "Hidden Categories", "OUTFLOW", -1073741824.0),
        (builtin::INCOME_MASTER, "Income", "INFLOW", 1073741823.0),
        (builtin::INTERNAL_MASTER, "__Internal__", "OUTFLOW", 1610612735.0),
    ] {
        let mut m = blank_entity("masterCategory", id);
        m.insert("name".into(), json!(n));
        m.insert("type".into(), json!(t));
        m.insert("sortableIndex".into(), num(idx));
        m.insert("expanded".into(), json!(true));
        m.insert("subCategories".into(), if id == builtin::HIDDEN_MASTER { json!([]) } else { Value::Null });
        store.insert(m, None)?;
    }
    let master_idx: Vec<f64> =
        spread(DEFAULT_CATEGORIES.len()).iter().map(|x| (1879048191.0 + (x / 2147483647.0) * (2147483647.0 - 1879048191.0)).floor()).collect();
    for (i, (master_name, subs)) in DEFAULT_CATEGORIES.iter().enumerate() {
        let mid = new_guid();
        let mut m = blank_entity("masterCategory", &mid);
        m.insert("name".into(), json!(master_name));
        m.insert("type".into(), json!("OUTFLOW"));
        m.insert("sortableIndex".into(), num(master_idx[i]));
        m.insert("deleteable".into(), json!(true));
        m.insert("expanded".into(), json!(true));
        m.insert("subCategories".into(), json!([]));
        store.insert(m, None)?;
        for (j, (sub, idx)) in subs.iter().zip(spread(subs.len())).enumerate() {
            let _ = j;
            let mut c = blank_entity("category", &new_guid());
            c.insert("name".into(), json!(sub));
            c.insert("type".into(), json!("OUTFLOW"));
            c.insert("masterCategoryId".into(), json!(mid));
            c.insert("sortableIndex".into(), num(idx));
            store.insert(c, Some(&mid))?;
        }
    }

    store.sync_file_meta();
    let knowledge = store.knowledge.to_string();
    write_atomic(&data.join(&guid).join("Budget.yfull"), serialize_full(&store).as_bytes())?;
    let device = DeviceRecord {
        format_version: Some(DEVICE_FORMAT_VERSION.into()),
        device_guid: guid.clone(),
        short_device_id: "A".into(),
        ynab_version: Some(format!("Budget4 v{}", opts.app_version)),
        device_type: Some(format!("Desktop (Budget4), OS:{}", std::env::consts::OS)),
        friendly_name: Some(opts.friendly_name.clone()),
        has_full_knowledge: true,
        knowledge: Some(knowledge.clone()),
        knowledge_in_full_budget_file: Some(knowledge),
        last_data_version_fully_known: Some(CURRENT_DATA_VERSION.into()),
        highest_data_version_imported: None,
    };
    let dv = serde_json::to_value(&device).map_err(|e| Error::Msg(e.to_string()))?;
    write_atomic(&data.join("devices").join("A.ydevice"), serialize_tabbed(&dv).as_bytes())?;
    write_atomic(&folder.join("Budget.ymeta"), serialize_tabbed(&json!({"formatVersion": "2", "relativeDataFolderName": data_name})).as_bytes())?;
    write_atomic(
        &folder.join("readme.txt"),
        b"This is a YNAB 4 format budget folder. Keep everything in it together; copy the whole folder to move it.\r\n",
    )?;
    BudgetPackage::open(&folder, OpenOptions { device_guid: Some(guid), backup_on_first_save: false, ..opts })
}
