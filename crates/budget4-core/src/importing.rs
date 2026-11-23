//! Bank file import with YNAB 4's semantics (ImporterOFX + OFXLineReader, ImportUtilities,
//! TransactionList.findMatchingTransaction / match, PayeeManager.applyRulesToImported,
//! AccountMappingManager). See docs/ynab4-format.md, "Importing".
//!
//! Flow: parse a file into [`BankTxn`]s (each gets YNAB 4's import id, `YNAB:<amount>:<date>:<n>`),
//! then [`Editor::import_bank_transactions`] skips what was imported before, matches the rest to
//! transactions you entered by hand, and adds what's left as unapproved, cleared transactions.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::dates::days_between;
use crate::editor::Editor;
use crate::json::{num, opt_str, Obj, ObjExt};
use crate::package::new_guid;
use crate::schema::{blank_entity, builtin, entity_fields};
use crate::{Error, Result};

/// One transaction read from a bank file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BankTxn {
    pub date: String,
    pub amount: f64,
    pub payee: Option<String>,
    pub memo: Option<String>,
    pub check_number: Option<String>,
    pub fitid: Option<String>,
    /// YNAB 4's duplicate key, set by [`assign_ynab_ids`].
    #[serde(default)]
    pub ynab_id: String,
}

/// One account's statement in an OFX/QFX file.
#[derive(Debug, Clone)]
pub struct Statement {
    /// "bank", "creditCard" or "investment".
    pub kind: &'static str,
    /// What YNAB 4 keys account mappings on: SIGNON's FID, else the statement's BANKID.
    pub fid: Option<String>,
    /// The full account number (only ever stored hashed).
    pub bank_account_id: Option<String>,
    pub account_type: Option<String>,
    pub txns: Vec<BankTxn>,
}

impl Statement {
    pub fn last4(&self) -> String {
        last_n(self.bank_account_id.as_deref().unwrap_or(""), 4)
    }
}

fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Msg(m.into()))
}

fn blank(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(|x| x.trim().is_empty())
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn last_n(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    c[c.len().saturating_sub(n)..].iter().collect()
}

/// mx.utils.StringUtil.trim: space, tab, CR, LF and form feed only (not NBSP).
fn trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Bank files are UTF-8, UTF-16 (with a BOM) or Windows-1252.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(b) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(b).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(bytes).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

// ---------------------------------------------------------------------- amounts and ids

/// CurrencyUtilities.parseCurrencyStringWithUnknownDecimalSeparator: "1,234.56", "1.234,56",
/// "-12.3", "(12.00)", "12.00-", "$5" …
pub fn parse_amount(s: &str) -> Option<f64> {
    let s = s.trim();
    let mut neg = false;
    let mut body: String = String::new();
    for c in s.chars() {
        match c {
            '0'..='9' | '.' | ',' => body.push(c),
            '-' | '\u{2212}' | '(' => neg = true,
            _ => {}
        }
    }
    if !body.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // With both separators, the last one is the decimal point. With one kind: repeated means
    // grouping ("1.234.567"); a lone comma followed by exactly three digits is grouping
    // ("1,234"); otherwise it's the decimal point ("12,50", "1.5").
    let normalized = match body.rfind(['.', ',']) {
        None => body,
        Some(i) => {
            let sep = body.as_bytes()[i] as char;
            let other = if sep == '.' { ',' } else { '.' };
            let after = body.len() - i - 1;
            let grouping = !body.contains(other) && (body.matches(sep).count() > 1 || (sep == ',' && after == 3));
            if grouping {
                body.replace([',', '.'], "")
            } else {
                format!("{}.{}", body[..i].replace([',', '.'], ""), &body[i + 1..])
            }
        }
    };
    let v: f64 = normalized.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// CommonFormatters.formatCurrencyNoSymbol for the en-US style YNAB 4 uses in import ids:
/// "-1,234.56".
pub fn format_amount_for_id(amount: f64, decimals: u32) -> String {
    let scale = 10u128.pow(decimals);
    let scaled = (amount.abs() * scale as f64).round() as u128;
    let int = (scaled / scale).to_string();
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut out = String::new();
    if amount < 0.0 && scaled != 0 {
        out.push('-');
    }
    out.push_str(&grouped);
    if decimals > 0 {
        out.push('.');
        out.push_str(&format!("{:0width$}", scaled % scale, width = decimals as usize));
    }
    out
}

/// ImportUtilities.assignYNABIDs: `YNAB:<amount>:<date>:<n>`, n counting from 1 for each
/// amount+date pair in the file.
pub fn assign_ynab_ids(txns: &mut [BankTxn], decimals: u32) {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for t in txns {
        let key = format!("{}:{}", format_amount_for_id(t.amount, decimals), t.date);
        let n = counts.entry(key.clone()).or_insert(0);
        *n += 1;
        t.ynab_id = format!("YNAB:{key}:{n}");
    }
}

// ---------------------------------------------------------------------- OFX / QFX

/// OFXLineReader.getLinesFromLine: splits a line before every '<'.
fn split_at_tags(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        match rest.find('<') {
            Some(0) => match rest[1..].find('<') {
                Some(j) => {
                    out.push(rest[..j + 1].to_string());
                    rest = &rest[j + 1..];
                }
                None => {
                    out.push(rest.to_string());
                    rest = "";
                }
            },
            Some(i) => {
                out.push(rest[..i].to_string());
                rest = &rest[i..];
            }
            None => {
                out.push(rest.to_string());
                rest = "";
            }
        }
    }
    out
}

/// OFXLineReader.cleanUpLines: one tag per line, text continuations joined with a space,
/// entities and CDATA unwrapped.
fn clean_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc: Option<String> = None;
    for raw in text.split(['\n', '\r']) {
        for piece in split_at_tags(raw) {
            let mut p = trim(&piece).to_string();
            if acc.as_deref().is_some_and(|a| !a.is_empty()) && p.starts_with('<') {
                if p.starts_with("<![CDATA[") {
                    p = p.replace("<![CDATA[", "").replace("]]>", "");
                } else {
                    out.push(acc.take().unwrap());
                }
            }
            let p = p
                .replace("&lt;br", "")
                .replace("&lt;b", "")
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace('\u{a0}', " ")
                .replace("&lt", "");
            let p = trim(&p).to_string();
            acc = match acc {
                Some(a) if !a.is_empty() => Some(format!("{a} {p}")),
                _ => Some(p),
            };
        }
    }
    if let Some(a) = acc.filter(|a| !a.is_empty()) {
        out.push(a);
    }
    out
}

struct OfxReader {
    lines: Vec<String>,
    next: usize,
}

impl OfxReader {
    fn read(&mut self) -> Option<String> {
        let l = self.lines.get(self.next).cloned();
        if l.is_some() {
            self.next += 1;
        }
        l
    }

    fn peek(&self) -> Option<&str> {
        self.lines.get(self.next).map(String::as_str)
    }

