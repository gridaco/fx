//! Node steps and their instances.
//!
//! `node`: the step's expansion is a node at once; a judge expands the step it judges first; take
//! numbers (`judge.rs`); one instance per take, in a frame with the take and, for a judge, the
//! judged take it reads, plus `feedback` when a judge regenerates the step with feedback; a
//! `maybe` context makes planned instances maybe; judge linking; this step's judges expanded
//! right after its instances, so they are listed after it; the takes sequenced; then
//! `independent_of`.
//!
//! `instance`: an id already made is returned as is; with-values; routes and prices
//! (`bind.rs`); `needs:`; the state (a result first: `failed` when it failed, else `done`; then
//! `blocked` when a value it reads failed, `absent` when an input it needs does not exist, then
//! `maybe` or `planned`); `an at: plan step makes no paid call` at `<w>.at`; the step identity
//! (spec/identity.md §8: `{"kind": "fx-step-v1", "type", "with": plain, "routes": {capability:
//! fingerprint}, "take"}`, `None` while a value is pending); insertion, which is the listing
//! position; then the step's assertions when it is planned or done.
//!
//! `with_values`: entries in authored order, each at `<w>.with.<name>`; a name the type does not
//! declare is `{type} has no input or setting {name}` and left out of the identity; the
//! `first_of` of the built-in select is evaluated in selecting mode and its top-level failed
//! items become missing; `./` and `../` paths given to input ports are read as project files
//! (`cannot read {value}: {reason}`), and so are absolute paths, which the registry refuses
//! (spec/identity.md §8); a template param given a project path is read, then rendered after the
//! defaults are filled (`{file}: {error}`), in declaration order; defaults are filled; `{type}
//! needs input {n}` and `{type} needs setting {n}` at `<w>.with`.
//!
//! FX refuses a node step read while its instances are being made (its own `with:`, or a `with:`
//! cycle through other nodes) as referring back to itself, where gnode read missing. The driver
//! tracks that window around `node`: it lasts while the expansion holds fewer instances than its
//! take numbers. So `node` sets the take numbers before it makes any instance and adds each
//! instance as soon as it exists; the step's judges, expanded after the last one, read it.
//! `needs:` still reads such a step silently (the instances made so far), as in gnode.
//!
//! The judges and `independent_of` targets a node step expands once its instances exist are not
//! what anything around it is waiting for. While a step under way is unfinished (still
//! `Expanding`, or a node whose instances are not all made: `unfinished`), that work is held
//! ([`Held`]) and released, oldest first, once none is (`release_held`, after each step); so a
//! judge or target that reads the unfinished step later reads it as pending instead of being
//! refused. gnode expanded them at once and read missing. A read of the step's result does wait
//! for its judges: it expands held judges first (`settle_judges`, from `chosen`), and a judge that
//! reads that reader back is then a real cycle and refused. A held step lists its judges and
//! targets later than gnode did. Inside a shadow (pricing a pending repeat) nothing is held.
//!
//! Holding must not turn a judge's own evidence into a cycle. In gnode's order a step's judges
//! expand right after its instances, so a step a judge reads (`review` reading `renders`, which
//! reads `measure`, which reads the judged `mesh`) is first reached inside the judge, while the
//! judge is not linked yet, and reads the judged take as it is. A reader that began after the
//! judges were held reaches them the other way round: it reads the step, the read settles the
//! judges inside it, and a judge that reads that reader finds it under way. Such a settlement is
//! a trial ([`Trial`]): when a judge reaches a step under way that began after the judges were
//! held, everything the trial expanded is undone (`end_trial`), the judges stay held, and the
//! reader reads the take unjudged, as it would inside the judge; the judges expand later and read
//! it made. Only a reader already under way when the judges were held (it reached the judged step
//! itself) waits for them while they read it: a real cycle, refused.

use super::frame::{ExpId, ExpKind, FrameChanges, FrameId};
use super::scope::TemplateScope;
use super::{Expander, Instance, Position, ResultStatus, State, instance_id};
use crate::docs::workflow::{OnReject, Step};
use crate::expr;
use crate::registry::ResolvedType;
use crate::routes::Route;
use crate::spec::{NodeSpec, Port, Shape};
use crate::text;
use crate::val::{FileValue, Val};
use crate::value::digest;
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::rc::Rc;

/// What `instance` gathers before the instance is made: with-values, the results they read,
/// bound routes with prices, and `needs:`.
#[derive(Default)]
struct Gathered {
    with: WithValues,
    bound: (IndexMap<String, Route>, Vec<super::CallPrice>),
    needs: Vec<String>,
}

/// `with:` as `with_values` gathers it.
#[derive(Default)]
struct WithValues {
    values: IndexMap<String, Val>,
    reads: BTreeSet<String>,
    bindings: BTreeSet<super::wiring::Binding>,
    interfaces: BTreeSet<super::wiring::Binding>,
    /// Template params given a project file, rendered once the defaults are in.
    templates: BTreeSet<String>,
}

/// What a node step's takes need (`node_plan`).
pub(crate) struct NodePlan {
    /// The position's frame.
    context: FrameId,
    /// The step a judge judges, by name, and its expansion when it is a node step.
    judged_name: Option<String>,
    judged: Option<ExpId>,
    numbers: Vec<u32>,
    /// The judges that hand each next take what they said (`feedback: true`).
    feedback: Vec<String>,
    first: u32,
}

