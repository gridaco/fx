//! Repeats: `for_each`, `matrix`, `max:`, pending repeats and shadow pricing.
//!
//! `for_each`: a collection's values; a pending list (or a non-list holding pending
//! values) is a pending repeat; missing or failed makes the step absent; a non-list is
//! `must be a list` at `<w>.for_each`; `N items exceed max: M` still expands every item; keys are
//! `key:` per item (pending → `a key must be known when the repeat expands`, the index kept;
//! duplicates → `key 'k' names two items`, later ones skipped) or the index. `matrix`: the
//! product of the axes in declaration order, last fastest, key `a.b`, suffix `['a']['b']`; any
//! pending element defers the whole matrix; an axis that is not a list or collection is
//! `a matrix axis is a list` at `<w>` (step absent); no duplicate check. Items expand in
//! `frame.derive(phase)` with `concurrency_group = prefix + name`, each child registered before
//! it expands.
//!
//! Pending repeats: `max` (`max: must be known while planning`, `max: is a whole number
//! from 1 to 10000`) and without it `the list comes from a step, so the plan cannot count it:
//! add max: (the most items this repeat may run)` at `<w>`; priced by a shadow expansion of one
//! item whose instances, problems, memo and scopes are restored afterwards.

use super::frame::{ExpId, ExpKind, FrameChanges, FrameId};
use super::{Expander, PendingRepeat, Position, quote_key};
use crate::docs::workflow::Step;
use crate::money::Usd;
use crate::text::py_repr_str;
use crate::val::{Pending, Val};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

/// One item of a static repeat: its key, path suffix and variables.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepeatItem {
    pub key: String,
    pub suffix: String,
    pub variables: IndexMap<String, Val>,
}

/// What a repeat's list evaluated to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Listed {
    Items(Vec<RepeatItem>),
    /// The list only a run produces: the values it waits on.
    Pending(Vec<Val>),
    /// No instances (missing, failed, or a refusal already reported).
    Absent,
}

/// The most items a repeat may declare (`max:`).
const MAX_ITEMS: f64 = 10_000.0;

