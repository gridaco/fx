//! Expansion: a workflow document into step instances, as stage-gen's engine expands it.
//!
//! The public model ([`Instance`], [`PendingRepeat`], [`Expansion`], [`NodeResult`]) is what
//! planning and the outputs read. The [`Expander`] is internal; its methods are spread over the
//! submodules by concern:
//!
//! | file | owns |
//! |---|---|
//! | `mod.rs` | the model, the driver ([`expand`]), the `step` memo and cycles, `single`, conditions, assertions, evaluation helpers and read sets, phases, outputs, type resolution, ids (identity.md §11) |
//! | `frame.rs` | frames and scopes, step expansions, `instance_ids`/`members` |
//! | `repeat.rs` | `for_each`, `matrix`, `max`, pending repeats, shadow pricing |
//! | `group.rs` | groups, regenerating groups, workflows used as steps |
//! | `scope.rs` | the step scope and prompt-file scope, step views |
//! | `node.rs` | node steps and instances: with-values, files, templates, identity, state, needs |
//! | `judge.rs` | takes, judges, sequencing, the chosen take, `pick`, `keep_best`, feedback, select reads |
//! | `bind.rs` | route binding, `requires`, per-step price, `independent_of` |
//!
//! The driver: the root frame, the workflow's `assert:` checks, every root step in declaration
//! order, then a sweep of every group and used-workflow scope in the order they were registered
//! (scopes registered during the sweep included), then the root outputs. A step expands once per
//! scope, on demand: a reference expands what it names, so the listing order is a post-order
//! walk driven by references. A plain step position is its condition, then its assertions, then
//! a group, a used workflow or a node.
//!
//! FX decisions recorded here:
//! - names: `fx/…`, `fx.yaml`, `grida-fx` in messages; key text of numbers is JCS (`2.0` → `2`);
//! - native select is keyed on the built-in `fx/select@1` only;
//! - top-level failed items of select's `first_of` become missing (gnode's rule kept);
//! - a failed or pending part of a mixed template makes the string failed or pending;
//! - every cycle is refused: a node reading itself or a mutual `with:` cycle reports
//!   `<decl path> refers back to itself` instead of reading missing; a `let` that refers to
//!   itself reports `let.<name> refers back to itself`; a workflow used inside itself (directly
//!   or through other workflows) reports `<uses> refers back to itself` at the step; `needs:`
//!   cycles stay silent (gnode); a step's judges and `independent_of` targets, which it expands
//!   for its own sake, are held while a step under way is unfinished, so they read that step as
//!   pending where gnode read missing, and list after it; held judges that a step begun later
//!   settles by reading the judged step are expanded on trial, undone when they reach that
//!   reader, which is their own evidence and reads the judged take unjudged, as in gnode
//!   (`node.rs`);
//! - at most [`MAX_CHAIN`] step expansions are under way inside each other (a chain of steps
//!   reading each other): the next is refused, `the chain of steps reading each other is longer
//!   than 2000`; the frames between two links are kept small (`expr::evaluate`);
//! - `reads` keep gnode's leak (a step first expanded while another step's `with:` is evaluated
//!   adds what it read to that step's reads) and phases gnode's rule (only a repeat's list starts
//!   a phase), for an exact comparison;
//! - template params render in declaration order; a missing declared resource is a problem on
//!   the step; lock drift is a problem on the step;
//! - an `at: plan` step whose type is paid is refused (`an at: plan step makes no paid call`) and
//!   never run.

pub mod bind;
pub mod display;
pub mod frame;
pub mod group;
pub mod judge;
pub mod node;
pub mod repeat;
pub mod scope;
pub mod wiring;

use crate::docs::takes::Takes;
use crate::docs::workflow::{Assertion, LoadedWorkflow, OnFail, OnReject, Step};
use crate::error::{Error, Problem, Result};
use crate::expr::{self, ExprError};
use crate::host::NodeHost;
use crate::money::Usd;
use crate::registry::{Registry, RegistryError, Resolved, ResolvedType};
use crate::routes::{Route, RouteTable};
use crate::val::{Pending, Val};
use frame::{ExpId, Frame, FrameId, StepExpansion};
use indexmap::IndexMap;
use scope::{StepScope, ViewData};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

/// The most step expansions that may be under way inside each other: a chain of steps each
/// reading the next one, with a group, a used workflow or a judge's subject counting as a link.
pub const MAX_CHAIN: usize = 2000;

/// An instance's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Planned,
    Maybe,
    Absent,
    Blocked,
    Failed,
    Done,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Planned => "planned",
            State::Maybe => "maybe",
            State::Absent => "absent",
            State::Blocked => "blocked",
            State::Failed => "failed",
            State::Done => "done",
        }
    }

    /// `planned` or `maybe`: what pricing counts.
    pub fn is_live(self) -> bool {
        matches!(self, State::Planned | State::Maybe)
    }
}

/// The price of one capability's calls in one instance (gnode `CallPrice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPrice {
    pub capability: String,
    /// The route id.
    pub route: String,
    pub calls: u32,
    pub low: Usd,
    pub high: Usd,
}

/// One node type run once, at one take, in one repeat position.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// `path#t1.t2` (identity.md §11).
    pub id: String,
    /// The step path with repeat keys.
    pub path: String,
    /// The declaration path, no keys, no takes.
    pub step: String,
    /// One take per regenerating level, outermost first.
    pub takes: Vec<u32>,
    /// The step's `uses` as written.
    pub uses: String,
    pub ty: Rc<ResolvedType>,
    /// Evaluated with-values, defaults filled in (identity.md §8).
    pub with: IndexMap<String, Val>,
    /// From `needs:`, deduplicated in first-seen order.
    pub needs: Vec<String>,
    pub state: State,
    /// The step identity; `None` while a with-value is pending.
    pub identity: Option<String>,
    /// Bound routes by capability, in the type's call order.
    pub routes: IndexMap<String, Route>,
    /// Empty for `fx/select@1` and free types.
    pub prices: Vec<CallPrice>,
    pub phase: u32,
    /// The key of a directly repeated node step; `None` for members of a repeated group.
    pub key: Option<String>,
    /// The instance this judge judges.
    pub judges: Option<String>,
    pub judge_policy: Option<OnReject>,
    /// Judges of this instance, in link order.
    pub judged_by: Vec<String>,
    pub view: Value,
    pub at_plan: bool,
    /// The innermost step budget: (owner path with keys, ceiling).
    pub budget: Option<(String, Usd)>,
    pub concurrency_group: Option<String>,
    pub concurrency: Option<u32>,
    pub timeout_s: Option<f64>,
    pub reason: Option<String>,
    /// Ids of instances whose results its evaluation read.
    pub reads: BTreeSet<String>,
    /// Display-only, resolved output/fact references per input or parameter.
    pub bindings: Vec<wiring::Binding>,
    pub interface_bindings: Vec<wiring::Binding>,
    pub display_scope: Option<String>,
}