/// What is left of a node step once its instances exist: expanding its judges (and sequencing
/// its takes) and checking `independent_of`. Held while a step under way is unfinished.
#[derive(Debug, Clone)]
pub(crate) struct Held {
    pub exp: ExpId,
    /// The position's frame.
    pub context: FrameId,
    pub declared: Rc<Step>,
    pub name: String,
    pub where_: String,
    /// The judges are still to expand.
    pub judges: bool,
    /// `independent_of` is still to check.
    pub independence: bool,
    /// The number of step expansions when the work was held: one numbered from here on began
    /// after it.
    pub since: usize,
}

/// Held judges settled for a reader while steps that began after they were held are under way
/// (`settle_judges`): the steps numbered `since..started`. A judge that reaches one of them
/// (`reached`) reaches its own evidence, which only the order of expansion put first; the trial
/// is then undone with what it kept. Step expansions, frames and views it made stay in their
/// arenas, unreachable.
#[derive(Debug)]
pub(crate) struct Trial {
    since: usize,
    started: usize,
    reached: bool,
    /// How many instances existed when it began: one of those it changes is recorded first, in
    /// `Expander::undo` from index `undo` on (`Expander::instance_mut`).
    pub(crate) instances: usize,
    undo: usize,
    /// Held work, and the innermost read set (the only one that gains reads meanwhile).
    held: Vec<Held>,
    reads: Option<BTreeSet<String>>,
    wiring: super::wiring::Trace,
    display_scopes: IndexMap<String, super::display::Scope>,
    /// How long the lists it only appends to were.
    pending: usize,
    problems: usize,
    scopes: usize,
    types: usize,
    workflows: usize,
}

