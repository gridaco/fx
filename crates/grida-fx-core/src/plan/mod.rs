//! Making a plan around expansion.
//!
//! `make_plan` expands, runs every ready `at: plan` instance (planned, nothing pending in its
//! with-values, a free type, no result yet) through the [`PlanTimeRunner`] in listing order, and
//! expands again until none is ready. Without a runner (step 2), each ready at-plan instance
//! gives the problem `<step>: running an at: plan step is not available until the runner lands`
//! and the loop stops. Problems, in order: the expansion's; those "not available" ones;
//! `<step>: failed while planning: <error>` per failed at-plan result; `<step>: an at: plan step
//! reads something only a run produces` per at-plan instance still planned without a result (only
//! with a runner); then deduplicated by `(where, message)`, the first kept
//! ([`crate::error::unique`]). `<step>` is the declaration path, without repeat keys. The
//! ceiling is `--max-usd`, else the workflow's `budget.max_usd`, else the planning project's.
//! `cached`: live instances with a known identity that the [`ResultCache`] holds.
//!
//! Money is exact micro-dollars ([`Usd`], identity.md §12):
//! - [`Plan::estimate`]: low sums the planned live instances that are not cached; high sums every
//!   live instance that is not cached, plus each pending repeat's `max × per-instance high`.
//!   Done instances never count; `maybe` instances count towards high only.
//! - [`Plan::phases`]: one summary per phase number in live phases ∪ pending phases ∪ {1}. Steps
//!   are the live members (cached and free ones included); calls and money count the members
//!   that have prices and are not cached (low: planned ones only; high: all of them, plus the
//!   phase's pending repeats).

pub mod output;
pub mod render;

use crate::error::{Problem, Result, unique};
use crate::expand::{ExpandEnv, Expansion, Instance, ResultStatus, State};
use crate::host::{NodeHost, PlanTimeRunner, ResultCache};
use crate::money::Usd;
use crate::project::Planner;
use std::collections::{BTreeSet, HashSet};

/// The problem of an at-plan instance while no plan-time runner exists.
pub const AT_PLAN_UNAVAILABLE: &str =
    "running an at: plan step is not available until the runner lands";

/// The plan's price range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Estimate {
    pub low: Usd,
    pub high: Usd,
}

/// One phase's summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSummary {
    pub phase: u32,
    /// Live members, cached and free ones included.
    pub steps: usize,
    pub calls_low: u64,
    pub calls_high: u64,
    pub low: Usd,
    pub high: Usd,
    /// `"<path> (up to <max>)"` per pending repeat of this phase.
    pub pending: Vec<String>,
}

/// A plan: an expansion with its problems, ceiling and cache state.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub expansion: Expansion,
    /// Deduplicated, in order.
    pub problems: Vec<Problem>,
    pub ceiling: Option<Usd>,
    /// Ids of live instances whose identity the store holds.
    pub cached: BTreeSet<String>,
}

impl Plan {
    /// No problems.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// Instances in state planned or maybe, in listing order.
    pub fn live(&self) -> impl Iterator<Item = &Instance> {
        self.expansion.ordered().filter(|i| i.state.is_live())
    }

    /// The price range (module doc).
    pub fn estimate(&self) -> Estimate {
        let uncached = || self.live().filter(|i| !self.cached.contains(&i.id));
        let low = uncached()
            .filter(|i| i.state == State::Planned)
            .map(Instance::low)
            .sum();
        let high = uncached().map(Instance::high).sum::<Usd>()
            + self
                .expansion
                .pending
                .iter()
                .map(|repeat| repeat.high())
                .sum();
        Estimate { low, high }
    }

    /// The summary per phase (module doc): phase numbers are live phases ∪ pending phases ∪ {1}.
    pub fn phases(&self) -> Vec<PhaseSummary> {
        let mut numbers: BTreeSet<u32> = self.live().map(|i| i.phase).collect();
        numbers.extend(self.expansion.pending.iter().map(|r| r.phase));
        numbers.insert(1);
        numbers
            .into_iter()
            .map(|number| {
                let members: Vec<&Instance> = self.live().filter(|i| i.phase == number).collect();
                let paid: Vec<&Instance> = members
                    .iter()
                    .copied()
                    .filter(|i| !i.prices.is_empty() && !self.cached.contains(&i.id))
                    .collect();
                let planned = || paid.iter().filter(|i| i.state == State::Planned);
                let calls =
                    |i: &&Instance| -> u64 { i.prices.iter().map(|p| u64::from(p.calls)).sum() };
                let repeats: Vec<_> = self
                    .expansion
                    .pending
                    .iter()
                    .filter(|r| r.phase == number)
                    .collect();
                PhaseSummary {
                    phase: number,
                    steps: members.len(),
                    calls_low: planned().map(calls).sum(),
                    calls_high: paid.iter().map(calls).sum(),
                    low: planned().map(|i| i.low()).sum(),
                    high: paid.iter().map(|i| i.high()).sum::<Usd>()
                        + repeats.iter().map(|r| r.high()).sum(),
                    pending: repeats
                        .iter()
                        .map(|r| format!("{} (up to {})", r.path, r.max))
                        .collect(),
                }
            })
            .collect()
    }