impl Instance {
    /// The refs of every pending value in `with` (`waiting_on`).
    pub fn waiting_on(&self) -> BTreeSet<String> {
        self.with.values().flat_map(Val::pending_refs).collect()
    }

    /// `(reads ∪ waiting_on ∪ needs) − {id}`, sorted: the graph's `reads`.
    pub fn inputs_from(&self) -> BTreeSet<String> {
        let mut all = self.reads.clone();
        all.extend(self.waiting_on());
        all.extend(self.needs.iter().cloned());
        all.remove(&self.id);
        all
    }

    pub fn low(&self) -> Usd {
        self.prices.iter().map(|p| p.low).sum()
    }

    pub fn high(&self) -> Usd {
        self.prices.iter().map(|p| p.high).sum()
    }

    /// The last take number.
    pub fn take(&self) -> u32 {
        self.takes.last().copied().unwrap_or(1)
    }
}

/// A repeat whose list only a run produces.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRepeat {
    pub display_scope: Option<String>,
    /// `prefix + name`, no own key suffix.
    pub path: String,
    /// `max:`, or 1 when absent or invalid.
    pub max: u32,
    pub waiting_on: BTreeSet<String>,
    pub per_instance_low: Usd,
    pub per_instance_high: Usd,
    pub phase: u32,
}

impl PendingRepeat {
    /// `max × per-instance high`.
    pub fn high(&self) -> Usd {
        self.per_instance_high.times(self.max as u64)
    }
}

/// A result's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultStatus {
    Succeeded,
    Failed,
    Skipped,
}

/// What an instance produced (gnode `Result`): outputs by port (files, collections, lists, or any
/// value for select), node facts, and an error for a failure.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeResult {
    pub status: ResultStatus,
    pub outputs: IndexMap<String, Val>,
    pub facts: IndexMap<String, Value>,
    pub error: Option<String>,
}

impl NodeResult {
    /// `facts.verdict` when it is text.
    pub fn verdict(&self) -> Option<&str> {
        self.facts.get("verdict").and_then(Value::as_str)
    }
}

/// The result of one expansion.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Expansion {
    pub scopes: Vec<display::Scope>,
    /// In insertion order: the graph's listing order.
    pub instances: IndexMap<String, Instance>,
    pub pending: Vec<PendingRepeat>,
    /// Not deduplicated (planning does that).
    pub problems: Vec<Problem>,
    /// The root workflow's outputs.
    pub outputs: IndexMap<String, Val>,
    /// Every workflow document the plan used, by plan-digest source: the root first, then each
    /// workflow used as a step (identity.md §10 `workflows`).
    pub workflows: IndexMap<String, Value>,
    /// Each distinct `uses` of a node type the expansion resolved, in first-resolved order.
    pub types: IndexMap<String, Rc<ResolvedType>>,
}

impl Expansion {
    /// Instances in listing order.
    pub fn ordered(&self) -> impl Iterator<Item = &Instance> {
        self.instances.values()
    }
}

/// What an expansion reads.
pub struct ExpandEnv<'a> {
    pub workflow: &'a Rc<LoadedWorkflow>,
    /// The root inputs' values (defaults filled in).
    pub inputs: &'a IndexMap<String, Val>,
    pub registry: &'a mut Registry,
    pub host: &'a mut dyn NodeHost,
    pub routes: &'a RouteTable,
    pub takes: &'a Takes,
    pub results: &'a IndexMap<String, NodeResult>,
}

/// Expands a workflow. Problems are collected; an `Err` is fatal (exit 2): a used workflow that
/// is not a valid document, a host that cannot start.
pub fn expand(env: ExpandEnv<'_>) -> Result<Expansion> {
    Expander::new(env).run()
}

/// `path + "#" + takes joined with "."` (identity.md §11).
pub fn instance_id(path: &str, takes: &[u32]) -> String {
    let takes: Vec<String> = takes.iter().map(u32::to_string).collect();
    format!("{path}#{}", takes.join("."))
}

/// A key quoted for a path: `'` + key with `\` and `'` escaped by a backslash + `'`.
pub fn quote_key(key: &str) -> String {
    format!("'{}'", key.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// One position of a declared step: the whole step, or one repeat item (gnode `_single`'s
/// arguments).
#[derive(Debug, Clone)]
pub(crate) struct Position {
    /// The scope frame the step is declared in.
    pub frame: FrameId,
    pub name: String,
    pub declared: Rc<Step>,
    /// The declaration path (`frame.decl_prefix + name`).
    pub where_: String,
    /// The repeat key, for a repeat item.
    pub key: Option<String>,
    /// Item variables (`item`/`as`, `matrix`).
    pub variables: IndexMap<String, Val>,
    /// `['key']…`, or empty.
    pub suffix: String,
    /// `prefix + name` for the items of a repeated node step.
    pub concurrency_group: Option<String>,
}

/// An `if:` outcome.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Condition {
    True,
    False,
    /// Only a run decides: the context becomes `maybe`.
    Pending(Pending),
}

/// The expander: one per expansion; every memo is per call.
pub(crate) struct Expander<'a> {
    pub(crate) env: ExpandEnv<'a>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) exps: Vec<StepExpansion>,
    /// `(scope frame, step name)` → its expansion.
    pub(crate) memo: HashMap<(FrameId, String), ExpId>,
    /// Scopes registered for the final sweep, in registration order.
    pub(crate) scopes: Vec<FrameId>,
    pub(crate) instances: IndexMap<String, Instance>,
    pub(crate) pending: Vec<PendingRepeat>,
    pub(crate) problems: Vec<Problem>,
    /// The active read sets, innermost last (`_evaluate_reading`).
    pub(crate) read_sets: Vec<BTreeSet<String>>,
    pub(crate) wiring: wiring::Trace,
    pub(crate) display_scopes: IndexMap<String, display::Scope>,
    /// Evaluating a select's `first_of`.
    pub(crate) selecting: bool,
    /// Views handed out to expressions; indices are [`crate::val::ViewId`]s.
    pub(crate) views: Vec<ViewData>,
    /// `let` names being evaluated, for the cycle refusal.
    pub(crate) lets_active: Vec<String>,
    pub(crate) workflows: IndexMap<String, Value>,
    pub(crate) types: IndexMap<String, Rc<ResolvedType>>,
    /// A fatal error; expansion stops making progress and [`Expander::run`] returns it.
    pub(crate) fatal: Option<Error>,
    /// Node steps whose instances are being created, innermost last: a reference to one of them
    /// through `steps.…` before its instances exist refers back to itself (FX refuses every
    /// cycle except `needs:`).
    instantiating: Vec<ExpId>,
    /// Step expansions under way ([`Expander::step`]), innermost last.
    pub(crate) active: Vec<ExpId>,
    /// Node-step work held while a step under way is unfinished, oldest first (`node.rs`).
    pub(crate) held: Vec<node::Held>,
    /// Held judges being settled on trial, innermost last (`node.rs`).
    pub(crate) trials: Vec<node::Trial>,
    /// While a trial is under way: instances that existed before it, as they were before it
    /// changed them (`(index, instance)`, oldest first).
    pub(crate) undo: Vec<(usize, Instance)>,
    /// Shadow pricing under way (`repeat.rs`): nothing is held inside a shadow.
    pub(crate) shadowing: u32,
    /// Set by [`Expander::single`] when its `uses` did not resolve: the problem it reported.
    /// gnode raises there, which leaves a whole repeat (or a pending repeat's pricing) absent,
    /// not just one item; the repeat and the shadow pricing read it right after `single`.
    escaped: Option<Problem>,
    /// Each workflow root frame: its workflow's source, and the context frame of the step that
    /// used it (`None` for the root workflow). A workflow that uses itself is refused with it.
    workflow_roots: HashMap<FrameId, (String, Option<FrameId>)>,
}