impl Expander<'_> {
    /// A node step at one position. `context` is the position's frame (variables, budget, maybe).
    ///
    /// Small on purpose (see `Expander::single`): the take numbers, each take and what follows
    /// the last one are functions of their own.
    pub(crate) fn node(
        &mut self,
        at: &Position,
        context: FrameId,
        path: &str,
        resolved: Rc<ResolvedType>,
        into: ExpId,
    ) {
        self.exp_mut(into).kind = ExpKind::Node;
        let Some(plan) = self.node_plan(at, context, path, into) else {
            return;
        };
        for &take in &plan.numbers {
            self.make_take(at, &plan, path, &resolved, into, take);
        }
        self.node_end(at, &plan, into);
    }

    /// The take numbers of a node step, and what each take needs. A judge expands the step it
    /// judges first; `None` when that step is still being expanded (the judge is left out).
    #[inline(never)]
    fn node_plan(
        &mut self,
        at: &Position,
        context: FrameId,
        path: &str,
        into: ExpId,
    ) -> Option<NodePlan> {
        let declared = Rc::clone(&at.declared);
        let where_ = at.where_.as_str();
        let name = at.name.as_str();
        let judged_name = declared.judges.clone();
        let judged = match &judged_name {
            Some(subject) => {
                // The subject is still being expanded (its `if:`, assertions or repeat list read
                // this judge): the error leaves the judge out, as any error escaping a step does.
                let scope = self.scope_of(context);
                let owner = self.find(context, subject).map(|o| self.scope_of(o));
                if owner == Some(scope)
                    && let Err(error) = self.step(scope, subject)
                {
                    self.problem(where_, error.0);
                    self.exp_mut(into).kind = ExpKind::Absent;
                    return None;
                }
                self.sibling(context, subject, where_)
            }
            None => None,
        };
        let numbers = match (&judged_name, judged) {
            (Some(_), Some(subject)) => {
                let numbers: Vec<u32> = self
                    .exp(subject)
                    .instances
                    .iter()
                    .filter_map(|id| self.instances.get(id).map(Instance::take))
                    .collect();
                if numbers.is_empty() { vec![1] } else { numbers }
            }
            (Some(_), None) => vec![1],
            (None, _) => self.take_numbers(context, name, &declared, path),
        };
        // Set before any instance exists: until each take has one, a reference to this step
        // refers back to itself (module doc).
        self.exp_mut(into).takes = numbers.clone();
        // Judges that regenerate this step with `feedback: true` hand each next take what they
        // said of the take before.
        let feedback: Vec<String> = self
            .frame(context)
            .steps
            .iter()
            .filter(|(_, other)| {
                other.judges.as_deref() == Some(name)
                    && matches!(&other.on_reject, OnReject::Regenerate(r) if r.feedback)
            })
            .map(|(judge, _)| judge.clone())
            .collect();
        let first = numbers.first().copied().unwrap_or(1);
        Some(NodePlan {
            context,
            judged_name,
            judged,
            numbers,
            feedback,
            first,
        })
    }

    /// One take of a node step: its frame, its instance, then the instance's links.
    #[inline(never)]
    fn make_take(
        &mut self,
        at: &Position,
        plan: &NodePlan,
        path: &str,
        resolved: &Rc<ResolvedType>,
        into: ExpId,
        take: u32,
    ) {
        let take_frame = self.take_frame(at, plan, take);
        let id = self.instance(at, take_frame, path, resolved);
        self.take_made(at, plan, into, take, id);
    }

    /// The frame of one take: its takes, the judged take it reads, and `feedback`.
    #[inline(never)]
    fn take_frame(&mut self, at: &Position, plan: &NodePlan, take: u32) -> FrameId {
        let mut takes = self.frame(plan.context).takes.clone();
        takes.push(take);
        let take_frame = self.derive(
            plan.context,
            FrameChanges {
                takes: Some(takes),
                judging: Some(plan.judged_name.clone().map(|subject| (subject, take))),
                ..FrameChanges::default()
            },
        );
        if plan.feedback.is_empty() {
            return take_frame;
        }
        let said = self.feedback(plan.context, &plan.feedback, &at.suffix, take, plan.first);
        let mut variables = self.frame(take_frame).variables.clone();
        variables.insert("feedback".into(), said);
        self.derive(
            take_frame,
            FrameChanges {
                variables: Some(variables),
                ..FrameChanges::default()
            },
        )
    }

    /// A take's instance made: `maybe` in a maybe context, linked to the take it judges, and
    /// added to the expansion.
    #[inline(never)]
    fn take_made(&mut self, at: &Position, plan: &NodePlan, into: ExpId, take: u32, id: String) {
        let where_ = at.where_.as_str();
        if self.frame(plan.context).maybe
            && let Some(instance) = self.instance_mut(&id)
            && instance.state == State::Planned
        {
            instance.state = State::Maybe;
        }
        if let Some(subject) = plan.judged {
            let target = self
                .exp(subject)
                .instances
                .iter()
                .find(|t| self.instances.get(*t).is_some_and(|i| i.take() == take))
                .cloned();
            match target {
                Some(target) => {
                    if let Some(instance) = self.instance_mut(&id) {
                        instance.judge_policy = Some(at.declared.on_reject.clone());
                    }
                    self.link_judge(&id, &target, where_);
                }
                None => self.not_a_judge(&id, where_),
            }
        }
        self.exp_mut(into).instances.push(id);
    }

    /// After the last take: a judge settles its subject's takes; then the step's own judges and
    /// `independent_of`, held while a step under way is unfinished.
    #[inline(never)]
    fn node_end(&mut self, at: &Position, plan: &NodePlan, into: ExpId) {
        let context = plan.context;
        let declared = Rc::clone(&at.declared);
        let where_ = at.where_.as_str();
        let name = at.name.as_str();
        // A later step's reference reached this judge first: the subject left its takes
        // unsettled for it; settle them now that every take is linked.
        if let Some(subject) = plan.judged {
            let ids = self.exp(subject).instances.clone();
            let plain_sequence = self
                .declared_of(subject)
                .is_some_and(|step| step.takes.is_none());
            if plain_sequence && ids.len() > 1 {
                self.sequence(&ids);
            }
        }
        let steps = Rc::clone(&self.frame(context).steps);
        // What is left expands other steps for this one's sake, not for its value: its judges and
        // its `independent_of` targets. While a step around it is unfinished, that work is held
        // (module doc).
        let tail = Held {
            exp: into,
            context,
            declared: Rc::clone(&declared),
            name: name.to_string(),
            where_: where_.to_string(),
            judges: true,
            independence: !declared.independent_of.is_empty(),
            since: self.exps.len(),
        };
        let judged_here = steps
            .iter()
            .any(|(judge, other)| judge != name && other.judges.as_deref() == Some(name));
        let hold = (judged_here || tail.independence) && self.shadowing == 0 && self.unfinished();
        let hold_judges = hold && judged_here;
        let hold_independence = hold && tail.independence;
        if !hold_judges {
            self.node_judges(&tail);
        }
        if !hold_independence {
            self.node_independence(&tail);
        }
        if hold_judges || hold_independence {
            self.held.push(Held {
                judges: hold_judges,
                independence: hold_independence,
                ..tail
            });
        }
    }

    /// This step's judges belong to its result: expands them, so whatever reads the step waits
    /// for them, then sequences its takes. A judge already under way (it reached this step
    /// first) links itself when it returns.
    pub(crate) fn node_judges(&mut self, tail: &Held) {
        let steps = Rc::clone(&self.frame(tail.context).steps);
        let mut present = false;
        for (judge, other) in steps.iter() {
            if *judge == tail.name || other.judges.as_deref() != Some(tail.name.as_str()) {
                continue;
            }
            match self.step(tail.context, judge) {
                // A judge left out by its own `if:` judges nothing.
                Ok(expansion) => present |= self.exp(expansion).kind != ExpKind::Absent,
                Err(error) => self.problem(&tail.where_, error.0),
            }
        }
        let ids = self.exp(tail.exp).instances.clone();
        let linked = !present
            || ids.iter().all(|id| {
                self.instances
                    .get(id)
                    .is_some_and(|i| !i.judged_by.is_empty())
            });
        let declared = &tail.declared;
        if linked && declared.judges.is_none() && declared.takes.is_none() && ids.len() > 1 {
            self.sequence(&ids);
        }
    }

    /// This step's `independent_of` check.
    fn node_independence(&mut self, tail: &Held) {
        let ids = self.exp(tail.exp).instances.clone();
        self.independence(tail.context, &tail.declared, &tail.where_, &ids);
    }

    /// Whether a step under way is unfinished: still `Expanding` (its `if:`, assertions, repeat
    /// list, …), or a node step whose instances are not all made. Expanding another step for a
    /// third one's sake then could read it before it can be read.
    pub(crate) fn unfinished(&self) -> bool {
        self.active
            .iter()
            .any(|&exp| self.exp(exp).kind == ExpKind::Expanding)
            || self
                .instantiating
                .iter()
                .any(|&exp| self.is_instantiating(exp))
    }

    /// Runs the held work, oldest first, while no step under way is unfinished.
    pub(crate) fn release_held(&mut self) {
        while !self.held.is_empty()
            && self.fatal.is_none()
            && self.shadowing == 0
            && !self.unfinished()
        {
            let tail = self.held.remove(0);
            // Held work runs between other steps' work: it must not leave a mark meant for them.
            let escaped = self.escaped.take();
            if tail.judges {
                self.node_judges(&tail);
            }
            if tail.independence {
                self.node_independence(&tail);
            }
            self.escaped = escaped;
        }
    }

    /// Expands the held judges of a node step now: something reads its result, which waits for
    /// them. Reading it this way is a data read, so a judge that reads back a reader that was
    /// already under way when the judges were held is a real cycle and is refused. While a step
    /// that began after that is under way, the judges are expanded on trial: if one reaches such
    /// a step, the trial is undone and the judges stay held (module doc).
    pub(crate) fn settle_judges(&mut self, exp: ExpId) {
        let Some(index) = self.held.iter().position(|h| h.exp == exp && h.judges) else {
            return;
        };
        let since = self.held[index].since;
        let trial = self
            .active
            .iter()
            .chain(&self.instantiating)
            .any(|under_way| under_way.0 >= since);
        if trial {
            self.begin_trial(since);
        }
        let tail = if self.held[index].independence {
            self.held[index].judges = false;
            self.held[index].clone()
        } else {
            self.held.remove(index)
        };
        let escaped = self.escaped.take();
        self.node_judges(&tail);
        if trial && let Some(trial) = self.trials.pop() {
            self.end_trial(trial);
        }
        self.escaped = escaped;
    }

    /// A step under way is read before it can be (a refusal, or a `needs:` that sees only the
    /// instances made so far): every trial it began within has reached its own evidence.
    pub(crate) fn reached_under_way(&mut self, exp: ExpId) {
        for trial in &mut self.trials {
            if (trial.since..trial.started).contains(&exp.0) {
                trial.reached = true;
            }
        }
    }

    /// Starts a trial for judges held when `since` step expansions existed.
    fn begin_trial(&mut self, since: usize) {
        let trial = Trial {
            since,
            started: self.exps.len(),
            reached: false,
            instances: self.instances.len(),
            undo: self.undo.len(),
            held: self.held.clone(),
            reads: self.read_sets.last().cloned(),
            wiring: self.wiring.clone(),
            display_scopes: self.display_scopes.clone(),
            pending: self.pending.len(),
            problems: self.problems.len(),
            scopes: self.scopes.len(),
            types: self.types.len(),
            workflows: self.workflows.len(),
        };
        self.trials.push(trial);
    }

    /// Ends a trial: kept when no judge reached a step that began after the judges were held,
    /// else undone, so everything it expanded expands again, for real, later.
    fn end_trial(&mut self, trial: Trial) {
        if !trial.reached {
            // An enclosing trial may still need the records.
            if self.trials.is_empty() {
                self.undo.truncate(trial.undo);
            }
            return;
        }
        let records: Vec<(usize, Instance)> = self.undo.drain(trial.undo..).collect();
        for (index, before) in records.into_iter().rev() {
            if let Some((_, instance)) = self.instances.get_index_mut(index) {
                *instance = before;
            }
        }
        self.instances.truncate(trial.instances);
        self.memo.retain(|_, expansion| expansion.0 < trial.started);
        self.held = trial.held;
        self.wiring = trial.wiring;
        self.display_scopes = trial.display_scopes;
        if let (Some(reads), Some(innermost)) = (trial.reads, self.read_sets.last_mut()) {
            *innermost = reads;
        }
        self.pending.truncate(trial.pending);
        self.problems.truncate(trial.problems);
        self.scopes.truncate(trial.scopes);
        self.types.truncate(trial.types);
        self.workflows.truncate(trial.workflows);
    }

    /// One instance; returns its id.
    pub(crate) fn instance(
        &mut self,
        at: &Position,
        frame: FrameId,
        path: &str,
        resolved: &Rc<ResolvedType>,
    ) -> String {
        // Small on purpose (see `Expander::single`): `with:` and `needs:` expand the steps they
        // read; the instance is made in `make_instance`.
        let id = instance_id(path, &self.frame(frame).takes);
        if self.instances.contains_key(&id) {
            return id;
        }
        let gathered = self.gather(at, frame, resolved);
        self.make_instance(at, frame, path, resolved, id, gathered)
    }

    /// What an instance is made from: `with:` evaluated, routes bound, `needs:` resolved.
    #[inline(never)]
    fn gather(&mut self, at: &Position, frame: FrameId, resolved: &ResolvedType) -> Gathered {
        let mut gathered = Gathered::default();
        for (name, raw) in &at.declared.with {
            self.with_entry(frame, &at.where_, resolved, name, raw, &mut gathered.with);
        }
        self.with_defaults(frame, &at.where_, resolved, &mut gathered.with);
        self.gather_routes(at, resolved, &mut gathered);
        self.gather_needs(at, frame, &mut gathered);
        gathered
    }

    /// The routes bound for the gathered with-values, and their prices.
    #[inline(never)]
    fn gather_routes(&mut self, at: &Position, resolved: &ResolvedType, gathered: &mut Gathered) {
        gathered.bound = self.bind_routes(
            &at.declared,
            &at.where_,
            &resolved.spec,
            &gathered.with.values,
        );
    }

    #[inline(never)]
    fn gather_needs(&mut self, at: &Position, frame: FrameId, gathered: &mut Gathered) {
        gathered.needs = self.needs(frame, &at.declared, &at.where_);
    }

    /// The instance of a take from what `instance` gathered: its state, identity and prices,
    /// inserted (its listing position), then the step's assertions when it is planned or done.
    #[inline(never)]
    fn make_instance(
        &mut self,
        at: &Position,
        frame: FrameId,
        path: &str,
        resolved: &Rc<ResolvedType>,
        id: String,
        gathered: Gathered,
    ) -> String {
        let Gathered {
            with:
                WithValues {
                    values: with,
                    reads,
                    bindings,
                    interfaces,
                    ..
                },
            bound: (routes, prices),
            needs,
        } = gathered;
        let takes = self.frame(frame).takes.clone();
        let declared = Rc::clone(&at.declared);
        let where_ = at.where_.as_str();
        let spec = Rc::clone(&resolved.spec);
        // A result wins over everything: fx-graph-v1 calls an instance whose result failed
        // `failed` (gnode listed it as done) and any other result `done`.
        let (state, reason) = match self.env.results.get(&id) {
            Some(result) if result.status == ResultStatus::Failed => {
                (State::Failed, result.error.clone())
            }
            Some(_) => (State::Done, None),
            None if with.values().any(Val::contains_failed) => (
                State::Blocked,
                Some("something it reads failed".to_string()),
            ),
            None if missing_required(&spec, &with) => (
                State::Absent,
                Some("an input it needs does not exist".to_string()),
            ),
            None if self.frame(frame).maybe => (State::Maybe, None),
            None => (State::Planned, None),
        };
        if declared.at_plan && spec.paid() {
            self.problem(
                &format!("{where_}.at"),
                "an at: plan step makes no paid call",
            );
        }
        let identity = step_identity(&resolved.identity, &with, &routes, &takes);
        // The engine runs select itself: it calls nothing and costs nothing.
        let prices = if resolved.is_select() {
            Vec::new()
        } else {
            prices
        };
        let context = self.frame(frame);
        let instance = Instance {
            id: id.clone(),
            path: path.to_string(),
            step: where_.to_string(),
            takes,
            uses: resolved.uses.clone(),
            ty: Rc::clone(resolved),
            with,
            needs,
            state,
            identity,
            routes,
            prices,
            phase: context.phase,
            key: at.key.clone(),
            judges: None,
            judge_policy: None,
            judged_by: Vec::new(),
            view: declared.view.to_value(),
            at_plan: declared.at_plan,
            budget: context.budget.clone(),
            concurrency_group: at.concurrency_group.clone(),
            concurrency: declared.concurrency,
            timeout_s: declared.timeout,
            reason,
            reads,
            bindings: bindings.into_iter().collect(),
            interface_bindings: interfaces.into_iter().collect(),
            display_scope: context.display_scope.clone(),
        };
        self.instances.insert(id.clone(), instance);
        if !declared.asserts.is_empty() && matches!(state, State::Planned | State::Done) {
            self.assertions(&declared.asserts, frame, where_, Some(&id));
        }
        id
    }

    /// Evaluates `with:`: the values, defaults filled in, and the ids of the results read, as
    /// `instance` gathers them (it does so without this frame).
    #[cfg(test)]
    pub(crate) fn with_values(
        &mut self,
        frame: FrameId,
        declared: &Step,
        where_: &str,
        resolved: &ResolvedType,
    ) -> (IndexMap<String, Val>, BTreeSet<String>) {
        let mut gathering = WithValues::default();
        for (name, raw) in &declared.with {
            self.with_entry(frame, where_, resolved, name, raw, &mut gathering);
        }
        self.with_defaults(frame, where_, resolved, &mut gathering);
        (gathering.values, gathering.reads)
    }

    /// One authored entry of `with:`, evaluated at `<w>.with.<name>`.
    #[inline(never)]
    fn with_entry(
        &mut self,
        frame: FrameId,
        where_: &str,
        resolved: &ResolvedType,
        name: &str,
        raw: &Value,
        gathering: &mut WithValues,
    ) {
        let spec = &resolved.spec;
        if !spec.inputs.contains_key(name) && !spec.params.contains_key(name) {
            // Left out entirely: it never enters the identity.
            self.unknown_with(where_, &spec.name, name);
            return;
        }
        let at = with_at(where_, name);
        // FX: selecting mode belongs to the built-in fx/select@1 only.
        let selecting = resolved.is_select() && name == "first_of";
        let outer = std::mem::replace(&mut self.selecting, selecting);
        self.wiring.begin_capture(name);
        let value = self.evaluate_into(frame, raw, &at, &mut gathering.reads);
        let (bindings, interfaces) = self.wiring.end_capture_full();
        gathering.bindings.extend(bindings);
        gathering.interfaces.extend(interfaces);
        self.selecting = outer;
        self.with_entry_value(resolved, name, &at, selecting, value, gathering);
    }

    /// `{type} has no input or setting {name}` at `<w>.with.<name>`.
    #[inline(never)]
    fn unknown_with(&mut self, where_: &str, type_name: &str, name: &str) {
        self.problem(
            &format!("{where_}.with.{name}"),
            format!("{type_name} has no input or setting {name}"),
        );
    }

    /// An evaluated entry of `with:` kept: select's failed candidates as missing, project files
    /// read.
    #[inline(never)]
    fn with_entry_value(
        &mut self,
        resolved: &ResolvedType,
        name: &str,
        at: &str,
        selecting: bool,
        value: Val,
        gathering: &mut WithValues,
    ) {
        let spec = &resolved.spec;
        let mut value = value;
        if selecting && let Val::List(items) = value {
            // Top level only: a failed candidate is one that does not exist.
            value = Val::List(
                items
                    .into_iter()
                    .map(|item| match item {
                        Val::Failed(_) => Val::Missing,
                        other => other,
                    })
                    .collect(),
            );
        }
        if let Some(port) = spec.inputs.get(name) {
            value = self.port_files(value, port, at);
        } else if spec.is_template(name)
            && let Some(path) = project_path(&value)
        {
            value = self.project_file(&path, at);
            gathering.templates.insert(name.to_string());
        }
        gathering.values.insert(name.to_string(), value);
    }

    /// What `with:` left out: required inputs and settings reported, defaults filled in, then
    /// prompt files rendered (after the defaults, so they see `vars`; in declaration order, FX).
    #[inline(never)]
    fn with_defaults(
        &mut self,
        frame: FrameId,
        where_: &str,
        resolved: &ResolvedType,
        gathering: &mut WithValues,
    ) {
        let spec = Rc::clone(&resolved.spec);
        let values = &mut gathering.values;
        for (name, port) in &spec.inputs {
            if !values.contains_key(name) && !port.optional {
                self.problem(
                    &format!("{where_}.with"),
                    format!("{} needs input {name}", spec.name),
                );
            }
        }
        for (name, schema) in &spec.params {
            if values.contains_key(name) {
                continue;
            }
            if let Some(default) = schema.get("default") {
                values.insert(name.clone(), Val::from_json(default));
            } else if !spec.is_optional_param(name) {
                self.problem(
                    &format!("{where_}.with"),
                    format!("{} needs setting {name}", spec.name),
                );
            }
        }
        for name in spec
            .params
            .keys()
            .filter(|n| gathering.templates.contains(*n))
        {
            if let Some(Val::File(file)) = gathering.values.get(name) {
                let file = file.as_ref().clone();
                let rendered = self.render_prompt(
                    frame,
                    &file,
                    &gathering.values,
                    &format!("{where_}.with.{name}"),
                );
                gathering.values.insert(name.clone(), rendered);
            }
        }
    }

    /// Reads `./` paths given to an input port: one value for a single port, each item of a list,
    /// each value of an object for a keyed port; anything else is kept as it is.
    pub(crate) fn port_files(&mut self, value: Val, port: &Port, where_: &str) -> Val {
        if port.shape == Shape::One {
            return self.port_file(value, where_);
        }
        match value {
            Val::List(items) => Val::List(
                items
                    .into_iter()
                    .map(|item| self.port_file(item, where_))
                    .collect(),
            ),
            Val::Object(entries) if port.shape == Shape::Keyed => Val::Object(
                entries
                    .into_iter()
                    .map(|(key, item)| (key, self.port_file(item, where_)))
                    .collect(),
            ),
            // A list port given one path keeps the string (gnode's rule).
            other => other,
        }
    }

    /// One value given to an input port: a project path becomes the file. FX: an absolute path
    /// is handed to the registry too, which refuses it (spec/identity.md §8).
    fn port_file(&mut self, value: Val, where_: &str) -> Val {
        match project_path(&value).or_else(|| absolute_path(&value)) {
            Some(path) => self.project_file(&path, where_),
            None => value,
        }
    }

    /// A project file named in `with:`; missing with a problem when it cannot be read.
    pub(crate) fn project_file(&mut self, value: &str, where_: &str) -> Val {
        match self.env.registry.project_file(value) {
            Ok(file) => Val::File(Box::new(file)),
            Err(reason) => {
                self.problem(where_, format!("cannot read {value}: {reason}"));
                Val::Missing
            }
        }
    }

    /// Renders a prompt file (spec/identity.md §5: decoded, comments removed, then rendered) with
    /// the step's `vars` (also each by its own name) and the workflow's inputs.
    pub(crate) fn render_prompt(
        &mut self,
        frame: FrameId,
        file: &FileValue,
        values: &IndexMap<String, Val>,
        where_: &str,
    ) -> Val {
        if file.location.is_none() {
            self.problem(where_, format!("{} has no text to render", file.name));
            return Val::Missing;
        }
        let bytes = match file.read_bytes() {
            Ok(bytes) => bytes,
            Err(reason) => {
                self.problem(where_, format!("cannot read {}: {reason}", file.name));
                return Val::Missing;
            }
        };
        let source = match text::decode_text(&bytes, &file.name) {
            Ok(source) => text::prompt_text(&source),
            Err(error) => {
                self.problem(where_, error.message);
                return Val::Missing;
            }
        };
        let parsed = match expr::template(&source) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => return Val::Str(source),
            Err(error) => {
                self.problem(where_, format!("{}: {error}", file.name));
                return Val::Missing;
            }
        };
        // `vars` when it holds something (Python's `values.get("vars") or {}`); a pending value
        // is kept so the prompt waits for it.
        let vars = match values.get("vars") {
            Some(vars) if matches!(vars, Val::Pending(_)) || vars.truthy() => vars.clone(),
            _ => Val::Object(IndexMap::new()),
        };
        let mut scope = TemplateScope {
            vars,
            inputs: Rc::clone(&self.frame(frame).inputs),
        };
        match expr::render(&parsed, &mut scope) {
            Ok(value) => value,
            Err(error) => {
                self.problem(where_, format!("{}: {error}", file.name));
                Val::Missing
            }
        }
    }

    /// Resolves `needs:` to instance ids: every instance of each named step (all takes, repeat
    /// items and group members), first-seen order, no repeats.
    pub(crate) fn needs(&mut self, frame: FrameId, declared: &Step, where_: &str) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for name in &declared.needs {
            let Some(owner) = self.find(frame, name) else {
                self.problem(&format!("{where_}.needs"), format!("no step {name}"));
                continue;
            };
            // A node step already under way (a `needs:` cycle, or itself) gives the instances it
            // has so far, silently.
            let expansion = match self.step(owner, name) {
                Ok(expansion) => expansion,
                Err(error) => {
                    self.problem(where_, error.0);
                    continue;
                }
            };
            for id in self.instance_ids(expansion) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }
}

