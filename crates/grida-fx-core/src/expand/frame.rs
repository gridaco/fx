//! Frames, scopes and step expansions.
//!
//! Frames live in the expander's arena (`Expander::frames`) and are named by [`FrameId`]. A
//! frame's *scope* is its `owner` when set, else itself: `derive` makes a frame of the same scope
//! with other variables, takes, judging, budget, phase or `maybe`; `nest` makes a new scope (a
//! group instance, one take of a regenerating group, a used workflow's root). `find` walks scope,
//! then each parent's scope, up to the current workflow's root.

use super::Expander;
use crate::docs::workflow::LoadedWorkflow;
use crate::docs::workflow::{Steps, Then, WorkflowDoc};
use crate::money::Usd;
use crate::val::{Pending, Val};
use indexmap::IndexMap;
use std::rc::Rc;

/// A frame in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct FrameId(pub usize);

/// A step expansion in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ExpId(pub usize);

/// Where steps are declared, with an evaluation context.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub display_scope: Option<String>,
    pub workflow_scope: Option<String>,
    pub steps: Steps,
    /// Instance path prefix (`""`, `"g['a']."`).
    pub prefix: String,
    /// Declaration prefix (`""`, `"g."`).
    pub decl_prefix: String,
    pub variables: IndexMap<String, Val>,
    pub variable_bindings: std::collections::BTreeMap<String, Vec<super::wiring::Binding>>,
    pub variable_interfaces: std::collections::BTreeMap<String, Vec<super::wiring::Binding>>,
    /// The frame a group was nested from; `None` for a workflow root (a used workflow's too).
    pub parent: Option<FrameId>,
    pub takes: Vec<u32>,
    pub workflow: Rc<WorkflowDoc>,
    pub inputs: Rc<IndexMap<String, Val>>,
    pub input_bindings: Rc<std::collections::BTreeMap<String, Vec<super::wiring::Binding>>>,
    pub phase: u32,
    /// `(judged step name, take)` while a judge's take is expanded.
    pub judging: Option<(String, u32)>,
    pub budget: Option<(String, Usd)>,
    /// When set, this frame is only an evaluation context of that scope.
    pub owner: Option<FrameId>,
    pub maybe: bool,
}

/// Changes for [`Expander::derive`] and [`Expander::nest`]; `None` keeps the field.
#[derive(Debug, Clone, Default)]
pub(crate) struct FrameChanges {
    pub steps: Option<Steps>,
    pub prefix: Option<String>,
    pub decl_prefix: Option<String>,
    pub variables: Option<IndexMap<String, Val>>,
    pub variable_bindings: Option<std::collections::BTreeMap<String, Vec<super::wiring::Binding>>>,
    pub variable_interfaces:
        Option<std::collections::BTreeMap<String, Vec<super::wiring::Binding>>>,
    pub parent: Option<Option<FrameId>>,
    pub takes: Option<Vec<u32>>,
    pub phase: Option<u32>,
    pub judging: Option<Option<(String, u32)>>,
    pub budget: Option<Option<(String, Usd)>>,
    pub maybe: Option<bool>,
}

/// What a declared step became.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExpKind {
    Expanding,
    Absent,
    Node,
    Repeat,
    Pending,
    Group,
    Workflow,
    Regenerating,
}

/// A regenerating group's `until` after one take: decided, or pending.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Until {
    Known(bool),
    Pending(Pending),
}

/// One declared step's expansion in one scope.
#[derive(Debug, Clone)]
pub(crate) struct StepExpansion {
    pub kind: ExpKind,
    /// The frame it was expanded from (its scope) and its name.
    pub frame: FrameId,
    pub name: String,
    /// `Node`: its instance ids, one per take.
    pub instances: Vec<String>,
    /// `Repeat`: `(key, child)` in item order; a child is added before it expands.
    pub children: Vec<(String, ExpId)>,
    /// `Group`, `Workflow`: the member scope.
    pub scope: Option<FrameId>,
    /// `Regenerating`: one scope per take.
    pub take_scopes: Vec<FrameId>,
    /// `Workflow`: the used workflow.
    pub document: Option<Rc<LoadedWorkflow>>,
    /// `Pending`: the value a reference reads.
    pub token: Option<Pending>,
    /// `Regenerating`: `until` after each take, and what happens when the last is false.
    pub until: Vec<Until>,
    pub then: Then,
    /// The take numbers of a node step (for judges).
    pub takes: Vec<u32>,
}

impl StepExpansion {
    pub(crate) fn new(frame: FrameId, name: &str) -> StepExpansion {
        StepExpansion {
            kind: ExpKind::Expanding,
            frame,
            name: name.to_string(),
            instances: Vec::new(),
            children: Vec::new(),
            scope: None,
            take_scopes: Vec::new(),
            document: None,
            token: None,
            until: Vec::new(),
            then: Then::Fail,
            takes: Vec::new(),
        }
    }
}

