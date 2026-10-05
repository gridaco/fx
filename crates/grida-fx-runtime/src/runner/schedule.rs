//! Scheduling decisions over one expansion: pure functions, tested without a runtime.
//!
//! - [`ready`]: an instance may start when it is `planned`; has no result and is not running; holds
//!   no pending value in `with`; every id in `needs` has a result or is absent from the expansion
//!   (a need on a judged step whose later takes became absent is met); every id in `reads` has a
//!   result or is absent, blocked or failed (an assertion that reads another step holds the step);
//!   and its concurrency group (`concurrency_group`, `concurrency`) has a free place ([`Groups`]).
//!   In listing order. An id the expansion does not list counts as absent, and an instance never
//!   waits on its own id.
//! - [`settle`]: instances in listing order with no result that are not running: `blocked` →
//!   failed, reason (default `something it reads failed`), event `node_skipped {reason, blocked:
//!   true}`; `failed` (a run-time assertion) → failed, event `node_failed {error}` (default `an
//!   assertion failed`). Both count as failures of this invocation.
//! - [`Gate`]: phases of live instances in order, minus approved ones; a phase whose members hold
//!   a pending value that waits on nothing is skipped (not yet priceable); else its `high` is its
//!   unfinished live members' high plus its pending repeats' high, `phase_planned {phase, steps,
//!   high_usd}` is emitted, and with `yes_up_to` set a phase after the first whose total (charged,
//!   plus held, plus its high) exceeds it is not approved: the gate's stop message. Approved
//!   phases stay approved for the invocation. A phase is announced again only when its steps or
//!   its high changed since it was last announced in this invocation, so a refused phase that is
//!   checked while earlier steps finish is not logged over and over.
//! - [`stuck`]: planned instances with no result once nothing runs or can start (each with the ids
//!   it still waits on).

use crate::events::Event;
use grida_fx_core::expand::{Expansion, Instance, NodeResult, State};
use grida_fx_core::money::Usd;
use grida_fx_core::val::Val;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Running members per concurrency group.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Groups {
    running: HashMap<String, u32>,
}

impl Groups {
    /// One more member of `group` runs.
    pub fn start(&mut self, group: &str) {
        *self.running.entry(group.to_string()).or_insert(0) += 1;
    }

    /// One member of `group` finished.
    pub fn finish(&mut self, group: &str) {
        if let Some(n) = self.running.get_mut(group) {
            *n = n.saturating_sub(1);
        }
    }

    pub fn running(&self, group: &str) -> u32 {
        self.running.get(group).copied().unwrap_or(0)
    }
}

/// The concurrency group an instance counts in, and its limit: only when both are set.
pub(crate) fn group_of(instance: &Instance) -> Option<(&str, u32)> {
    match (&instance.concurrency_group, instance.concurrency) {
        (Some(group), Some(limit)) => Some((group.as_str(), limit)),
        _ => None,
    }
}

/// A need is met by a result, or by an instance that is absent (or not listed at all).
fn need_met(id: &str, expansion: &Expansion, results: &IndexMap<String, NodeResult>) -> bool {
    results.contains_key(id)
        || expansion
            .instances
            .get(id)
            .is_none_or(|instance| instance.state == State::Absent)
}

/// A read is met by a result, or by an instance that will never give one: absent, blocked or
/// failed (or not listed at all).
fn read_met(id: &str, expansion: &Expansion, results: &IndexMap<String, NodeResult>) -> bool {
    results.contains_key(id)
        || expansion.instances.get(id).is_none_or(|instance| {
            matches!(
                instance.state,
                State::Absent | State::Blocked | State::Failed
            )
        })
}

