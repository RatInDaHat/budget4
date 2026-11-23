//! The YNAB 4 on-disk schema (com.ynab.ynabData.vo.*), as observed in Budget.yfull and *.ydiff.

use crate::json::{num, Obj};
use serde_json::Value;

pub const CURRENT_DATA_VERSION: &str = "4.2";

pub mod builtin {
    pub const HIDDEN_MASTER: &str = "MasterCategory/__Hidden__";
    pub const INCOME_MASTER: &str = "MasterCategory/__Income__";
    pub const INTERNAL_MASTER: &str = "MasterCategory/__Internal__";
    pub const IMMEDIATE_INCOME: &str = "Category/__ImmediateIncome__";
    pub const DEFERRED_INCOME: &str = "Category/__DeferredIncome__";
    pub const SPLIT: &str = "Category/__Split__";
}

pub fn is_income_category(id: Option<&str>) -> bool {
    matches!(id, Some(builtin::IMMEDIATE_INCOME) | Some(builtin::DEFERRED_INCOME))
}

pub fn transfer_payee_id(account_id: &str) -> String {
    format!("Payee/Transfer:{account_id}")
}

pub fn monthly_budget_id(month: &str) -> String {
    format!("MB/{}", &month[..7])
}

pub fn monthly_category_budget_id(month: &str, category_id: &str) -> String {
    format!("MCB/{}/{}", &month[..7], category_id)
}

/// Root collections of Budget.yfull: (entityType, key).
pub const ROOT_COLLECTIONS: &[(&str, &str)] = &[
    ("transaction", "transactions"),
    ("scheduledTransaction", "scheduledTransactions"),
    ("account", "accounts"),
    ("masterCategory", "masterCategories"),
    ("payee", "payees"),
    ("monthlyBudget", "monthlyBudgets"),
    ("accountMapping", "accountMappings"),
];

pub fn root_collection_for(etype: &str) -> Option<&'static str> {
    ROOT_COLLECTIONS.iter().find(|(t, _)| *t == etype).map(|(_, k)| *k)
}

pub fn root_type_for(key: &str) -> Option<&'static str> {
    ROOT_COLLECTIONS.iter().find(|(_, k)| *k == key).map(|(t, _)| *t)
}

#[derive(Debug, Clone, Copy)]
pub struct ChildSpec {
    pub parent_field: &'static str,
    pub array_field: &'static str,
}

/// Aggregate children: nested in the snapshot, separate items in diffs (parent array left out).
pub fn aggregate_child(etype: &str) -> Option<ChildSpec> {
    Some(match etype {
        "category" => ChildSpec { parent_field: "masterCategoryId", array_field: "subCategories" },
        "monthlyCategoryBudget" => ChildSpec { parent_field: "parentMonthlyBudgetId", array_field: "monthlySubCategoryBudgets" },
        "payeeStringCondition" => ChildSpec { parent_field: "parentPayeeId", array_field: "renameConditions" },
        "location" => ChildSpec { parent_field: "parentPayeeId", array_field: "locations" },
        _ => return None,
    })
}

/// Inline children: travel inside their parent; changing one re-versions the parent.
pub fn inline_child(etype: &str) -> Option<ChildSpec> {
    Some(match etype {
        "subTransaction" => ChildSpec { parent_field: "parentTransactionId", array_field: "subTransactions" },
        "matchedTransaction" => ChildSpec { parent_field: "parentTransactionIdIfMatched", array_field: "matchedTransactions" },
        _ => return None,
    })
}

pub fn child_spec(etype: &str) -> Option<ChildSpec> {
    aggregate_child(etype).or_else(|| inline_child(etype))
}

/// Array properties that hold child records, per entity type.
pub fn child_array_fields(etype: &str) -> &'static [&'static str] {
    match etype {
        "transaction" | "scheduledTransaction" => &["subTransactions", "matchedTransactions"],
        "matchedTransaction" => &["subTransactions"],
        "masterCategory" => &["subCategories"],
        "monthlyBudget" => &["monthlySubCategoryBudgets"],
        "payee" => &["renameConditions", "locations"],
        _ => &[],
    }
}

pub fn is_inline_array(field: &str) -> bool {
    field == "subTransactions" || field == "matchedTransactions"
}

/// Array properties excluded when the aggregate root is written into a diff.
pub fn aggregate_child_properties(etype: &str) -> &'static [&'static str] {
    match etype {
        "masterCategory" => &["subCategories"],
        "monthlyBudget" => &["monthlySubCategoryBudgets"],
        "payee" => &["renameConditions", "locations"],
        _ => &[],
    }
}

