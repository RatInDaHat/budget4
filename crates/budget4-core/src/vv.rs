//! Version vectors (com.ynab.entityFramework.sync.VersionVector).
//!
//! Serialized as "A-90031,B-49421,C-9595": elements sorted by device id. An entity's version is a
//! single element ("A-12345").

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element<'a> {
    pub device: &'a str,
    pub counter: i64,
}

/// Parses "A-123" into (device, counter).
pub fn parse_element(s: &str) -> Option<(String, i64)> {
    let i = s.rfind('-')?;
    if i == 0 {
        return None;
    }
    let counter = s[i + 1..].parse().ok()?;
    Some((s[..i].to_string(), counter))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionVector {
    counters: BTreeMap<String, i64>,
}

impl VersionVector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(s: Option<&str>) -> Self {
        let mut vv = Self::new();
        for part in s.unwrap_or("").split(',').filter(|p| !p.is_empty()) {
            if let Some((d, c)) = parse_element(part) {
                vv.counters.insert(d, c);
            }
        }
        vv
    }

    pub fn get(&self, device: &str) -> i64 {
        self.counters.get(device).copied().unwrap_or(0)
    }

    pub fn has(&self, device: &str) -> bool {
        self.counters.contains_key(device)
    }

    pub fn set(&mut self, device: &str, counter: i64) {
        self.counters.insert(device.to_string(), counter);
    }

    /// Ensures an element exists (YNAB's getVersionForDevice(id, true)).
    pub fn ensure(&mut self, device: &str) {
        self.counters.entry(device.to_string()).or_insert(0);
    }

    pub fn increment(&mut self, device: &str) -> i64 {
        let c = self.counters.entry(device.to_string()).or_insert(0);
        *c += 1;
        *c
    }

    pub fn devices(&self) -> impl Iterator<Item = &str> {
        self.counters.keys().map(String::as_str)
    }

    /// Every element of `other` is ≤ the matching element here.
    pub fn dominates(&self, other: &VersionVector) -> bool {
        other.counters.iter().all(|(d, &c)| self.get(d) >= c)
    }

    pub fn strictly_dominates(&self, other: &VersionVector) -> bool {
        self.dominates(other) && self.counters.iter().any(|(d, &c)| c > other.get(d))
    }

    pub fn dominates_element(&self, device: &str, counter: i64) -> bool {
        self.get(device) >= counter
    }

    /// Element-wise maximum (incorporateKnowledge).
    pub fn incorporate(&mut self, other: &VersionVector) {
        for (d, &c) in &other.counters {
            let e = self.counters.entry(d.clone()).or_insert(c);
            if *e < c {
                *e = c;
            }
        }
    }

    /// Element-wise minimum; elements missing from `other` become 0 (takeMinimums).
    pub fn take_minimums(&mut self, other: &VersionVector) {
        for d in other.counters.keys() {
            self.ensure(d);
        }
        for (d, c) in self.counters.iter_mut() {
            *c = if other.has(d) { (*c).min(other.get(d)) } else { 0 };
        }
    }

    pub fn equals(&self, other: &VersionVector) -> bool {
        self.dominates(other) && other.dominates(self)
    }

    /// Sort order used to pick the most up-to-date full file (VersionVector.compareForSort).
    pub fn compare_for_sort(a: &VersionVector, b: &VersionVector) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        let ea: Vec<(&String, &i64)> = a.counters.iter().filter(|(_, &c)| c != 0).collect();
        let eb: Vec<(&String, &i64)> = b.counters.iter().filter(|(_, &c)| c != 0).collect();
        let (mut i, mut j) = (0, 0);
        loop {
            if i >= ea.len() {
                return if j >= eb.len() { Equal } else { Less };
            }
            if j >= eb.len() {
                return Greater;
            }
            match ea[i].0.cmp(eb[j].0) {
                Equal => {}
                o => return o.reverse(),
            }
            match ea[i].1.cmp(eb[j].1) {
                Equal => {}
                o => return o,
            }
            i += 1;
            j += 1;
        }
    }
}

impl fmt::Display for VersionVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (d, c) in &self.counters {
            if !first {
                f.write_str(",")?;
            }
            first = false;
            write!(f, "{d}-{c}")?;
        }
        Ok(())
    }
}

/// compareEntities / compareTwoElements: -1 incoming wins, 1 mine wins, 0 conflict.
pub fn compare_versions(
    mine: (&str, i64),
    my_knowledge: &VersionVector,
    incoming: (&str, i64),
    incoming_knowledge: &VersionVector,
) -> i32 {
    if incoming_knowledge.dominates_element(mine.0, mine.1) {
        -1
    } else if my_knowledge.dominates_element(incoming.0, incoming.1) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compare_merge() {
        let mut a = VersionVector::parse(Some("A-5,B-3"));
        let b = VersionVector::parse(Some("A-4,B-3,C-1"));
        assert!(!a.dominates(&b));
        a.incorporate(&b);
        assert_eq!(a.to_string(), "A-5,B-3,C-1");
        assert!(a.dominates(&b));
        assert!(a.strictly_dominates(&b));
    }
}
