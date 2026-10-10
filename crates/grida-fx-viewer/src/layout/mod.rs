//! Canvas layout (spec/layout.md): every slot's automatic cell and the members' order, served
//! as `fx-layout-report-v1`. The browser places its own projection's cards in these cells; it
//! never computes a cell (§8).

mod cells;
mod input;
mod order;
mod report;
#[cfg(test)]
mod tests;

use crate::read::Snapshot;
use indexmap::IndexMap;
use input::{Input, Instance};
use order::{MemberKey, SlotKey};
use report::CellEntry;
pub(crate) use report::LayoutReport;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The report of a saved plan.
pub(crate) fn plan_report(plan: &Value) -> LayoutReport {
    report(&Input::from_plan(plan), None)
}

/// The report of one recorded prefix of a run, `plan` being that run's recorded plan and
/// `cursor` the prefix's observation cursor.
pub(crate) fn run_report(plan: &Value, snapshot: &Snapshot, cursor: String) -> LayoutReport {
    report(&Input::from_run(plan, snapshot), Some(cursor))
}

fn report(input: &Input, cursor: Option<String>) -> LayoutReport {
    LayoutReport {
        kind: report::KIND,
        file: None,
        revision: None,
        state: "none",
        cursor,
        diagnostics: Vec::new(),
        cells: cells(input),
        order: order(input),
    }
}