const SYNCABLE: &[&str] = &["entityType", "entityId", "isTombstone", "entityVersion", "madeWithKnowledge", "isResolvedConflict"];
const BASE_TXN: &[&str] = &["checkNumber", "memo", "targetAccountId", "transferTransactionId", "categoryId", "amount"];
const MATCHABLE: &[&str] = &[
    "date", "dateEnteredFromSchedule", "importedPayee", "payeeId", "subTransactions", "cleared", "FITID",
    "parentTransactionIdIfMatched", "flag", "accountId", "source", "accepted", "YNABID",
];

/// Every serializable field of each VO class (diff items carry all of them, nulls included).
pub fn entity_fields(etype: &str) -> Option<Vec<&'static str>> {
    let mut f: Vec<&'static str> = SYNCABLE.to_vec();
    let extra: &[&[&'static str]] = match etype {
        "transaction" => &[BASE_TXN, MATCHABLE, &["matchedTransactions"]],
        "matchedTransaction" => &[BASE_TXN, MATCHABLE],
        "scheduledTransaction" => &[BASE_TXN, MATCHABLE, &["matchedTransactions", "frequency", "twiceAMonthStartDay"]],
        "subTransaction" => &[BASE_TXN, &["parentTransactionId"]],
        "account" => &[&[
            "sortableIndex", "note", "accountName", "hidden", "lastReconciledDate", "lastReconciledBalance",
            "lastEnteredCheckNumber", "onBudget", "accountType",
        ]],
        "masterCategory" => &[&["type", "name", "note", "sortableIndex", "subCategories", "deleteable", "expanded"]],
        "category" => &[&["type", "name", "note", "sortableIndex", "cachedBalance", "masterCategoryId"]],
        "payee" => &[&[
            "renameConditions", "locations", "targetAccountId", "name", "enabled", "autoFillCategoryId", "autoFillMemo",
            "autoFillAmount",
        ]],
        "location" => &[&["latitude", "longitude", "parentPayeeId"]],
        "payeeStringCondition" => &[&["operator", "operand", "parentPayeeId"]],
        "monthlyBudget" => &[&["monthlySubCategoryBudgets", "month", "note"]],
        "monthlyCategoryBudget" => &[&["note", "budgeted", "overspendingHandling", "categoryId", "parentMonthlyBudgetId"]],
        "accountMapping" => &[&[
            "fid", "shortenedAccountId", "hash", "salt", "targetYNABAccountId", "dateSequence", "shouldFlipPayeesMemos",
            "shouldImportMemos", "skipImport",
        ]],
        "budgetMetaData" => &[&["budgetType", "currencyISOSymbol", "currencyLocale", "dateLocale", "strictBudget"]],
        _ => return None,
    };
    for part in extra {
        f.extend_from_slice(part);
    }
    Some(f)
}

/// Defaults for fields that aren't nullable in AS3 (Boolean / Number / int).
pub fn field_default(field: &str) -> Value {
    match field {
        "isTombstone" | "isResolvedConflict" | "accepted" | "hidden" | "onBudget" | "enabled" | "deleteable" | "expanded"
        | "shouldFlipPayeesMemos" | "shouldImportMemos" | "skipImport" => Value::Bool(false),
        "amount" | "budgeted" | "sortableIndex" | "cachedBalance" | "autoFillAmount" | "lastReconciledBalance"
        | "lastEnteredCheckNumber" | "twiceAMonthStartDay" | "latitude" | "longitude" => num(0.0),
        _ => Value::Null,
    }
}

/// Fields Budget.yfull leaves out when false or null (YNABVOToJSONBridge._fieldsNotToWriteIfFalseOrNull).
pub fn omit_if_false_or_null(field: &str) -> bool {
    matches!(
        field,
        "isTombstone" | "FID" | "FITID" | "YNABID" | "source" | "checkNumber" | "cleared" | "dateEnteredFromSchedule"
            | "targetAccountId" | "transferTransactionId" | "importedPayee" | "flag" | "payeeId" | "memo"
            | "subTransactions" | "matchedTransactions" | "parentTransactionIdIfMatched" | "bankAccountId" | "note"
            | "priorBalance" | "madeWithKnowledge" | "isResolvedConflict"
    )
}

/// A freshly constructed VO: every declared field with AS3 defaults.
pub fn blank_entity(etype: &str, id: &str) -> Obj {
    let mut o = Obj::new();
    for f in entity_fields(etype).unwrap_or_else(|| SYNCABLE.to_vec()) {
        o.insert(f.to_string(), field_default(f));
    }
    o.insert("entityType".into(), Value::String(etype.into()));
    o.insert("entityId".into(), Value::String(id.into()));
    o.insert("entityVersion".into(), Value::Null);
    o
}

/// Values of obj[key] as a mutable builder: `set(&mut o, "name", "x")`.
pub fn set<V: Into<Value>>(o: &mut Obj, key: &str, v: V) {
    o.insert(key.to_string(), v.into());
}
