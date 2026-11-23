//! Date helpers on "YYYY-MM-DD" / "YYYY-MM" strings.

use chrono::{Datelike, Duration, Local, NaiveDate};

pub fn today() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

pub fn month_key(date: &str) -> &str {
    &date[..7.min(date.len())]
}

fn ym(month: &str) -> (i32, i32) {
    let y = month[..4].parse().unwrap_or(1970);
    let m = month.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(1);
    (y, m)
}

/// "2026-11" + 3 → "2027-02".
pub fn add_months(month: &str, n: i32) -> String {
    let (y, m) = ym(month);
    let idx = y * 12 + (m - 1) + n;
    format!("{}-{:02}", idx.div_euclid(12), idx.rem_euclid(12) + 1)
}

pub fn month_range(from: &str, to: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut m = from.to_string();
    while m.as_str() <= to {
        out.push(m.clone());
        m = add_months(&m, 1);
    }
    out
}

fn parse(d: &str) -> NaiveDate {
    NaiveDate::parse_from_str(&d[..10.min(d.len())], "%Y-%m-%d").unwrap_or_default()
}

fn fmt(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (NaiveDate::from_ymd_opt(ny, nm, 1).unwrap() - Duration::days(1)).day()
}

/// Days from `a` to `b` (negative when `b` is earlier).
pub fn days_between(a: &str, b: &str) -> i64 {
    (parse(b) - parse(a)).num_days()
}

pub fn add_days(d: &str, n: i64) -> String {
    fmt(parse(d) + Duration::days(n))
}

/// Adds months, clamping to the end of shorter months (Jan 31 + 1 month = Feb 28/29).
pub fn add_months_to_date(d: &str, n: i32) -> String {
    let p = parse(d);
    let idx = p.year() * 12 + p.month0() as i32 + n;
    let (y, m) = (idx.div_euclid(12), idx.rem_euclid(12) as u32 + 1);
    fmt(NaiveDate::from_ymd_opt(y, m, p.day().min(days_in_month(y, m))).unwrap())
}

/// ScheduledTransaction.getNextOccurrence.
pub fn next_occurrence(freq: &str, d: &str, twice_a_month_start_day: u32) -> String {
    match freq {
        "Once" | "Never" => d.to_string(),
        "Daily" => add_days(d, 1),
        "Weekly" => add_days(d, 7),
        "EveryOtherWeek" => add_days(d, 14),
        "Every4Weeks" => add_days(d, 28),
        "Monthly" => add_months_to_date(d, 1),
        "EveryOtherMonth" => add_months_to_date(d, 2),
        "Every3Months" => add_months_to_date(d, 3),
        "Every4Months" => add_months_to_date(d, 4),
        "TwiceAYear" => add_months_to_date(d, 6),
        "Yearly" => add_months_to_date(d, 12),
        "EveryOtherYear" => add_months_to_date(d, 24),
        "TwiceAMonth" => {
            let p = parse(d);
            let start = if twice_a_month_start_day > 0 { twice_a_month_start_day } else { p.day() };
            if p.day() == start {
                let plus = p + Duration::days(15);
                if plus.month() != p.month() {
                    fmt(NaiveDate::from_ymd_opt(p.year(), p.month(), days_in_month(p.year(), p.month())).unwrap())
                } else {
                    fmt(plus)
                }
            } else {
                let (y, m) = if p.month() == 12 { (p.year() + 1, 1) } else { (p.year(), p.month() + 1) };
                fmt(NaiveDate::from_ymd_opt(y, m, start.min(days_in_month(y, m))).unwrap())
            }
        }
        _ => add_months_to_date(d, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_and_occurrences() {
        assert_eq!(add_months("2026-11", 3), "2027-02");
        assert_eq!(add_months("2026-01", -1), "2025-12");
        assert_eq!(month_range("2025-11", "2026-02"), ["2025-11", "2025-12", "2026-01", "2026-02"]);
        assert_eq!(next_occurrence("Monthly", "2026-01-31", 0), "2026-02-28");
        assert_eq!(next_occurrence("Yearly", "2024-02-29", 0), "2025-02-28");
        assert_eq!(next_occurrence("EveryOtherWeek", "2026-12-25", 0), "2027-01-08");
        assert_eq!(next_occurrence("TwiceAMonth", "2026-01-01", 1), "2026-01-16");
        assert_eq!(next_occurrence("TwiceAMonth", "2026-01-16", 1), "2026-02-01");
        assert_eq!(next_occurrence("TwiceAMonth", "2026-02-20", 20), "2026-02-28");
    }
}
