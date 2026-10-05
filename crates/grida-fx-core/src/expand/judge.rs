//! Takes, judges and which take downstream steps read.
//!
//! - `take_numbers`: with `takes:`, `1..=max(takes, the takes file's take)`; else a sequence
//!   from the takes file's take (default 1), as long as the largest `max` among the sibling
//!   judges whose `on_reject` regenerates (`sequence_length`; problems at
//!   `<judge name>.on_reject.max`). A judge uses the take numbers of the step it judges.
//! - `takes_max`: a count, or an expression that must give a whole number known while planning
//!   (`max: is a number known while planning`) from 1 to 12 (`max: is 1 to 12 takes`).
//! - judges: `judges: names a sibling, not {name}`; `{uses} is not a judge: it reports no
//!   verdict`; linking copies an absent or blocked subject's state and reason, or a maybe
//!   subject's maybe; `verdict` (accept when every present judge accepted, reject when one did not
//!   or reported no verdict, failed when a judge failed, unknown while one has not run);
//!   sequencing, where a later take exists only while the take before it was rejected and its
//!   judges regenerate (`nothing judges the take before it`, `an earlier take settled it`, `the
//!   take before it failed`, `the rejection does not regenerate`, `only if the take before it is
//!   rejected`).
//! - `chosen`: the take downstream steps receive (the last live take once decided; a rejection
//!   then fails, skips or continues, fail > skip > continue, with `keep_best` choosing among the
//!   live takes by a score in the judges' facts, the first of equal scores); `takes:` with `pick`
//!   (manual from the takes file, `first_accepted`, `best`); a pending choice waits on every live
//!   take and its judges.
//! - `node_result` and `node_take`: what `steps.x.outputs`, `.facts` and `.take` read, recording
//!   the results read; in selecting mode a rejected or failed take reads as missing; the built-in
//!   select's outputs are an any-port view; an optional output not made reads as missing.
//! - `feedback`: what the regenerating judges said of the take before, for the next take.

use super::frame::{ExpId, ExpKind, FrameId};
use super::scope::ViewData;
use super::{Expander, ResultStatus, State, instance_id};
use crate::docs::workflow::{KeepBest, OnReject, Pick, Regeneration, Step, TakesMax, Then};
use crate::expr::ExprError;
use crate::val::{Pending, Val, Verdict};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::rc::Rc;

/// `outputs` or `facts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum What {
    Outputs,
    Facts,
}

impl What {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            What::Outputs => "outputs",
            What::Facts => "facts",
        }
    }
}

/// The take a reader receives: an instance, or a value standing for it (missing, failed, pending).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Chosen {
    Instance(String),
    Value(Val),
}

/// What a rejection does, after every rejecting judge is considered (fail > skip > continue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Policy {
    Fail,
    Skip,
    Continue,
}

impl Policy {
    /// What one judge's `on_reject` contributes once no further take will come: a
    /// regeneration's `then`, where `keep_best` continues with the best take.
    fn of(on_reject: &OnReject) -> Policy {
        match on_reject {
            OnReject::Fail => Policy::Fail,
            OnReject::Skip => Policy::Skip,
            OnReject::Continue => Policy::Continue,
            OnReject::Regenerate(regeneration) => match &regeneration.then {
                Then::Fail => Policy::Fail,
                Then::Skip => Policy::Skip,
                Then::Continue | Then::KeepBest(_) => Policy::Continue,
            },
        }
    }
}