/// `<w>.with.<name>`.
#[inline(never)]
fn with_at(where_: &str, name: &str) -> String {
    format!("{where_}.with.{name}")
}

/// A `./` or `../` path with no newline: a project file (spec/identity.md §8).
fn project_path(value: &Val) -> Option<String> {
    match value {
        Val::Str(s) if (s.starts_with("./") || s.starts_with("../")) && !s.contains('\n') => {
            Some(s.clone())
        }
        _ => None,
    }
}

/// An absolute POSIX path with no newline.
fn absolute_path(value: &Val) -> Option<String> {
    match value {
        Val::Str(s) if s.starts_with('/') && !s.contains('\n') => Some(s.clone()),
        _ => None,
    }
}

/// The step identity (spec/identity.md §8), or `None` when a with-value is pending.
pub fn step_identity(
    type_identity: &str,
    with: &IndexMap<String, Val>,
    routes: &IndexMap<String, Route>,
    takes: &[u32],
) -> Option<String> {
    let mut plain = Map::new();
    for (name, value) in with {
        plain.insert(name.clone(), value.plain()?);
    }
    let routes: Map<String, Value> = routes
        .iter()
        .map(|(capability, route)| (capability.clone(), Value::String(route.fingerprint())))
        .collect();
    Some(digest(&json!({
        "kind": "fx-step-v1",
        "type": type_identity,
        "with": plain,
        "routes": routes,
        "take": takes,
    })))
}