/// The ids an instance still waits on: the refs of its pending with-values that have no result,
/// the needs and the reads that are not met; never its own id.
fn waits(
    instance: &Instance,
    expansion: &Expansion,
    results: &IndexMap<String, NodeResult>,
) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = instance
        .waiting_on()
        .into_iter()
        .filter(|id| !results.contains_key(id))
        .collect();
    ids.extend(
        instance
            .needs
            .iter()
            .filter(|id| !need_met(id, expansion, results))
            .cloned(),
    );
    ids.extend(
        instance
            .reads
            .iter()
            .filter(|id| !read_met(id, expansion, results))
            .cloned(),
    );
    ids.remove(&instance.id);
    ids
}

/// Ids that may start now, in listing order (module doc).
pub fn ready(
    expansion: &Expansion,
    results: &IndexMap<String, NodeResult>,
    running: &HashSet<String>,
    groups: &Groups,
) -> Vec<String> {
    // The instances chosen in this call take their places too.
    let mut groups = groups.clone();
    let mut ready = Vec::new();
    for instance in expansion.ordered() {
        if instance.state != State::Planned
            || results.contains_key(&instance.id)
            || running.contains(&instance.id)
            || instance.with.values().any(Val::contains_pending)
        {
            continue;
        }
        let own = instance.id.as_str();
        if !instance
            .needs
            .iter()
            .all(|id| id == own || need_met(id, expansion, results))
        {
            continue;
        }
        if !instance
            .reads
            .iter()
            .all(|id| id == own || read_met(id, expansion, results))
        {
            continue;
        }
        if let Some((group, limit)) = group_of(instance) {
            if groups.running(group) >= limit {
                continue;
            }
            groups.start(group);
        }
        ready.push(instance.id.clone());
    }
    ready
}

/// One instance settled without running.
#[derive(Debug, Clone, PartialEq)]
pub struct Settled {
    pub id: String,
    pub result: NodeResult,
    pub event: Event,
}

/// Instances that will never run (module doc).
pub fn settle(
    expansion: &Expansion,
    results: &IndexMap<String, NodeResult>,
    running: &HashSet<String>,
) -> Vec<Settled> {
    expansion
        .ordered()
        .filter(|i| !results.contains_key(&i.id) && !running.contains(&i.id))
        .filter_map(|instance| match instance.state {
            State::Blocked => {
                let reason = instance
                    .reason
                    .clone()
                    .unwrap_or_else(|| "something it reads failed".to_string());
                Some(Settled {
                    id: instance.id.clone(),
                    result: crate::executor::failed(&reason),
                    event: Event::NodeSkipped {
                        id: instance.id.clone(),
                        path: instance.path.clone(),
                        reason: Some(reason),
                        blocked: true,
                        error: None,
                        facts: None,
                        duration_ms: None,
                    },
                })
            }
            State::Failed => {
                let error = instance
                    .reason
                    .clone()
                    .unwrap_or_else(|| "an assertion failed".to_string());
                Some(Settled {
                    id: instance.id.clone(),
                    result: crate::executor::failed(&error),
                    event: Event::NodeFailed {
                        id: instance.id.clone(),
                        path: instance.path.clone(),
                        error: Some(error),
                        facts: None,
                        duration_ms: None,
                    },
                })
            }
            _ => None,
        })
        .collect()
}

/// The phase gate of one invocation (module doc).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Gate {
    approved: BTreeSet<u32>,
    /// What each phase was last announced with: `(steps, high)`.
    announced: BTreeMap<u32, (usize, Usd)>,
}

/// What the gate decided this round.
#[derive(Debug, Clone, PartialEq)]
pub struct GateCheck {
    /// `phase_planned` events to emit, in order.
    pub planned: Vec<Event>,
    /// The stop message, when a phase is not approved.
    pub stop: Option<String>,
}

impl Gate {
    pub fn new() -> Gate {
        Gate::default()
    }