impl Expander<'_> {
    /// Adds a frame to the arena.
    pub(crate) fn new_frame(&mut self, frame: Frame) -> FrameId {
        self.frames.push(frame);
        FrameId(self.frames.len() - 1)
    }

    pub(crate) fn frame(&self, id: FrameId) -> &Frame {
        &self.frames[id.0]
    }

    /// The scope of a frame: its owner, else itself.
    pub(crate) fn scope_of(&self, id: FrameId) -> FrameId {
        self.frames[id.0].owner.unwrap_or(id)
    }

    /// The same scope with changes (`owner = scope_of(id)`).
    pub(crate) fn derive(&mut self, id: FrameId, changes: FrameChanges) -> FrameId {
        let owner = self.scope_of(id);
        let mut frame = self.frames[id.0].clone();
        changes.apply(&mut frame);
        frame.owner = Some(owner);
        self.new_frame(frame)
    }

    /// A new scope with changes (`owner = None`).
    pub(crate) fn nest(&mut self, id: FrameId, changes: FrameChanges) -> FrameId {
        let mut frame = self.frames[id.0].clone();
        changes.apply(&mut frame);
        frame.owner = None;
        self.new_frame(frame)
    }

    /// The nearest scope, from `id`'s scope outwards through parents, whose steps hold `name`.
    pub(crate) fn find(&self, id: FrameId, name: &str) -> Option<FrameId> {
        let mut current = Some(self.scope_of(id));
        while let Some(scope) = current {
            let frame = &self.frames[scope.0];
            if frame.steps.contains_key(name) {
                return Some(scope);
            }
            current = frame.parent.map(|parent| self.scope_of(parent));
        }
        None
    }

    /// Adds a step expansion to the arena.
    pub(crate) fn new_exp(&mut self, frame: FrameId, name: &str) -> ExpId {
        self.exps.push(StepExpansion::new(frame, name));
        ExpId(self.exps.len() - 1)
    }

    pub(crate) fn exp(&self, id: ExpId) -> &StepExpansion {
        &self.exps[id.0]
    }

    pub(crate) fn exp_mut(&mut self, id: ExpId) -> &mut StepExpansion {
        &mut self.exps[id.0]
    }

    /// Its instances' ids, then each child's (recursively), then each member's: expands every
    /// member not yet expanded (`needs:` relies on it). A node whose instances are still being
    /// made gives those made so far.
    pub(crate) fn instance_ids(&mut self, id: ExpId) -> Vec<String> {
        if self.is_instantiating(id) {
            self.reached_under_way(id);
        }
        let mut ids = self.exps[id.0].instances.clone();
        let children: Vec<ExpId> = self.exps[id.0]
            .children
            .iter()
            .map(|(_, child)| *child)
            .collect();
        for child in children {
            ids.extend(self.instance_ids(child));
        }
        for member in self.members(id) {
            ids.extend(self.instance_ids(member));
        }
        ids
    }

    /// `step(scope, name)` for every name of the scope and of each take scope, in declaration
    /// order.
    ///
    /// A member still being expanded (a `needs:` that reaches back into its own group) is left
    /// out: `needs:` cycles stay silent.
    pub(crate) fn members(&mut self, id: ExpId) -> Vec<ExpId> {
        let exp = &self.exps[id.0];
        let scopes: Vec<FrameId> = exp.scope.iter().chain(&exp.take_scopes).copied().collect();
        let mut members = Vec::new();
        for scope in scopes {
            let names: Vec<String> = self.frames[scope.0].steps.keys().cloned().collect();
            for name in names {
                if let Ok(member) = self.step(scope, &name) {
                    members.push(member);
                }
            }
        }
        members
    }
}

impl FrameChanges {
    /// Writes every set field into `frame`.
    fn apply(self, frame: &mut Frame) {
        if let Some(steps) = self.steps {
            frame.steps = steps;
        }
        if let Some(prefix) = self.prefix {
            frame.prefix = prefix;
        }
        if let Some(decl_prefix) = self.decl_prefix {
            frame.decl_prefix = decl_prefix;
        }
        if let Some(variables) = self.variables {
            frame.variables = variables;
        }
        if let Some(interfaces) = self.variable_interfaces {
            frame.variable_interfaces = interfaces;
        }
        if let Some(bindings) = self.variable_bindings {
            frame.variable_bindings = bindings;
        }
        if let Some(parent) = self.parent {
            frame.parent = parent;
        }
        if let Some(takes) = self.takes {
            frame.takes = takes;
        }
        if let Some(phase) = self.phase {
            frame.phase = phase;
        }
        if let Some(judging) = self.judging {
            frame.judging = judging;
        }
        if let Some(budget) = self.budget {
            frame.budget = budget;
        }
        if let Some(maybe) = self.maybe {
            frame.maybe = maybe;
        }
    }
}
