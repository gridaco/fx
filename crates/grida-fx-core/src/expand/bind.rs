//! Route binding and per-step price (spec/identity.md §7, §12).
//!
//! `capability_calls` of the with-values; no calls: `{spec.name} makes no paid call` at
//! `<w>.route` when `route:` is set, `{spec.name} makes no paid call to check` at `<w>.requires`.
//! Per capability: the step's `route:` when the type calls exactly one capability (else it is
//! ignored), else fx.yaml's default; none: `no route for {cap}: set route: on the step or
//! routes.{cap} in fx.yaml`; [`crate::routes::RouteTable::resolve`] errors at `<w>.route`;
//! missing features `{route} does not support {a, b}` at `<w>.requires` (route still bound);
//! price `cost × count`. `independent_of`: `no step {name}`, and `shares the model {models}
//! with {name}; route one of them to a different model`, comparing underlying models of the other
//! step's direct instances.

use super::frame::FrameId;
use super::{CallPrice, Expander};
use crate::docs::workflow::Step;
use crate::routes::Route;
use crate::spec::NodeSpec;
use crate::val::Val;
use indexmap::IndexMap;
use std::collections::BTreeSet;

impl Expander<'_> {
    /// Binds routes and prices an instance's calls.
    pub(crate) fn bind_routes(
        &mut self,
        declared: &Step,
        where_: &str,
        spec: &NodeSpec,
        with: &IndexMap<String, Val>,
    ) -> (IndexMap<String, Route>, Vec<CallPrice>) {
        let mut routes = IndexMap::new();
        let mut prices = Vec::new();
        let calls = spec.capability_calls(with);
        if calls.is_empty() {
            if declared.route.is_some() {
                self.problem(
                    &format!("{where_}.route"),
                    format!("{} makes no paid call", spec.name),
                );
            }
            if !declared.requires.is_empty() {
                self.problem(
                    &format!("{where_}.requires"),
                    format!("{} makes no paid call to check", spec.name),
                );
            }
            return (routes, prices);
        }
        let requires: BTreeSet<&str> = declared.requires.iter().map(String::as_str).collect();
        for (capability, &count) in &calls {
            // `route:` names the route of a type that calls one capability; a type that calls
            // several takes each from fx.yaml (gnode ignores `route:` there, silently).
            let chosen = if calls.len() == 1 {
                declared.route.clone().filter(|route| !route.is_empty())
            } else {
                None
            };
            let chosen = chosen.or_else(|| {
                self.env
                    .registry
                    .route_default(capability)
                    .map(str::to_string)
            });
            let Some(chosen) = chosen else {
                self.problem(
                    &format!("{where_}.route"),
                    format!(
                        "no route for {capability}: set route: on the step or \
                         routes.{capability} in fx.yaml"
                    ),
                );
                continue;
            };
            let route = match self.env.routes.resolve(capability, &chosen) {
                Ok(route) => route.clone(),
                Err(message) => {
                    self.problem(&format!("{where_}.route"), message);
                    continue;
                }
            };
            let missing: Vec<&str> = requires
                .iter()
                .copied()
                .filter(|feature| !route.features.contains(*feature))
                .collect();
            if !missing.is_empty() {
                self.problem(
                    &format!("{where_}.requires"),
                    format!("{} does not support {}", route.id(), missing.join(", ")),
                );
            }
            prices.push(call_price(&route, with, count));
            routes.insert(capability.clone(), route);
        }
        (routes, prices)
    }

    /// Checks `independent_of` once per node step per frame; `own` are its instance ids.
    pub(crate) fn independence(
        &mut self,
        frame: FrameId,
        declared: &Step,
        where_: &str,
        own: &[String],
    ) {
        if declared.independent_of.is_empty() {
            return;
        }
        let ours = self.models(own);
        for name in &declared.independent_of {
            let at = format!("{where_}.independent_of");
            let Some(owner) = self.find(frame, name) else {
                self.problem(&at, format!("no step {name}"));
                continue;
            };
            let other = match self.step(owner, name) {
                Ok(other) => other,
                Err(error) => {
                    self.problem(where_, error.0);
                    continue;
                }
            };
            // Only a node step has direct instances: a group, repeat or used workflow never
            // conflicts (gnode's rule, kept).
            let theirs = self.models(&self.exp(other).instances.clone());
            let shared: Vec<String> = ours.intersection(&theirs).cloned().collect();
            if !shared.is_empty() {
                self.problem(
                    &at,
                    format!(
                        "shares the model {} with {name}; route one of them to a different model",
                        shared.join(", ")
                    ),
                );
            }
        }
    }

    /// The underlying models of every route of these instances.
    fn models(&self, ids: &[String]) -> BTreeSet<String> {
        ids.iter()
            .filter_map(|id| self.instances.get(id))
            .flat_map(|instance| instance.routes.values().map(Route::underlying_model))
            .collect()
    }
}

/// The price of `count` calls of a route for these with-values.
pub fn call_price(route: &Route, with: &IndexMap<String, Val>, count: u32) -> CallPrice {
    let (low, high) = route.cost(with);
    CallPrice {
        capability: route.capability.clone(),
        route: route.id(),
        calls: count,
        low: low.times(count as u64),
        high: high.times(count as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::judge::tests::{Fixture, ROOT, spec, step};

    #[test]
    fn a_free_type_takes_no_route() {
        let mut declared = step("./nodes/cases.py#shout");
        declared.route = Some("img-a@acme".into());
        declared.requires = vec!["alpha".into()];
        let mut f = Fixture::new(&[("shout", declared.clone())]);
        let mut ex = f.expander();
        let free = spec("shout", &[], &[], &[], false);
        let (routes, prices) = ex.bind_routes(&declared, "shout", &free, &IndexMap::new());
        assert!(routes.is_empty() && prices.is_empty());
        let problems: Vec<String> = ex.problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            problems,
            vec![
                "shout.route: shout makes no paid call",
                "shout.requires: shout makes no paid call to check",
            ]
        );
    }

    #[test]
    fn independent_of_an_unknown_step() {
        let mut declared = step("./nodes/cases.py#shout");
        declared.independent_of = vec!["nope".into()];
        let mut f = Fixture::new(&[("shout", declared.clone())]);
        let mut ex = f.expander();
        ex.independence(ROOT, &declared, "shout", &[]);
        let problems: Vec<String> = ex.problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(problems, vec!["shout.independent_of: no step nope"]);
    }
}