impl Expander<'_> {
    /// Expands a repeated step (`for_each` or `matrix` set) into `into`.
    pub(crate) fn repeat(&mut self, at: &Position, into: ExpId) {
        let (listed, reads) = if at.declared.matrix.is_some() {
            self.matrix_items(at)
        } else {
            self.for_each_items(at)
        };
        let items = match listed {
            Listed::Items(items) => items,
            Listed::Pending(waiting) => {
                self.pending_repeat(at, into, waiting);
                return;
            }
            Listed::Absent => {
                self.exps[into.0].kind = ExpKind::Absent;
                return;
            }
        };
        let phase = self.phase_of(&reads, at.frame);
        self.exps[into.0].kind = ExpKind::Repeat;
        let positioned = self.derive(
            at.frame,
            FrameChanges {
                phase: Some(phase),
                ..Default::default()
            },
        );
        let concurrency_group = format!("{}{}", self.frames[at.frame.0].prefix, at.name);
        for item in items {
            let child = self.new_exp(at.frame, &at.name);
            // Registered first: a later step of this very item may refer back into it.
            self.exps[into.0].children.push((item.key.clone(), child));
            let position = Position {
                frame: positioned,
                name: at.name.clone(),
                declared: Rc::clone(&at.declared),
                where_: at.where_.clone(),
                key: Some(item.key),
                variables: item.variables,
                suffix: item.suffix,
                concurrency_group: Some(concurrency_group.clone()),
            };
            self.single(&position, child);
            if self.escaped.take().is_some() {
                // gnode raises out of the whole repeat: the step is absent, its problem reported.
                self.exps[into.0].kind = ExpKind::Absent;
                return;
            }
            if self.exps[child.0].kind == ExpKind::Expanding {
                self.exps[child.0].kind = ExpKind::Absent;
            }
        }
    }

    /// The items of a `for_each`, and the ids its list read.
    pub(crate) fn for_each_items(
        &mut self,
        at: &Position,
    ) -> (Listed, std::collections::BTreeSet<String>) {
        let (listed, reads) = self.for_each_value(at);
        self.for_each_list(at, listed, reads)
    }

    /// The value of a `for_each`, and the ids it read.
    #[inline(never)]
    fn for_each_value(&mut self, at: &Position) -> (Val, BTreeSet<String>) {
        let source = at.declared.for_each.clone().unwrap_or(Value::Null);
        self.evaluate_reading(at.frame, &source, &format!("{}.for_each", at.where_))
    }

    /// The items of a `for_each` from its value.
    #[inline(never)]
    fn for_each_list(
        &mut self,
        at: &Position,
        listed: Val,
        reads: BTreeSet<String>,
    ) -> (Listed, BTreeSet<String>) {
        let declared = Rc::clone(&at.declared);
        // Keyed results are read as their values, in order.
        let listed = match listed {
            Val::Collection(collection) => Val::List(collection.values()),
            other => other,
        };
        let list = match listed {
            Val::List(list) => list,
            Val::Pending(_) => return (Listed::Pending(vec![listed]), reads),
            other if other.contains_pending() => return (Listed::Pending(vec![other]), reads),
            Val::Missing | Val::Failed(_) => return (Listed::Absent, reads),
            _ => {
                self.problem(&format!("{}.for_each", at.where_), "must be a list");
                return (Listed::Absent, reads);
            }
        };
        if let Some(limit) = self.max_limit(at.frame, &declared, &at.where_)
            && list.len() > limit as usize
        {
            // Every item is still expanded.
            self.problem(
                &format!("{}.for_each", at.where_),
                format!("{} items exceed max: {limit}", list.len()),
            );
        }
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for (index, item) in list.into_iter().enumerate() {
            let mut variables = IndexMap::new();
            variables.insert(declared.as_.clone(), item);
            let mut key = index.to_string();
            if let Some(key_source) = &declared.key {
                let mut probe_variables = self.frames[at.frame.0].variables.clone();
                for (name, value) in &variables {
                    probe_variables.insert(name.clone(), value.clone());
                }
                let probe = self.derive(
                    at.frame,
                    FrameChanges {
                        variables: Some(probe_variables),
                        ..Default::default()
                    },
                );
                let value = self.evaluate(
                    probe,
                    &Value::String(key_source.clone()),
                    &format!("{}.key", at.where_),
                );
                if value.contains_pending() {
                    // The item keeps its position as its key.
                    self.problem(
                        &format!("{}.key", at.where_),
                        "a key must be known when the repeat expands",
                    );
                } else {
                    match value.key_text() {
                        Ok(text) => key = text,
                        Err(error) => {
                            // No item survives, not even earlier ones.
                            self.problem(&at.where_, error.0);
                            return (Listed::Absent, reads);
                        }
                    }
                }
            }
            if !seen.insert(key.clone()) {
                self.problem(
                    &format!("{}.key", at.where_),
                    format!("key {} names two items", py_repr_str(&key)),
                );
                continue;
            }
            items.push(RepeatItem {
                suffix: format!("[{}]", quote_key(&key)),
                key,
                variables,
            });
        }
        (Listed::Items(items), reads)
    }

    /// The items of a `matrix`, and the ids its axes read.
    pub(crate) fn matrix_items(
        &mut self,
        at: &Position,
    ) -> (Listed, std::collections::BTreeSet<String>) {
        let declared = Rc::clone(&at.declared);
        let mut axes: Vec<(String, Val)> = Vec::new();
        let mut reads = BTreeSet::new();
        for (axis, values) in declared.matrix.iter().flatten() {
            let (value, read) =
                self.evaluate_reading(at.frame, values, &format!("{}.matrix.{axis}", at.where_));
            reads.extend(read);
            axes.push((axis.clone(), value));
        }
        // Any pending value anywhere in an axis defers the whole matrix.
        if axes.iter().any(|(_, value)| value.contains_pending()) {
            let waiting = axes.into_iter().map(|(_, value)| value).collect();
            return (Listed::Pending(waiting), reads);
        }
        let mut lists: Vec<Vec<Val>> = Vec::new();
        for (_, value) in &axes {
            match value {
                Val::Collection(collection) => lists.push(collection.values()),
                Val::List(list) => lists.push(list.clone()),
                _ => {
                    self.problem(&at.where_, "a matrix axis is a list");
                    return (Listed::Absent, reads);
                }
            }
        }
        let mut items = Vec::new();
        for combination in product(&lists) {
            let mut keys = Vec::with_capacity(combination.len());
            for value in &combination {
                match value.key_text() {
                    Ok(text) => keys.push(text),
                    Err(error) => {
                        self.problem(&at.where_, error.0);
                        return (Listed::Absent, reads);
                    }
                }
            }
            let matrix: IndexMap<String, Val> = axes
                .iter()
                .map(|(axis, _)| axis.clone())
                .zip(combination)
                .collect();
            let mut variables = IndexMap::new();
            variables.insert("matrix".to_string(), Val::Object(matrix));
            // No duplicate check: a repeated key names the same instance twice.
            items.push(RepeatItem {
                suffix: keys.iter().map(|k| format!("[{}]", quote_key(k))).collect(),
                key: keys.join("."),
                variables,
            });
        }
        (Listed::Items(items), reads)
    }

    /// The `max:` of a repeat, evaluated without item variables; `None` for no limit or a
    /// refusal (reported).
    pub(crate) fn max_limit(
        &mut self,
        frame: super::frame::FrameId,
        declared: &Step,
        where_: &str,
    ) -> Option<u32> {
        let declared_max = declared.max.as_ref()?;
        let limit = match declared_max {
            Value::Number(number) => Val::Number(crate::value::as_f64(number)),
            other => {
                let limit = self.evaluate(frame, other, &format!("{where_}.max"));
                if limit.contains_pending() {
                    self.problem(
                        &format!("{where_}.max"),
                        "max: must be known while planning",
                    );
                    return None;
                }
                limit
            }
        };
        match limit {
            Val::Null | Val::Missing => None,
            Val::Number(n) if n.fract() == 0.0 && (1.0..=MAX_ITEMS).contains(&n) => Some(n as u32),
            _ => {
                self.problem(
                    &format!("{where_}.max"),
                    "max: is a whole number from 1 to 10000",
                );
                None
            }
        }
    }

    /// Records a pending repeat and sets `into` to kind `Pending` with its token.
    pub(crate) fn pending_repeat(&mut self, at: &Position, into: ExpId, waiting: Vec<Val>) {
        let limit = self.max_limit(at.frame, &at.declared, &at.where_);
        if limit.is_none() {
            self.problem(
                &at.where_,
                "the list comes from a step, so the plan cannot count it: add max: \
                 (the most items this repeat may run)",
            );
        }
        let refs: BTreeSet<String> = waiting.iter().flat_map(Val::pending_refs).collect();
        let phase = self.phase_of(&refs, at.frame);
        let (low, high) = self.shadow_price(at, phase);
        if self.escaped.take().is_some() {
            // gnode raises out of the pricing: no pending repeat, the step absent.
            self.exps[into.0].kind = ExpKind::Absent;
            return;
        }
        self.pending.push(PendingRepeat {
            path: format!("{}{}", self.frames[at.frame.0].prefix, at.name),
            max: limit.unwrap_or(1),
            waiting_on: refs.clone(),
            per_instance_low: low,
            per_instance_high: high,
            phase,
        });
        let shown: Vec<Value> = waiting.iter().map(Val::token_form).collect();
        let token = Pending::new(refs, &json!({"repeat": at.where_, "of": shown}));
        let exp = &mut self.exps[into.0];
        exp.kind = ExpKind::Pending;
        exp.token = Some(token);
    }

    /// Prices one hypothetical item: `(per-instance low, per-instance high)`. Its item variable
    /// is pending (`<item>`, or `<axis>` per matrix axis); planned and maybe instances count
    /// towards the high price, planned ones towards the low, and nested pending repeats too.
    pub(crate) fn shadow_price(&mut self, at: &Position, phase: u32) -> (Usd, Usd) {
        let saved_instances = self.instances.clone();
        let saved_pending = self.pending.clone();
        let saved_problems = self.problems.clone();
        let saved_memo = self.memo.clone();
        let saved_held = self.held.clone();
        let scopes_before = self.scopes.len();
        // A hypothetical item holds nothing back: what it would expand is priced here and now.
        self.shadowing += 1;

        let mut variables = IndexMap::new();
        match &at.declared.matrix {
            Some(axes) => {
                let matrix = axes
                    .keys()
                    .map(|axis| {
                        let pending = Pending::of(&format!("<{axis}>"), None);
                        (axis.clone(), Val::Pending(Box::new(pending)))
                    })
                    .collect();
                variables.insert("matrix".to_string(), Val::Object(matrix));
            }
            None => {
                let name = &at.declared.as_;
                let pending = Pending::of(&format!("<{name}>"), None);
                variables.insert(name.clone(), Val::Pending(Box::new(pending)));
            }
        }
        let positioned = self.derive(
            at.frame,
            FrameChanges {
                phase: Some(phase),
                ..Default::default()
            },
        );
        let shadow = self.new_exp(at.frame, &at.name);
        let position = Position {
            frame: positioned,
            name: at.name.clone(),
            declared: Rc::clone(&at.declared),
            where_: at.where_.clone(),
            key: Some("<item>".to_string()),
            variables,
            suffix: "[<item>]".to_string(),
            concurrency_group: None,
        };
        let before_pending = self.pending.len();
        self.single(&position, shadow);
        let escaped = self.escaped.take();
        if escaped.is_none() {
            let mut index = scopes_before;
            while index < self.scopes.len() && self.fatal.is_none() {
                let scope: FrameId = self.scopes[index];
                let names: Vec<String> = self.frames[scope.0].steps.keys().cloned().collect();
                for name in names {
                    let _ = self.step(scope, &name);
                }
                index += 1;
            }
        }
        let mut low = Usd::ZERO;
        let mut high = Usd::ZERO;
        for (id, instance) in &self.instances {
            if saved_instances.contains_key(id) {
                continue;
            }
            match instance.state {
                super::State::Planned => {
                    low = low + instance.low();
                    high = high + instance.high();
                }
                super::State::Maybe => high = high + instance.high(),
                _ => {}
            }
        }
        for nested in self.pending.iter().skip(before_pending) {
            high = high + nested.high();
        }

        // Forget everything the hypothetical item expanded, steps it reached outside the repeat
        // included: they expand again, for real, when the plan reaches them.
        self.instances = saved_instances;
        self.pending = saved_pending;
        self.problems = saved_problems;
        self.memo = saved_memo;
        self.held = saved_held;
        self.scopes.truncate(scopes_before);
        self.shadowing -= 1;
        if let Some(problem) = escaped {
            self.problems.push(problem.clone());
            self.escaped = Some(problem);
        }
        (low, high)
    }
}

