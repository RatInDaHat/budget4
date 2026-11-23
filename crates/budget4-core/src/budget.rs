//! Derived budget figures, following YNAB 4's data.MonthlyBudget / MonthlyMasterCategoryBudget /
//! MonthlySubCategoryBudget / AccountBalanceCalculator.

use std::collections::HashMap;

use serde::Serialize;

use crate::dates::{add_months, month_key, month_range, today};
use crate::json::{Obj, ObjExt};
use crate::schema::{builtin, is_income_category};
use crate::store::Store;

/// CurrencyUtilities rounding: JavaScript Math.round(x·10^d)/10^d (halves round toward +∞).
#[derive(Clone, Copy)]
pub struct Rounder(f64);

impl Rounder {
    pub fn new(decimal_digits: u32) -> Self {
        Rounder(10f64.powi(decimal_digits as i32))
    }
    pub fn r(&self, x: f64) -> f64 {
        let v = (x * self.0 + 0.5).floor() / self.0;
        if v == 0.0 {
            0.0
        } else {
            v
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Handling {
    AffectsBuffer,
    Confined,
}

impl Handling {
    pub fn parse(s: Option<&str>) -> Option<Self> {
        match s {
            Some("Confined") => Some(Handling::Confined),
            Some("AffectsBuffer") => Some(Handling::AffectsBuffer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryMonth {
    pub budgeted: f64,
    /// Outflows as a positive number.
    pub spent: f64,
    pub prior_balance: f64,
    pub balance: f64,
    /// Effective handling (inherited from the previous month when not set).
    pub handling: Handling,
    pub handling_set: Option<Handling>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterMonth {
    pub budgeted: f64,
    pub spent: f64,
    pub balance: f64,
    pub carried_overspending: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthSummary {
    pub month: String,
    /// "Not Budgeted in <previous month>".
    pub not_budgeted_last_month: f64,
    /// "Overspent in <previous month>" (≤ 0).
    pub overspent_last_month: f64,
    /// "Income for <month>": this month's immediate income + last month's deferred income.
    pub income: f64,
    pub total_budgeted: f64,
    /// "Available to Budget".
    pub available: f64,
    pub starting_buffer: f64,
    pub ending_buffer: f64,
    pub supplemental_income: f64,
    pub deferred_income_from_this_month: f64,
    pub deferred_income_from_previous_month: f64,
    pub total_carried_overspending: f64,
    pub total_balance: f64,
    pub total_spent: f64,
}

#[derive(Debug, Clone)]
pub struct MonthModel {
    pub summary: MonthSummary,
    pub categories: HashMap<String, CategoryMonth>,
    pub masters: HashMap<String, MasterMonth>,
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub master_id: String,
    pub sub_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BudgetModel {
    pub months: Vec<String>,
    pub by_month: HashMap<String, MonthModel>,
    pub tree: Vec<TreeNode>,
}

fn sort_index(o: &Obj) -> f64 {
    o.f("sortableIndex")
}

/// Live master categories and their live subcategories, in display order.
pub fn category_tree(store: &Store) -> Vec<TreeNode> {
    let mut masters: Vec<&Obj> = store.collection("masterCategories").into_iter().filter(|m| !m.tomb()).collect();
    masters.sort_by(|a, b| sort_index(a).total_cmp(&sort_index(b)));
    masters
        .into_iter()
        .map(|m| {
            let mut subs: Vec<&Obj> = store.children(m.id(), "subCategories").into_iter().filter(|s| !s.tomb()).collect();
            subs.sort_by(|a, b| sort_index(a).total_cmp(&sort_index(b)));
            TreeNode { master_id: m.id().to_string(), sub_ids: subs.iter().map(|s| s.id().to_string()).collect() }
        })
        .collect()
}

/// Computes every month from the first month with data through `through_month` (YYYY-MM).
pub fn compute_budget(store: &Store, through_month: Option<&str>, decimal_digits: u32) -> BudgetModel {
    let rd = Rounder::new(decimal_digits);
    let tree = category_tree(store);
    let on_budget: HashMap<&str, bool> = store.collection("accounts").into_iter().map(|a| (a.id(), !a.tomb() && a.b("onBudget"))).collect();

    let mut spent: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut immediate: HashMap<String, f64> = HashMap::new();
    let mut deferred: HashMap<String, f64> = HashMap::new();
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;
    let extend = |m: &str, first: &mut Option<String>, last: &mut Option<String>| {
        if first.as_deref().is_none_or(|f| m < f) {
            *first = Some(m.to_string());
        }
        if last.as_deref().is_none_or(|l| m > l) {
            *last = Some(m.to_string());
        }
    };

    for t in store.collection("transactions") {
        if t.tomb() || !on_budget.get(t.str_of("accountId").unwrap_or("")).copied().unwrap_or(false) {
            continue;
        }
        let m = month_key(t.str_of("date").unwrap_or("")).to_string();
        extend(&m, &mut first, &mut last);
        let mut add = |cat: &str, amount: f64| {
            if is_income_category(Some(cat)) {
                let map = if cat == builtin::IMMEDIATE_INCOME { &mut immediate } else { &mut deferred };
                let e = map.entry(m.clone()).or_insert(0.0);
                *e = rd.r(*e + amount);
            } else if cat != builtin::SPLIT {
                let e = spent.entry(m.clone()).or_default().entry(cat.to_string()).or_insert(0.0);
                *e = rd.r(*e - amount);
            }
        };
        let cat = t.str_of("categoryId");
        if cat == Some(builtin::SPLIT) {
            for s in store.children(t.id(), "subTransactions") {
                if s.tomb() {
                    continue;
                }
                if let Some(c) = s.str_of("categoryId") {
                    add(c, s.f("amount"));
                }
            }
        } else if let Some(c) = cat {
            add(c, t.f("amount"));
        }
    }

    let mut mcbs: HashMap<String, HashMap<String, &Obj>> = HashMap::new();
    for mb in store.collection("monthlyBudgets") {
        if mb.tomb() {
            continue;
        }
        let m = month_key(mb.str_of("month").unwrap_or("")).to_string();
        extend(&m, &mut first, &mut last);
        let map = mcbs.entry(m).or_default();
        for c in store.children(mb.id(), "monthlySubCategoryBudgets") {
            if !c.tomb() {
                if let Some(cat) = c.str_of("categoryId") {
                    map.insert(cat.to_string(), c);
                }
            }
        }
    }

    let now = month_key(&today()).to_string();
    let through = through_month.map(str::to_string).unwrap_or_else(|| add_months(&now, 13));
    if last.as_deref().is_none_or(|l| through.as_str() > l) {
        last = Some(through);
    }
    let first = first.unwrap_or(now);
    let months = month_range(&first, last.as_deref().unwrap());

    let mut by_month: HashMap<String, MonthModel> = HashMap::new();
    let mut prev: Option<String> = None;
    for m in &months {
        let mut categories = HashMap::new();
        let mut masters = HashMap::new();
        let (mut total_budgeted, mut total_carried, mut total_balance, mut total_spent) = (0.0, 0.0, 0.0, 0.0);
        let p_model = prev.as_ref().and_then(|p| by_month.get(p));
        for node in &tree {
            let mut mm = MasterMonth::default();
            for sub in &node.sub_ids {
                let mcb = mcbs.get(m).and_then(|x| x.get(sub));
                let p = p_model.and_then(|pm| pm.categories.get(sub));
                let stored = mcb.and_then(|c| Handling::parse(c.str_of("overspendingHandling")));
                let handling = stored.or(p.map(|x| x.handling)).unwrap_or(Handling::AffectsBuffer);
                let prior = match p {
                    Some(p) if p.balance >= 0.0 || p.handling == Handling::Confined => p.balance,
                    _ => 0.0,
                };
                let budgeted = mcb.map(|c| c.f("budgeted")).unwrap_or(0.0);
                let s = rd.r(spent.get(m).and_then(|x| x.get(sub)).copied().unwrap_or(0.0));
                let balance = rd.r(prior + budgeted - s);
                mm.budgeted = rd.r(mm.budgeted + budgeted);
                mm.spent = rd.r(mm.spent + s);
                mm.balance = rd.r(mm.balance + balance);
                if balance < 0.0 && handling == Handling::AffectsBuffer {
                    mm.carried_overspending = rd.r(mm.carried_overspending + balance);
                }
                categories.insert(
                    sub.clone(),
                    CategoryMonth {
                        budgeted,
                        spent: s,
                        prior_balance: prior,
                        balance,
                        handling,
                        handling_set: stored,
                        note: mcb.and_then(|c| c.str_of("note")).map(str::to_string),
                    },
                );
            }
            total_budgeted = rd.r(total_budgeted + mm.budgeted);
            total_carried = rd.r(total_carried + mm.carried_overspending);
            total_balance = rd.r(total_balance + mm.balance);
            total_spent = rd.r(total_spent + mm.spent);
            masters.insert(node.master_id.clone(), mm);
        }
        let supplemental = immediate.get(m).copied().unwrap_or(0.0);
        let deferred_this = deferred.get(m).copied().unwrap_or(0.0);
        let deferred_prev = deferred.get(&add_months(m, -1)).copied().unwrap_or(0.0);
        let ps = p_model.map(|x| &x.summary);
        let starting = match ps {
            Some(ps) => rd.r(rd.r(deferred_prev + ps.available) + ps.total_carried_overspending),
            None => deferred_prev,
        };
        let available = rd.r(starting + supplemental - total_budgeted);
        let ending = rd.r(rd.r(available + deferred_this) + total_carried);
        let summary = MonthSummary {
            month: m.clone(),
            not_budgeted_last_month: ps.map(|x| x.available).unwrap_or(0.0),
            overspent_last_month: ps.map(|x| x.total_carried_overspending).unwrap_or(0.0),
            income: rd.r(supplemental + deferred_prev),
            total_budgeted,
            available,
            starting_buffer: starting,
            ending_buffer: ending,
            supplemental_income: supplemental,
            deferred_income_from_this_month: deferred_this,
            deferred_income_from_previous_month: deferred_prev,
            total_carried_overspending: total_carried,
            total_balance,
            total_spent,
        };
        by_month.insert(m.clone(), MonthModel { summary, categories, masters });
        prev = Some(m.clone());
    }
    BudgetModel { months, by_month, tree }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Balances {
    pub cleared: f64,
    pub uncleared: f64,
    pub reconciled: f64,
    pub working: f64,
}

/// AccountBalanceCalculator: cleared includes reconciled; working = cleared + uncleared.
pub fn account_balances(store: &Store, decimal_digits: u32) -> HashMap<String, Balances> {
    let rd = Rounder::new(decimal_digits);
    let mut out: HashMap<String, Balances> = store.collection("accounts").iter().map(|a| (a.id().to_string(), Balances::default())).collect();
    for t in store.collection("transactions") {
        if t.tomb() {
            continue;
        }
        let Some(b) = out.get_mut(t.str_of("accountId").unwrap_or("")) else { continue };
        let a = t.f("amount");
        match t.str_of("cleared").unwrap_or("Uncleared") {
            "Reconciled" => {
                b.reconciled = rd.r(b.reconciled + a);
                b.cleared = rd.r(b.cleared + a);
            }
            "Cleared" => b.cleared = rd.r(b.cleared + a),
            _ => b.uncleared = rd.r(b.uncleared + a),
        }
    }
    for b in out.values_mut() {
        b.working = rd.r(b.cleared + b.uncleared);
    }
    out
}

/// Register order (date, then id) with running balances.
pub fn register_rows<'a>(store: &'a Store, account_id: Option<&str>, decimal_digits: u32) -> Vec<(&'a Obj, f64)> {
    let rd = Rounder::new(decimal_digits);
    let mut rows: Vec<&Obj> = store
        .collection("transactions")
        .into_iter()
        .filter(|t| !t.tomb() && account_id.is_none_or(|a| t.str_of("accountId") == Some(a)))
        .collect();
    rows.sort_by(|a, b| (a.str_of("date"), a.id()).cmp(&(b.str_of("date"), b.id())));
    let mut running = 0.0;
    rows.into_iter()
        .map(|t| {
            running = rd.r(running + t.f("amount"));
            (t, running)
        })
        .collect()
}
