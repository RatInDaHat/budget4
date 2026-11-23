//! Budget.yfull, *.ydiff and *.ydevice writers.

use chrono::{DateTime, Datelike, Local, Offset, Timelike};
use serde_json::Value;

use crate::json::{stringify, Obj, ObjExt};
use crate::schema::{aggregate_child_properties, entity_fields, field_default, is_inline_array, omit_if_false_or_null};
use crate::store::Store;

/// Budget.yfull: tab-indented, with YNAB's false/null field elision.
pub fn serialize_full_value(root: &Value) -> String {
    stringify(root, "\t", &|k, v| omit_if_false_or_null(k) && (v.is_null() || *v == Value::Bool(false)))
}

pub fn serialize_full(store: &Store) -> String {
    serialize_full_value(&store.to_root())
}

/// .ydevice / Budget.ymeta: tab-indented plain JSON.
pub fn serialize_tabbed(v: &Value) -> String {
    stringify(v, "\t", &|_, _| false)
}

/// A record with every declared field present (nulls/defaults filled in), as diff items are written.
pub fn expand_value(o: &Obj) -> Value {
    let etype = o.etype();
    let Some(fields) = entity_fields(etype) else { return Value::Object(o.clone()) };
    let excluded = aggregate_child_properties(etype);
    let mut out = Obj::new();
    for f in &fields {
        if excluded.contains(f) {
            continue;
        }
        let mut v = o.get(*f).cloned().unwrap_or_else(|| field_default(f));
        if is_inline_array(f) {
            if let Value::Array(items) = &v {
                v = Value::Array(items.iter().map(|c| c.as_object().map(expand_value).unwrap_or_else(|| c.clone())).collect());
            }
        }
        out.insert(f.to_string(), v);
    }
    for (k, v) in o {
        if !out.contains_key(k) && !excluded.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

pub struct DiffFile {
    pub data_version: String,
    pub device_guid: String,
    pub short_device_id: String,
    pub start_version: String,
    pub end_version: String,
    pub publish_time: String,
    /// Materialized records (inline children nested).
    pub items: Vec<Value>,
}

/// *.ydiff: 2-space indented; aggregate children are separate items, never nested.
pub fn serialize_diff(d: &DiffFile) -> String {
    let mut o = Obj::new();
    o.insert("dataVersion".into(), Value::String(d.data_version.clone()));
    o.insert("budgetDataGUID".into(), Value::Null);
    o.insert("deviceGUID".into(), Value::String(d.device_guid.clone()));
    o.insert("shortDeviceId".into(), Value::String(d.short_device_id.clone()));
    o.insert("startVersion".into(), Value::String(d.start_version.clone()));
    o.insert("endVersion".into(), Value::String(d.end_version.clone()));
    o.insert("formatVersion".into(), Value::Null);
    o.insert("publishTime".into(), Value::String(d.publish_time.clone()));
    o.insert(
        "items".into(),
        Value::Array(d.items.iter().map(|i| i.as_object().map(expand_value).unwrap_or_else(|| i.clone())).collect()),
    );
    stringify(&Value::Object(o), "  ", &|_, _| false)
}

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// ActionScript Date.toString(), e.g. "Tue Jul 28 08:20:31 GMT-0700 2026".
pub fn as3_date_string(d: DateTime<Local>) -> String {
    let off = d.offset().fix().local_minus_utc() / 60;
    let sign = if off >= 0 { '+' } else { '-' };
    let a = off.abs();
    format!(
        "{} {} {} {:02}:{:02}:{:02} GMT{}{:02}{:02} {}",
        DAYS[d.weekday().num_days_from_monday() as usize],
        MONTHS[d.month0() as usize],
        d.day(),
        d.hour(),
        d.minute(),
        d.second(),
        sign,
        a / 60,
        a % 60,
        d.year()
    )
}
