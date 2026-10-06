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
//!
//! Feature names (spec/capabilities.md §1 "Features"): a name in `requires:` that is a feature of
//! none of the capabilities the type calls is refused once, before any route is bound, at
//! `<w>.requires`: `{name} is not an image.generate feature (spec/capabilities.md)` (several
//! capabilities: `an image.generate or structured.generate feature`), followed by `; FX calls it
//! {feature}` when the name is the predecessor's for one of their features (spec/providers.md
//! §11). It is then left out of every route's missing features. A type that calls a capability
//! FX does not define (spec/capabilities.md §13, or a project's own) has no vocabulary to check
//! against, so its routes alone decide. The vocabulary is data, `features.json`: each
//! capability's features and the renamed ones; `tools/check_spec.py` and this module's tests hold
//! it to spec/capabilities.md and spec/providers.md §11.

use super::frame::FrameId;
use super::{CallPrice, Expander};
use crate::docs::workflow::Step;
use crate::routes::Route;
use crate::spec::NodeSpec;
use crate::val::Val;
use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The embedded feature vocabulary: `{"kind": "fx-features-v1", "capabilities": {capability:
/// [feature, ...]}, "renamed": {old name: feature}}`, features in spec/capabilities.md's order.
pub const FEATURES_JSON: &str = include_str!("features.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Vocabulary {
    kind: String,
    capabilities: BTreeMap<String, Vec<String>>,
    renamed: BTreeMap<String, String>,
}

fn vocabulary() -> &'static Vocabulary {
    static VOCABULARY: OnceLock<Vocabulary> = OnceLock::new();
    VOCABULARY.get_or_init(|| {
        let vocabulary: Vocabulary =
            serde_json::from_str(FEATURES_JSON).expect("features.json is the feature vocabulary");
        assert_eq!(vocabulary.kind, "fx-features-v1", "features.json kind");
        vocabulary
    })
}

/// The features a route of `capability` may declare (spec/capabilities.md), or `None` for a
/// capability FX does not define (its §13, or a project's own).
pub fn capability_features(capability: &str) -> Option<&'static [String]> {
    vocabulary().capabilities.get(capability).map(Vec::as_slice)
}

/// FX's name for a feature its predecessor named otherwise (spec/providers.md §11).
pub fn renamed_feature(name: &str) -> Option<&'static str> {
    vocabulary().renamed.get(name).map(String::as_str)
}

