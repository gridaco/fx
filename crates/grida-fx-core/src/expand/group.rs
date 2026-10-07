//! Groups and workflows used as steps.
//!
//! A plain group registers its member scope (`prefix = path + "."`, `decl_prefix = where + "."`,
//! `parent = context`); members expand on reference or in the final sweep. A regenerating group
//! creates one nested take scope per take up to `max` (`takes_max` at `<w>.regenerate`), with
//! `feedback` when asked (take 1 missing, later takes `{member: facts}` of the previous take via
//! [`Expander::group_feedback`]), evaluates `until` in each (`<w>.regenerate.until`), stops after
//! the first true one, and makes later takes `maybe` while an `until` is pending. A used workflow:
//! its whole `with:` evaluated at `<w>.with` (a non-object becomes `{}`), unknown names
//! `{uses} has no input {name}` at `<w>.with.<name>` (sorted), bound with
//! [`crate::inputs::bind::bind_given`] (troubles at `<w>.with`), and a new root frame (no parent,
//! no variables, its own tables/let, the caller's takes, phase, budget and maybe); its document
//! is recorded for the plan digest under its project-relative path.

use super::frame::{ExpId, ExpKind, Frame, FrameChanges, FrameId, Until};
use super::{Expander, Position, instance_id};
use crate::docs::workflow::{LoadedWorkflow, Step, Then};
use crate::inputs;
use crate::val::{Pending, Val};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::rc::Rc;