    /// Instances with a known identity that are not absent (the `cached … of K known steps` K).
    pub fn known(&self) -> usize {
        self.expansion
            .ordered()
            .filter(|i| i.identity.is_some() && i.state != State::Absent)
            .count()
    }

    /// One note per takes-file entry (sorted) that names no instance path: `the takes file names
    /// {step}, which no step is any more; move it with grida-fx takes mv {id} "{step}" <new path>`.
    pub fn warnings(&self, planner: &Planner) -> Vec<String> {
        let paths: HashSet<&str> = self.expansion.ordered().map(|i| i.path.as_str()).collect();
        let mut steps: Vec<&String> = planner.takes.keys().collect();
        steps.sort();
        steps
            .into_iter()
            .filter(|step| !paths.contains(step.as_str()))
            .map(|step| {
                format!(
                    "the takes file names {step}, which no step is any more; move it with \
                     grida-fx takes mv {} \"{step}\" <new path>",
                    planner.workflow.workflow.id
                )
            })
            .collect()
    }
}

/// Whether an at-plan instance can run now: planned, nothing pending in its with-values, a free
/// type (a paid one is refused by expansion and never runs), and no result yet.
fn ready(instance: &Instance, planner: &Planner) -> bool {
    instance.at_plan
        && instance.state == State::Planned
        && !instance.with.values().any(|v| v.contains_pending())
        && !instance.ty.spec.paid()
        && !planner.results.contains_key(&instance.id)
}

/// Makes the plan (module doc).
pub fn make_plan(
    planner: &mut Planner,
    host: &mut dyn NodeHost,
    mut runner: Option<&mut dyn PlanTimeRunner>,
    cache: &dyn ResultCache,
) -> Result<Plan> {
    let mut unavailable = Vec::new();
    let expansion = loop {
        let expansion = crate::expand::expand(ExpandEnv {
            workflow: &planner.workflow,
            inputs: &planner.inputs.values,
            registry: &mut planner.registry,
            host: &mut *host,
            routes: &planner.routes,
            takes: &planner.takes,
            results: &planner.results,
        })?;
        let runnable: Vec<Instance> = expansion
            .ordered()
            .filter(|i| ready(i, planner))
            .cloned()
            .collect();
        if runnable.is_empty() {
            break expansion;
        }
        let Some(runner) = runner.as_deref_mut() else {
            unavailable.extend(
                runnable
                    .iter()
                    .map(|i| Problem::new(i.step.clone(), AT_PLAN_UNAVAILABLE)),
            );
            break expansion;
        };
        for instance in &runnable {
            let result = runner.run(instance, &*planner)?;
            planner.results.insert(instance.id.clone(), result);
        }
    };
    let mut problems = expansion.problems.clone();
    problems.extend(unavailable);
    for instance in expansion.ordered().filter(|i| i.at_plan) {
        if let Some(result) = planner.results.get(&instance.id) {
            if result.status == ResultStatus::Failed {
                problems.push(Problem::new(
                    instance.step.clone(),
                    format!(
                        "failed while planning: {}",
                        result.error.as_deref().unwrap_or_default()
                    ),
                ));
            }
        }
    }
    if runner.is_some() {
        problems.extend(
            expansion
                .ordered()
                .filter(|i| {
                    i.at_plan
                        && i.state == State::Planned
                        && !i.ty.spec.paid()
                        && !planner.results.contains_key(&i.id)
                })
                .map(|i| {
                    Problem::new(
                        i.step.clone(),
                        "an at: plan step reads something only a run produces",
                    )
                }),
        );
    }
    let ceiling = planner
        .max_usd
        .or_else(|| planner.workflow.workflow.budget.map(|b| b.max_usd))
        .or_else(|| planner.project.document.budget.map(|b| b.max_usd));
    let cached = expansion
        .ordered()
        .filter(|i| i.state.is_live())
        .filter_map(|i| {
            let identity = i.identity.as_deref()?;
            cache.has_result(identity).then(|| i.id.clone())
        })
        .collect();
    Ok(Plan {
        expansion,
        problems: unique(problems),
        ceiling,
        cached,
    })
}