    /// ImporterOFX.getTagAndValue: the next tag (optionally only one starting with a prefix)
    /// and its text, which may continue over following non-tag lines.
    fn tag(&mut self, prefixes: Option<&[&str]>) -> Result<Option<(String, String)>> {
        let mut line = self.read().map(|l| trim(&l).to_string());
        loop {
            while line.as_deref().is_some_and(|l| l.starts_with("<?") || l.ends_with("?>")) {
                line = self.read().map(|l| trim(&l).to_string());
            }
            let Some(l) = line.take() else { return Ok(None) };
            if !l.starts_with('<') {
                line = self.read().map(|l| trim(&l).to_string());
                continue;
            }
            let Some(idx) = l.find('>') else { return err("Malformed OFX file") };
            let tag = l[..=idx].to_string();
            if let Some(p) = prefixes {
                if !p.iter().any(|x| starts_with_ci(&tag, x)) {
                    line = self.read();
                    continue;
                }
            }
            let mut value = l[idx + 1..].to_string();
            while self.peek().is_some_and(|n| !n.starts_with('<')) {
                value.push_str(&self.read().unwrap());
            }
            return Ok(Some((tag, value)));
        }
    }
}

fn tag_starts_with(tag: &str, prefix: &str) -> bool {
    starts_with_ci(trim(tag), prefix)
}

/// ImporterOFX.parseOfxDate: YYYYMMDD, ignoring any time and zone that follow.
pub fn parse_ofx_date(s: &str) -> Option<String> {
    let s = trim(s);
    let y: i32 = s.get(0..4)?.parse().ok()?;
    let m: u32 = s.get(4..6)?.parse().ok()?;
    let d: u32 = s.get(6..8)?.parse().ok()?;
    NaiveDate::from_ymd_opt(y, m, d).map(|x| x.format("%Y-%m-%d").to_string())
}

fn parse_stmttrn(r: &mut OfxReader) -> Result<Option<BankTxn>> {
    let (mut trntype, mut refnum_used) = (String::new(), false);
    let (mut date, mut amount) = (None, None);
    let mut t = BankTxn { date: String::new(), amount: 0.0, payee: None, memo: None, check_number: None, fitid: None, ynab_id: String::new() };
    let mut check = String::new();
    while let Some((tag, value)) = r.tag(None)? {
        if tag_starts_with(&tag, "</STMTTRN") {
            break;
        }
        match tag.to_ascii_uppercase().as_str() {
            "<TRNTYPE>" => trntype = value,
            "<REFNUM>" if check.is_empty() => {
                refnum_used = true;
                check = value;
            }
            "<CHECKNUM>" if !value.is_empty() => {
                refnum_used = false;
                check = value;
            }
            "<DTPOSTED>" => date = parse_ofx_date(&value),
            "<TRNAMT>" => match parse_amount(&value) {
                Some(a) => amount = Some(a),
                None => return err(format!("Could not parse value: {value}")),
            },
            "<FITID>" => t.fitid = non_empty(trim(&value).to_string()),
            "<PAYEE>" | "<NAME>" => t.payee = non_empty(trim(&value).to_string()),
            "<MEMO>" => t.memo = non_empty(trim(&value).to_string()),
            _ => {}
        }
    }
    // A REFNUM only counts as a check number on CHECK transactions.
    if refnum_used && !trntype.is_empty() && !trntype.eq_ignore_ascii_case("check") {
        check.clear();
    }
    t.check_number = non_empty(trim(&check).to_string());
    match (date, amount) {
        (Some(d), Some(a)) => {
            t.date = d;
            t.amount = a;
            Ok(Some(t))
        }
        _ => Ok(None),
    }
}

fn parse_txn_list(r: &mut OfxReader, end: &str) -> Result<Vec<BankTxn>> {
    let mut out = Vec::new();
    while let Some((tag, _)) = r.tag(None)? {
        if tag_starts_with(&tag, end) {
            break;
        }
        if tag.eq_ignore_ascii_case("<STMTTRN>") {
            if let Some(t) = parse_stmttrn(r)? {
                out.push(t);
            }
        }
    }
    Ok(out)
}

fn parse_statement(r: &mut OfxReader, end: &str, kind: &'static str) -> Result<Statement> {
    let mut s = Statement { kind, fid: None, bank_account_id: None, account_type: None, txns: Vec::new() };
    while let Some((tag, value)) = r.tag(None)? {
        if tag_starts_with(&tag, end) {
            break;
        }
        match tag.to_ascii_uppercase().as_str() {
            "<BANKID>" => s.fid = non_empty(value),
            "<ACCTID>" => s.bank_account_id = non_empty(value),
            "<ACCTTYPE>" => s.account_type = non_empty(value),
            "<BANKTRANLIST>" => s.txns.extend(parse_txn_list(r, "</BANKTRANLIST")?),
            "<INVTRANLIST>" => s.txns.extend(parse_txn_list(r, "</INVTRANLIST")?),
            _ => {}
        }
    }
    Ok(s)
}

fn parse_msgsrs(r: &mut OfxReader, end: &str, start: &str, stmt_end: &str, kind: &'static str) -> Result<Vec<Statement>> {
    let mut out = Vec::new();
    while let Some((tag, _)) = r.tag(None)? {
        if tag_starts_with(&tag, end) {
            break;
        }
        if tag.eq_ignore_ascii_case(start) {
            out.push(parse_statement(r, stmt_end, kind)?);
        }
    }
    Ok(out)
}

/// ImporterOFX.parseFile. Statements without transactions are dropped; every transaction gets
/// its YNAB 4 import id.
pub fn parse_ofx(text: &str, decimals: u32) -> Result<Vec<Statement>> {
    let mut r = OfxReader { lines: clean_lines(text), next: 0 };
    let mut fid: Option<String> = None;
    let mut out = Vec::new();
    while let Some((tag, _)) = r.tag(None)? {
        if tag_starts_with(&tag, "</OFX") {
            break;
        }
        if tag_starts_with(&tag, "<SIGNONMSGSRS") {
            fid = match r.tag(Some(&["<FID>", "</SIGNONMSGSRS"]))? {
                Some((t, v)) if t == "<FID>" => non_empty(v),
                _ => None,
            };
        } else if tag_starts_with(&tag, "<BANKMSGSRS") {
            out.extend(parse_msgsrs(&mut r, "</BANKMSGSRS", "<STMTRS>", "</STMTRS", "bank")?);
        } else if tag_starts_with(&tag, "<CREDITCARDMSGSRS") {
            out.extend(parse_msgsrs(&mut r, "</CREDITCARDMSGSRS", "<CCSTMTRS>", "</CCSTMTRS", "creditCard")?);
        } else if tag_starts_with(&tag, "<INVSTMTMSGSRS") {
            out.extend(parse_msgsrs(&mut r, "</INVSTMTMSGSRS", "<INVSTMTRS>", "</INVSTMTRS", "investment")?);
        }
    }
    if out.is_empty() && !text.to_ascii_uppercase().contains("<OFX") {
        return err("This doesn't look like an OFX/QFX file");
    }
    if let Some(f) = fid {
        for s in &mut out {
            s.fid = Some(f.clone());
        }
    }
    out.retain(|s| !s.txns.is_empty());
    for s in &mut out {
        assign_ynab_ids(&mut s.txns, decimals);
    }
    Ok(out)
}