/// Every combination of one value per list, the last list varying fastest; one empty
/// combination for no lists, none when a list is empty.
fn product(lists: &[Vec<Val>]) -> Vec<Vec<Val>> {
    let mut combinations: Vec<Vec<Val>> = vec![Vec::new()];
    for list in lists {
        let mut next = Vec::with_capacity(combinations.len() * list.len());
        for combination in &combinations {
            for value in list {
                let mut extended = combination.clone();
                extended.push(value.clone());
                next.push(extended);
            }
        }
        combinations = next;
    }
    combinations
}

#[cfg(test)]
mod tests {
    use super::super::frame::FrameId;
    use super::super::testing::*;
    use super::*;
    use crate::docs::workflow::Step;

    fn strings(values: &[&str]) -> Vec<Val> {
        values.iter().map(|v| Val::Str((*v).into())).collect()
    }

    #[test]
    fn the_last_axis_varies_fastest() {
        let combos = product(&[strings(&["open", "shut"]), strings(&["smile", "flat"])]);
        assert_eq!(
            combos,
            [
                strings(&["open", "smile"]),
                strings(&["open", "flat"]),
                strings(&["shut", "smile"]),
                strings(&["shut", "flat"]),
            ]
        );
        // No axes: one empty combination; an empty axis: none.
        assert_eq!(product(&[]), vec![Vec::<Val>::new()]);
        assert!(product(&[strings(&["a"]), Vec::new()]).is_empty());
    }

