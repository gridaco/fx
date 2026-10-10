//! The records the layout reads (spec/layout.md §3), from a plan or from a run's projection.
//!
//! Reading is lenient: a member with an unexpected shape is left out, never guessed, so a
//! damaged record still draws what it can. Nothing here parses an instance id or a path.

use crate::read::{Node, Snapshot, strings, takes, text};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

/// One recorded instance. Members are drawn; the others (absent ones) only order frame cards
/// and add placement edges.
#[derive(Debug, Clone)]
pub(crate) struct Instance {
    pub id: String,
    pub path: Option<String>,
    pub step: Option<String>,
    pub take: Vec<u64>,
    pub key: Option<String>,
    pub member: bool,
    /// The instances this one reads, needs, judges or waits on, and the sources of its
    /// output and fact bindings (§4 rule 3).
    pub after: Vec<String>,
}

impl Instance {
    /// The slot key: the recorded step, else (an older record) the recorded path, else the id.
    pub(crate) fn address(&self) -> &str {
        self.step
            .as_deref()
            .or(self.path.as_deref())
            .unwrap_or(&self.id)
    }

    fn bare(id: &str, member: bool) -> Self {
        Self {
            id: id.to_string(),
            path: None,
            step: None,
            take: Vec::new(),
            key: None,
            member,
            after: Vec::new(),
        }
    }

    /// An fx-graph-v1 instance.
    fn from_plan(value: &Value, member: bool) -> Option<Self> {
        let mut instance = Self::bare(text(value, "id")?, member);
        instance.path = text(value, "path").map(str::to_string);
        instance.step = text(value, "step").map(str::to_string);
        instance.take = takes(value.get("take")).unwrap_or_default();
        instance.key = text(value, "key").map(str::to_string);
        instance.after = strings(value.get("reads"));
        instance.after.extend(strings(value.get("needs")));
        instance
            .after
            .extend(text(value, "judges").map(str::to_string));
        instance.after.extend(strings(value.get("waiting_on")));
        instance.after.extend(sources(value.get("bindings")));
        Some(instance)
    }

    /// A node of the run projection, which already holds what the newest listing records.
    fn from_node(node: &Node) -> Self {
        let mut instance = Self::bare(&node.id, true);
        instance.path = Some(node.path.clone());
        instance.step = node.step.clone();
        instance.take = node.take.clone().unwrap_or_default();
        instance.key = node.key.clone().flatten();
        instance.after = node.reads.clone();
        instance.after.extend(node.needs.iter().flatten().cloned());
        instance.after.extend(
            node.judges
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_string),
        );
        instance.after.extend(sources(node.bindings.as_ref()));
        instance
    }

    /// An instance a `scopes_updated` lists and nothing else records yet.
    fn from_listing(entry: &Value, member: bool) -> Option<Self> {
        let mut instance = Self::bare(text(entry, "id")?, member);
        instance.step = text(entry, "step").map(str::to_string);
        instance.take = takes(entry.get("take")).unwrap_or_default();
        instance.key = text(entry, "key").map(str::to_string);
        Some(instance)
    }
}

/// One display scope (fx-graph-v1 `scope`): a group iteration or take, or an imported
/// workflow occurrence. Its card fills the slot of its `step`.
#[derive(Debug, Clone)]
pub(crate) struct Scope {
    pub id: String,
    pub parent: Option<String>,
    pub path: String,
    pub step: String,
    pub take: Vec<u64>,
    /// The scope's direct member instances.
    pub nodes: Vec<String>,
    /// The paths of its pending repeats.
    pub pending: Vec<String>,
    /// The instances outside it that its input bindings read.
    pub after: Vec<String>,
}

impl Scope {
    fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            id: text(value, "id")?.to_string(),
            parent: text(value, "parent").map(str::to_string),
            path: text(value, "path")?.to_string(),
            step: text(value, "step")?.to_string(),
            take: takes(value.get("take")).unwrap_or_default(),
            nodes: strings(value.get("nodes")),
            pending: strings(value.get("pending")),
            after: sources(value.get("input_bindings")),
        })
    }
}

/// A repeat not yet expanded (fx-graph-v1 `pending`).
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub path: String,
    pub step: Option<String>,
    pub waiting_on: Vec<String>,
}

impl Pending {
    fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            path: text(value, "path")?.to_string(),
            step: text(value, "step").map(str::to_string),
            waiting_on: strings(value.get("waiting_on")),
        })
    }

    pub(crate) fn address(&self) -> &str {
        self.step.as_deref().unwrap_or(&self.path)
    }
}

/// Everything one view's cells and member order are computed from.
#[derive(Debug, Clone, Default)]
pub(crate) struct Input {
    /// The plan's declared steps and their `order`.
    pub declared: HashMap<String, Option<u64>>,
    /// One per id, in record order; a member's own record is the one kept.
    pub instances: Vec<Instance>,
    /// Empty when the record has no valid scopes: the view is then flat.
    pub scopes: Vec<Scope>,
    /// Every repeat recorded pending; in a run, also those that have since expanded.
    pub pending: Vec<Pending>,
    /// Each member's position in the newest recorded expansion order.
    pub expansion: HashMap<String, usize>,
    /// Each instance's position in the plan's `instances`.
    pub plan_index: HashMap<String, usize>,
    /// Each pending path's position: the plan's `pending`, then the run's.
    pub pending_index: HashMap<String, usize>,
}