impl<'a> Expander<'a> {
    pub(crate) fn new(env: ExpandEnv<'a>) -> Expander<'a> {
        Expander {
            env,
            frames: Vec::new(),
            exps: Vec::new(),
            memo: HashMap::new(),
            scopes: Vec::new(),
            instances: IndexMap::new(),
            pending: Vec::new(),
            problems: Vec::new(),
            read_sets: Vec::new(),
            wiring: wiring::Trace::default(),
            display_scopes: IndexMap::new(),
            selecting: false,
            views: Vec::new(),
            lets_active: Vec::new(),
            workflows: IndexMap::new(),
            types: IndexMap::new(),
            fatal: None,
            instantiating: Vec::new(),
            active: Vec::new(),
            held: Vec::new(),
            trials: Vec::new(),
            undo: Vec::new(),
            shadowing: 0,
            escaped: None,
            workflow_roots: HashMap::new(),
        }
    }

    /// The driver: the root frame, workflow assertions, root steps in
    /// declaration order, the scope sweep, then the outputs.
    pub(crate) fn run(mut self) -> Result<Expansion> {
        let root = self.root_frame();
        let workflow = Rc::clone(&self.frames[root.0].workflow);
        self.assertions(&workflow.asserts, root, "workflow", None);
        for name in workflow.steps.keys() {
            if self.fatal.is_some() {
                break;
            }
            // Nothing is being expanded at the top, so the step cannot refer back to itself.
            let _ = self.step(root, name);
        }
        // Groups and used workflows expand their members on demand; now expand whatever nothing
        // referred to, scopes registered during the sweep included.
        let mut index = 0;
        while index < self.scopes.len() && self.fatal.is_none() {
            let scope = self.scopes[index];
            let names: Vec<String> = self.frames[scope.0].steps.keys().cloned().collect();
            for name in names {
                let _ = self.step(scope, &name);
            }
            index += 1;
        }
        // Nothing is under way at the top, so nothing is held any more.
        self.release_held();
        if let Some(error) = self.fatal.take() {
            return Err(error);
        }
        let outputs = self.outputs(root);
        if let Some(error) = self.fatal.take() {
            return Err(error);
        }
        self.finish_display_scopes();
        Ok(Expansion {
            scopes: self.display_scopes.into_values().collect(),
            instances: self.instances,
            pending: self.pending,
            problems: self.problems,
            outputs,
            workflows: self.workflows,
            types: self.types,
        })
    }

    /// The root workflow's frame: no prefix, no variables, phase 1, the root inputs. Records the
    /// root document for the plan digest.
    fn root_frame(&mut self) -> FrameId {
        let loaded = Rc::clone(self.env.workflow);
        let workflow = Rc::clone(&loaded.workflow);
        self.workflows
            .insert(loaded.source.clone(), loaded.document.clone());
        let root = self.new_frame(Frame {
            steps: Rc::clone(&workflow.steps),
            prefix: String::new(),
            decl_prefix: String::new(),
            variables: IndexMap::new(),
            parent: None,
            takes: Vec::new(),
            workflow,
            inputs: Rc::new(self.env.inputs.clone()),
            input_bindings: Rc::default(),
            display_scope: None,
            workflow_scope: None,
            variable_interfaces: Default::default(),
            variable_bindings: Default::default(),
            phase: 1,
            judging: None,
            budget: None,
            owner: None,
            maybe: false,
        });
        self.workflow_roots
            .insert(root, (loaded.source.clone(), None));
        root
    }

    /// Expands a declared step once per scope. A reference while it is
    /// still expanding is `{decl_prefix}{name} refers back to itself`.
    pub(crate) fn step(
        &mut self,
        frame: FrameId,
        name: &str,
    ) -> std::result::Result<ExpId, ExprError> {
        let scope = self.scope_of(frame);
        let key = (scope, name.to_string());
        if let Some(&found) = self.memo.get(&key) {
            if self.exps[found.0].kind == frame::ExpKind::Expanding {
                self.reached_under_way(found);
                return Err(self.refers_back(found));
            }
            return Ok(found);
        }
        let exp = self.new_exp(scope, name);
        self.memo.insert(key, exp);
        self.wiring.suspend();
        self.expand_step(scope, name, exp);
        if self.exps[exp.0].kind == frame::ExpKind::Expanding {
            self.exps[exp.0].kind = frame::ExpKind::Absent;
        }
        self.release_held();
        self.wiring.resume();
        Ok(exp)
    }

    /// A step's expansion under way: its position, then its repeat or its one position.
    /// Each step a step reads is expanded inside it, so the nesting is bounded before the stack
    /// is: past [`MAX_CHAIN`] the step stays absent and the plan is refused.
    #[inline(never)]
    fn expand_step(&mut self, scope: FrameId, name: &str, exp: ExpId) {
        let position = self.step_position(scope, name);
        let Some(at) = &position else {
            return;
        };
        self.active.push(exp);
        self.expand_position(at, exp);
        self.active.pop();
        // A `uses` that did not resolve is already reported at the step.
        self.escaped = None;
    }

    /// The position of a declared step in its scope; `None` when it is not declared, after a
    /// fatal error, or past [`MAX_CHAIN`] (reported).
    #[inline(never)]
    fn step_position(&mut self, scope: FrameId, name: &str) -> Option<Position> {
        let declared = self.frames[scope.0].steps.get(name).cloned()?;
        if self.fatal.is_some() {
            return None;
        }
        let where_ = format!("{}{name}", self.frames[scope.0].decl_prefix);
        if self.active.len() >= MAX_CHAIN {
            self.problem(
                &where_,
                format!("the chain of steps reading each other is longer than {MAX_CHAIN}"),
            );
            return None;
        }
        Some(Position {
            frame: scope,
            name: name.to_string(),
            where_,
            declared,
            key: None,
            variables: IndexMap::new(),
            suffix: String::new(),
            concurrency_group: None,
        })
    }

    /// A declared step's whole expansion: its repeat, or its one position. Kept out of
    /// [`Expander::step`] (and never inlined) so the frames of a chain of steps reading each
    /// other stay small.
    #[inline(never)]
    fn expand_position(&mut self, at: &Position, exp: ExpId) {
        if repeats(&at.declared) {
            self.repeat(at, exp);
        } else {
            self.single(at, exp);
        }
    }

    /// One position of a step: condition, assertions, then group,
    /// used workflow or node.
    ///
    /// Small on purpose, like every frame between a step and a step it reads: the work around
    /// the recursion is in functions of its own (`expr::evaluate`).
    pub(crate) fn single(&mut self, at: &Position, into: ExpId) {
        self.escaped = None;
        if self.fatal.is_some() {
            return;
        }
        let Some((context, path)) = self.position_context(at, into) else {
            return;
        };
        if at.declared.steps.is_some() {
            self.single_group(at, context, &path, into);
            return;
        }
        match self.resolve_uses(at) {
            None => {}
            Some(Resolved::Workflow(used)) => self.single_workflow(at, context, &path, used, into),
            Some(Resolved::Node(resolved)) => {
                self.instantiating.push(into);
                self.node(at, context, &path, resolved, into);
                self.instantiating.pop();
                // A repeat's items are no steps of their own: release what each one held.
                self.release_held();
            }
        }
    }

    /// A position's context frame (its variables, budget and `maybe`) and instance path, once
    /// its condition and assertions let it in; `None` (the step absent) when they do not.
    #[inline(never)]
    fn position_context(&mut self, at: &Position, into: ExpId) -> Option<(FrameId, String)> {
        let (mut context, path) = self.position_frame(at);
        if at.declared.if_.is_some() {
            match self.condition(context, &at.declared, &at.where_) {
                Condition::True => {}
                Condition::False => {
                    self.exps[into.0].kind = frame::ExpKind::Absent;
                    return None;
                }
                Condition::Pending(_) => context = self.maybe_frame(context),
            }
        }
        if !self.assertions(&at.declared.asserts, context, &at.where_, None) {
            self.exps[into.0].kind = frame::ExpKind::Absent;
            return None;
        }
        Some((context, path))
    }

    /// A position's frame (its variables and budget) and instance path.
    #[inline(never)]
    fn position_frame(&mut self, at: &Position) -> (FrameId, String) {
        let declared = Rc::clone(&at.declared);
        let (path, variables, budget) = {
            let frame = &self.frames[at.frame.0];
            let path = format!("{}{}{}", frame.prefix, at.name, at.suffix);
            // The inner name wins: a nested repeat's `as:` shadows the outer one.
            let mut variables = frame.variables.clone();
            for (name, value) in &at.variables {
                variables.insert(name.clone(), value.clone());
            }
            let budget = match &declared.budget {
                Some(budget) => Some((path.clone(), budget.max_usd)),
                None => frame.budget.clone(),
            };
            (path, variables, budget)
        };
        let context = self.derive(
            at.frame,
            frame::FrameChanges {
                variables: Some(variables),
                budget: Some(budget),
                ..Default::default()
            },
        );
        (context, path)
    }

    /// The same frame, `maybe`: only a run decides whether the position runs.
    #[inline(never)]
    fn maybe_frame(&mut self, context: FrameId) -> FrameId {
        self.derive(
            context,
            frame::FrameChanges {
                maybe: Some(true),
                ..Default::default()
            },
        )
    }

    /// A group position: its member scope.
    #[inline(never)]
    fn single_group(&mut self, at: &Position, context: FrameId, path: &str, into: ExpId) {
        let declared = Rc::clone(&at.declared);
        let Some(steps) = &declared.steps else {
            return;
        };
        let child = self.nest(
            context,
            frame::FrameChanges {
                steps: Some(Rc::clone(steps)),
                prefix: Some(format!("{path}.")),
                decl_prefix: Some(format!("{}.", at.where_)),
                parent: Some(Some(context)),
                ..Default::default()
            },
        );
        self.group(child, &declared, &at.where_, into);
    }

    /// The position's `uses` resolved; `None` (reported, and kept in `escaped`) when it is not.
    #[inline(never)]
    fn resolve_uses(&mut self, at: &Position) -> Option<Resolved> {
        let uses = at.declared.uses.clone().unwrap_or_default();
        let resolved = self.resolve_type(&uses, &at.where_);
        if resolved.is_none() && self.fatal.is_none() {
            self.escaped = self.problems.last().cloned();
        }
        resolved
    }

    /// A position whose `uses` is a workflow.
    #[inline(never)]
    fn single_workflow(
        &mut self,
        at: &Position,
        context: FrameId,
        path: &str,
        used: Rc<LoadedWorkflow>,
        into: ExpId,
    ) {
        if self.uses_itself(context, &used.source) {
            let uses = at.declared.uses.clone().unwrap_or_default();
            self.problem(&at.where_, format!("{uses} refers back to itself"));
            self.escaped = self.problems.last().cloned();
            return;
        }
        self.workflow_step(at, context, path, used, into);
    }

    /// Evaluates `if:` per position: a pending value makes the context `maybe`, a failed one is
    /// false, anything else is its truthiness.
    pub(crate) fn condition(&mut self, frame: FrameId, declared: &Step, where_: &str) -> Condition {
        let Some(condition) = &declared.if_ else {
            return Condition::True;
        };
        let value = self.evaluate(frame, condition, &format!("{where_}.if"));
        match value {
            Val::Pending(pending) => Condition::Pending(*pending),
            value if value.contains_pending() => {
                Condition::Pending(Pending::new(value.pending_refs(), &value.token_form()))
            }
            Val::Failed(_) => Condition::False,
            value if value.truthy() => Condition::True,
            _ => Condition::False,
        }
    }

    /// Checks assertions: a failing check that read no results is a problem at
    /// `<where>.assert[i]` and returns false; one that read results sets `owner`'s state (`failed`
    /// or `absent`) with the message as reason. `where_` is `workflow` or the step's path.
    pub(crate) fn assertions(
        &mut self,
        asserts: &[Assertion],
        frame: FrameId,
        where_: &str,
        owner: Option<&str>,
    ) -> bool {
        let mut keep = true;
        for (index, assertion) in asserts.iter().enumerate() {
            let label = format!("{where_}.assert[{index}]");
            let (value, reads) = self.evaluate_reading(frame, &assertion.check, &label);
            if value.contains_pending() {
                // Not decidable yet: the step it belongs to waits for what it reads, so it is
                // decided before the step is dispatched (gnode dispatched it at once).
                if let Some(instance) = owner.and_then(|id| self.instance_mut(id)) {
                    let own = instance.id.clone();
                    instance.reads.extend(
                        value
                            .pending_refs()
                            .into_iter()
                            .chain(reads)
                            .filter(|id| *id != own),
                    );
                }
                continue;
            }
            if value.truthy() {
                continue;
            }
            let message = self.evaluate(frame, &Value::String(assertion.message.clone()), &label);
            let text = message_text(&message);
            if reads.is_empty() {
                // Over values the plan knows: the plan is refused.
                self.problem(&label, text);
                continue;
            }
            if assertion.on_fail == OnFail::Skip {
                keep = false;
            }
            if let Some(instance) = owner.and_then(|id| self.instance_mut(id)) {
                instance.state = match assertion.on_fail {
                    OnFail::Fail => State::Failed,
                    OnFail::Skip => State::Absent,
                };
                instance.reason = Some(text);
            }
        }
        keep
    }

    /// Evaluates a document value in a frame's step scope and finishes it; an error becomes a
    /// problem at `where_` and the value missing (gnode `_evaluate`).
    pub(crate) fn evaluate(&mut self, frame: FrameId, value: &Value, where_: &str) -> Val {
        match self.resolve_finished(frame, value) {
            Ok(value) => value,
            Err(error) => {
                self.problem(where_, error.0);
                Val::Missing
            }
        }
    }

    /// A document value resolved and finished in a frame's step scope. Small on purpose: every
    /// step an expression reads expands inside it (`expr::evaluate`).
    #[inline(never)]
    fn resolve_finished(&mut self, frame: FrameId, value: &Value) -> Result<Val, ExprError> {
        let mut scope = StepScope { ex: self, frame };
        match expr::resolve(value, &mut scope) {
            Ok(value) => expr::finish(&mut scope, value),
            Err(error) => Err(error),
        }
    }

    /// [`Expander::evaluate`] under a fresh read set; returns the ids read, and merges them into
    /// any enclosing read set (gnode `_evaluate_reading`, its leak kept).
    pub(crate) fn evaluate_reading(
        &mut self,
        frame: FrameId,
        value: &Value,
        where_: &str,
    ) -> (Val, BTreeSet<String>) {
        let mut reads = BTreeSet::new();
        let value = self.evaluate_into(frame, value, where_, &mut reads);
        (value, reads)
    }

    /// [`Expander::evaluate_reading`] adding the ids read to `reads`. Small on purpose: every
    /// step a with-value reads expands inside it (`expr::evaluate`).
    pub(crate) fn evaluate_into(
        &mut self,
        frame: FrameId,
        value: &Value,
        where_: &str,
        reads: &mut BTreeSet<String>,
    ) -> Val {
        self.read_sets.push(BTreeSet::new());
        let result = self.resolve_finished(frame, value);
        self.pop_reads(reads);
        match result {
            Ok(value) => value,
            Err(error) => self.refused(where_, error),
        }
    }

    /// A refused evaluation: a problem at `where_`, and the value missing.
    #[inline(never)]
    fn refused(&mut self, where_: &str, error: ExprError) -> Val {
        self.problem(where_, error.0);
        Val::Missing
    }

    /// The innermost read set, taken off, merged into the one around it and added to `reads`.
    #[inline(never)]
    fn pop_reads(&mut self, reads: &mut BTreeSet<String>) {
        let read = self.read_sets.pop().unwrap_or_default();
        if let Some(outer) = self.read_sets.last_mut() {
            outer.extend(read.iter().cloned());
        }
        reads.extend(read);
    }

    /// An instance to change. During a trial (`node.rs`), one that existed before it is recorded
    /// first, so the trial can be undone.
    pub(crate) fn instance_mut(&mut self, id: &str) -> Option<&mut Instance> {
        let index = self.instances.get_index_of(id)?;
        if self
            .trials
            .last()
            .is_some_and(|trial| index < trial.instances)
        {
            self.undo.push((index, self.instances[index].clone()));
        }
        self.instances
            .get_index_mut(index)
            .map(|(_, instance)| instance)
    }

    /// Records a read of an existing result into the active read set.
    pub(crate) fn read(&mut self, id: &str) {
        if let Some(set) = self.read_sets.last_mut() {
            set.insert(id.to_string());
        }
    }

    /// `max(frame.phase, phase of each read non-at-plan instance + 1)`.
    pub(crate) fn phase_of(&self, refs: &BTreeSet<String>, frame: FrameId) -> u32 {
        let mut phase = self.frames[frame.0].phase;
        for id in refs {
            // A value read from an `at: plan` step is known while planning: no new phase. Ids
            // that are no instance (a shadow's `<item>`) count for nothing.
            if let Some(instance) = self.instances.get(id)
                && !instance.at_plan
            {
                phase = phase.max(instance.phase + 1);
            }
        }
        phase
    }

    /// Adds a problem.
    pub(crate) fn problem(&mut self, where_: &str, message: impl Into<String>) {
        self.problems.push(Problem::new(where_, message));
    }

    /// Evaluates the root workflow's outputs in the root frame, at `outputs.<name>`.
    pub(crate) fn outputs(&mut self, root: FrameId) -> IndexMap<String, Val> {
        let workflow = Rc::clone(&self.frames[root.0].workflow);
        let mut outputs = IndexMap::new();
        for (name, value) in &workflow.outputs {
            let value = self.evaluate(root, value, &format!("outputs.{name}"));
            outputs.insert(name.clone(), value);
        }
        outputs
    }

    /// Resolves `uses` through the registry. A registry problem is reported at `where_` and gives
    /// `None`; a fatal error is kept in `fatal`. Records the type in `types`, and reports a
    /// type's lock drift at `where_`.
    pub(crate) fn resolve_type(&mut self, uses: &str, where_: &str) -> Option<Resolved> {
        if self.fatal.is_some() {
            return None;
        }
        let env = &mut self.env;
        let resolved = env.registry.node_type(uses, &mut *env.host);
        match resolved {
            Ok(Resolved::Node(resolved)) => {
                if !self.types.contains_key(uses) {
                    self.types.insert(uses.to_string(), Rc::clone(&resolved));
                }
                if let Some(drift) = &resolved.drift {
                    self.problem(where_, drift.clone());
                }
                Some(Resolved::Node(resolved))
            }
            Ok(Resolved::Workflow(used)) => {
                if !self.workflows.contains_key(&used.source) {
                    self.workflows
                        .insert(used.source.clone(), used.document.clone());
                }
                Some(Resolved::Workflow(used))
            }
            Err(RegistryError::Problem(message)) => {
                self.problem(where_, message);
                None
            }
            Err(RegistryError::Fatal(error)) => {
                self.fatal.get_or_insert(error);
                None
            }
        }
    }

    /// `{decl_prefix}{name} refers back to itself` for a step expansion.
    pub(crate) fn refers_back(&self, exp: ExpId) -> ExprError {
        let exp = &self.exps[exp.0];
        ExprError::new(format!(
            "{}{} refers back to itself",
            self.frames[exp.frame.0].decl_prefix, exp.name
        ))
    }

    /// Whether a node step's instances are being created right now (and not all exist yet): a
    /// reference to it through `steps.…` then reads itself.
    pub(crate) fn is_instantiating(&self, exp: ExpId) -> bool {
        let expansion = &self.exps[exp.0];
        self.instantiating.contains(&exp)
            && expansion.instances.len() < expansion.takes.len().max(1)
    }

    /// Whether the workflow `source` is already being expanded around `frame`: the workflow of
    /// `frame`, the workflow whose step used that one, and so on up to the root.
    fn uses_itself(&self, frame: FrameId, source: &str) -> bool {
        let mut current = Some(frame);
        while let Some(frame) = current {
            let mut scope = self.scope_of(frame);
            while let Some(parent) = self.frames[scope.0].parent {
                scope = self.scope_of(parent);
            }
            match self.workflow_roots.get(&scope) {
                Some((used, caller)) => {
                    if used == source {
                        return true;
                    }
                    current = *caller;
                }
                None => return false,
            }
        }
        false
    }

    /// Records a used workflow's root frame (see `workflow_roots`).
    pub(crate) fn workflow_root(&mut self, root: FrameId, source: &str, caller: FrameId) {
        self.workflow_roots
            .insert(root, (source.to_string(), Some(caller)));
    }
}

/// Whether a declared step repeats (`for_each: null` does not).
fn repeats(declared: &Step) -> bool {
    declared.matrix.is_some() || declared.for_each.as_ref().is_some_and(|v| !v.is_null())
}

/// An assertion message as text: a string itself, a failed or pending value as its shown form,
/// anything else through `text` (missing is `""`).
fn message_text(message: &Val) -> String {
    match message {
        Val::Str(text) => text.clone(),
        Val::Failed(_) | Val::Pending(_) => crate::value::canon(&message.shown()),
        _ if message.contains_pending() => crate::value::canon(&message.shown()),
        _ => message
            .text()
            .unwrap_or_else(|_| crate::value::canon(&message.shown())),
    }
}

/// Fixtures for the expander's unit tests: steps and workflows built by hand, an expander over
/// a fake host and an empty registry.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::docs::lock::LockFile;
    use crate::docs::workflow::{Steps, ViewSetting, WorkflowDoc};
    use crate::host::FakeHost;
    use crate::registry::TypeOrigin;
    use crate::spec::{BodyKind, NodeSpec};
    use std::path::PathBuf;