impl Expander<'_> {
    /// The take numbers of a node step at `path` (module doc).
    pub(crate) fn take_numbers(
        &mut self,
        frame: FrameId,
        name: &str,
        declared: &Step,
        path: &str,
    ) -> Vec<u32> {
        // The takes file names the step by its path with keys and group prefix, without takes.
        let chosen = self.env.takes.get(path).map(|choice| choice.take);
        if let Some(takes) = declared.takes {
            // A pick beyond `takes:` keeps that take.
            let last = takes.max(chosen.unwrap_or(0));
            return (1..=last).collect();
        }
        // A reroll starts the sequence at the take the file names.
        let first = chosen.unwrap_or(1).max(1);
        let length = self.sequence_length(frame, name);
        (0..length).map(|k| first.saturating_add(k)).collect()
    }

    /// How many takes a judged step may need: its regenerating judges' largest `max`, else 1.
    pub(crate) fn sequence_length(&mut self, frame: FrameId, name: &str) -> u32 {
        let steps = self.frame(frame).steps.clone();
        let mut longest = 1;
        for (judge, other) in steps.iter() {
            if other.judges.as_deref() != Some(name) {
                continue;
            }
            // A judge left out by its own `if:` still counts; the problem key is the judge's bare
            // name (gnode's key, kept).
            if let OnReject::Regenerate(regeneration) = &other.on_reject {
                let limit = self.takes_max(frame, regeneration, &format!("{judge}.on_reject"));
                longest = longest.max(limit);
            }
        }
        longest
    }

    /// A regeneration's `max`; `where_` is `<w>.regenerate` or `<judge>.on_reject`.
    pub(crate) fn takes_max(
        &mut self,
        frame: FrameId,
        regeneration: &Regeneration,
        where_: &str,
    ) -> u32 {
        let expression = match &regeneration.max {
            // The document holds a literal count to 1..12.
            TakesMax::Count(count) => return *count,
            TakesMax::Expr(text) => Value::String(text.clone()),
        };
        let at = format!("{where_}.max");
        let limit = self.evaluate(frame, &expression, &at);
        // One number type: a whole number is a count (`2.0` is 2); a pending value, a boolean,
        // a fraction or anything else is not a number known while planning.
        let whole = match limit {
            Val::Number(x) if x.fract() == 0.0 => Some(x),
            _ => None,
        };
        match whole {
            None => {
                self.problem(&at, "max: is a number known while planning");
                1
            }
            Some(x) if !(1.0..=12.0).contains(&x) => {
                self.problem(&at, "max: is 1 to 12 takes");
                1
            }
            Some(x) => x as u32,
        }
    }

    /// The node expansion a judge judges, expanding it first; it must be a sibling.
    pub(crate) fn sibling(&mut self, frame: FrameId, name: &str, where_: &str) -> Option<ExpId> {
        let scope = self.scope_of(frame);
        let owner = self.find(frame, name).map(|owner| self.scope_of(owner));
        if owner != Some(scope) {
            self.problem(
                &format!("{where_}.judges"),
                format!("judges: names a sibling, not {name}"),
            );
            return None;
        }
        match self.step(scope, name) {
            // A group, repeat, used workflow or absent step is judged by nobody, silently: the
            // judge runs once and links to nothing (gnode's rule, kept).
            Ok(expansion) => (self.exp(expansion).kind == ExpKind::Node).then_some(expansion),
            // The subject is under way: it reads this judge, a cycle. The judge runs once and
            // links to nothing; the cycle refuses the plan.
            Err(error) => {
                self.problem(where_, error.0);
                None
            }
        }
    }

    /// Links a judge instance to the take it judges.
    pub(crate) fn link_judge(&mut self, judge_id: &str, subject_id: &str, where_: &str) {
        self.not_a_judge(judge_id, where_);
        let Some(subject) = self.instances.get_mut(subject_id) else {
            return;
        };
        if !subject.judged_by.iter().any(|id| id == judge_id) {
            subject.judged_by.push(judge_id.to_string());
        }
        let (state, reason) = (subject.state, subject.reason.clone());
        let Some(judge) = self.instances.get_mut(judge_id) else {
            return;
        };
        judge.judges = Some(subject_id.to_string());
        if matches!(state, State::Absent | State::Blocked) && judge.state != State::Done {
            judge.state = state;
            judge.reason = reason;
        } else if state == State::Maybe && judge.state == State::Planned {
            judge.state = State::Maybe;
        }
    }

    /// `{uses} is not a judge: it reports no verdict` at `<w>.judges`, for a judge step whose type
    /// is not a judge (reported per take; planning deduplicates).
    pub(crate) fn not_a_judge(&mut self, judge_id: &str, where_: &str) {
        let Some(judge) = self.instances.get(judge_id) else {
            return;
        };
        if !judge.ty.spec.judge {
            let message = format!("{} is not a judge: it reports no verdict", judge.uses);
            self.problem(&format!("{where_}.judges"), message);
        }
    }

    /// Sequences a judged step's takes (module doc).
    pub(crate) fn sequence(&mut self, ids: &[String]) {
        for pair in ids.windows(2) {
            let (earlier, later) = (&pair[0], &pair[1]);
            let Some(later_state) = self.instances.get(later).map(|i| i.state) else {
                continue;
            };
            if later_state == State::Done {
                continue;
            }
            let verdict = self.verdict(earlier);
            let more = self.regenerates(earlier, verdict);
            let Some(before) = self.instances.get(earlier) else {
                continue;
            };
            let change = if before.judged_by.is_empty() {
                Some((State::Absent, "nothing judges the take before it"))
            } else if matches!(before.state, State::Absent | State::Blocked | State::Failed)
                || verdict == Some(Verdict::Accept)
            {
                Some((State::Absent, "an earlier take settled it"))
            } else if verdict == Some(Verdict::Failed) {
                Some((State::Absent, "the take before it failed"))
            } else if verdict == Some(Verdict::Reject) && !more {
                Some((State::Absent, "the rejection does not regenerate"))
            } else if verdict.is_none() && later_state == State::Planned {
                Some((State::Maybe, "only if the take before it is rejected"))
            } else {
                // A rejection that regenerates: the later take keeps its state.
                None
            };
            let Some(instance) = self.instances.get_mut(later) else {
                continue;
            };
            if let Some((state, reason)) = change {
                instance.state = state;
                instance.reason = Some(reason.to_string());
            }
            let state = instance.state;
            if !matches!(state, State::Absent | State::Maybe) {
                continue;
            }
            // The take's judges follow it, without a reason of their own.
            for judge_id in instance.judged_by.clone() {
                if let Some(judge) = self.instances.get_mut(&judge_id)
                    && judge.state != State::Done
                {
                    judge.state = state;
                }
            }
        }
    }

    /// Whether a rejection asks for another take: every judge that did not accept regenerates.
    pub(crate) fn regenerates(&self, id: &str, verdict: Option<Verdict>) -> bool {
        if verdict != Some(Verdict::Reject) {
            return false;
        }
        self.rejecting_policies(id)
            .iter()
            .all(|policy| matches!(policy, Some(OnReject::Regenerate(_))))
    }

    /// The verdict of a take; `None` while undecided or unjudged.
    pub(crate) fn verdict(&self, id: &str) -> Option<Verdict> {
        let instance = self.instances.get(id)?;
        if instance.judged_by.is_empty() {
            return None;
        }
        let mut judged = false;
        let mut accepted = true;
        for judge_id in &instance.judged_by {
            if self
                .instances
                .get(judge_id)
                .is_some_and(|judge| judge.state == State::Absent)
            {
                continue;
            }
            let result = self.env.results.get(judge_id)?;
            if result.status != ResultStatus::Succeeded {
                return Some(Verdict::Failed);
            }
            judged = true;
            // A judge that succeeds without a verdict counts as a rejection.
            accepted &= result.verdict() == Some("accept");
        }
        judged.then_some(if accepted {
            Verdict::Accept
        } else {
            Verdict::Reject
        })
    }

    /// What a rejection of a take means once no further take comes: the most severe policy among
    /// the judges that did not accept (fail > skip > continue); fail when none.
    pub(crate) fn rejection_policy(&self, id: &str) -> Policy {
        let policies: Vec<Policy> = self
            .rejecting_policies(id)
            .iter()
            .map(|policy| policy.as_ref().map_or(Policy::Fail, Policy::of))
            .collect();
        [Policy::Fail, Policy::Skip, Policy::Continue]
            .into_iter()
            .find(|severe| policies.contains(severe))
            .unwrap_or(Policy::Fail)
    }

    /// The policies of the judges of a take that have an instance and a result whose verdict is
    /// not `accept`, in link order.
    fn rejecting_policies(&self, id: &str) -> Vec<Option<OnReject>> {
        let Some(instance) = self.instances.get(id) else {
            return Vec::new();
        };
        instance
            .judged_by
            .iter()
            .filter_map(|judge_id| {
                let judge = self.instances.get(judge_id)?;
                let result = self.env.results.get(judge_id)?;
                (result.verdict() != Some("accept")).then(|| judge.judge_policy.clone())
            })
            .collect()
    }

    /// The take downstream steps receive; `pinned` while a judge reads the take it judges.
    pub(crate) fn chosen(&mut self, exp: ExpId, pinned: Option<u32>) -> Chosen {
        let ids = self.exp(exp).instances.clone();
        if let Some(take) = pinned
            && let Some(id) = ids
                .iter()
                .find(|id| self.instances.get(*id).is_some_and(|i| i.take() == take))
        {
            return Chosen::Instance(id.clone());
        }
        // The choice waits for the step's judges: expand any that were held (`node.rs`).
        self.settle_judges(exp);
        let live: Vec<String> = ids
            .into_iter()
            .filter(|id| {
                self.instances
                    .get(id)
                    .is_some_and(|i| i.state != State::Absent)
            })
            .collect();
        let Some(last) = live.last().cloned() else {
            return Chosen::Value(Val::Missing);
        };
        if let Some(declared) = self.declared_of(exp)
            && declared.takes.is_some()
        {
            return self.picked(&live, &declared);
        }
        // A sequence of takes is settled when its last live take is decided.
        if live
            .iter()
            .any(|id| self.state_of(id) == Some(State::Maybe))
        {
            return self.pending_choice(&live, "last");
        }
        if matches!(self.state_of(&last), Some(State::Blocked | State::Failed)) {
            return Chosen::Value(Val::Failed(last));
        }
        if self.is_judged(&last) {
            match self.verdict(&last) {
                None => return self.pending_choice(&live, "last"),
                Some(Verdict::Failed) => return Chosen::Value(Val::Failed(last)),
                Some(Verdict::Reject) => match self.rejection_policy(&last) {
                    Policy::Fail => return Chosen::Value(Val::Failed(last)),
                    Policy::Skip => return Chosen::Value(Val::Missing),
                    Policy::Continue => {
                        if let Some(best) = self.keep_best(&live) {
                            return Chosen::Instance(best);
                        }
                    }
                },
                Some(Verdict::Accept) => {}
            }
        }
        Chosen::Instance(last)
    }

    /// `takes: N` with `pick`; `live` is not empty.
    fn picked(&self, live: &[String], declared: &Step) -> Chosen {
        match declared.pick.as_ref().unwrap_or(&Pick::Manual) {
            Pick::Manual => {
                let path = self.instances.get(&live[0]).map(|i| i.path.as_str());
                let take = path
                    .and_then(|path| self.env.takes.get(path))
                    .map_or(1, |choice| choice.take);
                let chosen = live
                    .iter()
                    .find(|id| self.instances.get(*id).is_some_and(|i| i.take() == take))
                    .unwrap_or(&live[0]);
                if self.is_judged(chosen) && self.verdict(chosen).is_none() {
                    return self.pending_choice(std::slice::from_ref(chosen), "manual");
                }
                // A rejected take the author picked is still the one read.
                Chosen::Instance(chosen.clone())
            }
            Pick::FirstAccepted => {
                for id in live {
                    match self.verdict(id) {
                        None => return self.pending_choice(live, "first_accepted"),
                        Some(Verdict::Accept) => return Chosen::Instance(id.clone()),
                        Some(_) => {}
                    }
                }
                Chosen::Value(Val::Missing)
            }
            Pick::Best(rule) => {
                if live.iter().any(|id| self.verdict(id).is_none()) {
                    return self.pending_choice(live, "best");
                }
                Chosen::Instance(self.best(live, rule).unwrap_or_else(|| live[0].clone()))
            }
        }
    }

    /// `then: {keep_best}` of the first regenerating judge of the last live take that has one.
    fn keep_best(&self, live: &[String]) -> Option<String> {
        let last = self.instances.get(live.last()?)?;
        for judge_id in &last.judged_by {
            let Some(judge) = self.instances.get(judge_id) else {
                continue;
            };
            if let Some(OnReject::Regenerate(regeneration)) = &judge.judge_policy
                && let Then::KeepBest(rule) = &regeneration.then
            {
                return self.best(live, rule);
            }
        }
        None
    }

    /// The live take with the lowest or highest score in its judges' facts; the first of equal
    /// scores; every live take is scored, rejected ones included.
    fn best(&self, live: &[String], rule: &KeepBest) -> Option<String> {
        let mut best: Option<(f64, &String)> = None;
        for id in live {
            let Some(instance) = self.instances.get(id) else {
                continue;
            };
            for judge_id in &instance.judged_by {
                let score = self
                    .env
                    .results
                    .get(judge_id)
                    .and_then(|result| result.facts.get(&rule.by))
                    .and_then(Value::as_f64);
                let Some(score) = score else {
                    continue;
                };
                let better = match best {
                    None => true,
                    Some((held, _)) if rule.highest => score > held,
                    Some((held, _)) => score < held,
                };
                if better {
                    best = Some((score, id));
                }
            }
        }
        best.map(|(_, id)| id.clone())
    }

    /// `Pending(refs = live ids and their judges, token = digest({"choose": why, "among": …}))`.
    pub(crate) fn waiting(&self, ids: &[String], why: &str) -> Pending {
        let mut refs = BTreeSet::new();
        let mut among = Vec::new();
        for id in ids {
            let Some(instance) = self.instances.get(id) else {
                continue;
            };
            among.push(Value::String(
                instance.identity.clone().unwrap_or_else(|| id.clone()),
            ));
            if instance.state == State::Absent {
                continue;
            }
            refs.insert(id.clone());
            refs.extend(instance.judged_by.iter().cloned());
        }
        Pending::new(refs, &json!({"choose": why, "among": among}))
    }

    /// `steps.x.outputs` / `steps.x.facts`.
    pub(crate) fn node_result(
        &mut self,
        exp: ExpId,
        pinned: Option<u32>,
        what: What,
    ) -> Result<Val, ExprError> {
        let id = match self.chosen(exp, pinned) {
            Chosen::Instance(id) => id,
            Chosen::Value(Val::Pending(pending)) => {
                let token = json!({"of": pending.token, "what": what.as_str()});
                return Ok(Val::Pending(Box::new(Pending::new(
                    pending.refs.clone(),
                    &token,
                ))));
            }
            Chosen::Value(Val::Failed(id)) => {
                // A take whose body failed has a result, and reading it is a read (gnode listed
                // such a take as done and read its result).
                if self
                    .env
                    .results
                    .get(&id)
                    .is_some_and(|result| result.status == ResultStatus::Failed)
                {
                    self.read(&id);
                }
                return Ok(Val::Failed(id));
            }
            Chosen::Value(value) => return Ok(value),
        };
        let Some(chosen) = self.instances.get(&id) else {
            return Ok(Val::Missing);
        };
        if chosen.state == State::Blocked {
            return Ok(Val::Failed(id));
        }
        let Some(result) = self.env.results.get(&id) else {
            if chosen.state == State::Absent {
                return Ok(Val::Missing);
            }
            let mut refs = BTreeSet::from([id.clone()]);
            refs.extend(chosen.judged_by.iter().cloned());
            let token = json!({
                "result": chosen.identity.clone().unwrap_or_else(|| id.clone()),
                "what": what.as_str(),
            });
            return Ok(Val::Pending(Box::new(Pending::new(refs, &token))));
        };
        let ty = Rc::clone(&chosen.ty);
        self.read(&id);
        match result.status {
            ResultStatus::Skipped => return Ok(Val::Missing),
            ResultStatus::Failed => return Ok(Val::Failed(id)),
            ResultStatus::Succeeded => {}
        }
        if what == What::Facts {
            let facts = result
                .facts
                .iter()
                .map(|(name, value)| (name.clone(), Val::from_json(value)))
                .collect();
            return Ok(Val::Object(facts));
        }
        // `select` takes the first result that exists and was not rejected; every other reader
        // still reads a result kept with `on_reject: continue`.
        if self.selecting && matches!(self.verdict(&id), Some(Verdict::Reject | Verdict::Failed)) {
            return Ok(Val::Missing);
        }
        if ty.is_select() {
            let value = result.outputs.get("value").cloned().unwrap_or(Val::Missing);
            return Ok(self.new_view(ViewData::AnyPort { value }));
        }
        // An optional output the step did not make reads as missing, like a skipped step.
        let mut outputs: IndexMap<String, Val> = ty
            .spec
            .outputs
            .iter()
            .filter(|(_, port)| port.optional)
            .map(|(name, _)| (name.clone(), Val::Missing))
            .collect();
        for (name, value) in &result.outputs {
            outputs.insert(name.clone(), value.clone());
        }
        Ok(Val::Object(outputs))
    }

    /// `steps.x.take`.
    pub(crate) fn node_take(&mut self, exp: ExpId, pinned: Option<u32>) -> Result<Val, ExprError> {
        Ok(match self.chosen(exp, pinned) {
            Chosen::Instance(id) => {
                let take = self.instances.get(&id).map_or(1, |i| i.take());
                Val::Number(f64::from(take))
            }
            // The pending, missing or failed value itself, not derived.
            Chosen::Value(value) => value,
        })
    }

    /// The chosen take's verdict as `.*` records it: accept when unjudged, `None` undecided.
    pub(crate) fn verdict_of_chosen(&mut self, exp: ExpId, pinned: Option<u32>) -> Option<Verdict> {
        match self.chosen(exp, pinned) {
            Chosen::Instance(id) => self
                .verdict(&id)
                .or_else(|| (!self.is_judged(&id)).then_some(Verdict::Accept)),
            Chosen::Value(_) => None,
        }
    }

    /// `feedback` for take `take` of a judged step: missing for the first take or when every judge
    /// accepted, pending until the judges of the take before have run, else `{judge: facts}`.
    /// `suffix` is the position's key suffix.
    pub(crate) fn feedback(
        &mut self,
        frame: FrameId,
        judges: &[String],
        suffix: &str,
        take: u32,
        first: u32,
    ) -> Val {
        if take == first {
            return Val::Missing;
        }
        let context = self.frame(frame);
        let mut takes = context.takes.clone();
        takes.push(take.saturating_sub(1));
        let prefix = context.prefix.clone();
        let mut said: IndexMap<String, Val> = IndexMap::new();
        let mut accepted = true;
        for judge in judges {
            let id = instance_id(&format!("{prefix}{judge}{suffix}"), &takes);
            let Some(result) = self.env.results.get(&id) else {
                let token = json!({"feedback": id});
                return Val::Pending(Box::new(Pending::new(BTreeSet::from([id]), &token)));
            };
            accepted &= result.verdict() == Some("accept");
            let facts = result
                .facts
                .iter()
                .map(|(name, value)| (name.clone(), Val::from_json(value)))
                .collect();
            said.insert(judge.clone(), Val::Object(facts));
        }
        // Every judge accepted the take before: this take never runs, and a template reading a
        // mark only a rejection makes must not stop the run over it.
        if accepted {
            return Val::Missing;
        }
        Val::Object(said)
    }

    /// The declared step an expansion was made from.
    pub(crate) fn declared_of(&self, exp: ExpId) -> Option<Rc<Step>> {
        let expansion = self.exp(exp);
        self.frame(expansion.frame)
            .steps
            .get(&expansion.name)
            .cloned()
    }

    fn state_of(&self, id: &str) -> Option<State> {
        self.instances.get(id).map(|i| i.state)
    }

    fn is_judged(&self, id: &str) -> bool {
        self.instances
            .get(id)
            .is_some_and(|i| !i.judged_by.is_empty())
    }

    fn pending_choice(&self, ids: &[String], why: &str) -> Chosen {
        Chosen::Value(Val::Pending(Box::new(self.waiting(ids, why))))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! Unit tests over hand-made instances and results, and the fixture other expansion-B tests
    //! share.

    use super::*;
    use crate::docs::takes::{TakeChoice, Takes};
    use crate::docs::workflow::{LoadedWorkflow, Steps, ViewSetting, WorkflowDoc};
    use crate::expand::frame::Frame;
    use crate::expand::{ExpandEnv, Instance, NodeResult};
    use crate::host::FakeHost;
    use crate::registry::{Registry, ResolvedType, TypeOrigin};
    use crate::routes::RouteTable;
    use crate::spec::{BodyKind, NodeSpec, Port, Retry, Shape};
    use std::path::PathBuf;

    /// A step with every field at its default.
    pub(crate) fn step(uses: &str) -> Step {
        Step {
            uses: Some(uses.to_string()),
            steps: None,
            with: IndexMap::new(),
            if_: None,
            needs: Vec::new(),
            for_each: None,
            as_: "item".into(),
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

    pub(crate) fn port(kind: &str, shape: Shape, optional: bool) -> Port {
        Port {
            kind: kind.into(),
            shape,
            optional,
        }
    }

    /// A free spec with these inputs, params and outputs.
    pub(crate) fn spec(
        name: &str,
        inputs: &[(&str, Port)],
        params: &[(&str, Value)],
        outputs: &[(&str, Port)],
        judge: bool,
    ) -> NodeSpec {
        NodeSpec {
            name: name.into(),
            description: None,
            inputs: inputs
                .iter()
                .map(|(n, p)| (n.to_string(), p.clone()))
                .collect(),
            params: params
                .iter()
                .map(|(n, s)| (n.to_string(), s.clone()))
                .collect(),
            outputs: outputs
                .iter()
                .map(|(n, p)| (n.to_string(), p.clone()))
                .collect(),
            judge,
            capability: None,
            calls: IndexMap::new(),
            resources: Vec::new(),
            tools: Vec::new(),
            view: None,
            version: Some(1),
            retry: Retry::default(),
        }
    }

    pub(crate) fn project_type(uses: &str, spec: NodeSpec) -> Rc<ResolvedType> {
        Rc::new(ResolvedType {
            uses: uses.into(),
            identity: format!("{}@1", uses.trim_start_matches("./")),
            spec: Rc::new(spec),
            origin: TypeOrigin::Project {
                path: "nodes/cases.py".into(),
                attribute: uses.rsplit('#').next().unwrap_or_default().into(),
            },
            body: BodyKind::Project,
            source: None,
            drift: None,
        })
    }

    pub(crate) fn builtin_type(name: &str, spec: NodeSpec) -> Rc<ResolvedType> {
        Rc::new(ResolvedType {
            uses: format!("fx/{name}@1"),
            identity: format!("fx/{name}@1.1"),
            spec: Rc::new(spec),
            origin: TypeOrigin::Builtin {
                name: name.into(),
                major: 1,
            },
            body: BodyKind::Engine,
            source: None,
            drift: None,
        })
    }

    /// An image-making type: one optional output beside the image.
    pub(crate) fn drawing() -> Rc<ResolvedType> {
        project_type(
            "./nodes/cases.py#draw",
            spec(
                "draw",
                &[],
                &[("prompt", json!({"type": "string"}))],
                &[
                    ("image", port("image", Shape::One, false)),
                    ("mask", port("image", Shape::One, true)),
                ],
                false,
            ),
        )
    }

    pub(crate) fn verdict_type() -> Rc<ResolvedType> {
        project_type(
            "./nodes/cases.py#verdict",
            spec(
                "verdict",
                &[("subject", port("file", Shape::One, false))],
                &[],
                &[],
                true,
            ),
        )
    }

    pub(crate) fn instance(id: &str, ty: &Rc<ResolvedType>, state: State) -> Instance {
        let (path, takes) = id.split_once('#').unwrap();
        Instance {
            id: id.into(),
            path: path.into(),
            step: path.into(),
            takes: takes.split('.').map(|t| t.parse().unwrap()).collect(),
            uses: ty.uses.clone(),
            ty: Rc::clone(ty),
            with: IndexMap::new(),
            needs: Vec::new(),
            state,
            identity: None,
            routes: IndexMap::new(),
            prices: Vec::new(),
            phase: 1,
            key: None,
            judges: None,
            judge_policy: None,
            judged_by: Vec::new(),
            view: Value::Bool(false),
            at_plan: false,
            budget: None,
            concurrency_group: None,
            concurrency: None,
            timeout_s: None,
            reason: None,
            reads: BTreeSet::new(),
        }
    }

    pub(crate) fn succeeded(facts: Value) -> NodeResult {
        NodeResult {
            status: ResultStatus::Succeeded,
            outputs: IndexMap::new(),
            facts: facts
                .as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
            error: None,
        }
    }

    pub(crate) fn status(status: ResultStatus) -> NodeResult {
        NodeResult {
            status,
            outputs: IndexMap::new(),
            facts: IndexMap::new(),
            error: Some("boom".into()),
        }
    }

    /// What an expander reads, owned by the test.
    pub(crate) struct Fixture {
        pub workflow: Rc<LoadedWorkflow>,
        pub inputs: IndexMap<String, Val>,
        pub registry: Registry,
        pub host: FakeHost,
        pub routes: RouteTable,
        pub takes: Takes,
        pub results: IndexMap<String, NodeResult>,
        /// Keeps the project folder alive while the registry points at it.
        pub _dir: tempfile::TempDir,
    }

    impl Fixture {
        pub(crate) fn new(steps: &[(&str, Step)]) -> Fixture {
            let steps: Steps = Rc::new(
                steps
                    .iter()
                    .map(|(n, s)| (n.to_string(), Rc::new(s.clone())))
                    .collect(),
            );
            let doc = WorkflowDoc {
                id: "case".into(),
                title: "Case".into(),
                description: None,
                inputs: IndexMap::new(),
                tables: IndexMap::new(),
                let_: IndexMap::new(),
                budget: None,
                asserts: Vec::new(),
                steps,
                outputs: IndexMap::new(),
                view: None,
            };
            let root = tempfile::tempdir().unwrap();
            Fixture {
                workflow: Rc::new(LoadedWorkflow {
                    document: json!({}),
                    workflow: Rc::new(doc),
                    source: "workflows/case.yaml".into(),
                    path: PathBuf::from("workflows/case.yaml"),
                }),
                inputs: IndexMap::new(),
                registry: Registry::new(
                    root.path().to_path_buf(),
                    Vec::new(),
                    Default::default(),
                    IndexMap::new(),
                ),
                host: FakeHost::new(),
                routes: RouteTable::new(),
                takes: Takes::new(),
                results: IndexMap::new(),
                _dir: root,
            }
        }

        pub(crate) fn take(&mut self, path: &str, take: u32) {
            self.takes
                .insert(path.into(), TakeChoice { take, result: None });
        }

        /// An expander with the workflow's root frame (frame 0).
        pub(crate) fn expander(&mut self) -> Expander<'_> {
            let workflow = Rc::clone(&self.workflow.workflow);
            let mut ex = Expander::new(ExpandEnv {
                workflow: &self.workflow,
                inputs: &self.inputs,
                registry: &mut self.registry,
                host: &mut self.host,
                routes: &self.routes,
                takes: &self.takes,
                results: &self.results,
            });
            ex.new_frame(Frame {
                steps: Rc::clone(&workflow.steps),
                prefix: String::new(),
                decl_prefix: String::new(),
                variables: IndexMap::new(),
                parent: None,
                takes: Vec::new(),
                workflow,
                inputs: Rc::new(IndexMap::new()),
                phase: 1,
                judging: None,
                budget: None,
                owner: None,
                maybe: false,
            });
            ex
        }
    }

    pub(crate) const ROOT: FrameId = FrameId(0);

    /// Adds instances and a node expansion holding the first `subject` of them.
    fn node_exp(ex: &mut Expander<'_>, name: &str, ids: &[&str]) -> ExpId {
        let exp = ex.new_exp(ROOT, name);
        ex.exp_mut(exp).kind = ExpKind::Node;
        ex.exp_mut(exp).instances = ids.iter().map(|s| s.to_string()).collect();
        exp
    }

    fn add(ex: &mut Expander<'_>, instance: Instance) {
        ex.instances.insert(instance.id.clone(), instance);
    }

    /// `draw#1..n` judged by `check#1..n` with `on_reject`, states as instance() makes them.
    fn judged_draws(ex: &mut Expander<'_>, n: u32, on_reject: &OnReject) -> ExpId {
        let (draw, verdict) = (drawing(), verdict_type());
        let mut ids = Vec::new();
        for take in 1..=n {
            let draw_id = format!("draw#{take}");
            let check_id = format!("check#{take}");
            let state = |id: &str| match ex.env.results.get(id) {
                Some(result) if result.status == ResultStatus::Failed => State::Failed,
                Some(_) => State::Done,
                None => State::Planned,
            };
            let (draw_state, check_state) = (state(&draw_id), state(&check_id));
            add(ex, instance(&draw_id, &draw, draw_state));
            let mut judge = instance(&check_id, &verdict, check_state);
            judge.judge_policy = Some(on_reject.clone());
            add(ex, judge);
            ex.link_judge(&check_id, &draw_id, "check");
            ids.push(draw_id);
        }
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let exp = node_exp(ex, "draw", &refs);
        ex.sequence(&ids);
        // The judge's own `node` sequences again: sequencing is idempotent.
        ex.sequence(&ids);
        exp
    }

    fn regenerate(max: u32, then: Then) -> OnReject {
        OnReject::Regenerate(Regeneration {
            max: TakesMax::Count(max),
            then,
            until: None,
            feedback: false,
        })
    }

    fn states(ex: &Expander<'_>, ids: &[&str]) -> Vec<(State, Option<String>)> {
        ids.iter()
            .map(|id| {
                let i = &ex.instances[*id];
                (i.state, i.reason.clone())
            })
            .collect()
    }

    fn fixture_with_judge(on_reject: OnReject) -> Fixture {
        let mut check = step("./nodes/cases.py#verdict");
        check.judges = Some("draw".into());
        check.on_reject = on_reject;
        Fixture::new(&[("draw", step("fx/image.generate@1")), ("check", check)])
    }

    fn regen3() -> OnReject {
        regenerate(3, Then::Fail)
    }

    #[test]
    fn plan_time_states_with_no_results() {
        let mut f = fixture_with_judge(regen3());
        let mut ex = f.expander();
        judged_draws(&mut ex, 3, &regen3());
        let only_if = Some("only if the take before it is rejected".to_string());
        assert_eq!(
            states(&ex, &["draw#1", "draw#2", "draw#3"]),
            vec![
                (State::Planned, None),
                (State::Maybe, only_if.clone()),
                (State::Maybe, only_if)
            ]
        );
        assert_eq!(ex.instances["check#1"].state, State::Planned);
        assert_eq!(ex.instances["check#2"].state, State::Maybe);
        assert_eq!(ex.instances["check#3"].state, State::Maybe);
        // Judges follow without a reason of their own.
        assert_eq!(ex.instances["check#2"].reason, None);
        assert_eq!(ex.instances["draw#1"].judged_by, vec!["check#1"]);
        assert_eq!(ex.instances["check#1"].judges.as_deref(), Some("draw#1"));
    }

    #[test]
    fn a_rejected_first_take_plans_the_second() {
        let mut f = fixture_with_judge(regen3());
        f.results.insert("draw#1".into(), succeeded(json!({})));
        f.results
            .insert("check#1".into(), succeeded(json!({"verdict": "reject"})));
        let mut ex = f.expander();
        judged_draws(&mut ex, 3, &regen3());
        let st: Vec<State> = states(&ex, &["draw#1", "draw#2", "draw#3"])
            .into_iter()
            .map(|s| s.0)
            .collect();
        assert_eq!(st, vec![State::Done, State::Planned, State::Maybe]);
        assert_eq!(ex.instances["check#2"].state, State::Planned);
        assert_eq!(ex.instances["check#3"].state, State::Maybe);
    }

    #[test]
    fn an_accepted_first_take_settles_the_rest() {
        let mut f = fixture_with_judge(regen3());
        f.results.insert("draw#1".into(), succeeded(json!({})));
        f.results
            .insert("check#1".into(), succeeded(json!({"verdict": "accept"})));
        let mut ex = f.expander();
        let exp = judged_draws(&mut ex, 3, &regen3());
        let settled = Some("an earlier take settled it".to_string());
        assert_eq!(
            states(&ex, &["draw#2", "draw#3"]),
            vec![(State::Absent, settled.clone()), (State::Absent, settled)]
        );
        assert_eq!(ex.instances["check#2"].state, State::Absent);
        assert_eq!(ex.instances["check#3"].state, State::Absent);
        assert_eq!(ex.chosen(exp, None), Chosen::Instance("draw#1".into()));
        assert_eq!(
            ex.node_take(exp, None).unwrap(),
            Val::Number(1.0),
            "downstream reads take 1"
        );
    }

    #[test]
    fn a_failed_judge_ends_the_sequence() {
        let mut f = fixture_with_judge(regen3());
        f.results.insert("draw#1".into(), succeeded(json!({})));
        f.results
            .insert("check#1".into(), status(ResultStatus::Failed));
        let mut ex = f.expander();
        let exp = judged_draws(&mut ex, 3, &regen3());
        assert_eq!(
            states(&ex, &["draw#2", "draw#3"]),
            vec![
                (State::Absent, Some("the take before it failed".into())),
                (State::Absent, Some("an earlier take settled it".into()))
            ]
        );
        assert_eq!(ex.verdict("draw#1"), Some(Verdict::Failed));
        assert_eq!(
            ex.chosen(exp, None),
            Chosen::Value(Val::Failed("draw#1".into()))
        );
    }

    #[test]
    fn three_rejections_then_fail() {
        let mut f = fixture_with_judge(regen3());
        for take in 1..=3 {
            f.results
                .insert(format!("draw#{take}"), succeeded(json!({})));
            f.results.insert(
                format!("check#{take}"),
                succeeded(json!({"verdict": "reject"})),
            );
        }
        let mut ex = f.expander();
        let exp = judged_draws(&mut ex, 3, &regen3());
        assert!(
            states(&ex, &["draw#1", "draw#2", "draw#3"])
                .iter()
                .all(|s| s.0 == State::Done)
        );
        assert_eq!(
            ex.chosen(exp, None),
            Chosen::Value(Val::Failed("draw#3".into()))
        );
    }

    /// What downstream steps receive when every take is rejected, for each `on_reject`.
    #[test]
    fn rejection_outcomes() {
        let keep = Then::KeepBest(KeepBest {
            by: "score".into(),
            highest: true,
        });
        let cases: Vec<(OnReject, u32, Chosen)> = vec![
            (
                OnReject::Fail,
                1,
                Chosen::Value(Val::Failed("draw#1".into())),
            ),
            (OnReject::Continue, 1, Chosen::Instance("draw#1".into())),
            (OnReject::Skip, 1, Chosen::Value(Val::Missing)),
            (
                regenerate(2, Then::Continue),
                2,
                Chosen::Instance("draw#2".into()),
            ),
            (regenerate(2, Then::Skip), 2, Chosen::Value(Val::Missing)),
            (
                regenerate(3, Then::Fail),
                3,
                Chosen::Value(Val::Failed("draw#3".into())),
            ),
            (regenerate(3, keep), 3, Chosen::Instance("draw#2".into())),
        ];
        let scores = [5, 9, 7];
        for (on_reject, takes, want) in cases {
            let mut f = fixture_with_judge(on_reject.clone());
            for take in 1..=takes {
                f.results
                    .insert(format!("draw#{take}"), succeeded(json!({})));
                f.results.insert(
                    format!("check#{take}"),
                    succeeded(json!({"verdict": "reject", "score": scores[take as usize - 1]})),
                );
            }
            let mut ex = f.expander();
            let exp = judged_draws(&mut ex, takes, &on_reject);
            assert_eq!(ex.chosen(exp, None), want, "{on_reject:?}");
        }
    }

    #[test]
    fn keep_best_takes_the_first_of_equal_scores_and_lowest() {
        let mut f = fixture_with_judge(OnReject::Continue);
        for (take, score) in [(1, 4), (2, 2), (3, 2)] {
            f.results.insert(
                format!("check#{take}"),
                succeeded(json!({"verdict": "reject", "score": score})),
            );
        }
        let mut ex = f.expander();
        let (draw, verdict) = (drawing(), verdict_type());
        for take in 1..=3 {
            add(
                &mut ex,
                instance(&format!("draw#{take}"), &draw, State::Done),
            );
            add(
                &mut ex,
                instance(&format!("check#{take}"), &verdict, State::Done),
            );
            ex.link_judge(&format!("check#{take}"), &format!("draw#{take}"), "check");
        }
        let live: Vec<String> = (1..=3).map(|t| format!("draw#{t}")).collect();
        let lowest = KeepBest {
            by: "score".into(),
            highest: false,
        };
        let highest = KeepBest {
            by: "score".into(),
            highest: true,
        };
        assert_eq!(ex.best(&live, &lowest).as_deref(), Some("draw#2"));
        assert_eq!(ex.best(&live, &highest).as_deref(), Some("draw#1"));
        let unscored = KeepBest {
            by: "nope".into(),
            highest: true,
        };
        assert_eq!(ex.best(&live, &unscored), None);
    }

    #[test]
    fn verdicts() {
        let mut f = fixture_with_judge(OnReject::Fail);
        f.results
            .insert("a#1".into(), succeeded(json!({"verdict": "accept"})));
        f.results
            .insert("r#1".into(), succeeded(json!({"verdict": "reject"})));
        f.results.insert("n#1".into(), succeeded(json!({})));
        f.results
            .insert("f#1".into(), status(ResultStatus::Skipped));
        let mut ex = f.expander();
        let (draw, verdict) = (drawing(), verdict_type());
        let subject = |ex: &mut Expander<'_>, id: &str, judges: &[&str]| {
            let mut i = instance(id, &draw, State::Planned);
            i.judged_by = judges.iter().map(|s| s.to_string()).collect();
            add(ex, i);
        };
        for judge in ["a#1", "r#1", "n#1", "f#1", "wait#1"] {
            add(&mut ex, instance(judge, &verdict, State::Done));
        }
        add(&mut ex, instance("gone#1", &verdict, State::Absent));
        subject(&mut ex, "s1#1", &[]);
        subject(&mut ex, "s2#1", &["a#1"]);
        subject(&mut ex, "s3#1", &["a#1", "r#1"]);
        subject(&mut ex, "s4#1", &["n#1"]);
        subject(&mut ex, "s5#1", &["a#1", "f#1"]);
        subject(&mut ex, "s6#1", &["wait#1", "f#1"]);
        subject(&mut ex, "s7#1", &["gone#1"]);
        subject(&mut ex, "s8#1", &["gone#1", "a#1"]);
        assert_eq!(ex.verdict("s1#1"), None, "unjudged");
        assert_eq!(ex.verdict("s2#1"), Some(Verdict::Accept));
        assert_eq!(ex.verdict("s3#1"), Some(Verdict::Reject));
        assert_eq!(
            ex.verdict("s4#1"),
            Some(Verdict::Reject),
            "no verdict rejects"
        );
        assert_eq!(ex.verdict("s5#1"), Some(Verdict::Failed));
        assert_eq!(ex.verdict("s6#1"), None, "the first judge has not run");
        assert_eq!(ex.verdict("s7#1"), None, "only absent judges");
        assert_eq!(ex.verdict("s8#1"), Some(Verdict::Accept));
    }

    #[test]
    fn policies_and_regeneration() {
        let mut f = fixture_with_judge(OnReject::Fail);
        for (id, verdict) in [("j1#1", "reject"), ("j2#1", "reject"), ("j3#1", "accept")] {
            f.results
                .insert(id.into(), succeeded(json!({"verdict": verdict})));
        }
        let mut ex = f.expander();
        let (draw, verdict) = (drawing(), verdict_type());
        let judge = |ex: &mut Expander<'_>, id: &str, policy: OnReject| {
            let mut i = instance(id, &verdict, State::Done);
            i.judge_policy = Some(policy);
            add(ex, i);
        };
        let subject = |ex: &mut Expander<'_>, judges: &[&str]| {
            ex.instances.shift_remove("s#1");
            let mut i = instance("s#1", &draw, State::Done);
            i.judged_by = judges.iter().map(|s| s.to_string()).collect();
            add(ex, i);
        };
        judge(&mut ex, "j1#1", regenerate(3, Then::Skip));
        judge(&mut ex, "j2#1", OnReject::Continue);
        judge(&mut ex, "j3#1", OnReject::Fail);
        // One regenerating judge rejects, the other accepts: regenerate.
        subject(&mut ex, &["j1#1", "j3#1"]);
        assert!(ex.regenerates("s#1", Some(Verdict::Reject)));
        assert!(!ex.regenerates("s#1", Some(Verdict::Accept)));
        assert_eq!(ex.rejection_policy("s#1"), Policy::Skip);
        // Both reject and one says continue: the rejection does not regenerate.
        subject(&mut ex, &["j1#1", "j2#1"]);
        assert!(!ex.regenerates("s#1", Some(Verdict::Reject)));
        assert_eq!(
            ex.rejection_policy("s#1"),
            Policy::Skip,
            "skip beats continue"
        );
        subject(&mut ex, &["j2#1"]);
        assert_eq!(ex.rejection_policy("s#1"), Policy::Continue);
        subject(&mut ex, &["j3#1"]);
        assert_eq!(ex.rejection_policy("s#1"), Policy::Fail, "nothing rejected");
        judge(&mut ex, "j1#1", regenerate(3, Then::Fail));
        subject(&mut ex, &["j2#1", "j1#1"]);
        assert_eq!(ex.rejection_policy("s#1"), Policy::Fail, "fail beats all");
    }

    #[test]
    fn a_rejection_that_does_not_regenerate() {
        let mut f = fixture_with_judge(OnReject::Continue);
        f.results.insert("draw#1".into(), succeeded(json!({})));
        f.results
            .insert("check#1".into(), succeeded(json!({"verdict": "reject"})));
        let mut ex = f.expander();
        judged_draws(&mut ex, 2, &OnReject::Continue);
        assert_eq!(
            states(&ex, &["draw#2"]),
            vec![(
                State::Absent,
                Some("the rejection does not regenerate".into())
            )]
        );
    }

    #[test]
    fn an_unjudged_earlier_take() {
        let mut f = Fixture::new(&[("draw", step("fx/image.generate@1"))]);
        let mut ex = f.expander();
        let draw = drawing();
        add(&mut ex, instance("draw#1", &draw, State::Planned));
        add(&mut ex, instance("draw#2", &draw, State::Planned));
        ex.sequence(&["draw#1".into(), "draw#2".into()]);
        assert_eq!(
            states(&ex, &["draw#2"]),
            vec![(
                State::Absent,
                Some("nothing judges the take before it".into())
            )]
        );
    }

    #[test]
    fn linking_copies_the_subject_state() {
        let mut f = fixture_with_judge(OnReject::Fail);
        let mut ex = f.expander();
        let (draw, verdict) = (drawing(), verdict_type());
        let mut blocked = instance("draw#1", &draw, State::Blocked);
        blocked.reason = Some("something it reads failed".into());
        add(&mut ex, blocked);
        add(&mut ex, instance("draw#2", &draw, State::Maybe));
        add(&mut ex, instance("check#1", &verdict, State::Planned));
        add(&mut ex, instance("check#2", &verdict, State::Planned));
        ex.link_judge("check#1", "draw#1", "check");
        ex.link_judge("check#1", "draw#1", "check");
        ex.link_judge("check#2", "draw#2", "check");
        assert_eq!(ex.instances["draw#1"].judged_by, vec!["check#1"], "once");
        assert_eq!(ex.instances["check#1"].state, State::Blocked);
        assert_eq!(
            ex.instances["check#1"].reason.as_deref(),
            Some("something it reads failed")
        );
        assert_eq!(ex.instances["check#2"].state, State::Maybe);
        assert!(ex.problems.is_empty());
        // A judge step whose type reports no verdict.
        add(&mut ex, instance("shout#1", &draw, State::Planned));
        ex.link_judge("shout#1", "draw#2", "shout");
        assert_eq!(ex.problems.len(), 1);
        assert_eq!(ex.problems[0].where_, "shout.judges");
        assert_eq!(
            ex.problems[0].message,
            "./nodes/cases.py#draw is not a judge: it reports no verdict"
        );
    }

    #[test]
    fn pinned_reads_and_waiting() {
        let mut f = fixture_with_judge(regen3());
        let mut ex = f.expander();
        let exp = judged_draws(&mut ex, 3, &regen3());
        // A judge reads the take it judges.
        assert_eq!(ex.chosen(exp, Some(2)), Chosen::Instance("draw#2".into()));
        let Chosen::Value(Val::Pending(pending)) = ex.chosen(exp, None) else {
            panic!("pending")
        };
        let all: BTreeSet<String> = [
            "check#1", "check#2", "check#3", "draw#1", "draw#2", "draw#3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(pending.refs, all);
        let Val::Pending(read) = ex.node_result(exp, None, What::Outputs).unwrap() else {
            panic!("pending")
        };
        assert_eq!(read.refs, all);
        // A pinned read of an unfinished take waits on the take and its judges.
        let Val::Pending(pinned) = ex.node_result(exp, Some(1), What::Outputs).unwrap() else {
            panic!("pending")
        };
        assert_eq!(
            pinned.refs,
            BTreeSet::from(["check#1".to_string(), "draw#1".to_string()])
        );
        // Absent takes are left out of the refs but kept among the candidates.
        ex.instances["draw#3"].state = State::Absent;
        let waiting = ex.waiting(&["draw#2".into(), "draw#3".into()], "last");
        assert_eq!(
            waiting.refs,
            BTreeSet::from(["check#2".to_string(), "draw#2".to_string()])
        );
    }

    #[test]
    fn node_results() {
        let mut f = fixture_with_judge(OnReject::Continue);
        let mut outputs = IndexMap::new();
        outputs.insert("image".to_string(), Val::Str("img".into()));
        f.results.insert(
            "draw#1".into(),
            NodeResult {
                status: ResultStatus::Succeeded,
                outputs,
                facts: IndexMap::new(),
                error: None,
            },
        );
        f.results
            .insert("check#1".into(), succeeded(json!({"verdict": "reject"})));
        let mut ex = f.expander();
        let exp = judged_draws(&mut ex, 1, &OnReject::Continue);
        ex.read_sets.push(BTreeSet::new());
        let Val::Object(read) = ex.node_result(exp, None, What::Outputs).unwrap() else {
            panic!("outputs")
        };
        assert_eq!(read.get("image"), Some(&Val::Str("img".into())));
        assert_eq!(read.get("mask"), Some(&Val::Missing), "optional output");
        assert_eq!(
            ex.read_sets.last().unwrap(),
            &BTreeSet::from(["draw#1".to_string()])
        );
        // select's first_of reads a rejected take as missing.
        ex.selecting = true;
        assert_eq!(
            ex.node_result(exp, None, What::Outputs).unwrap(),
            Val::Missing
        );
        let Val::Object(facts) = ex.node_result(exp, None, What::Facts).unwrap() else {
            panic!("facts are kept in selecting mode")
        };
        assert!(facts.is_empty());
        ex.selecting = false;
        assert_eq!(ex.verdict_of_chosen(exp, None), Some(Verdict::Reject));
        // Blocked and failed takes read as failed results; a skipped one as missing.
        ex.instances["draw#1"].state = State::Blocked;
        assert_eq!(
            ex.node_result(exp, None, What::Outputs).unwrap(),
            Val::Failed("draw#1".into())
        );
    }

    #[test]
    fn statuses_and_select_outputs() {
        let mut f = Fixture::new(&[("a", step("x")), ("b", step("x")), ("pick", step("x"))]);
        f.results
            .insert("a#1".into(), status(ResultStatus::Skipped));
        f.results.insert("b#1".into(), status(ResultStatus::Failed));
        let mut outputs = IndexMap::new();
        outputs.insert("value".to_string(), Val::Str("v".into()));
        f.results.insert(
            "pick#1".into(),
            NodeResult {
                status: ResultStatus::Succeeded,
                outputs,
                facts: IndexMap::new(),
                error: None,
            },
        );
        let mut ex = f.expander();
        let draw = drawing();
        let select = builtin_type(
            "select",
            spec(
                "select",
                &[],
                &[("first_of", json!({"type": "array"}))],
                &[("value", port("file", Shape::One, true))],
                false,
            ),
        );
        add(&mut ex, instance("a#1", &draw, State::Done));
        add(&mut ex, instance("b#1", &draw, State::Done));
        add(&mut ex, instance("pick#1", &select, State::Done));
        add(&mut ex, instance("c#1", &draw, State::Absent));
        let a = node_exp(&mut ex, "a", &["a#1"]);
        let b = node_exp(&mut ex, "b", &["b#1"]);
        let pick = node_exp(&mut ex, "pick", &["pick#1"]);
        let c = node_exp(&mut ex, "c", &["c#1"]);
        assert_eq!(
            ex.node_result(a, None, What::Outputs).unwrap(),
            Val::Missing
        );
        assert_eq!(
            ex.node_result(b, None, What::Facts).unwrap(),
            Val::Failed("b#1".into())
        );
        assert_eq!(
            ex.node_result(c, None, What::Outputs).unwrap(),
            Val::Missing
        );
        assert_eq!(ex.node_take(c, None).unwrap(), Val::Missing);
        assert_eq!(ex.verdict_of_chosen(a, None), Some(Verdict::Accept));
        assert_eq!(ex.verdict_of_chosen(c, None), None);
        let Val::View(view) = ex.node_result(pick, None, What::Outputs).unwrap() else {
            panic!("any port")
        };
        assert_eq!(
            ex.views[view.0],
            ViewData::AnyPort {
                value: Val::Str("v".into())
            }
        );
    }

    #[test]
    fn picks_among_takes() {
        let judged = |pick: Pick, verdicts: &[Option<&str>], file: Option<u32>| {
            let mut draw = step("fx/image.generate@1");
            draw.takes = Some(verdicts.len() as u32);
            draw.pick = Some(pick);
            let mut check = step("./nodes/cases.py#verdict");
            check.judges = Some("draw".into());
            let mut f = Fixture::new(&[("draw", draw), ("check", check)]);
            for (k, verdict) in verdicts.iter().enumerate() {
                if let Some(verdict) = verdict {
                    f.results.insert(
                        format!("check#{}", k + 1),
                        succeeded(json!({"verdict": verdict, "score": k})),
                    );
                }
            }
            if let Some(take) = file {
                f.take("draw", take);
            }
            f
        };
        let run = |f: &mut Fixture, n: usize| {
            let mut ex = f.expander();
            let (draw, verdict) = (drawing(), verdict_type());
            let mut ids = Vec::new();
            for take in 1..=n {
                add(
                    &mut ex,
                    instance(&format!("draw#{take}"), &draw, State::Planned),
                );
                add(
                    &mut ex,
                    instance(&format!("check#{take}"), &verdict, State::Planned),
                );
                ex.link_judge(&format!("check#{take}"), &format!("draw#{take}"), "check");
                ids.push(format!("draw#{take}"));
            }
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let exp = node_exp(&mut ex, "draw", &refs);
            ex.chosen(exp, None)
        };
        let pending_refs = |chosen: Chosen| match chosen {
            Chosen::Value(Val::Pending(p)) => p.refs.into_iter().collect::<Vec<_>>(),
            other => panic!("not pending: {other:?}"),
        };
        let mut f = judged(
            Pick::FirstAccepted,
            &[Some("reject"), Some("accept"), None],
            None,
        );
        assert_eq!(run(&mut f, 3), Chosen::Instance("draw#2".into()));
        let mut f = judged(Pick::FirstAccepted, &[Some("reject"), None, None], None);
        assert_eq!(pending_refs(run(&mut f, 3)).len(), 6);
        let mut f = judged(
            Pick::FirstAccepted,
            &[Some("reject"), Some("reject"), Some("reject")],
            None,
        );
        assert_eq!(run(&mut f, 3), Chosen::Value(Val::Missing));
        let mut f = judged(Pick::Manual, &[None, None, None], Some(2));
        assert_eq!(pending_refs(run(&mut f, 3)), vec!["check#2", "draw#2"]);
        let mut f = judged(Pick::Manual, &[Some("reject"), Some("reject")], Some(2));
        assert_eq!(run(&mut f, 2), Chosen::Instance("draw#2".into()));
        let mut f = judged(Pick::Manual, &[Some("reject"), Some("reject")], Some(7));
        assert_eq!(run(&mut f, 2), Chosen::Instance("draw#1".into()));
        let best = Pick::Best(KeepBest {
            by: "score".into(),
            highest: true,
        });
        let mut f = judged(best.clone(), &[Some("reject"), Some("accept"), None], None);
        assert_eq!(pending_refs(run(&mut f, 3)).len(), 6);
        let mut f = judged(
            best,
            &[Some("reject"), Some("accept"), Some("reject")],
            None,
        );
        assert_eq!(run(&mut f, 3), Chosen::Instance("draw#3".into()));
        // Unjudged first_accepted waits forever (gnode's rule).
        let mut draw = step("fx/image.generate@1");
        draw.takes = Some(2);
        draw.pick = Some(Pick::FirstAccepted);
        let mut f = Fixture::new(&[("draw", draw)]);
        let mut ex = f.expander();
        let image = drawing();
        add(&mut ex, instance("draw#1", &image, State::Planned));
        add(&mut ex, instance("draw#2", &image, State::Planned));
        let exp = node_exp(&mut ex, "draw", &["draw#1", "draw#2"]);
        assert!(matches!(
            ex.chosen(exp, None),
            Chosen::Value(Val::Pending(_))
        ));
        // An unjudged manual pick reads take 1.
        let mut draw = step("fx/image.generate@1");
        draw.takes = Some(2);
        let mut f = Fixture::new(&[("draw", draw)]);
        let mut ex = f.expander();
        add(&mut ex, instance("draw#1", &image, State::Planned));
        add(&mut ex, instance("draw#2", &image, State::Planned));
        let exp = node_exp(&mut ex, "draw", &["draw#1", "draw#2"]);
        assert_eq!(ex.chosen(exp, None), Chosen::Instance("draw#1".into()));
    }

    #[test]
    fn take_numbers_and_the_takes_file() {
        // `takes: 3` with a pick of take 5 keeps five takes.
        let mut draw = step("fx/image.generate@1");
        draw.takes = Some(3);
        let mut f = Fixture::new(&[("draw", draw.clone())]);
        f.take("draw", 5);
        let mut ex = f.expander();
        assert_eq!(
            ex.take_numbers(ROOT, "draw", &draw, "draw"),
            vec![1, 2, 3, 4, 5]
        );
        // A plain step rerolled to take 3.
        let plain = step("fx/image.generate@1");
        let mut f = Fixture::new(&[("draw", plain.clone())]);
        f.take("draw", 3);
        let mut ex = f.expander();
        assert_eq!(ex.take_numbers(ROOT, "draw", &plain, "draw"), vec![3]);
        // A judged sequence of max 3 starting at take 2.
        let mut f = fixture_with_judge(regen3());
        f.take("draw", 2);
        let mut ex = f.expander();
        assert_eq!(ex.take_numbers(ROOT, "draw", &plain, "draw"), vec![2, 3, 4]);
        assert_eq!(
            ex.take_numbers(ROOT, "draw", &plain, "other"),
            vec![1, 2, 3]
        );
        // A max that is not known while planning counts one take.
        let mut f = fixture_with_judge(OnReject::Regenerate(Regeneration {
            max: TakesMax::Expr("${{ 'many' }}".into()),
            then: Then::Fail,
            until: None,
            feedback: false,
        }));
        let mut ex = f.expander();
        assert_eq!(ex.take_numbers(ROOT, "draw", &plain, "draw"), vec![1]);
        assert_eq!(ex.problems.last().unwrap().where_, "check.on_reject.max");
        assert_eq!(
            ex.problems.last().unwrap().message,
            "max: is a number known while planning"
        );
    }

    #[test]
    fn judge_feedback() {
        let mut f = fixture_with_judge(regen3());
        f.results.insert(
            "check#1".into(),
            succeeded(json!({"verdict": "reject", "note": "darker"})),
        );
        f.results
            .insert("check#2".into(), succeeded(json!({"verdict": "accept"})));
        let mut ex = f.expander();
        let judges = vec!["check".to_string()];
        assert_eq!(ex.feedback(ROOT, &judges, "", 1, 1), Val::Missing);
        let Val::Object(said) = ex.feedback(ROOT, &judges, "", 2, 1) else {
            panic!("object")
        };
        assert_eq!(said.keys().collect::<Vec<_>>(), vec!["check"]);
        assert_eq!(
            ex.feedback(ROOT, &judges, "", 3, 1),
            Val::Missing,
            "accepted"
        );
        let Val::Pending(pending) = ex.feedback(ROOT, &judges, "['a']", 2, 1) else {
            panic!("pending")
        };
        assert_eq!(pending.refs, BTreeSet::from(["check['a']#1".to_string()]));
    }
}