/// A declaration path's step names, or `None` for an address that is not one (an older record
/// keyed by a recorded path), which then stays one opaque slot of the root.
fn segments(address: &str) -> Option<Vec<&str>> {
    let names: Vec<&str> = address.split('.').collect();
    names
        .iter()
        .all(|name| {
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
        .then_some(names)
}

fn path(address: &str) -> Vec<&str> {
    segments(address).unwrap_or_else(|| vec![address])
}

/// Rules 1–2, 4 and 5–10: the slots of every container, ordered, and their cells, container by
/// container from the root.
fn cells(input: &Input) -> IndexMap<String, CellEntry> {
    let keys = slot_keys(input);
    let mut containers: BTreeMap<String, Vec<&SlotKey>> = BTreeMap::new();
    for key in keys.values() {
        let names = path(&key.address);
        containers
            .entry(names[..names.len() - 1].join("."))
            .or_default()
            .push(key);
    }
    for slots in containers.values_mut() {
        slots.sort();
    }
    let mut edges: HashMap<String, BTreeSet<(String, String)>> = HashMap::new();
    for (container, source, target) in placement(input) {
        edges.entry(container).or_default().insert((source, target));
    }
    let mut placed = IndexMap::new();
    let mut queue = vec![String::new()];
    while let Some(container) = queue.pop() {
        let Some(slots) = containers.get(&container) else {
            continue;
        };
        let index: HashMap<&str, usize> = slots
            .iter()
            .enumerate()
            .map(|(position, key)| (key.address.as_str(), position))
            .collect();
        let between: Vec<(usize, usize)> = edges
            .get(&container)
            .into_iter()
            .flatten()
            .map(|(source, target)| (index[source.as_str()], index[target.as_str()]))
            .collect();
        for (key, (column, row)) in slots.iter().zip(cells::cells(slots.len(), &between)) {
            placed.insert(
                key.address.clone(),
                CellEntry {
                    column,
                    row,
                    source: "automatic",
                },
            );
        }
        // Inner containers follow, in their slots' order key. An empty address is a root slot
        // and never a container.
        queue.extend(
            slots
                .iter()
                .rev()
                .filter(|key| !key.address.is_empty())
                .map(|key| key.address.clone()),
        );
    }
    placed
}

/// Rules 1–2 and 4: one key per slot. Slots are the declared steps, the steps any instance,
/// scope or pending repeat records, and every container they sit in.
fn slot_keys(input: &Input) -> BTreeMap<String, SlotKey> {
    let mut planned: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending: BTreeMap<String, usize> = BTreeMap::new();
    let mut addresses: BTreeSet<String> = input.declared.keys().cloned().collect();
    let mut note = |address: &str, position: Option<usize>, into: &mut BTreeMap<String, usize>| {
        let names = path(address);
        // A frame takes the smallest key among its descendants.
        for end in 1..=names.len() {
            let ancestor = names[..end].join(".");
            if let Some(position) = position {
                let slot = into.entry(ancestor.clone()).or_insert(position);
                *slot = (*slot).min(position);
            }
            addresses.insert(ancestor);
        }
    };
    for instance in &input.instances {
        let position = input.plan_index.get(&instance.id).copied();
        note(instance.address(), position, &mut planned);
    }
    for entry in &input.pending {
        let position = input.pending_index.get(&entry.path).copied();
        note(entry.address(), position, &mut pending);
    }
    for scope in &input.scopes {
        note(&scope.step, None, &mut planned);
    }
    addresses
        .into_iter()
        .map(|address| {
            let key = SlotKey::new(
                &address,
                input.declared.get(&address).copied().flatten(),
                planned.get(&address).copied(),
                pending.get(&address).copied(),
            );
            (address, key)
        })
        .collect()
}

/// Rule 3: every recorded relation, lifted to the innermost container holding both ends, as
/// (container, source slot, target slot). Edges inside one slot drop.
fn placement(input: &Input) -> Vec<(String, String, String)> {
    let mut addresses: HashMap<&str, &str> = HashMap::new();
    for instance in &input.instances {
        addresses
            .entry(instance.id.as_str())
            .or_insert(instance.address());
    }
    let relations = input
        .instances
        .iter()
        .map(|instance| (&instance.after, instance.address()))
        .chain(
            input
                .scopes
                .iter()
                .map(|scope| (&scope.after, scope.step.as_str())),
        )
        .chain(
            input
                .pending
                .iter()
                .map(|entry| (&entry.waiting_on, entry.address())),
        );
    relations
        .flat_map(|(sources, target)| {
            sources
                .iter()
                .filter_map(|source| addresses.get(source.as_str()))
                .map(move |source| (*source, target))
        })
        .filter_map(|(source, target)| lift(source, target))
        .collect()
}

fn lift(source: &str, target: &str) -> Option<(String, String, String)> {
    let (source, target) = (path(source), path(target));
    let shared = source
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count()
        .min(source.len() - 1)
        .min(target.len() - 1);
    let (from, to) = (source[..=shared].join("."), target[..=shared].join("."));
    (from != to).then(|| (source[..shared].join("."), from, to))
}

/// Rule 4: every member, instance or scope card, in order key.
fn order(input: &Input) -> Vec<String> {
    let instances: HashMap<&str, &Instance> = input
        .instances
        .iter()
        .map(|instance| (instance.id.as_str(), instance))
        .collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, scope) in input.scopes.iter().enumerate() {
        if let Some(parent) = &scope.parent {
            children.entry(parent.as_str()).or_default().push(index);
        }
    }
    let mut keys: Vec<(MemberKey, &str)> = input
        .instances
        .iter()
        .filter(|instance| instance.member)
        .map(|instance| (MemberKey::instance(input, instance), instance.id.as_str()))
        .collect();
    let mut memo = vec![None; input.scopes.len()];
    for index in 0..input.scopes.len() {
        let key = scope_key(input, &instances, &children, &mut memo, index);
        keys.push((key, input.scopes[index].id.as_str()));
    }
    keys.sort();
    keys.dedup_by(|left, right| left.1 == right.1);
    keys.into_iter().map(|(_, id)| id.to_string()).collect()
}

/// A frame or workflow card's key: the smallest among its member descendants; without one,
/// its absent descendants' plan positions, then its pending repeats', then its own path.
fn scope_key<'a>(
    input: &'a Input,
    instances: &HashMap<&str, &'a Instance>,
    children: &HashMap<&str, Vec<usize>>,
    memo: &mut Vec<Option<MemberKey<'a>>>,
    index: usize,
) -> MemberKey<'a> {
    if let Some(key) = memo[index] {
        return key;
    }
    let scope = &input.scopes[index];
    let nodes = scope
        .nodes
        .iter()
        .filter_map(|id| match instances.get(id.as_str()) {
            Some(instance) if instance.member => Some(MemberKey::instance(input, instance)),
            _ => input
                .plan_index
                .get(id)
                .map(|&position| MemberKey::absent(position)),
        });
    let pending = scope
        .pending
        .iter()
        .filter_map(|path| input.pending_index.get(path))
        .map(|&position| MemberKey::pending(position));
    let nested: Vec<MemberKey<'a>> = children
        .get(scope.id.as_str())
        .into_iter()
        .flatten()
        .map(|&child| scope_key(input, instances, children, memo, child))
        .collect();
    let key = nodes
        .chain(pending)
        .chain(nested)
        .min()
        .unwrap_or_else(|| MemberKey::scope(scope));
    memo[index] = Some(key);
    key
}