/// Whether a required input given in `with:` is nothing (null or missing), or any list or object
/// given to an input holds a nothing item. A collection is not checked.
pub fn missing_required(spec: &NodeSpec, with: &IndexMap<String, Val>) -> bool {
    spec.inputs.iter().any(|(name, port)| {
        let Some(value) = with.get(name) else {
            // A required input left out of `with:` is a problem already.
            return false;
        };
        if !port.optional && value.is_nothing() {
            return true;
        }
        // Every item written into a list or map is asked for; a collection is not checked.
        match value {
            Val::List(items) => items.iter().any(Val::is_nothing),
            Val::Object(entries) => entries.values().any(Val::is_nothing),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::judge::tests::{
        Fixture, ROOT, builtin_type, port, project_type, spec, step,
    };
    use crate::val::Collection;

    fn values(pairs: &[(&str, Val)]) -> IndexMap<String, Val> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn required_inputs() {
        let spec = spec(
            "x",
            &[
                ("image", port("image", Shape::One, false)),
                ("extra", port("image", Shape::One, true)),
                ("parts", port("text", Shape::Keyed, true)),
                ("refs", port("image", Shape::List, true)),
            ],
            &[("text", json!({"type": "string"}))],
            &[],
            false,
        );
        let file = Val::Str("x".into());
        assert!(!missing_required(&spec, &values(&[])), "left out");
        assert!(!missing_required(
            &spec,
            &values(&[("image", file.clone())])
        ));
        assert!(missing_required(&spec, &values(&[("image", Val::Missing)])));
        assert!(missing_required(&spec, &values(&[("image", Val::Null)])));
        assert!(
            !missing_required(&spec, &values(&[("extra", Val::Missing)])),
            "optional"
        );
        assert!(missing_required(
            &spec,
            &values(&[("refs", Val::List(vec![file.clone(), Val::Missing]))])
        ));
        let mut parts = IndexMap::new();
        parts.insert("a".to_string(), Val::Null);
        assert!(missing_required(
            &spec,
            &values(&[("parts", Val::Object(parts))])
        ));
        let collection = Collection {
            items: vec![("a".into(), Val::Missing)],
            verdicts: IndexMap::new(),
        };
        assert!(
            !missing_required(
                &spec,
                &values(&[("parts", Val::Collection(Box::new(collection)))])
            ),
            "a collection is not checked"
        );
        assert!(
            !missing_required(&spec, &values(&[("text", Val::Missing)])),
            "params never make an instance absent"
        );
    }

    #[test]
    fn with_values_names_defaults_and_requirements() {
        let mut declared = step("./nodes/cases.py#paint");
        declared.with.insert("bogus".into(), json!("x"));
        declared.with.insert("prompt".into(), json!("hello"));
        let mut f = Fixture::new(&[("paint", declared.clone())]);
        let mut ex = f.expander();
        let ty = project_type(
            "./nodes/cases.py#paint",
            spec(
                "paint",
                &[
                    ("image", port("image", Shape::One, false)),
                    ("refs", port("image", Shape::List, true)),
                ],
                &[
                    ("prompt", json!({"type": "string", "x-fx-template": true})),
                    ("background", json!({"enum": ["auto"], "default": "auto"})),
                    ("seed", json!({"type": "integer"})),
                ],
                &[],
                false,
            ),
        );
        let (with, reads) = ex.with_values(ROOT, &declared, "paint", &ty);
        assert!(reads.is_empty());
        assert_eq!(
            with.keys().collect::<Vec<_>>(),
            vec!["prompt", "background"],
            "authored entries, then defaults; no unknown name"
        );
        let problems: Vec<String> = ex.problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            problems,
            vec![
                "paint.with.bogus: paint has no input or setting bogus",
                "paint.with: paint needs input image",
                "paint.with: paint needs setting seed",
            ]
        );
    }

    #[test]
    fn an_optional_param_needs_nothing() {
        let declared = step("./nodes/cases.py#paint");
        let mut f = Fixture::new(&[("paint", declared.clone())]);
        let mut ex = f.expander();
        let ty = project_type(
            "./nodes/cases.py#paint",
            spec(
                "paint",
                &[],
                &[("size", json!({"type": "string", "x-fx-optional": true}))],
                &[],
                false,
            ),
        );
        let (with, _) = ex.with_values(ROOT, &declared, "paint", &ty);
        assert!(with.is_empty());
        assert!(ex.problems.is_empty());
    }

    #[test]
    fn port_files_by_shape() {
        let mut f = Fixture::new(&[]);
        let mut ex = f.expander();
        let path = || Val::Str("./absent.png".into());
        let one = ex.port_files(path(), &port("image", Shape::One, false), "w.with.image");
        assert_eq!(one, Val::Missing);
        let list = ex.port_files(
            Val::List(vec![path(), Val::Str("plain".into())]),
            &port("image", Shape::List, false),
            "w.with.refs",
        );
        assert_eq!(
            list,
            Val::List(vec![Val::Missing, Val::Str("plain".into())])
        );
        let mut entries = IndexMap::new();
        entries.insert("a".to_string(), path());
        let keyed = ex.port_files(
            Val::Object(entries.clone()),
            &port("image", Shape::Keyed, false),
            "w.with.parts",
        );
        let Val::Object(keyed) = keyed else {
            panic!("object")
        };
        assert_eq!(keyed.get("a"), Some(&Val::Missing));
        // A list port given one path, or a map given to a list port, keeps the value.
        assert_eq!(
            ex.port_files(path(), &port("image", Shape::List, false), "w.with.refs"),
            path()
        );
        assert_eq!(
            ex.port_files(
                Val::Object(entries.clone()),
                &port("image", Shape::List, false),
                "w.with.refs"
            ),
            Val::Object(entries)
        );
        // Text with a newline is not a path.
        let note = Val::Str("./a\nb".into());
        assert_eq!(
            ex.port_files(note.clone(), &port("text", Shape::One, false), "w.with.t"),
            note
        );
        // An absolute path is refused.
        assert_eq!(
            ex.port_files(
                Val::Str("/etc/hosts".into()),
                &port("text", Shape::One, false),
                "w.with.t"
            ),
            Val::Missing
        );
        let wheres: Vec<&str> = ex.problems.iter().map(|p| p.where_.as_str()).collect();
        assert_eq!(
            wheres,
            vec!["w.with.image", "w.with.refs", "w.with.parts", "w.with.t"]
        );
        assert!(
            ex.problems[0]
                .message
                .starts_with("cannot read ./absent.png: ")
        );
        assert!(
            ex.problems[3]
                .message
                .starts_with("cannot read /etc/hosts: ")
        );
    }

    #[test]
    fn select_plain_forms_vector() {
        let file = FileValue {
            digest: "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8".into(),
            kind: "text/plain".into(),
            name: "ada.txt".into(),
            size: 8,
            key: Some("ada".into()),
            content: None,
            location: None,
        };
        let collection = Collection {
            items: vec![
                ("ada".into(), Val::File(Box::new(file))),
                ("bo".into(), Val::Failed("entity['bo'].draw#1".into())),
            ],
            verdicts: IndexMap::new(),
        };
        let with = values(&[(
            "first_of",
            Val::List(vec![
                Val::Failed("draw#1".into()),
                Val::Missing,
                Val::Collection(Box::new(collection)),
            ]),
        )]);
        assert_eq!(
            step_identity("fx/select@1.1", &with, &IndexMap::new(), &[1]).as_deref(),
            Some("53c409bf47a7c2f5d831fd7987ae13bcc4d97647b468b51fbc1abe0686e13a65")
        );
        let pending = values(&[(
            "first_of",
            Val::Pending(Box::new(crate::val::Pending::of("draw#1", None))),
        )]);
        assert_eq!(
            step_identity("fx/select@1.1", &pending, &IndexMap::new(), &[1]),
            None
        );
    }

    #[test]
    fn select_is_the_builtin_only() {
        let select = builtin_type(
            "select",
            spec(
                "select",
                &[],
                &[("first_of", json!({"type": "array"}))],
                &[],
                false,
            ),
        );
        let lookalike = project_type(
            "./nodes/cases.py#select",
            spec(
                "select",
                &[],
                &[("first_of", json!({"type": "array"}))],
                &[],
                false,
            ),
        );
        assert!(select.is_select());
        assert!(!lookalike.is_select());
    }
}
