//! Edits with YNAB 4's entity-layer semantics (data.Account, data.Transaction, TransactionList,
//! ScheduledTransaction, MonthlyBudget …). Every change is stamped through Store so it syncs.

use serde::Deserialize;
use serde_json::Value;

use crate::budget::Rounder;
use crate::dates::{add_months, month_key, next_occurrence, today};
use crate::json::{num, opt_str, Obj, ObjExt};
use crate::package::new_guid;
use crate::schema::{blank_entity, builtin, monthly_budget_id, monthly_category_budget_id, pre_ynab_debt_category_id, transfer_payee_id};
use crate::store::Store;
use crate::{Error, Result};

const MAX_INT: f64 = 2147483647.0;

/// Sortable index between two neighbours (YNAB spreads indices across the int range).
pub fn index_between(before: Option<f64>, after: Option<f64>) -> f64 {
    let lo = before.unwrap_or(-1073741824.0);
    let hi = after.unwrap_or(MAX_INT);
    if hi - lo < 2.0 {
        lo + 1.0
    } else {
        (lo + (hi - lo) / 2.0).floor()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitInput {
    pub id: Option<String>,
    pub amount: f64,
    pub category_id: Option<String>,
    pub memo: Option<String>,
    pub transfer_account_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionInput {
    pub id: Option<String>,
    pub account_id: String,
    pub date: String,
    pub amount: f64,
    pub payee_id: Option<String>,
    /// Creates (or reuses by name) a payee when payee_id isn't given.
    pub payee_name: Option<String>,
    pub transfer_account_id: Option<String>,
    pub category_id: Option<String>,
    pub memo: Option<String>,
    pub cleared: Option<String>,
    pub flag: Option<String>,
    pub check_number: Option<String>,
    pub accepted: Option<bool>,
    pub splits: Option<Vec<SplitInput>>,
    /// Scheduled transactions only.
    pub frequency: Option<String>,
    pub twice_a_month_start_day: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: String,
    pub on_budget: bool,
    #[serde(default)]
    pub starting_balance: f64,
    pub starting_date: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUpdate {
    pub name: Option<String>,
    pub hidden: Option<bool>,
    pub note: Option<Option<String>>,
    pub on_budget: Option<bool>,
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Msg(m.into()))
}

pub struct Editor<'a> {
    pub store: &'a mut Store,
    rd: Rounder,
}

impl<'a> Editor<'a> {
    pub fn new(store: &'a mut Store, decimal_digits: u32) -> Self {
        Editor { store, rd: Rounder::new(decimal_digits) }
    }

    pub(crate) fn r(&self, x: f64) -> f64 {
        self.rd.r(x)
    }

    pub(crate) fn live(&self, id: Option<&str>) -> Option<&Obj> {
        self.store.live(id)
    }

    pub(crate) fn is_live(&self, id: Option<&str>) -> bool {
        self.live(id).is_some()
    }

    pub(crate) fn require(&self, id: &str, etype: &str, what: &str) -> Result<()> {
        match self.live(Some(id)) {
            Some(o) if o.etype() == etype => Ok(()),
            _ => err(format!("No such {what} {id}")),
        }
    }

    pub(crate) fn update(&mut self, id: &str, fields: Vec<(&str, Value)>) -> bool {
        self.store.update(id, fields)
    }

    pub(crate) fn insert(&mut self, o: Obj, parent: Option<&str>) -> Result<String> {
        self.store.insert(o, parent).map_err(Error::Msg)
    }

    fn sorted_live(&self, key: &str) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> =
            self.store.collection(key).into_iter().filter(|o| !o.tomb()).map(|o| (o.id().to_string(), o.f("sortableIndex"))).collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v
    }

    // ------------------------------------------------------------------ payees

    pub fn find_payee_by_name(&self, name: &str) -> Option<String> {
        let n = name.trim().to_lowercase();
        self.store
            .collection("payees")
            .into_iter()
            .find(|p| !p.tomb() && p.is_null("targetAccountId") && p.str_of("name").unwrap_or("").trim().to_lowercase() == n)
            .map(|p| p.id().to_string())
    }

    pub fn get_or_create_payee(&mut self, name: &str, enabled: bool) -> Result<String> {
        if let Some(id) = self.find_payee_by_name(name) {
            return Ok(id);
        }
        let mut p = blank_entity("payee", &new_guid());
        p.insert("name".into(), s(name.trim()));
        p.insert("enabled".into(), Value::Bool(enabled));
        self.insert(p, None)
    }

    pub fn rename_payee(&mut self, id: &str, name: &str) -> Result<()> {
        let Some(p) = self.live(Some(id)) else { return err(format!("No such payee {id}")) };
        if !p.is_null("targetAccountId") {
            return err("Transfer payees are named after their account");
        }
        self.update(id, vec![("name", s(name.trim()))]);
        Ok(())
    }

    /// Moves every transaction from `from` to `into` and deletes the merged payees.
    pub fn merge_payees(&mut self, from: &[String], into: &str) -> Result<()> {
        if !self.is_live(Some(into)) {
            return err(format!("No such payee {into}"));
        }
        let from: Vec<&String> = from.iter().filter(|f| *f != into).collect();
        for key in ["transactions", "scheduledTransactions"] {
            let ids: Vec<String> = self
                .store
                .collection(key)
                .into_iter()
                .filter(|t| !t.tomb() && t.str_of("payeeId").is_some_and(|p| from.iter().any(|f| *f == p)))
                .map(|t| t.id().to_string())
                .collect();
            for id in ids {
                self.update(&id, vec![("payeeId", s(into))]);
            }
        }
        for id in from {
            if self.live(Some(id)).is_some_and(|p| p.is_null("targetAccountId")) {
                self.update(id, vec![("isTombstone", Value::Bool(true))]);
            }
        }
        Ok(())
    }

    fn update_autofill(&mut self, payee_id: &str, txn_id: &str) {
        let Some(p) = self.live(Some(payee_id)) else { return };
        if !p.is_null("targetAccountId") {
            return;
        }
        let prev_cat = p.get("autoFillCategoryId").cloned().unwrap_or(Value::Null);
        let Some(t) = self.store.get(txn_id) else { return };
        let cat = if t.str_of("categoryId") == Some(builtin::SPLIT) { prev_cat } else { t.get("categoryId").cloned().unwrap_or(Value::Null) };
        let fields = vec![
            ("autoFillCategoryId", cat),
            ("autoFillAmount", num(t.f("amount"))),
            ("autoFillMemo", t.get("memo").cloned().unwrap_or(Value::Null)),
        ];
        self.update(payee_id, fields);
    }

    // ------------------------------------------------------------------ accounts

    /// YNAB 4's name for an account's Pre-YNAB debt subcategory (colons aren't allowed in it).
    fn pre_ynab_debt_name(account_name: &str) -> String {
        account_name.trim().replace(':', "-")
    }

    /// Makes sure the "Pre-YNAB Debt" group and this account's subcategory exist (and are live).
    fn ensure_pre_ynab_debt(&mut self, account_id: &str, account_name: &str) -> Result<String> {
        let master = builtin::PRE_YNAB_DEBT_MASTER;
        match self.store.get(master).map(|o| o.tomb()) {
            None => {
                let last = self.user_masters().last().map(|x| x.1);
                let mut m = blank_entity("masterCategory", master);
                m.insert("name".into(), s("Pre-YNAB Debt"));
                m.insert("type".into(), s("OUTFLOW"));
                m.insert("sortableIndex".into(), num(index_between(last, None)));
                m.insert("deleteable".into(), Value::Bool(false));
                m.insert("expanded".into(), Value::Bool(true));
                m.insert("subCategories".into(), Value::Array(vec![]));
                self.insert(m, None)?;
            }
            Some(true) => {
                self.update(master, vec![("isTombstone", Value::Bool(false))]);
            }
            Some(false) => {}
        }
        let sub = pre_ynab_debt_category_id(account_id);
        match self.store.get(&sub).map(|o| o.tomb()) {
            None => {
                let last = self.live_subs(master, None).last().map(|x| x.1);
                let mut c = blank_entity("category", &sub);
                c.insert("name".into(), s(&Self::pre_ynab_debt_name(account_name)));
                c.insert("type".into(), s("OUTFLOW"));
                c.insert("masterCategoryId".into(), s(master));
                c.insert("sortableIndex".into(), num(index_between(last, None)));
                self.insert(c, None)?;
            }
            Some(true) => {
                self.update(&sub, vec![("isTombstone", Value::Bool(false))]);
            }
            Some(false) => {}
        }
        Ok(sub)
    }

    pub fn create_account(&mut self, input: &AccountInput) -> Result<String> {
        let last = self.sorted_live("accounts").last().map(|x| x.1);
        let id = new_guid();
        let mut a = blank_entity("account", &id);
        a.insert("accountName".into(), s(input.name.trim()));
        a.insert("accountType".into(), s(&input.account_type));
        a.insert("onBudget".into(), Value::Bool(input.on_budget));
        a.insert("sortableIndex".into(), num(index_between(last, None)));
        a.insert("note".into(), opt_str(input.note.as_deref().filter(|n| !n.is_empty())));
        a.insert("lastEnteredCheckNumber".into(), num(-1.0));
        self.insert(a, None)?;

        // A budgeted credit card gets a Pre-YNAB debt category. Debt it started with (a zero or
        // negative starting balance) goes there, so it's budgeted to pay off rather than counted as income.
        let amount = self.r(input.starting_balance);
        let category = if input.on_budget && input.account_type == "CreditCard" {
            let debt = self.ensure_pre_ynab_debt(&id, &input.name)?;
            if amount <= 0.0 { Some(debt) } else { Some(builtin::IMMEDIATE_INCOME.to_string()) }
        } else if input.on_budget {
            Some(builtin::IMMEDIATE_INCOME.to_string())
        } else {
            None
        };

        let tp_id = transfer_payee_id(&id);
        if !self.store.contains(&tp_id) {
            let mut tp = blank_entity("payee", &tp_id);
            tp.insert("name".into(), s(&format!("Transfer : {}", input.name.trim())));
            tp.insert("targetAccountId".into(), s(&id));
            self.insert(tp, None)?;
        }
        let sb = self.get_or_create_payee("Starting Balance", false)?;
        let t = self.new_transaction(
            &new_guid(),
            vec![
                ("accountId", s(&id)),
                ("date", s(input.starting_date.as_deref().unwrap_or(&today()))),
                ("amount", num(amount)),
                ("payeeId", s(&sb)),
                ("categoryId", category.as_deref().map_or(Value::Null, s)),
                ("cleared", s("Cleared")),
            ],
        );
        self.insert(t, None)?;
        Ok(id)
    }

    pub fn update_account(&mut self, id: &str, f: &AccountUpdate) -> Result<()> {
        self.require(id, "account", "account")?;
        if let Some(ob) = f.on_budget {
            let current = self.store.get(id).unwrap().b("onBudget");
            let has_txns = self.store.collection("transactions").iter().any(|t| !t.tomb() && t.str_of("accountId") == Some(id));
            if ob != current && has_txns {
                return err("An account's budget status can't change once it has transactions");
            }
        }
        let mut fields = Vec::new();
        if let Some(n) = &f.name {
            fields.push(("accountName", s(n.trim())));
        }
        if let Some(h) = f.hidden {
            fields.push(("hidden", Value::Bool(h)));
        }
        if let Some(n) = &f.note {
            fields.push(("note", opt_str(n.as_deref().filter(|x| !x.is_empty()))));
        }
        if let Some(ob) = f.on_budget {
            fields.push(("onBudget", Value::Bool(ob)));
        }
        self.update(id, fields);
        if let Some(n) = &f.name {
            let tp = transfer_payee_id(id);
            if self.is_live(Some(&tp)) {
                self.update(&tp, vec![("name", s(&format!("Transfer : {}", n.trim())))]);
            }
            let debt = pre_ynab_debt_category_id(id);
            if self.is_live(Some(&debt)) {
                self.update(&debt, vec![("name", s(&Self::pre_ynab_debt_name(n)))]);
            }
        }
        Ok(())
    }

    pub fn move_account(&mut self, id: &str, before_id: Option<&str>) -> Result<()> {
        self.require(id, "account", "account")?;
        let list: Vec<(String, f64)> = self.sorted_live("accounts").into_iter().filter(|x| x.0 != id).collect();
        let i = before_id.and_then(|b| list.iter().position(|x| x.0 == b)).unwrap_or(list.len());
        let idx = index_between(i.checked_sub(1).map(|j| list[j].1), list.get(i).map(|x| x.1));
        self.update(id, vec![("sortableIndex", num(idx))]);
        Ok(())
    }

    pub fn delete_account(&mut self, id: &str) -> Result<()> {
        self.require(id, "account", "account")?;
        let txns: Vec<String> =
            self.store.collection("transactions").iter().filter(|t| !t.tomb() && t.str_of("accountId") == Some(id)).map(|t| t.id().to_string()).collect();
        for t in txns {
            self.delete_transaction_internal(&t);
        }
        let sched: Vec<String> =
            self.store.collection("scheduledTransactions").iter().filter(|t| !t.tomb() && t.str_of("accountId") == Some(id)).map(|t| t.id().to_string()).collect();
        for t in sched {
            self.update(&t, vec![("isTombstone", Value::Bool(true))]);
        }
        let tp = transfer_payee_id(id);
        if self.is_live(Some(&tp)) {
            self.update(&tp, vec![("isTombstone", Value::Bool(true))]);
        }
        let debt = pre_ynab_debt_category_id(id);
        if self.is_live(Some(&debt)) {
            self.update(&debt, vec![("isTombstone", Value::Bool(true))]);
        }
        self.update(id, vec![("isTombstone", Value::Bool(true))]);
        Ok(())
    }

    // ------------------------------------------------------------------ transactions

    pub(crate) fn new_transaction(&self, id: &str, fields: Vec<(&str, Value)>) -> Obj {
        let mut t = blank_entity("transaction", id);
        t.insert("cleared".into(), s("Uncleared"));
        t.insert("accepted".into(), Value::Bool(true));
        for (k, v) in fields {
            t.insert(k.into(), v);
        }
        t
    }

    fn resolve_payee(&mut self, input: &TransactionInput) -> Result<Option<String>> {
        if let Some(a) = &input.transfer_account_id {
            return Ok(Some(transfer_payee_id(a)));
        }
        if let Some(p) = input.payee_id.as_deref().filter(|p| !p.is_empty()) {
            return Ok(Some(p.to_string()));
        }
        match input.payee_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => Ok(Some(self.get_or_create_payee(n, true)?)),
            None => Ok(None),
        }
    }

    pub(crate) fn on_budget(&self, account: &str) -> bool {
        self.store.get(account).is_some_and(|a| a.b("onBudget"))
    }

    /// The category the on-budget side of a transfer keeps; on-budget ↔ on-budget transfers have none.
    fn transfer_category(&self, from: &str, to: &str, category: Option<&str>) -> Value {
        if self.on_budget(from) && !self.on_budget(to) {
            opt_str(category)
        } else {
            Value::Null
        }
    }

    fn transfer_id_for(&self, source: &str) -> String {
        (0..).map(|n| format!("{source}_T_{n}")).find(|id| !self.store.contains(id)).unwrap()
    }

    /// Creates or updates a transaction, keeping transfer counterparts and splits consistent.
    pub fn save_transaction(&mut self, input: &TransactionInput) -> Result<String> {
        self.require(&input.account_id, "account", "account")?;
        if let Some(t) = &input.transfer_account_id {
            self.require(t, "account", "account")?;
            if *t == input.account_id {
                return err("A transfer can't target its own account");
            }
        }
        let payee = self.resolve_payee(input)?;
        let amount = self.r(input.amount);
        let splits = input.splits.as_ref().filter(|s| !s.is_empty());
        if let Some(sp) = splits {
            let total = self.r(sp.iter().map(|x| x.amount).sum());
            if total != amount {
                return err(format!("Split amounts ({total}) must add up to the transaction amount ({amount})"));
            }
        }
        let target = input.transfer_account_id.as_deref();
        let category = if splits.is_some() {
            s(builtin::SPLIT)
        } else if let Some(t) = target {
            self.transfer_category(&input.account_id, t, input.category_id.as_deref())
        } else {
            opt_str(input.category_id.as_deref())
        };

        let existing = input.id.as_deref().filter(|id| self.live(Some(id)).is_some_and(|o| o.etype() == "transaction"));
        let id = match existing {
            Some(id) => {
                let cleared = input.cleared.clone().or_else(|| self.store.get(id).and_then(|t| t.str_of("cleared")).map(str::to_string));
                self.update(
                    id,
                    vec![
                        ("accountId", s(&input.account_id)),
                        ("date", s(&input.date)),
                        ("amount", num(amount)),
                        ("payeeId", opt_str(payee.as_deref())),
                        ("categoryId", category),
                        ("memo", opt_str(input.memo.as_deref())),
                        ("cleared", s(cleared.as_deref().unwrap_or("Uncleared"))),
                        ("flag", opt_str(input.flag.as_deref())),
                        ("checkNumber", opt_str(input.check_number.as_deref())),
                        ("accepted", Value::Bool(input.accepted.unwrap_or(true))),
                        ("targetAccountId", opt_str(target)),
                    ],
                );
                id.to_string()
            }
            None => {
                let id = input.id.clone().unwrap_or_else(new_guid);
                let t = self.new_transaction(
                    &id,
                    vec![
                        ("accountId", s(&input.account_id)),
                        ("date", s(&input.date)),
                        ("amount", num(amount)),
                        ("payeeId", opt_str(payee.as_deref())),
                        ("categoryId", category),
                        ("memo", opt_str(input.memo.as_deref())),
                        ("cleared", s(input.cleared.as_deref().unwrap_or("Uncleared"))),
                        ("flag", opt_str(input.flag.as_deref())),
                        ("checkNumber", opt_str(input.check_number.as_deref())),
                        ("accepted", Value::Bool(input.accepted.unwrap_or(true))),
                        ("targetAccountId", opt_str(target)),
                    ],
                );
                self.insert(t, None)?
            }
        };
        self.sync_transfer(&id)?;
        self.sync_splits(&id, splits.map(|v| v.as_slice()))?;
        if let (Some(p), None) = (payee.as_deref(), target) {
            self.update_autofill(p, &id);
        }
        Ok(id)
    }

    /// Creates, updates or removes the other side of a (non-split) transfer.
    fn sync_transfer(&mut self, id: &str) -> Result<()> {
        let t = self.store.get(id).unwrap().clone();
        let other_id = t.str_of("transferTransactionId").map(str::to_string);
        let other_live = self.live(other_id.as_deref()).map(|o| (o.str_of("accountId").map(str::to_string), o.str_of("categoryId").map(str::to_string)));
        let Some(target) = t.str_of("targetAccountId").map(str::to_string) else {
            if other_live.is_some() {
                self.update(other_id.as_deref().unwrap(), vec![("isTombstone", Value::Bool(true))]);
            }
            if other_id.is_some() {
                self.update(id, vec![("transferTransactionId", Value::Null)]);
            }
            return Ok(());
        };
        let account = t.str_of("accountId").unwrap_or("").to_string();
        let other_cat = other_live.as_ref().and_then(|x| x.1.clone());
        let fields = vec![
            ("accountId", s(&target)),
            ("targetAccountId", s(&account)),
            ("payeeId", s(&transfer_payee_id(&account))),
            ("date", t.get("date").cloned().unwrap_or(Value::Null)),
            ("amount", num(self.r(-t.f("amount")))),
            ("memo", t.get("memo").cloned().unwrap_or(Value::Null)),
            ("transferTransactionId", s(id)),
            ("categoryId", self.transfer_category(&target, &account, other_cat.as_deref())),
            ("dateEnteredFromSchedule", t.get("dateEnteredFromSchedule").cloned().unwrap_or(Value::Null)),
            ("isTombstone", Value::Bool(false)),
        ];
        match (&other_id, &other_live) {
            (Some(o), Some((Some(acct), _))) if *acct == target => {
                self.update(o, fields);
            }
            _ => {
                if let (Some(o), Some(_)) = (&other_id, &other_live) {
                    self.update(o, vec![("isTombstone", Value::Bool(true))]);
                }
                let mut f = fields;
                f.push(("cleared", s("Uncleared")));
                f.push(("accepted", Value::Bool(t.b("accepted") || t.is_null("accepted"))));
                let new_id = self.transfer_id_for(id);
                let n = self.new_transaction(&new_id, f);
                self.insert(n, None)?;
                self.update(id, vec![("transferTransactionId", s(&new_id))]);
            }
        }
        Ok(())
    }

    fn sync_splits(&mut self, id: &str, splits: Option<&[SplitInput]>) -> Result<()> {
        let current: Vec<String> = self.store.children(id, "subTransactions").iter().filter(|x| !x.tomb()).map(|x| x.id().to_string()).collect();
        let Some(splits) = splits else {
            for sid in current {
                self.remove_split_line(&sid);
            }
            return Ok(());
        };
        let keep: Vec<&str> = splits.iter().filter_map(|x| x.id.as_deref()).collect();
        for sid in &current {
            if !keep.contains(&sid.as_str()) {
                self.remove_split_line(sid);
            }
        }
        let account = self.store.get(id).unwrap().str_of("accountId").unwrap_or("").to_string();
        for input in splits {
            let target = input.transfer_account_id.as_deref();
            let category = match target {
                Some(t) => self.transfer_category(&account, t, input.category_id.as_deref()),
                None => opt_str(input.category_id.as_deref()),
            };
            let fields = vec![
                ("amount", num(self.r(input.amount))),
                ("categoryId", category),
                ("memo", opt_str(input.memo.as_deref())),
                ("targetAccountId", opt_str(target)),
            ];
            let sid = match input.id.as_deref().filter(|i| current.iter().any(|c| c == i)) {
                Some(sid) => {
                    self.update(sid, fields);
                    sid.to_string()
                }
                None => {
                    let mut sub = blank_entity("subTransaction", &new_guid());
                    for (k, v) in fields {
                        sub.insert(k.into(), v);
                    }
                    sub.insert("parentTransactionId".into(), s(id));
                    self.insert(sub, Some(id))?
                }
            };
            self.sync_split_transfer(id, &sid)?;
        }
        Ok(())
    }

    fn remove_split_line(&mut self, sid: &str) {
        let other = self.store.get(sid).and_then(|x| x.str_of("transferTransactionId")).map(str::to_string);
        if self.is_live(other.as_deref()) {
            self.update(other.as_deref().unwrap(), vec![("isTombstone", Value::Bool(true))]);
        }
        self.store.remove_inline_child(sid);
    }

    fn sync_split_transfer(&mut self, parent: &str, sid: &str) -> Result<()> {
        let t = self.store.get(parent).unwrap().clone();
        let sub = self.store.get(sid).unwrap().clone();
        let other_id = sub.str_of("transferTransactionId").map(str::to_string);
        let other_live = self.live(other_id.as_deref()).map(|o| (o.str_of("accountId").map(str::to_string), o.str_of("categoryId").map(str::to_string)));
        let Some(target) = sub.str_of("targetAccountId").map(str::to_string) else {
            if other_live.is_some() {
                self.update(other_id.as_deref().unwrap(), vec![("isTombstone", Value::Bool(true))]);
            }
            if other_id.is_some() {
                self.update(sid, vec![("transferTransactionId", Value::Null)]);
            }
            return Ok(());
        };
        let account = t.str_of("accountId").unwrap_or("").to_string();
        let other_cat = other_live.as_ref().and_then(|x| x.1.clone());
        let fields = vec![
            ("accountId", s(&target)),
            ("targetAccountId", s(&account)),
            ("payeeId", s(&transfer_payee_id(&account))),
            ("date", t.get("date").cloned().unwrap_or(Value::Null)),
            ("amount", num(self.r(-sub.f("amount")))),
            ("memo", sub.get("memo").cloned().unwrap_or(Value::Null)),
            ("transferTransactionId", s(sid)),
            ("categoryId", self.transfer_category(&target, &account, other_cat.as_deref())),
            ("isTombstone", Value::Bool(false)),
        ];
        match (&other_id, &other_live) {
            (Some(o), Some((Some(acct), _))) if *acct == target => {
                self.update(o, fields);
            }
            _ => {
                if let (Some(o), Some(_)) = (&other_id, &other_live) {
                    self.update(o, vec![("isTombstone", Value::Bool(true))]);
                }
                let new_id = self.transfer_id_for(sid);
                let mut f = fields;
                f.push(("cleared", s("Uncleared")));
                let n = self.new_transaction(&new_id, f);
                self.insert(n, None)?;
                self.update(sid, vec![("transferTransactionId", s(&new_id))]);
            }
        }
        Ok(())
    }

    fn delete_transaction_internal(&mut self, id: &str) {
        let Some(t) = self.live(Some(id)).cloned() else { return };
        let other = t.str_of("transferTransactionId").map(str::to_string);
        self.update(id, vec![("isTombstone", Value::Bool(true))]);
        if let Some(o) = self.live(other.as_deref()).map(|o| o.etype().to_string()) {
            let oid = other.as_deref().unwrap();
            if o == "transaction" {
                self.update(oid, vec![("isTombstone", Value::Bool(true))]);
            } else if o == "subTransaction" {
                // The far side of a split line's transfer: turn the line back into a plain one.
                self.update(oid, vec![("transferTransactionId", Value::Null), ("targetAccountId", Value::Null)]);
            }
        }
        let subs: Vec<Option<String>> = self.store.children(id, "subTransactions").iter().map(|x| x.str_of("transferTransactionId").map(str::to_string)).collect();
        for o in subs.into_iter().flatten() {
            if self.is_live(Some(&o)) {
                self.update(&o, vec![("isTombstone", Value::Bool(true))]);
            }
        }
    }

    pub fn delete_transactions(&mut self, ids: &[String]) {
        for id in ids {
            self.delete_transaction_internal(id);
        }
    }

    fn txn_ids(&self, ids: &[String]) -> Result<Vec<String>> {
        for id in ids {
            self.require(id, "transaction", "transaction")?;
        }
        Ok(ids.to_vec())
    }

    /// Register.toggleClearSelectedTransactions: reconciled rows never change.
    pub fn toggle_cleared(&mut self, ids: &[String]) -> Result<()> {
        let ids = self.txn_ids(ids)?;
        let state = |e: &Self, id: &str| e.store.get(id).and_then(|t| t.str_of("cleared")).unwrap_or("Uncleared").to_string();
        let r = ids.iter().filter(|i| state(self, i) == "Reconciled").count();
        let c = ids.iter().filter(|i| state(self, i) == "Cleared").count();
        if ids.len() == r {
            return Ok(());
        }
        let to = if ids.len() - r == c { "Uncleared" } else { "Cleared" };
        for id in &ids {
            if state(self, id) != "Reconciled" {
                self.update(id, vec![("cleared", s(to))]);
            }
        }
        Ok(())
    }

    pub fn set_cleared(&mut self, ids: &[String], cleared: &str) -> Result<()> {
        for id in self.txn_ids(ids)? {
            self.update(&id, vec![("cleared", s(cleared))]);
        }
        Ok(())
    }

    /// Transaction.set accepted(true): drops the kept copies of a match (leaving an empty
    /// matchedTransactions array, as YNAB 4 does) and approves the other side of a transfer too.
    pub fn approve(&mut self, ids: &[String]) -> Result<()> {
        for id in self.txn_ids(ids)? {
            self.update(&id, vec![("accepted", Value::Bool(true))]);
            let children: Vec<String> = self.store.children(&id, "matchedTransactions").iter().map(|c| c.id().to_string()).collect();
            for c in children {
                self.store.remove_inline_child(&c);
            }
            let other = self.store.get(&id).and_then(|t| t.str_of("transferTransactionId")).map(str::to_string);
            let approve_other = self.live(other.as_deref()).is_some_and(|o| {
                o.etype() == "transaction" && o.get("accepted") == Some(&Value::Bool(false)) && self.store.children(o.id(), "matchedTransactions").is_empty()
            });
            if approve_other {
                self.update(other.as_deref().unwrap(), vec![("accepted", Value::Bool(true))]);
            }
        }
        Ok(())
    }

    /// Adds an adjustment if the cleared balance differs from the statement, then marks cleared
    /// transactions on or before `date` reconciled. Returns the adjustment.
    pub fn reconcile(&mut self, account_id: &str, statement: f64, date: &str) -> Result<f64> {
        self.require(account_id, "account", "account")?;
        let txns = |e: &Self| -> Vec<String> {
            e.store.collection("transactions").iter().filter(|t| !t.tomb() && t.str_of("accountId") == Some(account_id)).map(|t| t.id().to_string()).collect()
        };
        let cleared = self.r(txns(self)
            .iter()
            .filter_map(|i| self.store.get(i))
            .filter(|t| matches!(t.str_of("cleared"), Some("Cleared") | Some("Reconciled")))
            .map(|t| t.f("amount"))
            .sum());
        let diff = self.r(statement - cleared);
        if diff != 0.0 {
            let p = self.get_or_create_payee("Reconciliation Balance Adjustment", false)?;
            let on_budget = self.on_budget(account_id);
            let t = self.new_transaction(
                &new_guid(),
                vec![
                    ("accountId", s(account_id)),
                    ("date", s(date)),
                    ("amount", num(diff)),
                    ("payeeId", s(&p)),
                    ("categoryId", if on_budget { s(builtin::IMMEDIATE_INCOME) } else { Value::Null }),
                    ("cleared", s("Cleared")),
                ],
            );
            self.insert(t, None)?;
        }
        let mut reconciled = 0.0;
        for id in txns(self) {
            let (c, d, a) = {
                let t = self.store.get(&id).unwrap();
                (t.str_of("cleared").unwrap_or("").to_string(), t.str_of("date").unwrap_or("").to_string(), t.f("amount"))
            };
            if c == "Cleared" && d.as_str() <= date {
                self.update(&id, vec![("cleared", s("Reconciled"))]);
                reconciled = self.r(reconciled + a);
            } else if c == "Reconciled" {
                reconciled = self.r(reconciled + a);
            }
        }
        self.update(account_id, vec![("lastReconciledDate", s(date)), ("lastReconciledBalance", num(reconciled))]);
        Ok(diff)
    }

    // ------------------------------------------------------------------ categories

    fn user_masters(&self) -> Vec<(String, f64)> {
        self.sorted_live("masterCategories")
            .into_iter()
            .filter(|(id, _)| self.store.get(id).is_some_and(|m| m.str_of("type") == Some("OUTFLOW") && m.get("deleteable") != Some(&Value::Bool(false))))
            .collect()
    }

    pub fn create_master_category(&mut self, name: &str) -> Result<String> {
        let masters = self.user_masters();
        let mut m = blank_entity("masterCategory", &new_guid());
        m.insert("name".into(), s(name.trim()));
        m.insert("type".into(), s("OUTFLOW"));
        m.insert("sortableIndex".into(), num(index_between(masters.last().map(|x| x.1), None)));
        m.insert("deleteable".into(), Value::Bool(true));
        m.insert("expanded".into(), Value::Bool(true));
        m.insert("subCategories".into(), Value::Array(vec![]));
        self.insert(m, None)
    }

    fn live_subs(&self, master: &str, except: Option<&str>) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = self
            .store
            .children(master, "subCategories")
            .into_iter()
            .filter(|x| !x.tomb() && Some(x.id()) != except)
            .map(|x| (x.id().to_string(), x.f("sortableIndex")))
            .collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v
    }

    pub fn create_category(&mut self, master_id: &str, name: &str) -> Result<String> {
        self.require(master_id, "masterCategory", "master category")?;
        let last = self.live_subs(master_id, None).last().map(|x| x.1);
        let mut c = blank_entity("category", &new_guid());
        c.insert("name".into(), s(name.trim()));
        c.insert("type".into(), s("OUTFLOW"));
        c.insert("masterCategoryId".into(), s(master_id));
        c.insert("sortableIndex".into(), num(index_between(last, None)));
        self.insert(c, None)
    }

    pub fn rename_category(&mut self, id: &str, name: &str) -> Result<()> {
        if !self.is_live(Some(id)) {
            return err(format!("No such category {id}"));
        }
        self.update(id, vec![("name", s(name.trim()))]);
        Ok(())
    }

    pub fn set_category_note(&mut self, id: &str, note: Option<&str>) -> Result<()> {
        if !self.is_live(Some(id)) {
            return err(format!("No such category {id}"));
        }
        self.update(id, vec![("note", opt_str(note.filter(|n| !n.is_empty())))]);
        Ok(())
    }

    /// Moves a subcategory to the hidden master, encoding where it came from in its name.
    pub fn hide_category(&mut self, id: &str) -> Result<()> {
        self.require(id, "category", "category")?;
        let (name, master) = {
            let c = self.store.get(id).unwrap();
            (c.str_of("name").unwrap_or("").to_string(), c.str_of("masterCategoryId").unwrap_or("").to_string())
        };
        if master == builtin::HIDDEN_MASTER {
            return Ok(());
        }
        let master_name = self.store.get(&master).and_then(|m| m.str_of("name")).unwrap_or("").to_string();
        if let Some(c) = self.store.get_mut(id) {
            c.insert("name".into(), s(&format!("{master_name} ` {name} ` {master}")));
        }
        self.store.move_child(id, builtin::HIDDEN_MASTER);
        Ok(())
    }

    pub fn unhide_category(&mut self, id: &str) -> Result<()> {
        let Some(c) = self.live(Some(id)) else { return Ok(()) };
        if c.str_of("masterCategoryId") != Some(builtin::HIDDEN_MASTER) {
            return Ok(());
        }
        let full = c.str_of("name").unwrap_or("").to_string();
        let parts: Vec<&str> = full.split(" ` ").collect();
        let (name, mut master) = if parts.len() >= 3 {
            (parts[1..parts.len() - 1].join(" ` "), Some(parts[parts.len() - 1].to_string()))
        } else {
            (full.clone(), None)
        };
        if !master.as_deref().is_some_and(|m| self.is_live(Some(m))) {
            master = Some(match self.user_masters().first() {
                Some((m, _)) => m.clone(),
                None => self.create_master_category("Unhidden")?,
            });
        }
        if let Some(c) = self.store.get_mut(id) {
            c.insert("name".into(), s(&name));
        }
        self.store.move_child(id, master.as_deref().unwrap());
        Ok(())
    }

    pub fn move_category(&mut self, id: &str, master_id: &str, before_id: Option<&str>) -> Result<()> {
        if !self.is_live(Some(id)) {
            return err(format!("No such category {id}"));
        }
        self.require(master_id, "masterCategory", "master category")?;
        let siblings = self.live_subs(master_id, Some(id));
        let i = before_id.and_then(|b| siblings.iter().position(|x| x.0 == b)).unwrap_or(siblings.len());
        let idx = index_between(i.checked_sub(1).map(|j| siblings[j].1), siblings.get(i).map(|x| x.1));
        let same = self.store.get(id).unwrap().str_of("masterCategoryId") == Some(master_id);
        if let Some(c) = self.store.get_mut(id) {
            c.insert("sortableIndex".into(), num(idx));
        }
        if same {
            self.store.touch(id, false);
        } else {
            self.store.move_child(id, master_id);
        }
        Ok(())
    }

    pub fn move_master_category(&mut self, id: &str, before_id: Option<&str>) -> Result<()> {
        self.require(id, "masterCategory", "master category")?;
        let others: Vec<(String, f64)> = self.user_masters().into_iter().filter(|x| x.0 != id).collect();
        let i = before_id.and_then(|b| others.iter().position(|x| x.0 == b)).unwrap_or(others.len());
        let idx = index_between(i.checked_sub(1).map(|j| others[j].1), others.get(i).map(|x| x.1));
        self.update(id, vec![("sortableIndex", num(idx))]);
        Ok(())
    }

    /// Deletes a category, moving its transactions and budgeted amounts to another one.
    pub fn delete_category(&mut self, id: &str, reassign_to: Option<&str>) -> Result<()> {
        if !self.is_live(Some(id)) {
            return err(format!("No such category {id}"));
        }
        for key in ["transactions", "scheduledTransactions"] {
            let ids: Vec<String> = self.store.collection(key).iter().filter(|t| !t.tomb()).map(|t| t.id().to_string()).collect();
            for tid in ids {
                if self.store.get(&tid).unwrap().str_of("categoryId") == Some(id) {
                    self.update(&tid, vec![("categoryId", opt_str(reassign_to))]);
                }
                let subs: Vec<String> =
                    self.store.children(&tid, "subTransactions").iter().filter(|x| x.str_of("categoryId") == Some(id)).map(|x| x.id().to_string()).collect();
                for sid in subs {
                    self.update(&sid, vec![("categoryId", opt_str(reassign_to))]);
                }
            }
        }
        if let Some(to) = reassign_to {
            let months: Vec<(String, String, f64)> = self
                .store
                .collection("monthlyBudgets")
                .iter()
                .flat_map(|mb| {
                    let m = month_key(mb.str_of("month").unwrap_or("")).to_string();
                    self.store
                        .children(mb.id(), "monthlySubCategoryBudgets")
                        .into_iter()
                        .filter(|c| !c.tomb() && c.str_of("categoryId") == Some(id) && c.f("budgeted") != 0.0)
                        .map(move |c| (m.clone(), c.id().to_string(), c.f("budgeted")))
                        .collect::<Vec<_>>()
                })
                .collect();
            for (m, from_id, amount) in months {
                let existing = self.live(Some(&monthly_category_budget_id(&m, to))).map(|c| c.f("budgeted")).unwrap_or(0.0);
                self.set_budgeted(&m, to, existing + amount)?;
                self.update(&from_id, vec![("budgeted", num(0.0))]);
            }
        }
        self.update(id, vec![("isTombstone", Value::Bool(true))]);
        Ok(())
    }

    pub fn delete_master_category(&mut self, id: &str) -> Result<()> {
        self.require(id, "masterCategory", "master category")?;
        if self.store.get(id).unwrap().get("deleteable") == Some(&Value::Bool(false)) {
            return err("Built-in master categories can't be deleted");
        }
        if !self.live_subs(id, None).is_empty() {
            return err("Delete or move its categories first");
        }
        self.update(id, vec![("isTombstone", Value::Bool(true))]);
        Ok(())
    }

    // ------------------------------------------------------------------ monthly budgets

    /// MonthlyBudgets.getOrCreate("MB/YYYY-MM").
    pub fn monthly_budget(&mut self, month: &str) -> Result<String> {
        let id = monthly_budget_id(month);
        match self.store.get(&id).map(|o| o.tomb()) {
            None => {
                let mut mb = blank_entity("monthlyBudget", &id);
                mb.insert("month".into(), s(&format!("{}-01", &month[..7])));
                mb.insert("monthlySubCategoryBudgets".into(), Value::Array(vec![]));
                self.insert(mb, None)?;
            }
            Some(true) => {
                self.update(&id, vec![("isTombstone", Value::Bool(false))]);
            }
            Some(false) => {}
        }
        Ok(id)
    }

    fn category_budget(&mut self, month: &str, category: &str) -> Result<String> {
        let mb = self.monthly_budget(month)?;
        let id = monthly_category_budget_id(month, category);
        match self.store.get(&id).map(|o| o.tomb()) {
            None => {
                let mut c = blank_entity("monthlyCategoryBudget", &id);
                c.insert("categoryId".into(), s(category));
                c.insert("parentMonthlyBudgetId".into(), s(&mb));
                self.insert(c, None)?;
            }
            Some(true) => {
                self.update(&id, vec![("isTombstone", Value::Bool(false))]);
            }
            Some(false) => {}
        }
        Ok(id)
    }

    pub fn set_budgeted(&mut self, month: &str, category: &str, amount: f64) -> Result<()> {
        let id = self.category_budget(month, category)?;
        let v = num(self.r(amount));
        self.update(&id, vec![("budgeted", v)]);
        Ok(())
    }

    pub fn set_overspending_handling(&mut self, month: &str, category: &str, handling: Option<&str>) -> Result<()> {
        let id = self.category_budget(month, category)?;
        self.update(&id, vec![("overspendingHandling", opt_str(handling))]);
        Ok(())
    }

    pub fn set_monthly_category_note(&mut self, month: &str, category: &str, note: Option<&str>) -> Result<()> {
        let id = self.category_budget(month, category)?;
        self.update(&id, vec![("note", opt_str(note.filter(|n| !n.is_empty())))]);
        Ok(())
    }

    pub fn set_month_note(&mut self, month: &str, note: Option<&str>) -> Result<()> {
        let id = self.monthly_budget(month)?;
        self.update(&id, vec![("note", opt_str(note.filter(|n| !n.is_empty())))]);
        Ok(())
    }

    /// Months -13..+13 around today exist (internalEnsureDataIsConsistentWithCurrentDate).
    pub fn ensure_months(&mut self, today: &str) -> Result<usize> {
        let this = month_key(today);
        let mut created = 0;
        for i in -13..=13 {
            let m = add_months(this, i);
            if !self.store.contains(&monthly_budget_id(&m)) {
                self.monthly_budget(&m)?;
                created += 1;
            }
        }
        Ok(created)
    }

    // ------------------------------------------------------------------ scheduled transactions

    pub fn save_scheduled(&mut self, input: &TransactionInput) -> Result<String> {
        self.require(&input.account_id, "account", "account")?;
        let frequency = input.frequency.clone().unwrap_or_else(|| "Monthly".into());
        let payee = self.resolve_payee(input)?;
        let splits = input.splits.as_ref().filter(|s| !s.is_empty());
        let target = input.transfer_account_id.as_deref();
        let category = if splits.is_some() {
            s(builtin::SPLIT)
        } else if let Some(t) = target {
            self.transfer_category(&input.account_id, t, input.category_id.as_deref())
        } else {
            opt_str(input.category_id.as_deref())
        };
        let fields = vec![
            ("accountId", s(&input.account_id)),
            ("date", s(&input.date)),
            ("amount", num(self.r(input.amount))),
            ("payeeId", opt_str(payee.as_deref())),
            ("categoryId", category),
            ("memo", opt_str(input.memo.as_deref())),
            ("flag", opt_str(input.flag.as_deref())),
            ("checkNumber", opt_str(input.check_number.as_deref())),
            ("targetAccountId", opt_str(target)),
            ("frequency", s(&frequency)),
            ("twiceAMonthStartDay", num(input.twice_a_month_start_day.unwrap_or(0) as f64)),
            ("cleared", s("Uncleared")),
            ("accepted", Value::Bool(true)),
        ];
        let existing = input.id.as_deref().filter(|id| self.live(Some(id)).is_some_and(|o| o.etype() == "scheduledTransaction"));
        let id = match existing {
            Some(id) => {
                self.update(id, fields);
                id.to_string()
            }
            None => {
                let mut o = blank_entity("scheduledTransaction", &input.id.clone().unwrap_or_else(new_guid));
                for (k, v) in fields {
                    o.insert(k.into(), v);
                }
                self.insert(o, None)?
            }
        };
        let current: Vec<String> = self.store.children(&id, "subTransactions").iter().map(|x| x.id().to_string()).collect();
        for c in current {
            self.store.remove_inline_child(&c);
        }
        for sp in splits.map(|v| v.as_slice()).unwrap_or(&[]) {
            let mut sub = blank_entity("subTransaction", &new_guid());
            sub.insert("amount".into(), num(self.r(sp.amount)));
            sub.insert("categoryId".into(), opt_str(sp.category_id.as_deref()));
            sub.insert("memo".into(), opt_str(sp.memo.as_deref()));
            sub.insert("targetAccountId".into(), opt_str(sp.transfer_account_id.as_deref()));
            sub.insert("parentTransactionId".into(), s(&id));
            self.insert(sub, Some(&id))?;
        }
        Ok(id)
    }

    pub fn delete_scheduled(&mut self, id: &str) {
        if self.is_live(Some(id)) {
            self.update(id, vec![("isTombstone", Value::Bool(true))]);
        }
    }

    /// Enters every occurrence dated on or before today (enterDueScheduledTransactions).
    pub fn enter_due_scheduled(&mut self, today: &str) -> Result<Vec<String>> {
        let mut entered = Vec::new();
        let ids: Vec<String> = self.store.collection("scheduledTransactions").iter().filter(|s| !s.tomb()).map(|s| s.id().to_string()).collect();
        for id in ids {
            let freq = self.store.get(&id).unwrap().str_of("frequency").unwrap_or("Monthly").to_string();
            if freq == "Never" {
                continue;
            }
            for _ in 0..5000 {
                let (date, start_day) = {
                    let o = self.store.get(&id).unwrap();
                    (o.str_of("date").unwrap_or("").to_string(), o.f("twiceAMonthStartDay") as u32)
                };
                if date.as_str() > today {
                    break;
                }
                entered.push(self.enter_occurrence(&id, today, false)?);
                if freq == "Once" {
                    self.update(&id, vec![("isTombstone", Value::Bool(true))]);
                    break;
                }
                let next = next_occurrence(&freq, &date, start_day);
                if next <= date {
                    break;
                }
                self.update(&id, vec![("date", s(&next))]);
            }
        }
        Ok(entered)
    }

    /// Enters one occurrence (ScheduledTransaction.addToRegister) without changing the schedule.
    pub fn enter_occurrence(&mut self, sched_id: &str, today: &str, accepted: bool) -> Result<String> {
        let st = self.store.get(sched_id).ok_or_else(|| Error::Msg(format!("No such scheduled transaction {sched_id}")))?.clone();
        let date = st.str_of("date").unwrap_or("").to_string();
        let id = (0..).map(|n| format!("{sched_id}_{date}_{n}")).find(|i| !self.store.contains(i)).unwrap();
        let copy = |k: &str| st.get(k).cloned().unwrap_or(Value::Null);
        let t = self.new_transaction(
            &id,
            vec![
                ("accountId", copy("accountId")),
                ("date", s(&date)),
                ("amount", copy("amount")),
                ("payeeId", copy("payeeId")),
                ("categoryId", copy("categoryId")),
                ("memo", copy("memo")),
                ("flag", copy("flag")),
                ("cleared", s("Uncleared")),
                ("accepted", Value::Bool(accepted)),
                ("targetAccountId", copy("targetAccountId")),
                ("dateEnteredFromSchedule", s(today)),
            ],
        );
        self.insert(t, None)?;
        let subs: Vec<Obj> = self.store.children(sched_id, "subTransactions").into_iter().filter(|x| !x.tomb()).cloned().collect();
        for sub in subs {
            let mut c = blank_entity("subTransaction", &new_guid());
            for k in ["amount", "categoryId", "memo", "targetAccountId"] {
                c.insert(k.into(), sub.get(k).cloned().unwrap_or(Value::Null));
            }
            c.insert("parentTransactionId".into(), s(&id));
            let cid = self.insert(c, Some(&id))?;
            self.sync_split_transfer(&id, &cid)?;
        }
        self.sync_transfer(&id)?;
        Ok(id)
    }

    /// "Enter now": enters the next occurrence early and advances the schedule.
    pub fn enter_scheduled_now(&mut self, id: &str) -> Result<String> {
        if !self.is_live(Some(id)) {
            return err(format!("No such scheduled transaction {id}"));
        }
        let t = self.enter_occurrence(id, &today(), true)?;
        self.advance_schedule(id);
        Ok(t)
    }

    /// Skips the next occurrence without entering it.
    pub fn skip_scheduled(&mut self, id: &str) {
        if self.is_live(Some(id)) {
            self.advance_schedule(id);
        }
    }

    fn advance_schedule(&mut self, id: &str) {
        let (freq, date, start) = {
            let o = self.store.get(id).unwrap();
            (o.str_of("frequency").unwrap_or("Monthly").to_string(), o.str_of("date").unwrap_or("").to_string(), o.f("twiceAMonthStartDay") as u32)
        };
        if freq == "Once" {
            self.update(id, vec![("isTombstone", Value::Bool(true))]);
        } else {
            self.update(id, vec![("date", s(&next_occurrence(&freq, &date, start)))]);
        }
    }
}
