//! `plan`, `expand`, `identity`, `price`.
//!
//! Each: build a [`grida_fx_core::project::PlanRequest`] from the arguments (cwd = the process's
//! working directory, canonical; `--max-usd` through `Usd::parse`), make the planner with a lazy
//! Python host for the home project, make the plan with no plan-time runner (step 2) and
//! `NoCache`, then print:
//! - `plan`: [`grida_fx_core::plan::render::render`] + `\n`, or with `--json` the graph; exit 1
//!   with `--check` and problems; with `--expect-cached`, `not cached: <ids, ", " or "pending
//!   repeats">` and exit 1 when anything live is uncached, unknown or pending;
//! - `expand`: the graph document; `identity`: the identity document; `price`: the price
//!   document; each exits 1 when the plan has problems (the document is still printed).

use crate::cli::PlanArgs;
use crate::print::{print_json, print_line};
use grida_fx_core::Error;
use grida_fx_core::host::NoCache;
use grida_fx_core::money::Usd;
use grida_fx_core::plan::{Plan, make_plan, output, render};
use grida_fx_core::project::{PlanRequest, make_planner};
use std::path::PathBuf;

/// Which planning verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanVerb {
    Plan,
    Expand,
    Identity,
    Price,
}

/// Runs a planning verb.
pub fn run(verb: PlanVerb, args: &PlanArgs) -> Result<u8, Error> {
    let request = request(args)?;
    let mut host = crate::print::host();
    let mut planner = make_planner(&request, &mut host)?;
    let plan = make_plan(&mut planner, &mut host, None, &NoCache)?;
    let refused = u8::from(!plan.ok());
    Ok(match verb {
        PlanVerb::Plan => {
            if args.json {
                print_json(&output::graph_document(&plan, &planner));
            } else {
                print_line(&render::render(&plan, &planner));
            }
            let mut status = if args.check { refused } else { 0 };
            if args.expect_cached {
                if let Some(line) = not_cached(&plan) {
                    print_line(&line);
                    status = 1;
                }
            }
            status
        }
        PlanVerb::Expand => {
            print_json(&output::graph_document(&plan, &planner));
            refused
        }
        PlanVerb::Identity => {
            print_json(&output::identity_document(&plan));
            refused
        }
        PlanVerb::Price => {
            print_json(&output::price_document(&plan));
            refused
        }
    })
}

/// The request of a planning verb's arguments.
fn request(args: &PlanArgs) -> Result<PlanRequest, Error> {
    let max_usd = args.max_usd.as_deref().map(max_usd).transpose()?;
    let arguments = crate::args::parse_arguments(&args.arg)?;
    Ok(PlanRequest {
        target: args.target.clone(),
        cwd: working_directory()?,
        input_files: args.inputs.clone(),
        rest: args.rest.clone(),
        arguments,
        routes: args.routes.clone(),
        max_usd,
    })
}

/// `--max-usd`, read with the money rules (identity.md §12): a usage error when it is not an
/// amount.
fn max_usd(text: &str) -> Result<Usd, Error> {
    Usd::parse(text).map_err(|reason| Error::usage(format!("--max-usd {text}: {reason}")))
}

/// The process's working directory, absolute with symbolic links resolved.
pub(crate) fn working_directory() -> Result<PathBuf, Error> {
    std::env::current_dir()
        .and_then(|cwd| cwd.canonicalize())
        .map_err(|error| Error::io("the working directory", &error))
}