impl Input {
    /// A plan view: every non-absent plan instance is a member.
    pub(crate) fn from_plan(plan: &Value) -> Self {
        let mut input = Self::common(plan);
        let mut seen = BTreeSet::new();
        for value in plan_instances(plan) {
            let member = text(value, "state") != Some("absent");
            if let Some(instance) = Instance::from_plan(value, member)
                && seen.insert(instance.id.clone())
            {
                input.instances.push(instance);
            }
        }
        input.expansion = input.plan_index.clone();
        input.scopes = scopes(plan.get("scopes"));
        input.pending = plan_pending(plan).filter_map(Pending::parse).collect();
        input
    }

    /// A run view from the projection of one recorded prefix. With a recorded listing, its
    /// instances and every instance with an event are members; otherwise the projection's
    /// nodes are (members join as they start). Plan instances and earlier listings that are
    /// neither are absent.
    pub(crate) fn from_run(plan: &Value, snapshot: &Snapshot) -> Self {
        let document = &snapshot.document;
        let recorded = &snapshot.recorded;
        let mut input = Self::common(plan);
        let nodes: HashMap<&str, &Node> = document
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let planned: HashMap<&str, &Value> = plan_instances(plan)
            .filter_map(|value| Some((text(value, "id")?, value)))
            .collect();
        let mut seen = BTreeSet::new();
        let mut add = |instance: Option<Instance>, input: &mut Self| {
            if let Some(instance) = instance
                && seen.insert(instance.id.clone())
            {
                input.instances.push(instance);
            }
        };
        if let Some(listing) = &recorded.instances {
            for (position, entry) in listing.iter().enumerate() {
                let Some(id) = text(entry, "id") else {
                    continue;
                };
                input.expansion.entry(id.to_string()).or_insert(position);
                let instance = match (nodes.get(id), planned.get(id)) {
                    (Some(node), _) => Some(Instance::from_node(node)),
                    (None, Some(value)) => Instance::from_plan(value, true),
                    (None, None) => Instance::from_listing(entry, true),
                };
                add(instance, &mut input);
            }
            for node in &document.nodes {
                if recorded.started.contains(&node.id) {
                    add(Some(Instance::from_node(node)), &mut input);
                }
            }
        } else {
            input.expansion = input.plan_index.clone();
            for node in &document.nodes {
                add(Some(Instance::from_node(node)), &mut input);
            }
        }
        for value in plan_instances(plan) {
            add(Instance::from_plan(value, false), &mut input);
        }
        for entry in recorded.listed.values() {
            add(Instance::from_listing(entry, false), &mut input);
        }
        input.scopes = scopes(recorded.scopes.as_ref());
        // Every repeat the plan or a snapshot recorded pending, expanded or not: one edge set for
        // both views, which a recorded expansion never removes (§4 rule 3, §10).
        input.pending = plan_pending(plan)
            .chain(recorded.waited.values())
            .filter_map(Pending::parse)
            .collect();
        for entry in &input.pending {
            let next = input.pending_index.len();
            input
                .pending_index
                .entry(entry.path.clone())
                .or_insert(next);
        }
        input
    }

    /// The declared steps, plan order and plan pending order both views share.
    fn common(plan: &Value) -> Self {
        let mut input = Self {
            declared: plan
                .get("steps")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(address, step)| (address.clone(), step.get("order").and_then(Value::as_u64)))
                .collect(),
            ..Self::default()
        };
        for (position, value) in plan_instances(plan).enumerate() {
            if let Some(id) = text(value, "id") {
                input.plan_index.entry(id.to_string()).or_insert(position);
            }
        }
        for entry in plan_pending(plan) {
            if let Some(path) = text(entry, "path") {
                let next = input.pending_index.len();
                input.pending_index.entry(path.to_string()).or_insert(next);
            }
        }
        input
    }
}

fn plan_instances(plan: &Value) -> impl Iterator<Item = &Value> {
    plan.get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn plan_pending(plan: &Value) -> impl Iterator<Item = &Value> {
    plan.get("pending")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Every scope, or none when one has an unsupported shape.
fn scopes(value: Option<&Value>) -> Vec<Scope> {
    value
        .and_then(Value::as_array)
        .and_then(|entries| entries.iter().map(Scope::parse).collect())
        .unwrap_or_default()
}

/// The instances that output and fact bindings read; scope inputs and outputs are boundaries,
/// not instances.
fn sources(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|binding| matches!(text(binding, "source_kind"), Some("output" | "fact")))
        .filter_map(|binding| text(binding, "source"))
        .map(str::to_string)
        .collect()
}