impl Expander<'_> {
    /// A plain or regenerating group whose member scope is `child`.
    pub(crate) fn group(&mut self, child: FrameId, declared: &Rc<Step>, where_: &str, into: ExpId) {
        if declared.regenerate.is_some() {
            self.regenerating(child, declared, where_, into);
            return;
        }
        self.display_scope(child, declared, where_, None, Vec::new());
        // Members expand when something refers to them, or in the final sweep.
        let exp = &mut self.exps[into.0];
        exp.kind = ExpKind::Group;
        exp.scope = Some(child);
        self.scopes.push(child);
    }

    /// The takes of a regenerating group.
    pub(crate) fn regenerating(
        &mut self,
        child: FrameId,
        declared: &Rc<Step>,
        where_: &str,
        into: ExpId,
    ) {
        let Some(regeneration) = declared.regenerate.clone() else {
            return;
        };
        let exp = &mut self.exps[into.0];
        exp.kind = ExpKind::Regenerating;
        // `keep_best` on a group acts as `continue`.
        exp.then = match &regeneration.then {
            Then::KeepBest(_) => Then::Continue,
            then => then.clone(),
        };
        let until_source = Value::String(regeneration.until.clone().unwrap_or_default());
        let until_where = format!("{where_}.regenerate.until");
        let max = self.takes_max(child, &regeneration, &format!("{where_}.regenerate"));
        let mut previous = Until::Known(false);
        for take in 1..=max {
            if previous == Until::Known(true) {
                // Later takes are never created.
                break;
            }
            let (mut variables, mut takes, maybe) = {
                let frame = &self.frames[child.0];
                let maybe = frame.maybe || matches!(previous, Until::Pending(_));
                (frame.variables.clone(), frame.takes.clone(), maybe)
            };
            if regeneration.feedback {
                let said = if take == 1 {
                    Val::Missing
                } else {
                    self.group_feedback(declared, child, take - 1)
                };
                variables.insert("feedback".to_string(), said);
            }
            takes.push(take);
            let take_scope = self.nest(
                child,
                FrameChanges {
                    takes: Some(takes),
                    maybe: Some(maybe),
                    variables: Some(variables),
                    ..Default::default()
                },
            );
            self.display_scope(take_scope, declared, where_, None, Vec::new());
            self.exps[into.0].take_scopes.push(take_scope);
            self.scopes.push(take_scope);
            if let Until::Pending(pending) = &previous {
                // Undecided: every later take only runs if the run says so.
                let after = Pending::new(pending.refs.clone(), &json!({"after": pending.token}));
                self.exps[into.0].until.push(previous.clone());
                previous = Until::Pending(after);
                continue;
            }
            let value = self.evaluate(take_scope, &until_source, &until_where);
            previous = if value.contains_pending() {
                Until::Pending(Pending::new(value.pending_refs(), &value.token_form()))
            } else {
                Until::Known(value.truthy())
            };
            self.exps[into.0].until.push(previous.clone());
        }
    }

    /// A workflow used as a step. `context` is the position's frame,
    /// `path` its instance path.
    pub(crate) fn workflow_step(
        &mut self,
        at: &Position,
        context: FrameId,
        path: &str,
        used: Rc<LoadedWorkflow>,
        into: ExpId,
    ) {
        let where_ = &at.where_;
        let uses = at.declared.uses.clone().unwrap_or_default();
        let inner = Rc::clone(&used.workflow);
        // The whole `with:` in one evaluation: any error loses every given value.
        let with: serde_json::Map<String, Value> = at
            .declared
            .with
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let (given, input_bindings, input_interfaces) =
            self.workflow_with(context, &with, &format!("{where_}.with"));
        let given = match given {
            Val::Object(given) => given,
            _ => IndexMap::new(),
        };
        let mut unknown: Vec<&String> = given
            .keys()
            .filter(|name| !inner.inputs.contains_key(*name))
            .collect();
        unknown.sort();
        for name in unknown {
            self.problem(
                &format!("{where_}.with.{name}"),
                format!("{uses} has no input {name}"),
            );
        }
        let known: IndexMap<String, Val> = given
            .iter()
            .filter(|(name, _)| inner.inputs.contains_key(*name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let home = self.env.registry.root.clone();
        let mut resolve_ref = |reference: &str| inputs::resolve_ref(&home, reference);
        let (bound, troubles) = match inputs::compile_inputs(&inner.inputs, Some(&mut resolve_ref))
        {
            Ok(schema) => {
                let registry = &mut *self.env.registry;
                let mut read = |file: &str, kind: &str| registry.input_file(file, kind);
                inputs::bind::bind_given(&schema, known, &mut read)
            }
            // The inputs cannot be bound: the inner steps see what was given, as given.
            Err(error) => (given, vec![error.message]),
        };
        for trouble in troubles {
            self.problem(&format!("{where_}.with"), trouble);
        }
        let (takes, phase, budget, maybe) = {
            let frame = &self.frames[context.0];
            (
                frame.takes.clone(),
                frame.phase,
                frame.budget.clone(),
                frame.maybe,
            )
        };
        // A root of its own: no parent, no variables, its own tables and let; the caller's takes,
        // phase, budget and maybe.
        let root = self.new_frame(Frame {
            display_scope: self.frames[context.0].display_scope.clone(),
            workflow_scope: None,
            variable_interfaces: Default::default(),
            steps: Rc::clone(&inner.steps),
            prefix: format!("{path}."),
            decl_prefix: format!("{where_}."),
            variables: IndexMap::new(),
            parent: None,
            takes,
            workflow: Rc::clone(&inner),
            inputs: Rc::new(bound),
            input_bindings: Rc::new(input_bindings),
            variable_bindings: Default::default(),
            phase,
            judging: None,
            budget,
            owner: None,
            maybe,
        });
        self.display_scope(
            root,
            &at.declared,
            where_,
            Some(&used),
            input_interfaces
                .into_iter()
                .filter(|b| inner.inputs.contains_key(&b.target_port))
                .collect(),
        );
        self.workflow_root(root, &used.source, context);
        let exp = &mut self.exps[into.0];
        exp.kind = ExpKind::Workflow;
        exp.scope = Some(root);
        exp.document = Some(used);
        self.scopes.push(root);
    }

    /// The same one-pass object resolution as `evaluate`, observing each input's lineage.
    /// As before, one refused member loses the whole object; finishing still happens once,
    /// after every member has resolved. Metadata does not enter the bound values.
    fn workflow_with(
        &mut self,
        frame: FrameId,
        with: &serde_json::Map<String, Value>,
        where_: &str,
    ) -> (
        Val,
        std::collections::BTreeMap<String, Vec<super::wiring::Binding>>,
        Vec<super::wiring::Binding>,
    ) {
        let mut values = IndexMap::new();
        let mut bindings = std::collections::BTreeMap::new();
        let mut interfaces = Vec::new();
        for (name, raw) in with {
            self.wiring.begin_capture(name);
            let value = crate::expr::resolve(raw, &mut super::scope::StepScope { ex: self, frame });
            let (leaf, boundary) = self.wiring.end_capture_full();
            bindings.insert(name.clone(), leaf);
            interfaces.extend(boundary);
            match value {
                Ok(value) => {
                    values.insert(name.clone(), value);
                }
                Err(error) => return (self.refused(where_, error), Default::default(), Vec::new()),
            }
        }
        let value = crate::expr::finish(
            &mut super::scope::StepScope { ex: self, frame },
            Val::Object(values),
        );
        match value {
            Ok(value) => (value, bindings, interfaces),
            Err(error) => (self.refused(where_, error), Default::default(), Vec::new()),
        }
    }

    /// `{member: facts}` of each direct node member's latest result in group take `take`
    /// (gnode `_group_feedback`); `{}` when nothing has run.
    pub(crate) fn group_feedback(&self, declared: &Step, child: FrameId, take: u32) -> Val {
        let frame = &self.frames[child.0];
        let mut takes = frame.takes.clone();
        takes.push(take);
        let mut said = IndexMap::new();
        for name in declared.steps.iter().flat_map(|steps| steps.keys()) {
            let stem = instance_id(&format!("{}{name}", frame.prefix), &takes);
            let nested = format!("{stem}.");
            let latest = self
                .env
                .results
                .iter()
                .filter(|(id, _)| *id == &stem || id.starts_with(&nested))
                .max_by_key(|(id, _)| take_path(id));
            if let Some((_, result)) = latest {
                let facts = result
                    .facts
                    .iter()
                    .map(|(key, value)| (key.clone(), Val::from_json(value)))
                    .collect();
                said.insert(name.clone(), Val::Object(facts));
            }
        }
        Val::Object(said)
    }
}

/// An instance id's take numbers, outermost first: `part.mesh#2.10` is `[2, 10]`.
fn take_path(id: &str) -> Vec<u64> {
    let takes = id.rsplit_once('#').map_or(id, |(_, takes)| takes);
    takes
        .split('.')
        .filter(|take| !take.is_empty() && take.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|take| take.parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::frame::FrameChanges;
    use super::super::testing::*;
    use super::super::{NodeResult, ResultStatus};
    use super::*;
    use crate::docs::workflow::{Regeneration, TakesMax};

    fn result(fact: &str) -> NodeResult {
        let mut facts = IndexMap::new();
        facts.insert(fact.to_string(), Value::from(1));
        NodeResult {
            status: ResultStatus::Succeeded,
            outputs: IndexMap::new(),
            facts,
            error: None,
        }
    }

    #[test]
    fn take_paths_of_ids() {
        assert_eq!(take_path("part.mesh#2.10"), [2, 10]);
        assert_eq!(take_path("draw#1"), [1]);
        assert_eq!(take_path("q['a#b'].x#3"), [3]);
        assert!(take_path("nothing").is_empty());
    }

    #[test]
    fn group_feedback_is_each_member_latest_result_in_a_take() {
        let declared = group_step(vec![
            ("mesh", node_step("nope")),
            ("audit", node_step("nope")),
            ("idle", node_step("nope")),
        ]);
        let mut fixture = Fixture::new(workflow_doc(vec![("build", declared.clone())]));
        fixture
            .results
            .insert("build['k'].mesh#2".into(), result("first"));
        fixture
            .results
            .insert("build['k'].mesh#2.10".into(), result("latest"));
        fixture
            .results
            .insert("build['k'].mesh#2.3".into(), result("middle"));
        fixture
            .results
            .insert("build['k'].mesh#1".into(), result("old"));
        fixture
            .results
            .insert("build['k'].audit#2".into(), result("audit"));
        fixture
            .results
            .insert("build['k'].meshy#2".into(), result("other"));
        let (mut ex, root) = fixture.expander();
        let child = ex.nest(
            root,
            FrameChanges {
                prefix: Some("build['k'].".into()),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        let Val::Object(said) = ex.group_feedback(&declared, child, 2) else {
            panic!("not an object");
        };
        assert_eq!(said.keys().collect::<Vec<_>>(), ["mesh", "audit"]);
        let Val::Object(mesh) = &said["mesh"] else {
            panic!("not an object");
        };
        assert_eq!(mesh.keys().collect::<Vec<_>>(), ["latest"]);
        // Nothing has run in take 3.
        assert_eq!(
            ex.group_feedback(&declared, child, 3),
            Val::Object(IndexMap::new())
        );
    }

    #[test]
    fn a_plain_group_registers_its_scope_and_waits() {
        let mut fixture = Fixture::new(workflow_doc(vec![(
            "g",
            group_step(vec![("m", node_step("nope"))]),
        )]));
        let (mut ex, root) = fixture.expander();
        let exp = ex.step(root, "g").unwrap();
        assert_eq!(ex.exp(exp).kind, ExpKind::Group);
        let scope = ex.exp(exp).scope.unwrap();
        assert_eq!(ex.scopes, [scope]);
        let frame = ex.frame(scope);
        assert_eq!(frame.prefix, "g.");
        assert_eq!(frame.decl_prefix, "g.");
        assert_eq!(frame.owner, None);
        let parent = frame.parent.unwrap();
        assert_eq!(ex.scope_of(parent), root);
        // Members wait for a reference or the sweep.
        assert!(!ex.memo.contains_key(&(scope, "m".to_string())));
        assert!(ex.problems.is_empty());
    }

    #[test]
    fn a_regenerating_group_makes_take_scopes() {
        let mut declared = group_step(vec![("m", node_step("nope"))]);
        declared.regenerate = Some(Regeneration {
            max: TakesMax::Count(1),
            then: Then::KeepBest(crate::docs::workflow::KeepBest {
                by: "score".into(),
                highest: true,
            }),
            until: Some("${{ true }}".into()),
            feedback: true,
        });
        let mut fixture = Fixture::new(workflow_doc(vec![("build", declared)]));
        let (mut ex, root) = fixture.expander();
        let exp = ex.step(root, "build").unwrap();
        let expansion = ex.exp(exp);
        assert_eq!(expansion.kind, ExpKind::Regenerating);
        assert_eq!(expansion.then, Then::Continue);
        assert_eq!(expansion.take_scopes.len(), 1);
        assert_eq!(expansion.until.len(), 1);
        let take = expansion.take_scopes[0];
        assert_eq!(ex.scopes, [take]);
        let frame = ex.frame(take);
        assert_eq!(frame.takes, [1]);
        assert_eq!(frame.prefix, "build.");
        assert!(!frame.maybe);
        assert_eq!(frame.variables.get("feedback"), Some(&Val::Missing));
        // Only problems about `until` may come up (none once expressions evaluate).
        assert!(
            ex.problems
                .iter()
                .all(|p| p.where_ == "build.regenerate.until")
        );
    }

    #[test]
    fn a_used_workflow_gets_a_root_of_its_own() {
        let inner = workflow_doc(vec![("loud", node_step("nope"))]);
        let used = Rc::new(LoadedWorkflow {
            document: Value::Null,
            workflow: Rc::new(inner),
            source: "workflows/inner.yaml".into(),
            path: "/nowhere/workflows/inner.yaml".into(),
        });
        let mut declared = node_step("./workflows/inner.yaml");
        declared.budget = None;
        let mut fixture = Fixture::new(workflow_doc(vec![("w", declared.clone())]));
        let (mut ex, root) = fixture.expander();
        let mut variables = IndexMap::new();
        variables.insert("item".to_string(), Val::Str("ada".into()));
        let context = ex.derive(
            root,
            FrameChanges {
                variables: Some(variables),
                takes: Some(vec![2]),
                phase: Some(3),
                maybe: Some(true),
                ..Default::default()
            },
        );
        let into = ex.new_exp(root, "each");
        let position = Position {
            frame: root,
            name: "each".into(),
            declared: Rc::new(declared),
            where_: "each".into(),
            key: Some("ada".into()),
            variables: IndexMap::new(),
            suffix: "['ada']".into(),
            concurrency_group: Some("each".into()),
        };
        ex.workflow_step(&position, context, "each['ada']", used, into);
        let expansion = ex.exp(into);
        assert_eq!(expansion.kind, ExpKind::Workflow);
        assert!(expansion.document.is_some());
        let scope = expansion.scope.unwrap();
        assert_eq!(ex.scopes, [scope]);
        let frame = ex.frame(scope);
        assert_eq!(frame.prefix, "each['ada'].");
        assert_eq!(frame.decl_prefix, "each.");
        assert_eq!(frame.parent, None);
        assert_eq!(frame.owner, None);
        // It does not see the caller's item.
        assert!(frame.variables.is_empty());
        assert_eq!(frame.takes, [2]);
        assert_eq!(frame.phase, 3);
        assert!(frame.maybe);
        assert!(frame.steps.contains_key("loud"));
        assert!(ex.uses_itself(scope, "workflows/inner.yaml"));
        assert!(ex.problems.iter().all(|p| p.where_ == "each.with"));
    }
}