    /// Checks the phases (module doc). `charged` and `held` are the ledger's.
    pub fn check(
        &mut self,
        expansion: &Expansion,
        results: &IndexMap<String, NodeResult>,
        charged: Usd,
        held: Usd,
        yes_up_to: Option<Usd>,
    ) -> GateCheck {
        let phases: BTreeSet<u32> = expansion
            .ordered()
            .filter(|i| i.state.is_live() && !results.contains_key(&i.id))
            .map(|i| i.phase)
            .filter(|phase| !self.approved.contains(phase))
            .collect();
        let mut planned = Vec::new();
        for phase in phases {
            let members: Vec<&Instance> = expansion
                .ordered()
                .filter(|i| i.state.is_live() && i.phase == phase && !results.contains_key(&i.id))
                .collect();
            let unpriceable = members
                .iter()
                .any(|i| i.with.values().any(Val::contains_pending) && i.waiting_on().is_empty());
            if unpriceable {
                continue;
            }
            let high = members.iter().map(|i| i.high()).sum::<Usd>()
                + expansion
                    .pending
                    .iter()
                    .filter(|repeat| repeat.phase == phase)
                    .map(|repeat| repeat.high())
                    .sum();
            let steps = members.len();
            if self.announced.get(&phase) != Some(&(steps, high)) {
                self.announced.insert(phase, (steps, high));
                planned.push(Event::PhasePlanned {
                    phase,
                    steps,
                    high_usd: high,
                });
            }
            if let Some(limit) = yes_up_to
                && phase > 1
                && charged + held + high > limit
            {
                let stop = format!(
                    "phase {phase} may cost up to {}, which takes the run past --yes-up-to \
                     {limit}; approve it with a higher --yes-up-to",
                    high.dollars_2()
                );
                return GateCheck {
                    planned,
                    stop: Some(stop),
                };
            }
            self.approved.insert(phase);
        }
        GateCheck {
            planned,
            stop: None,
        }
    }
}

