//! The in-memory budget (SyncAwareObjectContext): every syncable record indexed by entityId, the
//! tree layout of Budget.yfull, and YNAB's versioning and merge rules.
//!
//! Records are stored flat. A parent's child-array fields (`subCategories`, `subTransactions`, …)
//! hold the children's ids while loaded and are expanded back into objects when saving.

use std::collections::HashMap;

use serde_json::Value;

use crate::json::{Obj, ObjExt};
use crate::schema::{
    aggregate_child, aggregate_child_properties, blank_entity, child_array_fields, child_spec, entity_fields, inline_child,
    is_inline_array, root_collection_for, root_type_for,
};
use crate::vv::{compare_versions, parse_element, VersionVector};

#[derive(Debug, Clone)]
enum RootSlot {
    Collection(Vec<String>),
    Single(String),
    Raw(Value),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConflictRecord {
    pub entity_id: String,
    pub entity_type: String,
    pub winner: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyResult {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub conflicts: Vec<ConflictRecord>,
}

#[derive(Debug, Clone)]
pub struct Store {
    root: Vec<(String, RootSlot)>,
    ents: HashMap<String, Obj>,
    parent_of: HashMap<String, String>,
    /// Records kept verbatim (duplicates, malformed entries) under synthetic ids.
    raw: HashMap<String, Value>,
    orphans: Vec<String>,
    raw_counter: usize,
    pub knowledge: VersionVector,
    /// This device's short id ("A", "B", …).
    pub device: String,
    /// Bumped on every change, so derived views know when to recompute.
    pub revision: u64,
    /// While an edit is being recorded for undo: each touched record as it was before.
    journal: Option<Journal>,
}

#[derive(Debug, Clone, Default)]
struct Journal {
    order: Vec<String>,
    before: HashMap<String, Option<Obj>>,
    /// A record was stamped without being remembered first, so the edit can't be undone safely.
    missed: bool,
}

/// One undoable edit: every record it changed, as it was before (`None` = the edit created it),
/// and the version the edit left it at (undo refuses if something changed it since).
#[derive(Debug, Clone, Default)]
pub struct UndoStep {
    entries: Vec<(String, Option<Obj>, String)>,
    /// False when the edit couldn't be fully recorded.
    pub complete: bool,
}

impl UndoStep {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

const RAW_PREFIX: &str = "\u{0}raw:";

impl Store {
    pub fn from_root(root: Value, device: &str) -> Self {
        let mut s = Store {
            root: Vec::new(),
            ents: HashMap::new(),
            parent_of: HashMap::new(),
            raw: HashMap::new(),
            orphans: Vec::new(),
            raw_counter: 0,
            knowledge: VersionVector::new(),
            device: device.to_string(),
            revision: 0,
            journal: None,
        };
        let Value::Object(map) = root else { return s };
        for (k, v) in map {
            let slot = match (root_type_for(&k), v) {
                (Some(_), Value::Array(items)) => {
                    RootSlot::Collection(items.into_iter().map(|item| s.register_any(item, None)).collect())
                }
                (None, Value::Object(o)) if k == "budgetMetaData" && !o.id().is_empty() => RootSlot::Single(s.register_tree(o, None)),
                (_, v) => RootSlot::Raw(v),
            };
            s.root.push((k, slot));
        }
        s.knowledge = VersionVector::parse(s.file_meta().and_then(|m| m.str_of("currentKnowledge")));
        s.knowledge.ensure(device);
        s
    }

    fn register_any(&mut self, v: Value, parent: Option<&str>) -> String {
        match v {
            Value::Object(o) => self.register_tree(o, parent),
            other => self.keep_raw(other),
        }
    }

    fn keep_raw(&mut self, v: Value) -> String {
        self.raw_counter += 1;
        let id = format!("{RAW_PREFIX}{}", self.raw_counter);
        self.raw.insert(id.clone(), v);
        id
    }

    /// Indexes a record and its nested children, replacing child objects with their ids.
    fn register_tree(&mut self, mut o: Obj, parent: Option<&str>) -> String {
        let id = o.id().to_string();
        if id.is_empty() || self.ents.contains_key(&id) {
            return self.keep_raw(Value::Object(o));
        }
        let etype = o.etype().to_string();
        for &field in child_array_fields(&etype) {
            // Replace in place: Map::remove would reorder keys and break byte-exact saving.
            if let Some(slot) = o.get_mut(field) {
                match std::mem::take(slot) {
                    Value::Array(items) => {
                        let ids: Vec<Value> = items.into_iter().map(|c| Value::String(self.register_any(c, Some(&id)))).collect();
                        *slot = Value::Array(ids);
                    }
                    other => *slot = other,
                }
            }
        }
        if let Some(p) = parent {
            self.parent_of.insert(id.clone(), p.to_string());
        }
        self.ents.insert(id.clone(), o);
        id
    }

    fn unindex_tree(&mut self, id: &str) {
        if let Some(o) = self.ents.remove(id) {
            for &field in child_array_fields(o.etype()) {
                for c in child_ids(&o, field) {
                    self.unindex_tree(&c);
                }
            }
        }
        self.raw.remove(id);
        self.parent_of.remove(id);
        self.orphans.retain(|x| x != id);
    }

    // ------------------------------------------------------------------ access

    pub fn get(&self, id: &str) -> Option<&Obj> {
        self.ents.get(id)
    }

    pub fn get_opt(&self, id: Option<&str>) -> Option<&Obj> {
        id.and_then(|i| self.ents.get(i))
    }

    /// Live (non-deleted) record.
    pub fn live(&self, id: Option<&str>) -> Option<&Obj> {
        self.get_opt(id).filter(|o| !o.tomb())
    }

    /// Mutable access for an edit (remembered for undo; call `touch` afterwards).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Obj> {
        self.remember(id);
        self.ents.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ents.contains_key(id)
    }

    pub fn parent(&self, id: &str) -> Option<&str> {
        self.parent_of.get(id).map(String::as_str)
    }

    pub fn all(&self) -> impl Iterator<Item = &Obj> {
        self.ents.values()
    }

    pub fn len(&self) -> usize {
        self.ents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ents.is_empty()
    }

    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    /// Records of a root collection ("transactions", "accounts", …) in file order.
    pub fn collection(&self, key: &str) -> Vec<&Obj> {
        self.root
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, slot)| match slot {
                RootSlot::Collection(ids) => ids.iter().filter_map(|i| self.ents.get(i)).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default()
    }

    /// Children of `id` held in `field` (e.g. a master category's subCategories).
    pub fn children(&self, id: &str, field: &str) -> Vec<&Obj> {
        self.ents.get(id).map(|o| child_ids(o, field).iter().filter_map(|c| self.ents.get(c)).collect()).unwrap_or_default()
    }

    pub fn budget_meta(&self) -> Option<&Obj> {
        self.root.iter().find_map(|(_, s)| match s {
            RootSlot::Single(id) => self.ents.get(id),
            _ => None,
        })
    }

    pub fn file_meta(&self) -> Option<&Obj> {
        self.root.iter().find_map(|(k, s)| match (k.as_str(), s) {
            ("fileMetaData", RootSlot::Raw(Value::Object(o))) => Some(o),
            _ => None,
        })
    }

    // ------------------------------------------------------------------ versioning

    /// Stamps a local change (updateVOVersion), re-versioning the parent of an inline child.
    pub fn touch(&mut self, id: &str, as_conflict_resolution: bool) {
        if self.journal.is_some() && self.ents.contains_key(id) {
            let root = self.journal_root(id);
            if let Some(j) = self.journal.as_mut() {
                j.missed |= !j.before.contains_key(&root);
            }
        }
        let counter = self.knowledge.increment(&self.device);
        let version = format!("{}-{}", self.device, counter);
        let Some(o) = self.ents.get_mut(id) else { return };
        o.insert("entityVersion".into(), Value::String(version));
        o.insert("isResolvedConflict".into(), Value::Bool(as_conflict_resolution));
        let inline = inline_child(o.etype()).is_some();
        self.revision += 1;
        if inline {
            if let Some(p) = self.parent_of.get(id).cloned() {
                self.touch(&p, as_conflict_resolution);
            }
        }
    }

    /// Ids of root records and aggregate children changed by this device after `since`.
    pub fn changes_since(&self, since: i64) -> Vec<String> {
        let mut out: Vec<(i64, &String)> = self
            .ents
            .iter()
            .filter(|(_, o)| inline_child(o.etype()).is_none())
            .filter_map(|(id, o)| {
                let (d, c) = parse_element(o.str_of("entityVersion")?)?;
                (d == self.device && c > since).then_some((c, id))
            })
            .collect();
        out.sort();
        out.into_iter().map(|(_, id)| id.clone()).collect()
    }

    // ------------------------------------------------------------------ structural edits

    /// Adds a new record (root, aggregate child, or inline child under `parent`) and stamps it.
    pub fn insert(&mut self, o: Obj, parent: Option<&str>) -> Result<String, String> {
        let id = o.id().to_string();
        if id.is_empty() || self.ents.contains_key(&id) {
            return Err(format!("Duplicate entityId {id}"));
        }
        match inline_child(o.etype()) {
            Some(spec) => {
                if let Some(p) = parent.map(str::to_string).or_else(|| o.str_of(spec.parent_field).map(str::to_string)) {
                    self.remember(&p);
                }
            }
            None => self.remember_new(&id),
        }
        let id = self.attach(o, parent)?;
        self.touch(&id, false);
        Ok(id)
    }

    fn attach(&mut self, o: Obj, parent: Option<&str>) -> Result<String, String> {
        let etype = o.etype().to_string();
        if let Some(spec) = child_spec(&etype) {
            let pid = parent.map(str::to_string).or_else(|| o.str_of(spec.parent_field).map(str::to_string));
            match pid.filter(|p| self.ents.contains_key(p)) {
                Some(p) => {
                    let id = self.register_tree(o, Some(&p));
                    push_child(self.ents.get_mut(&p).unwrap(), spec.array_field, &id);
                    Ok(id)
                }
                None => {
                    let id = self.register_tree(o, None);
                    self.orphans.push(id.clone());
                    Ok(id)
                }
            }
        } else if etype == "budgetMetaData" {
            let id = self.register_tree(o, None);
            match self.root.iter_mut().find(|(k, _)| k == "budgetMetaData") {
                Some((_, slot)) => *slot = RootSlot::Single(id.clone()),
                None => self.root.push(("budgetMetaData".into(), RootSlot::Single(id.clone()))),
            }
            Ok(id)
        } else {
            let key = root_collection_for(&etype).ok_or_else(|| format!("Don't know where to store entityType {etype}"))?;
            let id = self.register_tree(o, None);
            match self.root.iter_mut().find(|(k, _)| k == key) {
                Some((_, RootSlot::Collection(ids))) => ids.push(id.clone()),
                Some((_, slot)) => *slot = RootSlot::Collection(vec![id.clone()]),
                None => self.root.push((key.to_string(), RootSlot::Collection(vec![id.clone()]))),
            }
            Ok(id)
        }
    }

    /// Sets fields on a record; stamps it only if something actually changed.
    pub fn update(&mut self, id: &str, fields: Vec<(&str, Value)>) -> bool {
        self.remember(id);
        let Some(o) = self.ents.get_mut(id) else { return false };
        let mut changed = false;
        for (k, v) in fields {
            if !o.get(k).is_some_and(|cur| crate::json::value_eq(cur, &v)) {
                o.insert(k.to_string(), v);
                changed = true;
            }
        }
        if changed {
            self.touch(id, false);
        }
        changed
    }

    /// Removes an inline child (sub-transaction) outright, re-versioning its parent.
    pub fn remove_inline_child(&mut self, id: &str) {
        self.remember(id);
        let Some(spec) = self.ents.get(id).and_then(|o| inline_child(o.etype())) else { return };
        let Some(p) = self.parent_of.get(id).cloned() else { return };
        if let Some(po) = self.ents.get_mut(&p) {
            remove_child(po, spec.array_field, id);
        }
        self.unindex_tree(id);
        self.touch(&p, false);
    }

    /// Moves an aggregate child to another parent (e.g. a category to another master) and stamps it.
    pub fn move_child(&mut self, id: &str, new_parent: &str) {
        self.remember(id);
        let Some(spec) = self.ents.get(id).and_then(|o| aggregate_child(o.etype())) else { return };
        if let Some(o) = self.ents.get_mut(id) {
            o.insert(spec.parent_field.into(), Value::String(new_parent.into()));
        }
        self.reparent_if_needed(id);
        self.touch(id, false);
    }

    fn reparent_if_needed(&mut self, id: &str) {
        let Some(o) = self.ents.get(id) else { return };
        let Some(spec) = aggregate_child(o.etype()) else { return };
        let wanted = o.str_of(spec.parent_field).unwrap_or("").to_string();
        let current = self.parent_of.get(id).cloned();
        if current.as_deref() == Some(wanted.as_str()) {
            return;
        }
        if let Some(c) = current {
            if let Some(po) = self.ents.get_mut(&c) {
                remove_child(po, spec.array_field, id);
            }
            self.parent_of.remove(id);
        }
        self.orphans.retain(|x| x != id);
        if wanted != id && self.ents.contains_key(&wanted) {
            push_child(self.ents.get_mut(&wanted).unwrap(), spec.array_field, id);
            self.parent_of.insert(id.to_string(), wanted);
        } else {
            self.orphans.push(id.to_string());
        }
    }

    fn retry_orphans(&mut self) {
        for id in self.orphans.clone() {
            self.reparent_if_needed(&id);
        }
    }

    // ------------------------------------------------------------------ incoming sync

    /// Merges one diff's items (VOToSyncDiffBridge.incorporateSyncDiff).
    pub fn apply_diff_items(&mut self, items: Vec<Obj>, diff_end: &VersionVector) -> Result<ApplyResult, String> {
        let mut result = ApplyResult::default();
        let mut to_reversion: Vec<String> = Vec::new();
        let mut ordered = items;
        // Aggregate roots first so children in the same diff can find their parent.
        ordered.sort_by_key(|o| aggregate_child(o.etype()).is_some());

        for incoming in ordered {
            let id = incoming.id().to_string();
            if id.is_empty() {
                return Err("Expected an entity ID".into());
            }
            let incoming_knowledge = match incoming.str_of("madeWithKnowledge") {
                Some(k) if !k.is_empty() => VersionVector::parse(Some(k)),
                _ => diff_end.clone(),
            };
            let (in_dev, in_ctr) = incoming
                .str_of("entityVersion")
                .and_then(parse_element)
                .ok_or_else(|| format!("{id} has no entityVersion"))?;

            let Some(existing) = self.ents.get(&id) else {
                let etype = incoming.etype().to_string();
                let mut o = if entity_fields(&etype).is_some() { blank_entity(&etype, &id) } else { Obj::new() };
                for (k, v) in incoming {
                    if !aggregate_child_properties(&etype).contains(&k.as_str()) {
                        o.insert(k, v);
                    }
                }
                o.insert("madeWithKnowledge".into(), Value::Null);
                self.attach(o, None)?;
                result.created += 1;
                continue;
            };

            let my_knowledge = match existing.str_of("madeWithKnowledge") {
                Some(k) if !k.is_empty() => VersionVector::parse(Some(k)),
                _ => self.knowledge.clone(),
            };
            let (my_dev, my_ctr) = existing.str_of("entityVersion").and_then(parse_element).unwrap_or_default();
            match compare_versions((&my_dev, my_ctr), &my_knowledge, (&in_dev, in_ctr), &incoming_knowledge) {
                -1 => {
                    self.copy_into(&id, incoming);
                    result.updated += 1;
                }
                1 => result.skipped += 1,
                _ => {
                    let (existing_wins, reversion, reason) = if existing.b("isResolvedConflict") && incoming.b("isResolvedConflict") {
                        (my_dev < in_dev, false, "bothResolved")
                    } else if existing.tomb() != incoming.tomb() {
                        (existing.tomb(), true, "tombstone")
                    } else {
                        (my_dev < in_dev, true, "deviceOrder")
                    };
                    let etype = existing.etype().to_string();
                    if !existing_wins {
                        self.copy_into(&id, incoming);
                    }
                    if reversion {
                        to_reversion.push(id.clone());
                    }
                    if let Some(o) = self.ents.get_mut(&id) {
                        o.insert("madeWithKnowledge".into(), Value::Null);
                    }
                    result.conflicts.push(ConflictRecord {
                        entity_id: id,
                        entity_type: etype,
                        winner: if existing_wins { "existing" } else { "incoming" },
                        reason,
                    });
                }
            }
        }

        self.retry_orphans();
        self.knowledge.incorporate(diff_end);
        self.knowledge.ensure(&self.device.clone());
        for id in to_reversion {
            self.touch(&id, true);
        }
        self.revision += 1;
        Ok(result)
    }

    /// copyUntypedToEntity: copies every field present on the incoming item.
    fn copy_into(&mut self, target: &str, incoming: Obj) {
        let Some(t) = self.ents.get(target) else { return };
        let etype = t.etype().to_string();
        let old_children: Vec<String> = child_array_fields(&etype)
            .iter()
            .filter(|f| is_inline_array(f) && incoming.contains_key(**f))
            .flat_map(|f| child_ids(t, f))
            .collect();
        for c in old_children {
            self.unindex_tree(&c);
        }
        let skip = aggregate_child_properties(&etype);
        let mut values: Vec<(String, Value)> = Vec::new();
        for (k, v) in incoming {
            if k == "madeWithKnowledge" || skip.contains(&k.as_str()) {
                continue;
            }
            if is_inline_array(&k) {
                if let Value::Array(items) = v {
                    let ids: Vec<Value> = items.into_iter().map(|c| Value::String(self.register_any(c, Some(target)))).collect();
                    values.push((k, Value::Array(ids)));
                    continue;
                }
            }
            values.push((k, v));
        }
        if let Some(t) = self.ents.get_mut(target) {
            for (k, v) in values {
                t.insert(k, v);
            }
        }
        self.reparent_if_needed(target);
    }

    // ------------------------------------------------------------------ undo

    /// Starts recording an edit for undo.
    pub fn begin_journal(&mut self) {
        self.journal = Some(Journal::default());
    }

    /// Stops recording; the step holds only the records the edit really changed.
    pub fn end_journal(&mut self) -> Option<UndoStep> {
        let j = self.journal.take()?;
        let mut entries = Vec::new();
        for id in j.order {
            let before = j.before.get(&id).cloned().flatten();
            let Some(now) = self.ents.get(&id) else { continue };
            let after = now.str_of("entityVersion").unwrap_or("").to_string();
            let changed = match &before {
                None => true,
                Some(b) => b.str_of("entityVersion").unwrap_or("") != after,
            };
            if changed {
                entries.push((id, before, after));
            }
        }
        Some(UndoStep { entries, complete: !j.missed })
    }

    /// The record an undo step keeps: inline children belong to their root parent.
    fn journal_root(&self, id: &str) -> String {
        let mut cur = id.to_string();
        while let Some(o) = self.ents.get(&cur) {
            if inline_child(o.etype()).is_none() {
                break;
            }
            match self.parent_of.get(&cur) {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        cur
    }

    fn remember(&mut self, id: &str) {
        if self.journal.is_none() || !self.ents.contains_key(id) {
            return;
        }
        let root = self.journal_root(id);
        if self.journal.as_ref().is_some_and(|j| j.before.contains_key(&root)) {
            return;
        }
        let snap = self.ents.get(&root).map(|o| self.with_inline_children(o));
        if let Some(j) = self.journal.as_mut() {
            j.order.push(root.clone());
            j.before.insert(root, snap);
        }
    }

    fn remember_new(&mut self, id: &str) {
        if let Some(j) = self.journal.as_mut() {
            if !j.before.contains_key(id) {
                j.order.push(id.to_string());
                j.before.insert(id.to_string(), None);
            }
        }
    }

    /// A record's own fields with inline children expanded (aggregate children stay as ids).
    fn with_inline_children(&self, o: &Obj) -> Obj {
        let mut out = o.clone();
        for &field in child_array_fields(o.etype()) {
            if is_inline_array(field) {
                if let Some(Value::Array(ids)) = out.get_mut(field) {
                    let kids: Vec<Value> = ids.iter().filter_map(Value::as_str).map(|c| self.materialize(c)).collect();
                    *ids = kids;
                }
            }
        }
        out
    }

    fn inline_ids(&self, id: &str, out: &mut Vec<String>) {
        let Some(o) = self.ents.get(id) else { return };
        for &field in child_array_fields(o.etype()) {
            if is_inline_array(field) {
                for c in child_ids(o, field) {
                    self.inline_ids(&c, out);
                    out.push(c);
                }
            }
        }
    }

    /// Puts back what an undo step remembers, as a new local edit (so it syncs like any other).
    /// Run it inside a journal to get the step that redoes it. Changes nothing when it refuses.
    pub fn restore(&mut self, step: &UndoStep) -> Result<(), String> {
        for (id, before, after) in &step.entries {
            let now = self.ents.get(id).and_then(|o| o.str_of("entityVersion")).unwrap_or("");
            if now != after {
                return Err("it was changed again since, here or on another device".into());
            }
            if let Some(b) = before {
                let mut kids = Vec::new();
                for &field in child_array_fields(b.etype()) {
                    if let (true, Some(Value::Array(items))) = (is_inline_array(field), b.get(field)) {
                        collect_ids(items, &mut kids);
                    }
                }
                if let Some(k) = kids.iter().find(|k| self.ents.contains_key(*k) && self.journal_root(k) != *id) {
                    return Err(format!("{k} is used elsewhere now"));
                }
            }
        }
        for (id, before, _) in &step.entries {
            match before {
                None => {
                    // Created by the edit: delete it the way YNAB does (a tombstone). Months are
                    // expected to exist and an empty one is harmless, so those stay.
                    let keep = self.ents.get(id).is_none_or(|o| o.tomb() || o.etype() == "monthlyBudget");
                    if !keep {
                        self.update(id, vec![("isTombstone", Value::Bool(true))]);
                    }
                }
                Some(b) => self.put_back(id, b),
            }
        }
        Ok(())
    }

    fn put_back(&mut self, id: &str, b: &Obj) {
        self.remember(id);
        let Some(cur) = self.ents.get(id).cloned() else { return };
        let fields = child_array_fields(cur.etype());
        let mut old = Vec::new();
        self.inline_ids(id, &mut old);
        for c in &old {
            self.unindex_tree(c);
        }
        let mut out = Obj::new();
        for (k, v) in b {
            let f = k.as_str();
            if !fields.contains(&f) {
                out.insert(k.clone(), v.clone());
            } else if !is_inline_array(f) {
                // Aggregate children follow their own parent field; keep today's list.
                out.insert(k.clone(), cur.get(f).cloned().unwrap_or_else(|| v.clone()));
            } else {
                let Value::Array(items) = v.clone() else {
                    out.insert(k.clone(), v.clone());
                    continue;
                };
                let ids: Vec<Value> = items.into_iter().map(|c| Value::String(self.register_any(c, Some(id)))).collect();
                out.insert(k.clone(), Value::Array(ids));
            }
        }
        for &f in fields {
            if !is_inline_array(f) && !out.contains_key(f) {
                if let Some(v) = cur.get(f) {
                    out.insert(f.to_string(), v.clone());
                }
            }
        }
        self.ents.insert(id.to_string(), out);
        self.reparent_if_needed(id);
        // Fresh versions on the children too, so devices that merge them one by one take them.
        let mut kids = Vec::new();
        self.inline_ids(id, &mut kids);
        for c in kids {
            let counter = self.knowledge.increment(&self.device);
            if let Some(o) = self.ents.get_mut(&c) {
                o.insert("entityVersion".into(), Value::String(format!("{}-{}", self.device, counter)));
            }
        }
        self.touch(id, false);
    }

    // ------------------------------------------------------------------ saving

    /// Writes current knowledge into fileMetaData (before a full save).
    pub fn sync_file_meta(&mut self) {
        let k = self.knowledge.to_string();
        for (key, slot) in self.root.iter_mut() {
            if key == "fileMetaData" {
                if let RootSlot::Raw(Value::Object(o)) = slot {
                    o.insert("currentKnowledge".into(), Value::String(k.clone()));
                }
            }
        }
    }

    /// A record with its children expanded back into nested objects.
    pub fn materialize(&self, id: &str) -> Value {
        if let Some(v) = self.raw.get(id) {
            return v.clone();
        }
        let Some(o) = self.ents.get(id) else { return Value::Null };
        let fields = child_array_fields(o.etype());
        let mut out = Obj::new();
        for (k, v) in o {
            if fields.contains(&k.as_str()) {
                if let Value::Array(ids) = v {
                    out.insert(k.clone(), Value::Array(ids.iter().filter_map(Value::as_str).map(|c| self.materialize(c)).collect()));
                    continue;
                }
            }
            out.insert(k.clone(), v.clone());
        }
        Value::Object(out)
    }

    /// The whole budget as the Budget.yfull root object.
    pub fn to_root(&self) -> Value {
        let mut out = Obj::new();
        for (k, slot) in &self.root {
            let v = match slot {
                RootSlot::Collection(ids) => Value::Array(ids.iter().map(|i| self.materialize(i)).collect()),
                RootSlot::Single(id) => self.materialize(id),
                RootSlot::Raw(v) => v.clone(),
            };
            out.insert(k.clone(), v);
        }
        Value::Object(out)
    }
}

fn child_ids(o: &Obj, field: &str) -> Vec<String> {
    match o.get(field) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn collect_ids(items: &[Value], out: &mut Vec<String>) {
    for it in items {
        if let Value::Object(o) = it {
            out.push(o.id().to_string());
            for &field in child_array_fields(o.etype()) {
                if let Some(Value::Array(a)) = o.get(field) {
                    collect_ids(a, out);
                }
            }
        }
    }
}

fn push_child(parent: &mut Obj, field: &str, id: &str) {
    match parent.get_mut(field) {
        Some(Value::Array(a)) => a.push(Value::String(id.into())),
        _ => {
            parent.insert(field.into(), Value::Array(vec![Value::String(id.into())]));
        }
    }
}

fn remove_child(parent: &mut Obj, field: &str, id: &str) {
    if let Some(Value::Array(a)) = parent.get_mut(field) {
        a.retain(|v| v.as_str() != Some(id));
    }
}