/// `plan --expect-cached`: `not cached: <ids>` when a live instance with a known identity is not
/// cached (first), a live instance's identity is unknown (then), or, with neither, when a repeat
/// is still pending (`not cached: pending repeats`). `None` when everything is cached.
fn not_cached(plan: &Plan) -> Option<String> {
    let live: Vec<_> = plan.live().collect();
    let mut ids: Vec<&str> = live
        .iter()
        .filter(|i| i.identity.is_some() && !plan.cached.contains(&i.id))
        .map(|i| i.id.as_str())
        .collect();
    ids.extend(
        live.iter()
            .filter(|i| i.identity.is_none())
            .map(|i| i.id.as_str()),
    );
    if ids.is_empty() && plan.expansion.pending.is_empty() {
        return None;
    }
    let named = if ids.is_empty() {
        "pending repeats".to_string()
    } else {
        ids.join(", ")
    };
    Some(format!("not cached: {named}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::expand::{Expansion, Instance, PendingRepeat, State};
    use grida_fx_core::registry::{ResolvedType, TypeOrigin};
    use grida_fx_core::spec::{BodyKind, NodeSpec, Retry};
    use indexmap::IndexMap;
    use std::collections::BTreeSet;
    use std::rc::Rc;

    fn instance(id: &str, state: State, identity: Option<&str>) -> Instance {
        let spec = NodeSpec {
            name: "files.copy".into(),
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
        Instance {
            id: id.into(),
            path: id.split('#').next().unwrap().into(),
            step: id.split('#').next().unwrap().into(),
            takes: vec![1],
            uses: "fx/files.copy@1".into(),
            ty: Rc::new(ResolvedType {
                uses: "fx/files.copy@1".into(),
                identity: "fx/files.copy@1.1".into(),
                spec: Rc::new(spec),
                origin: TypeOrigin::Builtin {
                    name: "files.copy".into(),
                    major: 1,
                },
                body: BodyKind::Python,
                source: None,
                drift: None,
            }),
            with: IndexMap::new(),
            needs: Vec::new(),
            state,
            identity: identity.map(str::to_string),
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

    fn plan(instances: Vec<Instance>, cached: &[&str], pending: bool) -> Plan {
        let mut expansion = Expansion::default();
        for i in instances {
            expansion.instances.insert(i.id.clone(), i);
        }
        if pending {
            expansion.pending.push(PendingRepeat {
                path: "draw".into(),
                max: 6,
                waiting_on: BTreeSet::new(),
                per_instance_low: Usd(0),
                per_instance_high: Usd(0),
                phase: 2,
            });
        }
        Plan {
            expansion,
            problems: Vec::new(),
            ceiling: None,
            cached: cached.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn uncached_ids_then_unknown_ones() {
        let digest = "a".repeat(64);
        let plan = plan(
            vec![
                instance("late#1", State::Planned, None),
                instance("count#1", State::Planned, Some(&digest)),
                instance("done#1", State::Done, Some(&digest)),
                instance("gone#1", State::Absent, Some(&digest)),
                instance("cached#1", State::Maybe, Some(&digest)),
                instance("draw#1", State::Maybe, Some(&digest)),
            ],
            &["cached#1"],
            true,
        );
        assert_eq!(
            not_cached(&plan).as_deref(),
            Some("not cached: count#1, draw#1, late#1")
        );
    }

    #[test]
    fn pending_repeats_alone() {
        let digest = "b".repeat(64);
        let plan = plan(
            vec![instance("count#1", State::Planned, Some(&digest))],
            &["count#1"],
            true,
        );
        assert_eq!(
            not_cached(&plan).as_deref(),
            Some("not cached: pending repeats")
        );
    }

    #[test]
    fn everything_cached() {
        let digest = "c".repeat(64);
        let plan = plan(
            vec![
                instance("count#1", State::Planned, Some(&digest)),
                instance("blocked#1", State::Blocked, None),
            ],
            &["count#1"],
            false,
        );
        assert_eq!(not_cached(&plan), None);
    }

    #[test]
    fn a_bad_ceiling_is_a_usage_error() {
        let error = max_usd("nan").unwrap_err();
        assert_eq!(error.kind, grida_fx_core::ErrorKind::Usage);
        assert!(error.message.starts_with("--max-usd nan: "), "{error}");
    }
}