    fn at(frame: FrameId, declared: Step) -> Position {
        Position {
            frame,
            name: "rep".into(),
            declared: Rc::new(declared),
            where_: "rep".into(),
            key: None,
            variables: IndexMap::new(),
            suffix: String::new(),
            concurrency_group: None,
        }
    }

    fn body() -> Step {
        let mut declared = group_step(vec![("m", node_step("nope"))]);
        declared.for_each = Some(Value::String("${{ steps.split.outputs.items }}".into()));
        declared
    }

    fn wheres(problems: &[crate::error::Problem]) -> Vec<(&str, &str)> {
        problems
            .iter()
            .map(|p| (p.where_.as_str(), p.message.as_str()))
            .collect()
    }

    const ADD_MAX: &str = "the list comes from a step, so the plan cannot count it: add max: \
                           (the most items this repeat may run)";

    #[test]
    fn a_pending_repeat_without_max_asks_for_one() {
        let mut fixture = Fixture::new(workflow_doc(vec![("rep", body())]));
        let (mut ex, root) = fixture.expander();
        let into = ex.new_exp(root, "rep");
        let scopes = ex.scopes.len();
        ex.pending_repeat(&at(root, body()), into, Vec::new());
        // The shadow's own problems (its member's refused `uses`) are discarded.
        assert_eq!(wheres(&ex.problems), [("rep", ADD_MAX)]);
        assert_eq!(ex.pending.len(), 1);
        let pending = &ex.pending[0];
        assert_eq!(pending.path, "rep");
        assert_eq!(pending.max, 1);
        assert_eq!(pending.phase, 1);
        assert_eq!(pending.per_instance_high, Usd::ZERO);
        assert_eq!(ex.exp(into).kind, ExpKind::Pending);
        assert!(ex.exp(into).token.is_some());
        // Nothing the shadow expanded stays.
        assert_eq!(ex.scopes.len(), scopes);
        assert!(ex.instances.is_empty());
    }