// ---------------------------------------------------------------------- CSV

/// How the columns of a CSV file map to transaction fields (0-based column indexes).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvMapping {
    pub has_header: bool,
    pub date: Option<usize>,
    pub payee: Option<usize>,
    pub memo: Option<usize>,
    /// One signed amount column …
    pub amount: Option<usize>,
    /// … or separate outflow and inflow columns.
    pub outflow: Option<usize>,
    pub inflow: Option<usize>,
    pub check_number: Option<usize>,
    /// "MDY", "DMY" or "YMD".
    pub date_order: String,
    /// Flip signs (files where charges are positive).
    #[serde(default)]
    pub negate: bool,
}

pub fn read_csv(text: &str) -> Vec<Vec<String>> {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let count = |d: char| {
        let mut q = false;
        first.chars().filter(|&c| {
            if c == '"' {
                q = !q;
            }
            !q && c == d
        }).count()
    };
    let delim = [',', ';', '\t', '|'].into_iter().max_by_key(|d| count(*d)).filter(|d| count(*d) > 0).unwrap_or(',');
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.trim().is_empty() {
            field.clear();
            quoted = true;
        } else if c == delim {
            row.push(field.trim().to_string());
            field.clear();
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(field.trim().to_string());
            field.clear();
            if row.iter().any(|f| !f.is_empty()) {
                rows.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    row.push(field.trim().to_string());
    if row.iter().any(|f| !f.is_empty()) {
        rows.push(row);
    }
    rows
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Reads a date in the given field order ("MDY", "DMY", "YMD"); month names and YYYYMMDD work
/// in any order. Anything after a space or 'T' (a time) is ignored.
pub fn parse_date(s: &str, order: &str) -> Option<String> {
    let s = s.trim();
    let date_part = if s.len() > 10 && s.as_bytes().get(10) == Some(&b'T') { &s[..10] } else { s };
    let mut nums: Vec<&str> = Vec::new();
    let mut month_name: Option<u32> = None;
    let mut start: Option<usize> = None;
    let bytes = date_part.as_bytes();
    let mut tokens: Vec<&str> = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        let alnum = b.is_ascii_alphanumeric();
        match (alnum, start) {
            (true, None) => start = Some(i),
            (false, Some(st)) => {
                tokens.push(&date_part[st..i]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(st) = start {
        tokens.push(&date_part[st..]);
    }
    for t in tokens {
        if t.bytes().all(|b| b.is_ascii_digit()) {
            nums.push(t);
        } else if let Some(m) = MONTHS.iter().position(|m| t.len() >= 3 && t[..3].eq_ignore_ascii_case(m)) {
            month_name.get_or_insert(m as u32 + 1);
        } else if nums.len() >= 3 || (month_name.is_some() && nums.len() >= 2) {
            break; // a trailing time like "12:00 AM"
        }
    }
    let year = |y: &str| -> Option<i32> {
        let v: i32 = y.parse().ok()?;
        Some(match y.len() {
            4 => v,
            1 | 2 => if v < 70 { 2000 + v } else { 1900 + v },
            _ => return None,
        })
    };
    let (y, m, d): (i32, u32, u32) = if let Some(m) = month_name {
        if nums.len() < 2 {
            return None;
        }
        let (a, b) = (nums[0], nums[1]);
        if a.len() == 4 { (year(a)?, m, b.parse().ok()?) } else if b.len() == 4 || order != "YMD" { (year(b)?, m, a.parse().ok()?) } else { (year(a)?, m, b.parse().ok()?) }
    } else if nums.len() == 1 && nums[0].len() == 8 {
        let n = nums[0];
        (n[..4].parse().ok()?, n[4..6].parse().ok()?, n[6..8].parse().ok()?)
    } else if nums.len() >= 3 {
        let (a, b, c) = (nums[0], nums[1], nums[2]);
        match order {
            "DMY" => (year(c)?, b.parse().ok()?, a.parse().ok()?),
            "YMD" => (year(a)?, b.parse().ok()?, c.parse().ok()?),
            _ => (year(c)?, a.parse().ok()?, b.parse().ok()?),
        }
    } else {
        return None;
    };
    NaiveDate::from_ymd_opt(y, m, d).map(|x| x.format("%Y-%m-%d").to_string())
}

fn header_matches(h: &str, keys: &[&str]) -> bool {
    let h = h.to_lowercase();
    keys.iter().any(|k| h == *k || h.contains(k))
}

fn data_rows<'a>(rows: &'a [Vec<String>], m: &CsvMapping) -> &'a [Vec<String>] {
    if m.has_header && !rows.is_empty() {
        &rows[1..]
    } else {
        rows
    }
}

/// Field orders under which every date in the mapped column reads as a real date
/// (ImportUtilities.getValidDateSequencesForDateStrings).
pub fn valid_date_orders(rows: &[Vec<String>], m: &CsvMapping) -> Vec<&'static str> {
    let Some(col) = m.date else { return vec![] };
    ["MDY", "DMY", "YMD"]
        .into_iter()
        .filter(|o| data_rows(rows, m).iter().filter_map(|r| r.get(col)).filter(|c| !c.is_empty()).all(|c| parse_date(c, o).is_some()))
        .collect()
}

/// A starting guess at the column mapping, from header names (or contents when there's no
/// header row). `preferred_order` breaks date-order ties (the budget's locale).
pub fn guess_csv_mapping(rows: &[Vec<String>], preferred_order: &str) -> CsvMapping {
    let mut m = CsvMapping { date_order: preferred_order.to_string(), ..Default::default() };
    let Some(first) = rows.first() else { return m };
    let any_date = |c: &str| ["MDY", "DMY", "YMD"].iter().any(|o| parse_date(c, o).is_some());
    m.has_header = !first.iter().any(|c| any_date(c)) && first.iter().any(|c| c.chars().any(char::is_alphabetic));
    if m.has_header {
        // Keys in priority order; exact header names win over partial ones.
        let find = |keys: &[&str], except: &[Option<usize>]| {
            let free = |i: usize| !except.contains(&Some(i));
            keys.iter().find_map(|k| {
                first.iter().enumerate().find(|(i, h)| free(*i) && h.trim().eq_ignore_ascii_case(k)).map(|(i, _)| i).or_else(|| {
                    first.iter().enumerate().find(|(i, h)| free(*i) && header_matches(h, &[k])).map(|(i, _)| i)
                })
            })
        };
        m.date = find(&["posted date", "post date", "posting date"], &[])
            .or_else(|| find(&["transaction date", "trans date"], &[]))
            .or_else(|| find(&["date"], &[]));
        m.check_number = find(&["check", "cheque", "chk", "check number"], &[m.date]);
        m.payee = find(&["payee"], &[m.date])
            .or_else(|| find(&["description", "merchant", "name", "transaction", "narrative"], &[m.date, m.check_number]));
        m.memo = find(&["memo", "notes", "note", "details", "reference"], &[m.date, m.payee, m.check_number]);
        m.outflow = find(&["outflow", "debit", "withdrawal", "payments", "money out", "spent"], &[m.date, m.payee, m.memo]);
        m.inflow = find(&["inflow", "credit", "deposit", "money in", "received"], &[m.date, m.payee, m.memo, m.outflow]);
        m.amount = find(&["amount", "value"], &[m.date, m.payee, m.memo, m.outflow, m.inflow]);
        if m.amount.is_some() && (m.outflow.is_none() || m.inflow.is_none()) {
            m.outflow = None;
            m.inflow = None;
        }
    } else {
        let ncol = rows.iter().map(Vec::len).max().unwrap_or(0);
        let share = |col: usize, f: &dyn Fn(&str) -> bool| {
            let cells: Vec<&String> = rows.iter().filter_map(|r| r.get(col)).filter(|c| !c.is_empty()).collect();
            if cells.is_empty() { 0.0 } else { cells.iter().filter(|c| f(c.as_str())).count() as f64 / cells.len() as f64 }
        };
        m.date = (0..ncol).find(|&c| share(c, &any_date) > 0.9);
        m.amount = (0..ncol).filter(|&c| Some(c) != m.date).find(|&c| share(c, &|x| parse_amount(x).is_some() && x.contains(['.', ','])) > 0.9);
        m.payee = (0..ncol)
            .filter(|&c| Some(c) != m.date && Some(c) != m.amount)
            .max_by_key(|&c| rows.iter().filter_map(|r| r.get(c)).filter(|x| x.chars().any(char::is_alphabetic)).map(String::len).sum::<usize>());
    }
    let orders = valid_date_orders(rows, &m);
    if !orders.contains(&m.date_order.as_str()) {
        if let Some(o) = orders.first() {
            m.date_order = o.to_string();
        }
    }
    m
}

/// Turns CSV rows into bank transactions. Rows that can't be read come back as messages.
pub fn csv_transactions(rows: &[Vec<String>], m: &CsvMapping, decimals: u32) -> (Vec<BankTxn>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let offset = if m.has_header { 2 } else { 1 };
    let cell = |r: &Vec<String>, c: Option<usize>| c.and_then(|c| r.get(c)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    for (i, r) in data_rows(rows, m).iter().enumerate() {
        let line = i + offset;
        let Some(ds) = cell(r, m.date) else {
            problems.push(format!("Row {line}: no date"));
            continue;
        };
        let Some(date) = parse_date(&ds, &m.date_order) else {
            problems.push(format!("Row {line}: couldn't read the date \"{ds}\""));
            continue;
        };
        let amount = if m.amount.is_some() {
            cell(r, m.amount).and_then(|a| parse_amount(&a))
        } else {
            let o = cell(r, m.outflow).map(|a| parse_amount(&a));
            let n = cell(r, m.inflow).map(|a| parse_amount(&a));
            match (o, n) {
                (Some(None), _) | (_, Some(None)) => None,
                (None, None) => None,
                (o, n) => Some(n.flatten().map(f64::abs).unwrap_or(0.0) - o.flatten().map(f64::abs).unwrap_or(0.0)),
            }
        };
        let Some(amount) = amount else {
            problems.push(format!("Row {line}: couldn't read the amount"));
            continue;
        };
        let amount = if m.negate { -amount } else { amount };
        out.push(BankTxn {
            date,
            amount: if amount == 0.0 { 0.0 } else { amount },
            payee: cell(r, m.payee),
            memo: cell(r, m.memo),
            check_number: cell(r, m.check_number),
            fitid: None,
            ynab_id: String::new(),
        });
    }
    assign_ynab_ids(&mut out, decimals);
    (out, problems)
}

// ---------------------------------------------------------------------- account mappings

/// HashUtilities.hashString: base64(sha256(UTF-8 of value + salt)).
pub fn hash_account_id(bank_account_id: &str, salt: &str) -> String {
    let digest = Sha256::digest(format!("{bank_account_id}{salt}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// HashUtilities.createSalt(8): 32 random bytes, base64.
pub fn new_salt() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A saved "this bank account imports into that YNAB account" choice.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMappingInfo {
    pub id: String,
    pub account_id: Option<String>,
    pub flip_payee_memo: bool,
    pub import_memos: bool,
    pub skip: bool,
}

// ---------------------------------------------------------------------- applying an import

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    pub account_id: String,
    #[serde(default = "yes")]
    pub import_memos: bool,
    #[serde(default)]
    pub flip_payee_memo: bool,
    /// Import transactions dated before the account's starting balance, moving the starting
    /// balance so today's balance doesn't change.
    #[serde(default)]
    pub include_before_start: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedWith {
    pub id: String,
    pub date: String,
    pub payee_id: Option<String>,
    pub memo: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRow {
    /// "new", "matched", "duplicate" or "beforeStart".
    pub status: &'static str,
    pub date: String,
    pub amount: f64,
    /// The payee the transaction ends up with (after rename rules).
    pub payee: Option<String>,
    pub imported_payee: Option<String>,
    pub memo: Option<String>,
    pub category_id: Option<String>,
    pub matched: Option<MatchedWith>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub imported: usize,
    pub matched: usize,
    pub duplicates: usize,
    pub before_start: usize,
    pub start_date: Option<String>,
    pub starting_balance_adjustment: f64,
    pub rows: Vec<ImportRow>,
}

/// Where an imported payee string ends up.
enum PayeeTarget {
    Existing { id: String, name: String, category: Option<String> },
    New(String),
    None,
}

impl PayeeTarget {
    fn name(&self) -> Option<&str> {
        match self {
            PayeeTarget::Existing { name, .. } | PayeeTarget::New(name) => Some(name),
            PayeeTarget::None => None,
        }
    }
}

/// StringUtilities.damerauLevenshteinDistance (optimal string alignment).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return n.max(m);
    }
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + cost);
            }
        }
    }
    d[n][m]
}

fn check_number_match(a: Option<&str>, b: Option<&str>) -> bool {
    let n = |x: Option<&str>| x.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<i64>().ok());
    matches!((n(a), n(b)), (Some(x), Some(y)) if x == y)
}

const MATCHED_PREFIX: &str = "MatchedTransaction/";

impl<'a> Editor<'a> {
    // ------------------------------------------------------------------ account mappings

    /// AccountMappingManager.findTargetAccount: the saved mapping for a bank account, matched on
    /// the salted hash of its number (bank ids compared with whitespace trimmed), falling back to
    /// fid + last four digits for mappings saved without a hash.
    pub fn find_account_mapping(&self, fid: Option<&str>, bank_account_id: &str) -> Option<AccountMappingInfo> {
        let fid = fid.map(trim).unwrap_or("");
        let maps: Vec<&Obj> = self.store.collection("accountMappings").into_iter().filter(|m| !m.tomb()).collect();
        let hashed: Vec<&&Obj> = maps
            .iter()
            .filter(|m| {
                let (h, s) = (m.str_of("hash").unwrap_or(""), m.str_of("salt").unwrap_or(""));
                !h.is_empty() && !s.is_empty() && hash_account_id(bank_account_id, s) == h
            })
            .collect();
        let found = hashed
            .iter()
            .find(|m| trim(m.str_of("fid").unwrap_or("")) == fid)
            .or_else(|| hashed.first())
            .map(|m| **m)
            .or_else(|| {
                maps.iter()
                    .find(|m| {
                        m.str_of("hash").unwrap_or("").is_empty()
                            && trim(m.str_of("fid").unwrap_or("")) == fid
                            && m.str_of("shortenedAccountId") == Some(last_n(bank_account_id, 4).as_str())
                    })
                    .copied()
            })?;
        let target = found.str_of("targetYNABAccountId").filter(|a| self.live(Some(*a)).is_some_and(|o| o.etype() == "account"));
        Some(AccountMappingInfo {
            id: found.id().to_string(),
            account_id: target.map(str::to_string),
            flip_payee_memo: found.b("shouldFlipPayeesMemos"),
            import_memos: found.get("shouldImportMemos") != Some(&Value::Bool(false)),
            skip: found.b("skipImport"),
        })
    }

    /// AccountMappingManager.mapAccount: saves (or updates) the mapping, synced like any record.
    pub fn save_account_mapping(
        &mut self,
        fid: Option<&str>,
        bank_account_id: &str,
        target_account: Option<&str>,
        flip_payee_memo: bool,
        import_memos: bool,
        skip: bool,
    ) -> Result<String> {
        if bank_account_id.is_empty() {
            return err("The file doesn't name its account, so the choice can't be remembered");
        }
        let fields = |t: Option<&str>| {
            vec![
                ("targetYNABAccountId", opt_str(t)),
                ("shouldFlipPayeesMemos", Value::Bool(flip_payee_memo)),
                ("shouldImportMemos", Value::Bool(import_memos)),
                ("skipImport", Value::Bool(skip)),
            ]
        };
        if let Some(m) = self.find_account_mapping(fid, bank_account_id) {
            self.update(&m.id, fields(target_account));
            return Ok(m.id);
        }
        let salt = new_salt();
        let mut o = blank_entity("accountMapping", &new_guid());
        o.insert("fid".into(), opt_str(fid));
        o.insert("shortenedAccountId".into(), Value::String(last_n(bank_account_id, 4)));
        o.insert("hash".into(), Value::String(hash_account_id(bank_account_id, &salt)));
        o.insert("salt".into(), Value::String(salt));
        for (k, v) in fields(target_account) {
            o.insert(k.into(), v);
        }
        self.insert(o, None)
    }

    // ------------------------------------------------------------------ import

    fn starting_balance_txn(&self, account: &str) -> Option<String> {
        let mut found: Vec<&Obj> = self
            .store
            .collection("transactions")
            .into_iter()
            .filter(|t| !t.tomb() && t.str_of("accountId") == Some(account))
            .filter(|t| {
                self.store.get_opt(t.str_of("payeeId")).is_some_and(|p| p.is_null("targetAccountId") && p.str_of("name") == Some("Starting Balance"))
            })
            .collect();
        found.sort_by(|a, b| a.str_of("date").cmp(&b.str_of("date")));
        found.first().map(|t| t.id().to_string())
    }

    /// Account.startDate: the starting balance's date (else the first transaction's).
    pub fn account_start_date(&self, account: &str) -> Option<String> {
        if let Some(id) = self.starting_balance_txn(account) {
            return self.store.get(&id).and_then(|t| t.str_of("date")).map(str::to_string);
        }
        self.store
            .collection("transactions")
            .into_iter()
            .filter(|t| !t.tomb() && t.str_of("accountId") == Some(account))
            .filter_map(|t| t.str_of("date"))
            .min()
            .map(str::to_string)
    }

    /// PayeeManager.applyRulesToImported: an enabled payee with exactly this name, else the first
    /// enabled payee (by name) whose rename rules match, else a new payee.
    fn payee_for_imported(&self, name: Option<&str>) -> PayeeTarget {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else { return PayeeTarget::None };
        let mut payees: Vec<&Obj> = self
            .store
            .collection("payees")
            .into_iter()
            .filter(|p| !p.tomb() && p.is_null("targetAccountId") && p.get("enabled") != Some(&Value::Bool(false)))
            .collect();
        let lower = name.to_lowercase();
        let exact = payees.iter().find(|p| p.str_of("name").is_some_and(|n| n.to_lowercase() == lower)).copied();
        let hit = exact.or_else(|| {
            payees.sort_by_key(|p| p.str_of("name").unwrap_or("").to_lowercase());
            payees.into_iter().find(|p| {
                self.store.children(p.id(), "renameConditions").iter().filter(|c| !c.tomb()).any(|c| {
                    let operand = c.str_of("operand").map(trim).unwrap_or("").to_lowercase();
                    !operand.is_empty()
                        && match c.str_of("operator").unwrap_or("") {
                            "Is" => lower == operand,
                            "Contains" => lower.contains(&operand),
                            "StartsWith" => lower.starts_with(&operand),
                            "EndsWith" => lower.ends_with(&operand),
                            _ => false,
                        }
                })
            })
        });
        match hit {
            Some(p) => {
                let cat = p.str_of("autoFillCategoryId").filter(|c| *c != builtin::SPLIT && self.store.live(Some(c)).is_some()).map(str::to_string);
                PayeeTarget::Existing { id: p.id().to_string(), name: p.str_of("name").unwrap_or(name).to_string(), category: cat }
            }
            None => PayeeTarget::New(name.to_string()),
        }
    }

    fn payee_id_for(&mut self, target: &PayeeTarget) -> Result<Option<String>> {
        Ok(match target {
            PayeeTarget::Existing { id, .. } => Some(id.clone()),
            PayeeTarget::New(n) => Some(self.get_or_create_payee(n, true)?),
            PayeeTarget::None => None,
        })
    }

    fn payee_name(&self, id: Option<&str>) -> Option<String> {
        self.store.get_opt(id).and_then(|p| p.str_of("name")).map(str::to_string)
    }

    /// Whether a transaction can take part in a match without breaking a split transfer.
    fn matchable(&self, t: &Obj) -> bool {
        let other_is_split_line = self.store.get_opt(t.str_of("transferTransactionId")).is_some_and(|o| o.etype() == "subTransaction");
        let has_transfer_lines = self.store.children(t.id(), "subTransactions").iter().any(|s| !s.tomb() && !s.is_null("targetAccountId"));
        !other_is_split_line && !has_transfer_lines
    }

    /// TransactionList.findMatchingTransaction, as the OFX importer calls it: a transaction you
    /// entered (not one imported with an id), same amount, within 10 days (60 when check numbers
    /// agree), not already matched or imported in this batch. Ties go to the matching check
    /// number, then the closest date, then the closest payee name.
    fn find_match(&self, account: &str, date: &str, amount: f64, check: Option<&str>, payee: Option<&str>, batch: &HashSet<String>) -> Option<String> {
        let mut rows: Vec<&Obj> = self
            .store
            .collection("transactions")
            .into_iter()
            .filter(|t| !t.tomb() && t.str_of("accountId") == Some(account))
            .filter(|t| self.r(t.f("amount")) == self.r(amount))
            .filter(|t| {
                let within = if check_number_match(t.str_of("checkNumber"), check) { 60 } else { 10 };
                days_between(t.str_of("date").unwrap_or(""), date).abs() <= within
            })
            .filter(|t| !batch.contains(t.id()))
            .filter(|t| self.store.children(t.id(), "matchedTransactions").is_empty())
            .filter(|t| t.str_of("YNABID").unwrap_or("").is_empty() && t.str_of("FITID").unwrap_or("").is_empty())
            .filter(|t| self.matchable(t))
            .collect();
        rows.sort_by(|a, b| (a.str_of("date"), a.id()).cmp(&(b.str_of("date"), b.id())));
        if rows.len() <= 1 {
            return rows.first().map(|t| t.id().to_string());
        }
        let numeric_check = check.is_some_and(|c| c.trim().parse::<i64>().is_ok());
        let by_check: Vec<&Obj> = if numeric_check { rows.iter().copied().filter(|t| check_number_match(t.str_of("checkNumber"), check)).collect() } else { vec![] };
        let pool = if !by_check.is_empty() {
            by_check
        } else {
            let days = |t: &Obj| days_between(t.str_of("date").unwrap_or(""), date).abs();
            let best = rows.iter().map(|t| days(t)).min().unwrap_or(0);
            rows.into_iter().filter(|t| days(t) == best).collect()
        };
        let mut best: Option<(usize, &Obj)> = None;
        for t in pool {
            let d = match (self.payee_name(t.str_of("payeeId")), payee) {
                (Some(a), Some(b)) => edit_distance(&a, b),
                (Some(a), None) => a.chars().count(),
                (None, _) => usize::MAX,
            };
            if best.is_none_or(|(b, _)| d <= b) {
                best = Some((d, t));
            }
        }
        best.map(|(_, t)| t.id().to_string())
    }

    /// A kept copy of a matched transaction (MatchedTransaction.cloneFrom), id
    /// `MT/<merged id>/MatchedTransaction/<original id>`.
    fn matched_copy(&self, src: &Obj, parent: &str) -> Obj {
        let id = format!("MT/{parent}/{MATCHED_PREFIX}{}", src.id());
        let mut c = blank_entity("matchedTransaction", &id);
        for f in entity_fields("matchedTransaction").unwrap_or_default() {
            if matches!(f, "entityType" | "entityId" | "entityVersion" | "isTombstone" | "madeWithKnowledge" | "isResolvedConflict" | "subTransactions") {
                continue;
            }
            if let Some(v) = src.get(f) {
                c.insert(f.to_string(), v.clone());
            }
        }
        c.insert("parentTransactionIdIfMatched".into(), Value::String(parent.to_string()));
        c
    }

    /// TransactionList.match: one transaction under the bank's id carrying your date, payee,
    /// category, memo and splits and the bank's ids, cleared, unapproved, with both originals kept
    /// in matchedTransactions. Your original is deleted.
    fn merge_match(&mut self, user_id: &str, bank: Obj) -> Result<String> {
        let user = self.store.get(user_id).cloned().ok_or_else(|| Error::Msg(format!("No such transaction {user_id}")))?;
        let new_id = bank.id().to_string();
        let g = |o: &Obj, k: &str| o.get(k).cloned().unwrap_or(Value::Null);
        let cleared = if user.str_of("cleared") == Some("Reconciled") { "Reconciled" } else { "Cleared" };
        let check = if user.str_of("checkNumber").is_some_and(|c| !c.is_empty()) { g(&user, "checkNumber") } else { g(&bank, "checkNumber") };
        let user_has_payee = !user.is_null("payeeId");
        let (payee, target, transfer) = if user_has_payee {
            (g(&user, "payeeId"), g(&user, "targetAccountId"), g(&user, "transferTransactionId"))
        } else {
            (g(&bank, "payeeId"), Value::Null, Value::Null)
        };
        let merged = self.new_transaction(
            &new_id,
            vec![
                ("accountId", g(&bank, "accountId")),
                ("date", g(&user, "date")),
                ("amount", g(&bank, "amount")),
                ("payeeId", payee),
                ("categoryId", g(&user, "categoryId")),
                ("memo", g(&user, "memo")),
                ("cleared", Value::String(cleared.into())),
                ("flag", g(&user, "flag")),
                ("checkNumber", check),
                ("accepted", Value::Bool(false)),
                ("source", Value::String("Imported".into())),
                ("YNABID", g(&bank, "YNABID")),
                ("FITID", g(&bank, "FITID")),
                ("importedPayee", g(&bank, "importedPayee")),
                ("targetAccountId", target),
                ("transferTransactionId", transfer.clone()),
            ],
        );
        self.insert(merged, None)?;
        let subs: Vec<Obj> = self.store.children(user_id, "subTransactions").into_iter().filter(|s| !s.tomb()).cloned().collect();
        for s in subs {
            let mut sub = blank_entity("subTransaction", &new_guid());
            for k in ["amount", "categoryId", "memo", "checkNumber"] {
                sub.insert(k.into(), g(&s, k));
            }
            sub.insert("parentTransactionId".into(), Value::String(new_id.clone()));
            self.insert(sub, Some(&new_id))?;
        }
        let bank_copy = self.matched_copy(&bank, &new_id);
        let user_copy = self.matched_copy(&user, &new_id);
        self.insert(bank_copy, Some(&new_id))?;
        self.insert(user_copy, Some(&new_id))?;
        self.update(user_id, vec![("isTombstone", Value::Bool(true))]);
        if let Some(other) = transfer.as_str().filter(|o| self.live(Some(*o)).is_some_and(|x| x.etype() == "transaction")) {
            let other = other.to_string();
            self.update(&other, vec![("transferTransactionId", Value::String(new_id.clone()))]);
        }
        Ok(new_id)
    }

    /// Imports bank transactions into an account the way YNAB 4 does (ImportUtilities.importAccount):
    /// skips ones already imported (same import id, or same FITID and amount within 4 days),
    /// leaves out ones dated before the account's start unless asked, applies payee rename rules
    /// and auto-fill categories, and matches the rest to transactions you entered.
    pub fn import_bank_transactions(&mut self, txns: &[BankTxn], o: &ImportOptions) -> Result<ImportSummary> {
        self.require(&o.account_id, "account", "account")?;
        let account = o.account_id.as_str();
        let on_budget = self.on_budget(account);
        let start = self.account_start_date(account);
        let flip = o.flip_payee_memo || (txns.iter().all(|t| blank(&t.payee)) && txns.iter().any(|t| !blank(&t.memo)));

        let mut by_ynabid: HashSet<String> = HashSet::new();
        let mut by_fitid: HashMap<String, Vec<String>> = HashMap::new();
        for t in self.store.collection("transactions") {
            if t.tomb() || t.str_of("accountId") != Some(account) {
                continue;
            }
            if let Some(y) = t.str_of("YNABID").filter(|y| !y.is_empty()) {
                by_ynabid.insert(y.to_string());
            }
            if let Some(f) = t.str_of("FITID").filter(|f| !f.is_empty()) {
                by_fitid.entry(f.to_string()).or_default().push(t.id().to_string());
            }
        }

        let mut out = ImportSummary { start_date: start.clone(), ..Default::default() };
        let mut batch: HashSet<String> = HashSet::new();
        let mut before_start_total = 0.0;
        let mut earliest_included: Option<String> = None;
        for t in txns {
            let amount = self.r(t.amount);
            let (payee_text, memo) = if flip {
                (t.memo.clone(), if o.import_memos { t.payee.clone() } else { None })
            } else {
                (t.payee.clone(), if o.import_memos { t.memo.clone() } else { None })
            };
            let target = self.payee_for_imported(payee_text.as_deref());
            let category = match &target {
                PayeeTarget::Existing { category, .. } if on_budget => category.clone(),
                _ => None,
            };
            let mut row = ImportRow {
                status: "new",
                date: t.date.clone(),
                amount,
                payee: target.name().map(str::to_string),
                imported_payee: payee_text.clone(),
                memo: memo.clone(),
                category_id: category.clone(),
                matched: None,
            };

            if by_ynabid.contains(&t.ynab_id) {
                row.status = "duplicate";
                out.duplicates += 1;
                out.rows.push(row);
                continue;
            }
            if let Some(f) = t.fitid.as_deref().filter(|f| !f.is_empty()) {
                let hit = by_fitid.get(f).and_then(|ids| {
                    ids.iter()
                        .find(|id| {
                            self.live(Some(id.as_str())).is_some_and(|e| {
                                self.r(e.f("amount")) == amount && days_between(e.str_of("date").unwrap_or(""), &t.date).abs() <= 4
                            })
                        })
                        .cloned()
                });
                if let Some(id) = hit {
                    self.update(&id, vec![("YNABID", Value::String(t.ynab_id.clone()))]);
                    by_ynabid.insert(t.ynab_id.clone());
                    row.status = "duplicate";
                    out.duplicates += 1;
                    out.rows.push(row);
                    continue;
                }
            }
            if start.as_deref().is_some_and(|s| t.date.as_str() < s) {
                if !o.include_before_start {
                    row.status = "beforeStart";
                    out.before_start += 1;
                    out.rows.push(row);
                    continue;
                }
                before_start_total = self.r(before_start_total + amount);
                if earliest_included.as_deref().is_none_or(|e| t.date.as_str() < e) {
                    earliest_included = Some(t.date.clone());
                }
            }

            let mut id = format!("Transaction/{}", t.ynab_id);
            let mut n = 0;
            while self.store.contains(&id) {
                id = format!("Transaction/{}_{n}", t.ynab_id);
                n += 1;
            }
            let check = t.check_number.as_deref().filter(|c| !c.is_empty());
            let matched = self.find_match(account, &t.date, amount, check, target.name(), &batch);
            // Only create a payee the transaction will actually use.
            let needs_payee = match &matched {
                Some(m) => self.store.get(m).is_some_and(|u| u.is_null("payeeId")),
                None => true,
            };
            let payee_id = if needs_payee {
                self.payee_id_for(&target)?
            } else if let PayeeTarget::Existing { id, .. } = &target {
                Some(id.clone())
            } else {
                None
            };
            let bank = self.new_transaction(
                &id,
                vec![
                    ("accountId", Value::String(account.to_string())),
                    ("date", Value::String(t.date.clone())),
                    ("amount", num(amount)),
                    ("payeeId", opt_str(payee_id.as_deref())),
                    ("categoryId", opt_str(category.as_deref())),
                    ("memo", opt_str(memo.as_deref().filter(|m| !m.is_empty()))),
                    ("cleared", Value::String("Cleared".into())),
                    ("accepted", Value::Bool(false)),
                    ("source", Value::String("Imported".into())),
                    ("YNABID", Value::String(t.ynab_id.clone())),
                    ("FITID", opt_str(t.fitid.as_deref())),
                    ("importedPayee", opt_str(payee_text.as_deref().filter(|p| !p.is_empty()))),
                    ("checkNumber", opt_str(check)),
                ],
            );
            let new_id = match matched {
                Some(user_id) => {
                    let u = self.store.get(&user_id).unwrap();
                    row.matched = Some(MatchedWith {
                        id: user_id.clone(),
                        date: u.str_of("date").unwrap_or("").to_string(),
                        payee_id: u.str_of("payeeId").map(str::to_string),
                        memo: u.str_of("memo").map(str::to_string),
                    });
                    row.status = "matched";
                    out.matched += 1;
                    self.merge_match(&user_id, bank)?
                }
                None => {
                    out.imported += 1;
                    self.insert(bank, None)?
                }
            };
            batch.insert(new_id);
            by_ynabid.insert(t.ynab_id.clone());
            out.rows.push(row);
        }

        if o.include_before_start && before_start_total != 0.0 {
            if let Some(sb) = self.starting_balance_txn(account) {
                let current = self.store.get(&sb).map(|t| t.f("amount")).unwrap_or(0.0);
                let adjusted = self.r(current - before_start_total);
                let mut fields = vec![("amount", num(adjusted))];
                if let Some(e) = &earliest_included {
                    fields.push(("date", Value::String(e.clone())));
                }
                self.update(&sb, fields);
                out.starting_balance_adjustment = self.r(-before_start_total);
            }
        }
        Ok(out)
    }

    /// Undoes a match (TransactionList.extractMatchedTransactionAsRealTransaction): your original
    /// transaction comes back and this one goes back to being just the bank's, unapproved.
    pub fn unmatch(&mut self, id: &str) -> Result<()> {
        self.require(id, "transaction", "transaction")?;
        let children: Vec<Obj> = self.store.children(id, "matchedTransactions").into_iter().cloned().collect();
        if children.is_empty() {
            return err("This transaction isn't matched");
        }
        let is_bank = |c: &Obj| !c.str_of("YNABID").unwrap_or("").is_empty() || !c.str_of("FITID").unwrap_or("").is_empty();
        let bank = children.iter().find(|c| is_bank(c)).cloned();
        let user = children.iter().find(|c| !is_bank(c)).cloned();
        let merged = self.store.get(id).cloned().unwrap();
        let g = |o: &Obj, k: &str| o.get(k).cloned().unwrap_or(Value::Null);

        if let Some(u) = &user {
            let orig = u.id().rsplit(MATCHED_PREFIX).next().unwrap_or("").to_string();
            let restored = if !orig.is_empty() && self.store.get(&orig).is_some_and(|o| o.etype() == "transaction") {
                self.update(&orig, vec![("isTombstone", Value::Bool(false))]);
                orig
            } else {
                let nid = if orig.is_empty() || self.store.contains(&orig) { new_guid() } else { orig };
                let mut t = self.new_transaction(&nid, vec![]);
                for k in ["accountId", "date", "amount", "payeeId", "categoryId", "memo", "cleared", "flag", "checkNumber", "targetAccountId", "transferTransactionId", "dateEnteredFromSchedule"] {
                    t.insert(k.into(), g(u, k));
                }
                t.insert("accepted".into(), Value::Bool(true));
                self.insert(t, None)?
            };
            if let Some(other) = merged.str_of("transferTransactionId").filter(|o| self.live(Some(*o)).is_some_and(|x| x.etype() == "transaction")) {
                let other = other.to_string();
                self.update(&other, vec![("transferTransactionId", Value::String(restored.clone()))]);
                self.update(&restored, vec![("transferTransactionId", Value::String(other)), ("targetAccountId", g(&merged, "targetAccountId"))]);
            }
        }

        let subs: Vec<String> = self.store.children(id, "subTransactions").iter().map(|s| s.id().to_string()).collect();
        for s in subs {
            self.store.remove_inline_child(&s);
        }
        for c in &children {
            self.store.remove_inline_child(c.id());
        }
        let src = bank.unwrap_or(merged.clone());
        let payee = if src.is_null("payeeId") {
            let target = self.payee_for_imported(src.str_of("importedPayee"));
            self.payee_id_for(&target)?.map(Value::String).unwrap_or(Value::Null)
        } else {
            g(&src, "payeeId")
        };
        let mut fields = vec![
            ("payeeId", payee),
            ("targetAccountId", Value::Null),
            ("transferTransactionId", Value::Null),
            ("accepted", Value::Bool(false)),
        ];
        for k in ["date", "amount", "categoryId", "memo", "cleared", "flag", "checkNumber", "source", "YNABID", "FITID", "importedPayee"] {
            fields.push((k, g(&src, k)));
        }
        self.update(id, fields);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("-1,234.56"), Some(-1234.56));
        assert_eq!(parse_amount("1.234,56"), Some(1234.56));
        assert_eq!(parse_amount("1,234"), Some(1234.0));
        assert_eq!(parse_amount("12,5"), Some(12.5));
        assert_eq!(parse_amount("(12.00)"), Some(-12.0));
        assert_eq!(parse_amount("12.00-"), Some(-12.0));
        assert_eq!(parse_amount("$5"), Some(5.0));
        assert_eq!(parse_amount("+.5"), Some(0.5));
        assert_eq!(parse_amount("1.234.567"), Some(1234567.0));
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(format_amount_for_id(-1234.5, 2), "-1,234.50");
        assert_eq!(format_amount_for_id(12345678.0, 2), "12,345,678.00");
        assert_eq!(format_amount_for_id(-0.0, 2), "0.00");
        assert_eq!(format_amount_for_id(0.05, 2), "0.05");
        assert_eq!(format_amount_for_id(-1500.0, 0), "-1,500");
    }

    #[test]
    fn dates() {
        assert_eq!(parse_ofx_date("20261031120000.000[-5:EST]").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_ofx_date("2026103"), None);
        assert_eq!(parse_date("10/31/2026", "MDY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("31/10/2026", "DMY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("2026-10-31", "YMD").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("2026-10-31T00:00:00", "YMD").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("10/31/26", "MDY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("31-Oct-2026", "MDY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("Oct 31, 2026", "DMY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("20261031", "MDY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("10/31/2026 12:00 AM", "MDY").as_deref(), Some("2026-10-31"));
        assert_eq!(parse_date("31/10/2026", "MDY"), None);
    }

    #[test]
    fn ofx_lines_follow_ynab() {
        assert_eq!(split_at_tags("<A>1<B>2"), vec!["<A>1", "<B>2"]);
        assert_eq!(split_at_tags("x<A>"), vec!["x", "<A>"]);
        let l = clean_lines("OFXHEADER:100\nDATA:OFXSGML\n<OFX>\n<NAME>JOE&amp;S\nCAFE\n<MEMO><![CDATA[a b]]>\n");
        assert_eq!(l, vec!["OFXHEADER:100 DATA:OFXSGML", "<OFX>", "<NAME>JOE&S CAFE", "<MEMO> a b"]);
    }

    #[test]
    fn ids_and_distance() {
        let mut t = vec![
            BankTxn { date: "2026-10-01".into(), amount: -4.5, payee: None, memo: None, check_number: None, fitid: None, ynab_id: String::new() },
            BankTxn { date: "2026-10-01".into(), amount: -4.5, payee: None, memo: None, check_number: None, fitid: None, ynab_id: String::new() },
            BankTxn { date: "2026-10-02".into(), amount: -1234.0, payee: None, memo: None, check_number: None, fitid: None, ynab_id: String::new() },
        ];
        assign_ynab_ids(&mut t, 2);
        assert_eq!(t[0].ynab_id, "YNAB:-4.50:2026-10-01:1");
        assert_eq!(t[1].ynab_id, "YNAB:-4.50:2026-10-01:2");
        assert_eq!(t[2].ynab_id, "YNAB:-1,234.00:2026-10-02:1");
        assert_eq!(edit_distance("ca", "ac"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(hash_account_id("123456789", "c2FsdA==").len(), 44);
    }

    #[test]
    fn csv_reading_and_guessing() {
        let text = "Posted Date,Description,Amount,Check Number\r\n10/30/2026,\"COFFEE, INC\",-4.50,\r\n10/31/2026,PAYROLL,\"1,500.00\",\r\n11/01/2026,CHECK 101,-80.00,101\r\n";
        let rows = read_csv(text);
        assert_eq!(rows[1], vec!["10/30/2026", "COFFEE, INC", "-4.50", ""]);
        let m = guess_csv_mapping(&rows, "MDY");
        assert!(m.has_header);
        assert_eq!((m.date, m.payee, m.amount, m.check_number), (Some(0), Some(1), Some(2), Some(3)));
        let (txns, problems) = csv_transactions(&rows, &m, 2);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(txns.len(), 3);
        assert_eq!(txns[1].amount, 1500.0);
        assert_eq!(txns[2].check_number.as_deref(), Some("101"));

        let text = "Date;Payee;Debit;Credit\n31/10/2026;Shop;12,50;\n01/11/2026;Salary;;1.000,00\n";
        let rows = read_csv(text);
        let m = guess_csv_mapping(&rows, "MDY");
        assert_eq!(m.date_order, "DMY");
        assert_eq!((m.outflow, m.inflow, m.amount), (Some(2), Some(3), None));
        let (txns, _) = csv_transactions(&rows, &m, 2);
        assert_eq!(txns[0].amount, -12.5);
        assert_eq!(txns[1].amount, 1000.0);
        assert_eq!(txns[1].date, "2026-11-01");
    }
}