/// `an image.generate`, `a video.generate or mesh.generate`: the capabilities a feature name was
/// looked up in, with the first one's article.
fn capability_phrase(capabilities: &[&str]) -> String {
    let article = match capabilities.first().and_then(|c| c.chars().next()) {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    };
    let names = match capabilities {
        [] => String::new(),
        [only] => (*only).to_string(),
        [init @ .., last] => format!("{} or {last}", init.join(", ")),
    };
    format!("{article} {names}")
}

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
        let capabilities: Vec<&str> = calls.keys().map(String::as_str).collect();
        let unknown = self.unknown_features(where_, &capabilities, &requires);
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
                .filter(|feature| !unknown.contains(feature) && !route.features.contains(*feature))
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

    /// The names in `requires` that are a feature of none of `capabilities` (spec/capabilities.md),
    /// each refused at `<w>.requires` (module doc). Empty when one of them has no vocabulary.
    fn unknown_features<'a>(
        &mut self,
        where_: &str,
        capabilities: &[&str],
        requires: &BTreeSet<&'a str>,
    ) -> BTreeSet<&'a str> {
        let Some(vocabularies) = capabilities
            .iter()
            .map(|capability| capability_features(capability))
            .collect::<Option<Vec<_>>>()
        else {
            return BTreeSet::new();
        };
        let known = |name: &str| {
            vocabularies
                .iter()
                .any(|features| features.iter().any(|feature| feature == name))
        };
        let unknown: BTreeSet<&str> = requires.iter().copied().filter(|f| !known(f)).collect();
        for name in &unknown {
            let mut message = format!(
                "{name} is not {} feature (spec/capabilities.md)",
                capability_phrase(capabilities)
            );
            if let Some(feature) = renamed_feature(name).filter(|feature| known(feature)) {
                message.push_str(&format!("; FX calls it {feature}"));
            }
            self.problem(&format!("{where_}.requires"), message);
        }
        unknown
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

    /// A paid spec of one capability.
    fn paid(name: &str, capability: &str) -> NodeSpec {
        let mut paid = spec(name, &[], &[], &[], false);
        paid.capability = Some(capability.into());
        paid
    }

    fn routes(entries: serde_json::Value) -> crate::routes::RouteTable {
        crate::routes::RouteTable::from_document(
            &serde_json::json!({"fx": "routes/v1", "routes": entries}),
            "routes.yaml",
        )
        .unwrap()
    }

    #[test]
    fn a_predecessor_feature_name_is_refused_as_unknown_naming_fx_s() {
        let mut declared = step("fx/image.generate@1");
        declared.route = Some("img-a@acme".into());
        declared.requires = vec![
            "transparent_background".into(),
            "masked_edit".into(),
            "reference_images".into(),
            "sparkle".into(),
            "exact_size".into(),
        ];
        let mut f = Fixture::new(&[("draw", declared.clone())]);
        // The route declares the old name too: an unknown name has no meaning, whoever claims it.
        f.routes = routes(serde_json::json!([{
            "capability": "image.generate", "route": "img-a@acme",
            "price": {"low_usd": 0.01, "high_usd": 0.04},
            "features": ["alpha", "transparent_background"],
        }]));
        let mut ex = f.expander();
        let (bound, prices) = ex.bind_routes(
            &declared,
            "draw",
            &paid("image.generate", "image.generate"),
            &IndexMap::new(),
        );
        let problems: Vec<String> = ex.problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            problems,
            vec![
                // masked_edit is image.edit's mask: image.generate has no mask, so no hint.
                "draw.requires: masked_edit is not an image.generate feature \
                 (spec/capabilities.md)",
                "draw.requires: reference_images is not an image.generate feature \
                 (spec/capabilities.md); FX calls it image_input",
                "draw.requires: sparkle is not an image.generate feature (spec/capabilities.md)",
                "draw.requires: transparent_background is not an image.generate feature \
                 (spec/capabilities.md); FX calls it alpha",
                // Only a known feature the route lacks is the route's.
                "draw.requires: img-a@acme does not support exact_size",
            ]
        );
        // The route is still bound and priced.
        assert_eq!(bound["image.generate"].id(), "img-a@acme");
        assert_eq!(prices.len(), 1);
    }

    #[test]
    fn each_alias_names_fx_s_feature_where_the_capability_has_it() {
        for (capability, alias, feature) in [
            ("image.generate", "transparent_background", "alpha"),
            ("image.edit", "transparent_background", "alpha"),
            ("image.edit", "masked_edit", "mask"),
            ("image.edit", "reference_images", "image_input"),
            ("image.edit", "data_url_reference_input", "image_input"),
            ("image.generate", "data_url_reference_input", "image_input"),
            ("structured.generate", "reference_images", "image_input"),
            ("agent.turn", "data_url_reference_input", "image_input"),
        ] {
            let mut declared = step(&format!("fx/{capability}@1"));
            declared.route = Some("m-a@acme".into());
            declared.requires = vec![alias.into()];
            let mut f = Fixture::new(&[("s", declared.clone())]);
            f.routes = routes(serde_json::json!([{
                "capability": capability, "route": "m-a@acme", "price": {"usd": 0.01},
            }]));
            let mut ex = f.expander();
            ex.bind_routes(
                &declared,
                "s",
                &paid(capability, capability),
                &IndexMap::new(),
            );
            let problems: Vec<String> = ex.problems.iter().map(|p| p.to_string()).collect();
            let article = if capability.starts_with(['a', 'i']) {
                "an"
            } else {
                "a"
            };
            assert_eq!(
                problems,
                vec![format!(
                    "s.requires: {alias} is not {article} {capability} feature \
                     (spec/capabilities.md); FX calls it {feature}"
                )],
                "{capability} {alias}"
            );
        }
    }

    #[test]
    fn a_capability_without_a_vocabulary_leaves_features_to_its_route() {
        // vision.review has no section in spec/capabilities.md (its §13): only the route decides.
        let mut declared = step("fx/vision.review@1");
        declared.route = Some("llm-a@other".into());
        declared.requires = vec!["transparent_background".into(), "tool_use".into()];
        let mut f = Fixture::new(&[("review", declared.clone())]);
        f.routes = routes(serde_json::json!([{
            "capability": "vision.review", "route": "llm-a@other", "price": {"usd": 0.01},
            "features": ["tool_use"],
        }]));
        let mut ex = f.expander();
        ex.bind_routes(
            &declared,
            "review",
            &paid("vision.review", "vision.review"),
            &IndexMap::new(),
        );
        let problems: Vec<String> = ex.problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            problems,
            vec!["review.requires: llm-a@other does not support transparent_background"]
        );
    }

    #[test]
    fn capability_phrases() {
        assert_eq!(capability_phrase(&["image.generate"]), "an image.generate");
        assert_eq!(capability_phrase(&["agent.turn"]), "an agent.turn");
        assert_eq!(capability_phrase(&["video.generate"]), "a video.generate");
        assert_eq!(
            capability_phrase(&["structured.generate", "image.generate"]),
            "a structured.generate or image.generate"
        );
        assert_eq!(
            capability_phrase(&["image.edit", "mesh.rig", "music.generate"]),
            "an image.edit, mesh.rig or music.generate"
        );
    }

    // --- features.json against the spec --------------------------------------------------------

    fn spec_text(name: &str) -> String {
        let path = format!("{}/../../spec/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// Each `## <n>. `<capability>`` section of capabilities.md and its feature table, in order.
    fn spec_features() -> BTreeMap<String, Vec<String>> {
        let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut current: Option<String> = None;
        let mut in_table = false;
        for line in spec_text("capabilities.md").lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                in_table = false;
                current = rest
                    .split_once(". `")
                    .and_then(|(_, tail)| tail.strip_suffix('`'))
                    .filter(|name| name.contains('.'))
                    .map(str::to_string);
                if let Some(name) = &current {
                    found.insert(name.clone(), Vec::new());
                }
                continue;
            }
            let Some(name) = &current else { continue };
            if line.starts_with("| Feature | Meaning |") {
                in_table = true;
            } else if in_table && line.starts_with("| `") {
                let feature = line[3..].split('`').next().unwrap_or_default();
                found.get_mut(name).unwrap().push(feature.to_string());
            } else if in_table && !line.starts_with('|') {
                in_table = false;
            }
        }
        found
    }

    #[test]
    fn the_vocabulary_is_spec_capabilities_md() {
        let spec = spec_features();
        assert_eq!(spec.len(), 11, "{spec:?}");
        assert_eq!(vocabulary().capabilities, spec);
        assert_eq!(capability_features("vision.review"), None);
        assert_eq!(capability_features("music.generate"), Some(&[][..]));
    }

    #[test]
    fn every_renamed_feature_is_one_providers_md_drops_for_a_feature_of_fx() {
        let providers = spec_text("providers.md");
        let changes = &providers[providers.find("## 11. ").expect("providers.md §11")..];
        let all: BTreeSet<&str> = vocabulary()
            .capabilities
            .values()
            .flatten()
            .map(String::as_str)
            .collect();
        assert_eq!(vocabulary().renamed.len(), 4);
        for (old, new) in &vocabulary().renamed {
            assert!(
                changes.contains(&format!("`{old}`")),
                "{old} in providers.md §11"
            );
            assert!(!all.contains(old.as_str()), "{old} is still a feature");
            assert!(all.contains(new.as_str()), "{new} is a feature");
        }
    }
}