    #[test]
    fn a_pending_repeat_with_max() {
        let mut declared = body();
        declared.max = Some(Value::from(6));
        let mut fixture = Fixture::new(workflow_doc(vec![("rep", declared.clone())]));
        let (mut ex, root) = fixture.expander();
        let into = ex.new_exp(root, "rep");
        ex.pending_repeat(&at(root, declared), into, Vec::new());
        assert!(ex.problems.is_empty());
        assert_eq!(ex.pending[0].max, 6);
    }

    #[test]
    fn an_invalid_max_gives_both_problems() {
        for max in [Value::from(0), Value::from(10001), serde_json::json!(2.5)] {
            let mut declared = body();
            declared.max = Some(max);
            let mut fixture = Fixture::new(workflow_doc(vec![("rep", declared.clone())]));
            let (mut ex, root) = fixture.expander();
            let into = ex.new_exp(root, "rep");
            ex.pending_repeat(&at(root, declared), into, Vec::new());
            assert_eq!(
                wheres(&ex.problems),
                [
                    ("rep.max", "max: is a whole number from 1 to 10000"),
                    ("rep", ADD_MAX)
                ]
            );
            assert_eq!(ex.pending[0].max, 1);
        }
        let mut declared = body();
        declared.max = Some(Value::from(10000));
        let mut fixture = Fixture::new(workflow_doc(vec![("rep", declared.clone())]));
        let (mut ex, root) = fixture.expander();
        assert_eq!(ex.max_limit(root, &declared, "rep"), Some(10000));
        declared.max = None;
        assert_eq!(ex.max_limit(root, &declared, "rep"), None);
        assert!(ex.problems.is_empty());
    }

    #[test]
    fn a_body_whose_uses_does_not_resolve_leaves_the_step_absent() {
        let mut declared = node_step("nope");
        declared.for_each = Some(Value::String("${{ steps.split.outputs.items }}".into()));
        let mut fixture = Fixture::new(workflow_doc(vec![("rep", declared.clone())]));
        let (mut ex, root) = fixture.expander();
        let into = ex.new_exp(root, "rep");
        ex.pending_repeat(&at(root, declared), into, Vec::new());
        assert!(ex.pending.is_empty());
        assert_eq!(ex.exp(into).kind, ExpKind::Absent);
        let problems = wheres(&ex.problems);
        assert_eq!(problems.len(), 2);
        assert_eq!(problems[0], ("rep", ADD_MAX));
        // The `uses` refusal, reported once at the step.
        assert_eq!(problems[1].0, "rep");
        assert!(ex.escaped.is_none());
    }
}