    /// A node step with every other field at its default.
    pub(crate) fn node_step(uses: &str) -> Step {
        Step {
            uses: Some(uses.to_string()),
            steps: None,
            with: IndexMap::new(),
            if_: None,
            needs: Vec::new(),
            for_each: None,
            as_: "item".to_string(),
            key: None,
            max: None,
            matrix: None,
            judges: None,
            on_reject: OnReject::Fail,
            regenerate: None,
            takes: None,
            pick: None,
            asserts: Vec::new(),
            at_plan: false,
            budget: None,
            concurrency: None,
            route: None,
            requires: Vec::new(),
            independent_of: Vec::new(),
            view: ViewSetting::Flag(false),
            timeout: None,
            title: None,
            description: None,
        }
    }

    /// A plain group.
    pub(crate) fn group_step(steps: Vec<(&str, Step)>) -> Step {
        Step {
            uses: None,
            steps: Some(steps_of(steps)),
            ..node_step("")
        }
    }

    pub(crate) fn steps_of(steps: Vec<(&str, Step)>) -> Steps {
        Rc::new(
            steps
                .into_iter()
                .map(|(name, step)| (name.to_string(), Rc::new(step)))
                .collect(),
        )
    }

    pub(crate) fn workflow_doc(steps: Vec<(&str, Step)>) -> WorkflowDoc {
        WorkflowDoc {
            id: "case".to_string(),
            title: "Case".to_string(),
            description: None,
            inputs: IndexMap::new(),
            tables: IndexMap::new(),
            let_: IndexMap::new(),
            budget: None,
            asserts: Vec::new(),
            steps: steps_of(steps),
            outputs: IndexMap::new(),
            view: None,
        }
    }