/// Planned instances that never ran: `(id, ids it waits on)` (module doc).
pub fn stuck(
    expansion: &Expansion,
    results: &IndexMap<String, NodeResult>,
) -> Vec<(String, Vec<String>)> {
    expansion
        .ordered()
        .filter(|i| i.state == State::Planned && !results.contains_key(&i.id))
        .map(|i| {
            (
                i.id.clone(),
                waits(i, expansion, results).into_iter().collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::expand::{CallPrice, PendingRepeat, ResultStatus};
    use grida_fx_core::registry::{ResolvedType, TypeOrigin};
    use grida_fx_core::spec::{BodyKind, NodeSpec, Retry};
    use grida_fx_core::val::Pending;
    use std::rc::Rc;

    fn usd(text: &str) -> Usd {
        Usd::parse(text).unwrap()
    }

    /// A planned instance of a free project type at take 1, phase 1.
    fn instance(id: &str) -> Instance {
        let spec = NodeSpec {
            name: "shout".into(),
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
            version: Some(1),
            retry: Retry::Service,
        };
        let path = id.split('#').next().unwrap().to_string();
        Instance {
            id: id.into(),
            path: path.clone(),
            step: path,
            takes: vec![1],
            uses: "./nodes/cases.py#shout".into(),
            ty: Rc::new(ResolvedType {
                uses: "./nodes/cases.py#shout".into(),
                identity: "source:0".into(),
                spec: Rc::new(spec),
                origin: TypeOrigin::Project {
                    path: "nodes/cases.py".into(),
                    attribute: "shout".into(),
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
            phase: 1,
            key: None,
            judges: None,
            judge_policy: None,
            judged_by: Vec::new(),
            view: serde_json::Value::Bool(false),
            at_plan: false,
            budget: None,
            concurrency_group: None,
            concurrency: None,
            timeout_s: None,
            reason: None,
            reads: BTreeSet::new(),
        }
    }

    fn with_state(mut i: Instance, state: State) -> Instance {
        i.state = state;
        i
    }

    fn pending_on(mut i: Instance, refs: &[&str]) -> Instance {
        let refs: BTreeSet<String> = refs.iter().map(|r| r.to_string()).collect();
        i.with.insert(
            "text".into(),
            Val::Pending(Box::new(Pending::new(refs, &serde_json::json!("t")))),
        );
        i
    }

    fn priced(mut i: Instance, phase: u32, high: &str) -> Instance {
        i.phase = phase;
        i.prices.push(CallPrice {
            capability: "image.generate".into(),
            route: "img-a@acme".into(),
            calls: 1,
            low: Usd::ZERO,
            high: usd(high),
        });
        i
    }

    fn expansion(instances: Vec<Instance>) -> Expansion {
        Expansion {
            instances: instances.into_iter().map(|i| (i.id.clone(), i)).collect(),
            ..Expansion::default()
        }
    }

    fn done() -> NodeResult {
        NodeResult {
            status: ResultStatus::Succeeded,
            outputs: IndexMap::new(),
            facts: IndexMap::new(),
            error: None,
        }
    }

    fn results(ids: &[&str]) -> IndexMap<String, NodeResult> {
        ids.iter().map(|id| (id.to_string(), done())).collect()
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn ready_in_listing_order_without_results_or_running_ones() {
        let e = expansion(vec![
            instance("c#1"),
            instance("a#1"),
            instance("b#1"),
            with_state(instance("m#1"), State::Maybe),
            with_state(instance("x#1"), State::Absent),
        ]);
        assert_eq!(
            ready(&e, &IndexMap::new(), &none(), &Groups::default()),
            ["c#1", "a#1", "b#1"]
        );
        let running: HashSet<String> = ["a#1".to_string()].into();
        assert_eq!(
            ready(&e, &results(&["c#1"]), &running, &Groups::default()),
            ["b#1"]
        );
    }

    #[test]
    fn a_pending_with_value_holds_a_step() {
        let e = expansion(vec![
            instance("a#1"),
            pending_on(instance("b#1"), &["a#1"]),
            // Pending on nothing at all still holds it.
            pending_on(instance("c#1"), &[]),
        ]);
        assert_eq!(
            ready(&e, &IndexMap::new(), &none(), &Groups::default()),
            ["a#1"]
        );
    }

    #[test]
    fn needs_wait_for_results_but_not_for_absent_takes() {
        let mut needing = instance("join#1");
        needing.needs = vec!["draw#1".into(), "draw#2".into(), "draw#3".into()];
        let e = expansion(vec![
            instance("draw#1"),
            with_state(instance("draw#2"), State::Absent),
            with_state(instance("draw#3"), State::Absent),
            needing.clone(),
        ]);
        // draw#1 has no result yet: the need holds.
        assert_eq!(
            ready(&e, &IndexMap::new(), &none(), &Groups::default()),
            ["draw#1"]
        );
        // Take 1 finished; takes 2 and 3 are absent: the need is met.
        assert_eq!(
            ready(&e, &results(&["draw#1"]), &none(), &Groups::default()),
            ["join#1"]
        );
        // A need on an id the expansion does not list counts as absent.
        let mut lone = instance("lone#1");
        lone.needs = vec!["gone#1".into()];
        let e = expansion(vec![lone]);
        assert_eq!(
            ready(&e, &IndexMap::new(), &none(), &Groups::default()),
            ["lone#1"]
        );
        // A need on a planned instance that is still running holds.
        let e = expansion(vec![instance("draw#1"), needing]);
        let running: HashSet<String> = ["draw#1".to_string()].into();
        assert!(ready(&e, &IndexMap::new(), &running, &Groups::default()).is_empty());
    }

    #[test]
    fn a_planned_maybe_or_blocked_need_holds_but_its_own_id_does_not() {
        let mut needing = instance("b#1");
        needing.needs = vec!["b#1".into(), "m#1".into()];
        let e = expansion(vec![with_state(instance("m#1"), State::Maybe), needing]);
        assert!(ready(&e, &IndexMap::new(), &none(), &Groups::default()).is_empty());
        assert_eq!(
            ready(&e, &results(&["m#1"]), &none(), &Groups::default()),
            ["b#1"]
        );
    }

    #[test]
    fn reads_hold_a_step_until_they_settle() {
        // `gated` asserts on `source`'s result: it reads it without a pending with-value.
        let mut gated = instance("gated#1");
        gated.reads = ["source#1".to_string(), "gated#1".to_string()].into();
        let e = expansion(vec![instance("source#1"), gated.clone()]);
        assert_eq!(
            ready(&e, &IndexMap::new(), &none(), &Groups::default()),
            ["source#1"]
        );
        let running: HashSet<String> = ["source#1".to_string()].into();
        assert!(ready(&e, &IndexMap::new(), &running, &Groups::default()).is_empty());
        assert_eq!(
            ready(&e, &results(&["source#1"]), &none(), &Groups::default()),
            ["gated#1"]
        );
        // Absent, blocked and failed reads never give a result: they do not hold it.
        for state in [State::Absent, State::Blocked, State::Failed] {
            let e = expansion(vec![with_state(instance("source#1"), state), gated.clone()]);
            assert_eq!(
                ready(&e, &IndexMap::new(), &none(), &Groups::default()),
                ["gated#1"],
                "{state:?}"
            );
        }
        // A maybe read may still run.
        let e = expansion(vec![with_state(instance("source#1"), State::Maybe), gated]);
        assert!(ready(&e, &IndexMap::new(), &none(), &Groups::default()).is_empty());
    }

    #[test]
    fn concurrency_groups_count_running_and_chosen_members() {
        let member = |id: &str| {
            let mut i = instance(id);
            i.concurrency_group = Some("draw".into());
            i.concurrency = Some(2);
            i
        };
        let mut ungrouped = instance("solo#1");
        // A group without a limit does not count.
        ungrouped.concurrency_group = Some("draw".into());
        let e = expansion(vec![
            member("draw['a']#1"),
            member("draw['b']#1"),
            member("draw['c']#1"),
            ungrouped,
        ]);
        let mut groups = Groups::default();
        let first = ready(&e, &IndexMap::new(), &none(), &groups);
        assert_eq!(first, ["draw['a']#1", "draw['b']#1", "solo#1"]);
        // The caller's groups are not changed by `ready`.
        assert_eq!(groups.running("draw"), 0);
        groups.start("draw");
        groups.start("draw");
        let running: HashSet<String> =
            ["draw['a']#1".to_string(), "draw['b']#1".to_string()].into();
        assert!(
            ready(&e, &IndexMap::new(), &running, &groups)
                .iter()
                .all(|id| id == "solo#1")
        );
        groups.finish("draw");
        let running: HashSet<String> = ["draw['b']#1".to_string()].into();
        assert_eq!(
            ready(&e, &results(&["draw['a']#1", "solo#1"]), &running, &groups),
            ["draw['c']#1"]
        );
        groups.finish("draw");
        groups.finish("draw");
        assert_eq!(groups.running("draw"), 0);
        assert_eq!(groups.running("other"), 0);
    }

    #[test]
    fn settle_blocked_and_failed_instances() {
        let mut blocked = with_state(instance("b#1"), State::Blocked);
        blocked.reason = Some("something it reads failed".into());
        let unexplained = with_state(instance("u#1"), State::Blocked);
        let mut asserted = with_state(instance("x['k']#1"), State::Failed);
        asserted.reason = Some("x: the image is too small".into());
        let bare = with_state(instance("y#1"), State::Failed);
        let e = expansion(vec![
            instance("p#1"),
            blocked,
            unexplained,
            asserted,
            bare,
            with_state(instance("r#1"), State::Blocked),
            with_state(instance("q#1"), State::Failed),
            with_state(instance("a#1"), State::Absent),
        ]);
        // r#1 is running, q#1 already has a result (a failed run leaves state failed).
        let running: HashSet<String> = ["r#1".to_string()].into();
        let settled = settle(&e, &results(&["q#1"]), &running);
        let ids: Vec<&str> = settled.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["b#1", "u#1", "x['k']#1", "y#1"]);
        assert_eq!(
            settled[0].event,
            Event::NodeSkipped {
                id: "b#1".into(),
                path: "b".into(),
                reason: Some("something it reads failed".into()),
                blocked: true,
                error: None,
                facts: None,
                duration_ms: None,
            }
        );
        assert_eq!(settled[0].result.status, ResultStatus::Failed);
        assert_eq!(
            settled[1].result.error.as_deref(),
            Some("something it reads failed")
        );
        assert_eq!(
            settled[2].event,
            Event::NodeFailed {
                id: "x['k']#1".into(),
                path: "x['k']".into(),
                error: Some("x: the image is too small".into()),
                facts: None,
                duration_ms: None,
            }
        );
        assert_eq!(
            settled[2].result,
            NodeResult {
                status: ResultStatus::Failed,
                outputs: IndexMap::new(),
                facts: IndexMap::new(),
                error: Some("x: the image is too small".into()),
            }
        );
        assert_eq!(
            settled[3].result.error.as_deref(),
            Some("an assertion failed")
        );
    }

    #[test]
    fn the_gate_announces_each_phase_once_and_approves_without_a_limit() {
        let e = expansion(vec![
            priced(instance("a#1"), 1, "0.04"),
            priced(instance("b#1"), 2, "0.5"),
            priced(instance("c#1"), 2, "0.25"),
            with_state(priced(instance("d#1"), 3, "9"), State::Absent),
        ]);
        let mut gate = Gate::new();
        let check = gate.check(&e, &IndexMap::new(), Usd::ZERO, Usd::ZERO, None);
        assert_eq!(check.stop, None);
        assert_eq!(
            check.planned,
            [
                Event::PhasePlanned {
                    phase: 1,
                    steps: 1,
                    high_usd: usd("0.04"),
                },
                Event::PhasePlanned {
                    phase: 2,
                    steps: 2,
                    high_usd: usd("0.75"),
                },
            ]
        );
        // Approved phases are not announced again in this invocation.
        let again = gate.check(&e, &results(&["a#1"]), Usd::ZERO, Usd::ZERO, None);
        assert_eq!(again.planned, []);
        // A new invocation announces the phases that still have unfinished members.
        let check = Gate::new().check(&e, &results(&["a#1"]), Usd::ZERO, Usd::ZERO, None);
        assert_eq!(
            check.planned,
            [Event::PhasePlanned {
                phase: 2,
                steps: 2,
                high_usd: usd("0.75"),
            }]
        );
        // Nothing live is left: nothing is announced.
        let finished = expansion(vec![with_state(instance("a#1"), State::Done)]);
        let check = Gate::new().check(&finished, &results(&["a#1"]), Usd::ZERO, Usd::ZERO, None);
        assert_eq!(
            check,
            GateCheck {
                planned: Vec::new(),
                stop: None
            }
        );
    }

    #[test]
    fn yes_up_to_stops_a_later_phase_never_the_first() {
        // Phase 1 alone is past the limit: it is never stopped.
        let e = expansion(vec![
            priced(instance("a#1"), 1, "5"),
            priced(instance("b#1"), 2, "1.5"),
        ]);
        let mut gate = Gate::new();
        let check = gate.check(&e, &IndexMap::new(), Usd::ZERO, Usd::ZERO, Some(usd("1")));
        assert_eq!(check.planned.len(), 2);
        assert_eq!(
            check.stop.as_deref(),
            Some(
                "phase 2 may cost up to $1.50, which takes the run past --yes-up-to 1; approve \
                 it with a higher --yes-up-to"
            )
        );
        // Phase 1 stays approved; phase 2 is checked again, but not announced again unchanged.
        let check = gate.check(&e, &IndexMap::new(), Usd::ZERO, Usd::ZERO, Some(usd("1")));
        assert_eq!(check.planned, []);
        assert!(check.stop.is_some());
        // Charged and held money counts: a phase that fits alone can still be stopped.
        let e = expansion(vec![priced(instance("b#1"), 2, "0.5")]);
        let mut gate = Gate::new();
        let fits = gate.clone().check(
            &e,
            &IndexMap::new(),
            usd("0.25"),
            usd("0.25"),
            Some(usd("1")),
        );
        assert_eq!(fits.stop, None);
        let check = gate.check(
            &e,
            &IndexMap::new(),
            usd("0.5"),
            usd("0.25"),
            Some(usd("1.2")),
        );
        assert_eq!(
            check.stop.as_deref(),
            Some(
                "phase 2 may cost up to $0.50, which takes the run past --yes-up-to 1.2; \
                 approve it with a higher --yes-up-to"
            )
        );
        // Once held money settles for less, the same phase fits and is approved without being
        // announced again.
        let check = gate.check(
            &e,
            &IndexMap::new(),
            usd("0.6"),
            Usd::ZERO,
            Some(usd("1.2")),
        );
        assert_eq!(
            check,
            GateCheck {
                planned: Vec::new(),
                stop: None
            }
        );
        let check = gate.check(&e, &IndexMap::new(), usd("5"), Usd::ZERO, Some(usd("1.2")));
        assert_eq!(check.stop, None, "approved phases stay approved");
    }

    #[test]
    fn the_gate_waits_for_unpriceable_phases_and_counts_pending_repeats() {
        let mut e = expansion(vec![
            priced(instance("a#1"), 1, "0.1"),
            // Waits on nothing and is pending: not yet priceable.
            pending_on(priced(instance("b#1"), 2, "0.2"), &[]),
            // Waits on a#1: priceable.
            pending_on(priced(instance("c#1"), 3, "0.3"), &["a#1"]),
            with_state(priced(instance("m#1"), 3, "0.05"), State::Maybe),
        ]);
        e.pending.push(PendingRepeat {
            path: "more".into(),
            max: 4,
            waiting_on: ["a#1".to_string()].into(),
            per_instance_low: Usd::ZERO,
            per_instance_high: usd("0.25"),
            phase: 3,
        });
        let mut gate = Gate::new();
        let check = gate.check(&e, &IndexMap::new(), Usd::ZERO, Usd::ZERO, Some(usd("2")));
        assert_eq!(
            check.planned,
            [
                Event::PhasePlanned {
                    phase: 1,
                    steps: 1,
                    high_usd: usd("0.1"),
                },
                Event::PhasePlanned {
                    phase: 3,
                    steps: 2,
                    high_usd: usd("1.35"),
                },
            ]
        );
        assert_eq!(check.stop, None);
        // Phase 2 is announced once it can be priced.
        e.instances
            .insert("b#1".into(), priced(instance("b#1"), 2, "0.2"));
        let check = gate.check(&e, &results(&["a#1"]), Usd::ZERO, Usd::ZERO, Some(usd("2")));
        assert_eq!(
            check.planned,
            [Event::PhasePlanned {
                phase: 2,
                steps: 1,
                high_usd: usd("0.2"),
            }]
        );
    }

    #[test]
    fn stuck_names_what_each_planned_instance_waits_on() {
        let mut needing = instance("join#1");
        needing.needs = vec!["never#1".into(), "join#1".into(), "gone#1".into()];
        let mut reading = instance("check#1");
        reading.reads = ["join#1".to_string(), "done#1".to_string()].into();
        let e = expansion(vec![
            with_state(instance("never#1"), State::Maybe),
            instance("done#1"),
            needing,
            pending_on(reading, &["later#1", "done#1"]),
            pending_on(instance("lost#1"), &[]),
            with_state(instance("a#1"), State::Absent),
        ]);
        assert_eq!(
            stuck(&e, &results(&["done#1"])),
            [
                ("join#1".to_string(), vec!["never#1".to_string()]),
                (
                    "check#1".to_string(),
                    vec!["join#1".to_string(), "later#1".to_string()]
                ),
                ("lost#1".to_string(), Vec::new()),
            ]
        );
        let all = expansion(vec![with_state(instance("done#1"), State::Done)]);
        assert!(stuck(&all, &results(&["done#1"])).is_empty());
    }
}