    /// Everything an expansion reads, owned.
    pub(crate) struct Fixture {
        pub workflow: Rc<LoadedWorkflow>,
        pub inputs: IndexMap<String, Val>,
        pub registry: Registry,
        pub host: FakeHost,
        pub routes: RouteTable,
        pub takes: Takes,
        pub results: IndexMap<String, NodeResult>,
    }

    impl Fixture {
        pub(crate) fn new(workflow: WorkflowDoc) -> Fixture {
            Fixture {
                workflow: Rc::new(LoadedWorkflow {
                    document: Value::Null,
                    workflow: Rc::new(workflow),
                    source: "workflows/case.yaml".to_string(),
                    path: PathBuf::from("/nowhere/workflows/case.yaml"),
                }),
                inputs: IndexMap::new(),
                registry: Registry::new(
                    PathBuf::from("/nowhere"),
                    Vec::new(),
                    LockFile::default(),
                    IndexMap::new(),
                ),
                host: FakeHost::new(),
                routes: RouteTable::new(),
                takes: Takes::new(),
                results: IndexMap::new(),
            }
        }

        pub(crate) fn env(&mut self) -> ExpandEnv<'_> {
            ExpandEnv {
                workflow: &self.workflow,
                inputs: &self.inputs,
                registry: &mut self.registry,
                host: &mut self.host,
                routes: &self.routes,
                takes: &self.takes,
                results: &self.results,
            }
        }

        /// An expander with its root frame made.
        pub(crate) fn expander(&mut self) -> (Expander<'_>, FrameId) {
            let mut expander = Expander::new(self.env());
            let root = expander.root_frame();
            (expander, root)
        }
    }

    /// A planned instance of a free type, with only what a test needs.
    pub(crate) fn instance(id: &str, phase: u32, at_plan: bool) -> Instance {
        let spec = NodeSpec {
            name: "shout".to_string(),
            description: None,
            inputs: IndexMap::new(),
            params: IndexMap::new(),
            outputs: IndexMap::new(),
            judge: false,
            capability: None,
            calls: IndexMap::new(),
            resources: Vec::new(),
            tools: Vec::new(),
            view: None,
            version: None,
            retry: Default::default(),
        };
        let path = id.split('#').next().unwrap_or(id).to_string();
        Instance {
            id: id.to_string(),
            step: path.clone(),
            path,
            takes: vec![1],
            uses: "./nodes/cases.py#shout".to_string(),
            ty: Rc::new(ResolvedType {
                uses: "./nodes/cases.py#shout".to_string(),
                identity: "nodes/cases.py#shout@source:0".to_string(),
                spec: Rc::new(spec),
                origin: TypeOrigin::Project {
                    path: "nodes/cases.py".to_string(),
                    attribute: "shout".to_string(),
                },
                body: BodyKind::Project,
                source: None,
                drift: None,
            }),
            with: IndexMap::new(),
            needs: Vec::new(),
            state: State::Planned,
            identity: None,
            routes: IndexMap::new(),
            prices: Vec::new(),
            phase,
            key: None,
            judges: None,
            judge_policy: None,
            judged_by: Vec::new(),
            view: Value::Bool(false),
            at_plan,
            budget: None,
            concurrency_group: None,
            concurrency: None,
            timeout_s: None,
            reason: None,
            reads: BTreeSet::new(),
            bindings: Vec::new(),
            interface_bindings: Vec::new(),
            display_scope: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::frame::{ExpKind, FrameChanges};
    use super::testing::*;
    use super::*;

    fn wheres(problems: &[Problem]) -> Vec<&str> {
        problems.iter().map(|p| p.where_.as_str()).collect()
    }

    #[test]
    fn ids_and_quoted_keys() {
        assert_eq!(instance_id("draw", &[1]), "draw#1");
        assert_eq!(
            instance_id("build['k'].mesh", &[2, 1]),
            "build['k'].mesh#2.1"
        );
        assert_eq!(quote_key("it's"), r"'it\'s'");
        assert_eq!(quote_key(r"a\b"), r"'a\\b'");
        assert_eq!(quote_key("x]y"), "'x]y'");
    }

    #[test]
    fn derive_keeps_the_scope_and_nest_makes_one() {
        let mut fixture = Fixture::new(workflow_doc(vec![("a", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let mut variables = IndexMap::new();
        variables.insert("item".to_string(), Val::Str("x".into()));
        let derived = ex.derive(
            root,
            FrameChanges {
                variables: Some(variables),
                phase: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(ex.scope_of(derived), root);
        assert_eq!(ex.frame(derived).phase, 2);
        assert_eq!(ex.frame(root).phase, 1);
        let nested = ex.nest(
            derived,
            FrameChanges {
                prefix: Some("g.".into()),
                parent: Some(Some(derived)),
                ..Default::default()
            },
        );
        assert_eq!(ex.scope_of(nested), nested);
        assert_eq!(ex.frame(nested).owner, None);
        // A nested scope inherits variables and phase.
        assert!(ex.frame(nested).variables.contains_key("item"));
        assert_eq!(ex.frame(nested).phase, 2);
        let again = ex.derive(nested, FrameChanges::default());
        assert_eq!(ex.scope_of(again), nested);
    }

    #[test]
    fn find_walks_the_scopes_outwards_to_the_workflow_root() {
        let inner = steps_of(vec![("m", node_step("nope")), ("a", node_step("nope"))]);
        let mut fixture = Fixture::new(workflow_doc(vec![
            ("a", node_step("nope")),
            ("b", node_step("nope")),
        ]));
        let (mut ex, root) = fixture.expander();
        let context = ex.derive(root, FrameChanges::default());
        let child = ex.nest(
            context,
            FrameChanges {
                steps: Some(Rc::clone(&inner)),
                parent: Some(Some(context)),
                ..Default::default()
            },
        );
        let in_child = ex.derive(child, FrameChanges::default());
        assert_eq!(ex.find(in_child, "m"), Some(child));
        // An inner name shadows an outer one.
        assert_eq!(ex.find(in_child, "a"), Some(child));
        assert_eq!(ex.find(in_child, "b"), Some(root));
        assert_eq!(ex.find(in_child, "zz"), None);
        // A used workflow's root has no parent: it never sees its caller's steps.
        let used = ex.nest(
            root,
            FrameChanges {
                steps: Some(inner),
                parent: Some(None),
                ..Default::default()
            },
        );
        assert_eq!(ex.find(used, "b"), None);
    }

    #[test]
    fn a_step_still_expanding_refers_back_to_itself() {
        let mut fixture = Fixture::new(workflow_doc(vec![("a", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let group = ex.nest(
            root,
            FrameChanges {
                steps: Some(steps_of(vec![("draw", node_step("nope"))])),
                decl_prefix: Some("entity.".into()),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        let exp = ex.new_exp(group, "draw");
        ex.memo.insert((group, "draw".to_string()), exp);
        let error = ex.step(group, "draw").unwrap_err();
        assert_eq!(error.0, "entity.draw refers back to itself");
        // Once it became something, the memo answers.
        ex.exp_mut(exp).kind = ExpKind::Absent;
        assert_eq!(ex.step(group, "draw"), Ok(exp));
    }

    #[test]
    fn root_steps_then_scopes_breadth_first() {
        // Every `uses` here is refused, so each step leaves one problem where it expanded: the
        // problems show the expansion order.
        let mut fixture = Fixture::new(workflow_doc(vec![
            ("a", node_step("nope")),
            (
                "g",
                group_step(vec![
                    ("m1", node_step("nope")),
                    ("h", group_step(vec![("n1", node_step("nope"))])),
                    ("m2", node_step("nope")),
                ]),
            ),
            ("b", node_step("nope")),
        ]));
        let expansion = expand(fixture.env()).unwrap();
        assert_eq!(
            wheres(&expansion.problems),
            ["a", "b", "g.m1", "g.m2", "g.h.n1"]
        );
        assert!(expansion.instances.is_empty());
        assert_eq!(
            expansion.workflows.keys().collect::<Vec<_>>(),
            ["workflows/case.yaml"]
        );
    }

    #[test]
    fn instance_ids_are_own_then_children_then_members() {
        let members = vec![("m1", node_step("nope")), ("m2", node_step("nope"))];
        let mut fixture = Fixture::new(workflow_doc(vec![("a", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let scope = ex.nest(
            root,
            FrameChanges {
                steps: Some(steps_of(members)),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        // Members already expanded (the memo answers them).
        for (name, id) in [("m1", "g.m1#1"), ("m2", "g.m2#1")] {
            let member = ex.new_exp(scope, name);
            ex.exp_mut(member).kind = ExpKind::Node;
            ex.exp_mut(member).instances = vec![id.to_string()];
            ex.memo.insert((scope, name.to_string()), member);
        }
        let group = ex.new_exp(root, "g");
        ex.exp_mut(group).kind = ExpKind::Group;
        ex.exp_mut(group).scope = Some(scope);
        let repeat = ex.new_exp(root, "rep");
        ex.exp_mut(repeat).kind = ExpKind::Repeat;
        for (key, id) in [("a", "rep['a']#1"), ("b", "rep['b']#1")] {
            let child = ex.new_exp(root, "rep");
            ex.exp_mut(child).kind = ExpKind::Node;
            ex.exp_mut(child).instances = vec![id.to_string()];
            ex.exp_mut(repeat).children.push((key.to_string(), child));
        }
        let node = ex.new_exp(root, "x");
        ex.exp_mut(node).kind = ExpKind::Node;
        ex.exp_mut(node).instances = vec!["x#1".into(), "x#2".into()];
        ex.exp_mut(node).children.push(("c".into(), repeat));
        ex.exp_mut(node).scope = Some(scope);
        assert_eq!(
            ex.instance_ids(node),
            ["x#1", "x#2", "rep['a']#1", "rep['b']#1", "g.m1#1", "g.m2#1"]
        );
        assert_eq!(ex.instance_ids(group), ["g.m1#1", "g.m2#1"]);
        // A pending or absent step names nothing.
        let pending = ex.new_exp(root, "p");
        ex.exp_mut(pending).kind = ExpKind::Pending;
        assert!(ex.instance_ids(pending).is_empty());
    }

    #[test]
    fn members_expand_and_skip_a_member_still_expanding() {
        let mut fixture = Fixture::new(workflow_doc(vec![("a", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let scope = ex.nest(
            root,
            FrameChanges {
                steps: Some(steps_of(vec![
                    ("m1", node_step("nope")),
                    ("m2", node_step("nope")),
                ])),
                decl_prefix: Some("g.".into()),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        let busy = ex.new_exp(scope, "m1");
        ex.memo.insert((scope, "m1".to_string()), busy);
        let group = ex.new_exp(root, "g");
        ex.exp_mut(group).kind = ExpKind::Group;
        ex.exp_mut(group).scope = Some(scope);
        let members = ex.members(group);
        assert_eq!(members.len(), 1);
        assert_eq!(ex.exp(members[0]).name, "m2");
        // m2 was expanded now: its `uses` is refused.
        assert_eq!(ex.exp(members[0]).kind, ExpKind::Absent);
        assert_eq!(wheres(&ex.problems), ["g.m2"]);
    }

    #[test]
    fn phases_count_reads_of_steps_that_run() {
        let mut fixture = Fixture::new(workflow_doc(vec![("a", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        ex.instances.insert("a#1".into(), instance("a#1", 1, false));
        ex.instances.insert("b#1".into(), instance("b#1", 2, false));
        ex.instances
            .insert("src#1".into(), instance("src#1", 4, true));
        let refs = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(ex.phase_of(&refs(&[]), root), 1);
        assert_eq!(ex.phase_of(&refs(&["a#1"]), root), 2);
        assert_eq!(ex.phase_of(&refs(&["a#1", "b#1"]), root), 3);
        // An at: plan step is known while planning, and a shadow's `<item>` is no instance.
        assert_eq!(ex.phase_of(&refs(&["src#1", "<item>"]), root), 1);
        let late = ex.derive(
            root,
            FrameChanges {
                phase: Some(5),
                ..Default::default()
            },
        );
        assert_eq!(ex.phase_of(&refs(&["b#1"]), late), 5);
    }

    #[test]
    fn a_workflow_that_uses_itself_is_found() {
        let mut fixture = Fixture::new(workflow_doc(vec![("a", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let context = ex.derive(root, FrameChanges::default());
        let inner = ex.nest(
            root,
            FrameChanges {
                prefix: Some("w.".into()),
                parent: Some(None),
                ..Default::default()
            },
        );
        ex.workflow_root(inner, "workflows/inner.yaml", context);
        let group = ex.nest(
            inner,
            FrameChanges {
                parent: Some(Some(inner)),
                ..Default::default()
            },
        );
        let in_group = ex.derive(group, FrameChanges::default());
        assert!(ex.uses_itself(in_group, "workflows/inner.yaml"));
        assert!(ex.uses_itself(in_group, "workflows/case.yaml"));
        assert!(!ex.uses_itself(in_group, "workflows/other.yaml"));
        assert!(!ex.uses_itself(context, "workflows/inner.yaml"));
    }

    #[test]
    fn an_assertion_message_reads_as_text() {
        assert_eq!(message_text(&Val::Str("at most one".into())), "at most one");
    }

    #[test]
    fn an_assertion_message_of_runtime_values() {
        assert_eq!(message_text(&Val::Missing), "");
        assert_eq!(message_text(&Val::Number(3.0)), "3");
        assert_eq!(
            message_text(&Val::Failed("a#1".into())),
            r#"{"failed":"a#1"}"#
        );
    }
}
