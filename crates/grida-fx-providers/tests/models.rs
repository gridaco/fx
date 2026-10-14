//! The built-in route table against the public facts of Grida Models (spec/providers.md §3):
//! `routes/models.json`, the projection of a pinned release that tools/make_models.py writes. FX
//! never reads it when it runs; these tests are where it is used.
//!
//! Each route is held to its model's card: its wire and endpoint ids, its features both ways, its
//! limits within the provider's, GG's 3D endpoints, and the text routes' prices as bounds. A fact
//! the release does not record is excused in [`EXCEPTIONS`], with its reason, and an exception no
//! check needs fails, so the table shrinks as Grida Models records more. A feature that no member
//! of Grida Models' schema can show is in [`UNMAPPED`].

use grida_fx_core::money::Usd;
use grida_fx_core::routes::Route;
use grida_fx_providers::capabilities::capability;
use grida_fx_providers::routes::default_table;
use grida_fx_providers::{chat, fal, openrouter, tripo, wire};
use serde_json::Value;
use std::collections::BTreeSet;

const OPENAI_IMAGE: &str = "gpt-image-2.5-sunburst@openai";
const GG_IMAGE: &str = "openai/gpt-image-2.5-sunburst@gg";
const OPENROUTER_IMAGE: &str = "openai/gpt-image-2.5-sunburst@openrouter";
const FAL_IMAGE: &str = "openai/gpt-image-2.5/sunburst@fal";
const GENERATE: &str = "image.generate";
const EDIT: &str = "image.edit";
const IMAGES: &[&str] = &[GENERATE, EDIT];

/// The GPT Image adapters: OpenAI's, fal's, GG's and OpenRouter's.
const GPT_IMAGE: &[&str] = &[
    "fx-openai-image-v1",
    fal::image::CONTRACT_ADAPTER,
    "gg-image",
    openrouter::image::CONTRACT_ADAPTER,
];

/// A check that the pinned release of Grida Models cannot pass, for each route and capability
/// listed.
struct Exception {
    routes: &'static [&'static str],
    capabilities: &'static [&'static str],
    /// `card`, `feature <name>` or `limit <what>`.
    checks: &'static [&'static str],
    reason: &'static str,
}

/// The parameter-backed features of a GPT Image route that a provider records nothing for.
const IMAGE_PARAMETERS: &[&str] = &[
    "feature alpha",
    "feature auto_background",
    "feature opaque_background",
    "feature exact_size",
    "feature custom_exact_size",
    "feature flexible_size",
    "feature maximum_quality",
    "feature png_output",
];

/// Every exception, in one table (module doc).
const EXCEPTIONS: &[Exception] = &[
    Exception {
        routes: &["imagegen@codex", "imagine@grok"],
        capabilities: IMAGES,
        checks: &["card"],
        reason: "a runtime on the user's own subscription, which publishes no provider facts",
    },
    Exception {
        routes: &["google/gemini-omni-flash/v1.1/image-to-video@fal"],
        capabilities: &["video.generate"],
        checks: &["card"],
        reason: "Grida Models has no card for Gemini Omni Flash",
    },
    Exception {
        routes: &[OPENAI_IMAGE],
        capabilities: IMAGES,
        checks: IMAGE_PARAMETERS,
        reason: "Grida Models records no endpoint on the OpenAI binding",
    },
    Exception {
        routes: &[OPENAI_IMAGE],
        capabilities: &[GENERATE],
        checks: &["feature text_to_image", "limit size envelope"],
        reason: "Grida Models records no endpoint on the OpenAI binding",
    },
    Exception {
        routes: &[OPENAI_IMAGE],
        capabilities: &[EDIT],
        checks: &[
            "feature image_input",
            "feature mask",
            "limit input pictures",
            "limit size envelope",
        ],
        reason: "Grida Models records no endpoint on the OpenAI binding",
    },
    Exception {
        routes: &[GG_IMAGE],
        capabilities: IMAGES,
        checks: IMAGE_PARAMETERS,
        reason: "GG's image endpoints record no request parameters",
    },
    Exception {
        routes: &[GG_IMAGE],
        capabilities: &[EDIT],
        checks: &["limit input pictures"],
        reason: "GG's image endpoints record no request parameters",
    },
    Exception {
        routes: &[GG_IMAGE],
        capabilities: IMAGES,
        checks: &["limit size envelope"],
        reason: "GG's image endpoints record no image_constraints",
    },
    Exception {
        routes: &[OPENROUTER_IMAGE],
        capabilities: IMAGES,
        checks: &["feature exact_size"],
        reason: "Grida Models records no size parameter on OpenRouter",
    },
    Exception {
        routes: &[OPENROUTER_IMAGE],
        capabilities: IMAGES,
        checks: &["feature png_output"],
        reason: "Grida Models records no output_format parameter on OpenRouter",
    },
    Exception {
        routes: &[OPENROUTER_IMAGE],
        capabilities: IMAGES,
        checks: &["limit size envelope"],
        reason: "Grida Models records image_constraints only on fal",
    },
    Exception {
        routes: &[FAL_IMAGE],
        capabilities: &[EDIT],
        checks: &["feature mask"],
        reason: "Grida Models records no mask parameter",
    },
    Exception {
        routes: &["tripo/p2@gg"],
        capabilities: &["mesh.generate"],
        checks: &["feature pbr", "feature textured_mesh"],
        reason: "GG's 3D endpoints record no request parameters",
    },
    Exception {
        routes: &["tripo/rig-v1.0@gg"],
        capabilities: &["mesh.rig"],
        checks: &["feature biped"],
        reason: "GG's 3D endpoints record no request parameters",
    },
    Exception {
        routes: &["eleven_text_to_sound_v2@elevenlabs"],
        capabilities: &["sound.generate"],
        checks: &["feature exact_duration"],
        reason: "Grida Models has no ElevenLabs binding, only its cards",
    },
    Exception {
        routes: &["eleven_v3@elevenlabs"],
        capabilities: &["speech.generate"],
        checks: &["feature stability"],
        reason: "Grida Models has no ElevenLabs binding, only its cards",
    },
];

/// The features that no member of Grida Models' schema can show, by capability, with the reason.
const UNMAPPED: &[(&str, &str, &str)] = &[
    (
        GENERATE,
        "authored_prompt_passthrough",
        "whether the prompt is sent verbatim is the adapter's doing",
    ),
    (
        EDIT,
        "authored_prompt_passthrough",
        "whether the prompt is sent verbatim is the adapter's doing",
    ),
    (
        "speech.generate",
        "audio_tags",
        "the card's input.audio_tags is not a member of the schema",
    ),
    (
        "mesh.rig",
        "glb_input",
        "the rigging card's input.formats is not a member of the schema",
    ),
];

// The projection --------------------------------------------------------------------------------

fn models() -> Value {
    let path = format!("{}/routes/models.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn routes() -> Vec<Route> {
    default_table().unwrap().entries.into_values().collect()
}

/// Grida Models' name for a provider of FX's.
fn models_provider(provider: &str) -> &str {
    match provider {
        "gg" => "grida",
        other => other,
    }
}

/// What the projection records for one route's provider.
struct Facts<'a> {
    card: &'a Value,
    /// The provider's binding, when the card has one.
    binding: Option<&'a Value>,
    /// The provider's endpoints: its binding's, or the card's own when the provider is the card's
    /// reference provider.
    endpoints: Vec<&'a Value>,
    /// A rig's check: the endpoints of the rig-check operation for the provider.
    check_endpoints: Vec<&'a Value>,
    /// Whether Grida Models lists the provider for the model (`listed`).
    coverage: Option<&'a str>,
}

fn list(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

/// The provider's binding of `place` (a card or the rig-check operation), and its endpoints.
fn provider_part<'a>(place: &'a Value, provider: &str) -> (Option<&'a Value>, Vec<&'a Value>) {
    let binding = place["providers"].get(provider);
    let endpoints = match binding.and_then(|b| b.get("endpoints")) {
        Some(endpoints) => list(Some(endpoints)),
        None => list(place.get("endpoints")),
    };
    (binding, endpoints)
}

/// The route's facts; `None` when Grida Models has no card for it.
fn facts<'a>(models: &'a Value, route: &Route) -> Option<Facts<'a>> {
    let entry = models["routes"].get(route.id()).filter(|e| !e.is_null())?;
    let provider = models_provider(&route.provider);
    let card = &entry["card"];
    let (binding, endpoints) = provider_part(card, provider);
    let check_endpoints = match entry.get("rig_check") {
        Some(check) => provider_part(check, provider).1,
        None => Vec::new(),
    };
    Some(Facts {
        card,
        binding,
        endpoints,
        check_endpoints,
        coverage: card["provider_coverage"][provider]["disposition"].as_str(),
    })
}

/// The endpoints among `facts` that the route's adapter sends to: by id where FX names one (fal's
/// `<model>/<action>`, Tripo's paths, the GG 3D routes' `endpoint`, else the model), and on GG's
/// image and video APIs by interface.
fn sent_to<'a>(route: &Route, facts: &Facts<'a>) -> Vec<&'a Value> {
    let edit = route.capability == EDIT;
    let by = |member: &str, value: &str| -> Vec<&'a Value> {
        facts
            .endpoints
            .iter()
            .copied()
            .filter(|endpoint| endpoint[member] == value)
            .collect()
    };
    match route.contract["adapter"].as_str().unwrap_or("") {
        fal::image::CONTRACT_ADAPTER => {
            let action = if edit {
                fal::image::EDIT_ENDPOINT
            } else {
                fal::image::GENERATE_ENDPOINT
            };
            by("id", &format!("{}/{action}", route.model))
        }
        "gg-image" => by(
            "interface",
            if edit {
                "images.edits"
            } else {
                "images.generations"
            },
        ),
        "gg-video" => by("interface", "videos"),
        "gg-mesh" | "gg-rig" => by("id", route.contract["endpoint"].as_str().unwrap_or("")),
        tripo::mesh::ADAPTER => by("id", tripo::mesh::PATH),
        tripo::rig::ADAPTER => by("id", tripo::rig::PATH),
        _ => by("id", &route.model),
    }
}

/// A parameter of the endpoints the route sends to.
fn parameter<'a>(sent: &[&'a Value], name: &str) -> Option<&'a Value> {
    sent.iter()
        .find_map(|endpoint| endpoint["parameters"].get(name))
}

fn takes(sent: &[&Value], name: &str, value: &str) -> bool {
    parameter(sent, name)
        .and_then(|p| p["enum"].as_array())
        .is_some_and(|values| values.iter().any(|v| v == value))
}

fn has(sent: &[&Value], name: &str) -> bool {
    parameter(sent, name).is_some()
}

fn input(sent: &[&Value], kinds: &[&str]) -> bool {
    sent.iter()
        .any(|endpoint| kinds.iter().any(|kind| endpoint["input"] == *kind))
}

/// The checks of one test, and the exceptions they used.
struct Gate {
    /// The checks this test makes: `card`, `feature` or `limit` (no exception names another).
    kind: &'static str,
    problems: Vec<String>,
    excused: BTreeSet<(String, String, String)>,
}

impl Gate {
    fn new(kind: &'static str) -> Gate {
        Gate {
            kind,
            problems: Vec::new(),
            excused: BTreeSet::new(),
        }
    }

    /// Holds `route` to `check`: a failure is a problem unless [`EXCEPTIONS`] excuses it.
    fn hold(&mut self, route: &Route, check: &str, ok: bool, why: impl FnOnce() -> String) {
        if ok {
            return;
        }
        let key = (route.id(), route.capability.clone(), check.to_string());
        if exception_items().any(|(item, _)| item == key) {
            self.excused.insert(key);
        } else {
            self.problems
                .push(format!("{} {}: {}", route.id(), route.capability, why()));
        }
    }

    /// A failure no exception may excuse.
    fn fail(&mut self, route: &Route, why: String) {
        self.problems
            .push(format!("{} {}: {why}", route.id(), route.capability));
    }

    /// Every exception of this test's kind must have been needed, and no check failed.
    fn finish(mut self) {
        for ((route, capability, check), reason) in exception_items() {
            if check.split(' ').next() == Some(self.kind)
                && !self
                    .excused
                    .contains(&(route.clone(), capability.clone(), check.clone()))
            {
                self.problems.push(format!(
                    "{route} {capability}: the exception for `{check}` is no longer needed \
                     ({reason})"
                ));
            }
        }
        assert!(self.problems.is_empty(), "\n{}", self.problems.join("\n"));
    }
}

/// Each (route, capability, check) [`EXCEPTIONS`] excuses, with its reason.
fn exception_items() -> impl Iterator<Item = ((String, String, String), &'static str)> {
    EXCEPTIONS.iter().flat_map(|exception| {
        exception.routes.iter().flat_map(move |route| {
            exception.capabilities.iter().flat_map(move |capability| {
                exception.checks.iter().map(move |check| {
                    (
                        (route.to_string(), capability.to_string(), check.to_string()),
                        exception.reason,
                    )
                })
            })
        })
    })
}

// Cards, wire ids and endpoints -------------------------------------------------------------------

/// Every route but a runtime's and a model's without a card has its model's card, and Grida
/// Models lists the route's provider for it.
#[test]
fn every_route_has_its_model_s_card() {
    let models = models();
    let mut gate = Gate::new("card");
    let mut ids: Vec<String> = routes().iter().map(Route::id).collect();
    ids.sort();
    ids.dedup();
    let mut projected: Vec<String> = models["routes"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    projected.sort();
    assert_eq!(
        projected, ids,
        "routes/models.json holds the table's route ids"
    );
    for route in routes() {
        let facts = facts(&models, &route);
        gate.hold(&route, "card", facts.is_some(), || {
            "Grida Models has no card for its model".into()
        });
        if let Some(facts) = facts {
            gate.hold(&route, "card", facts.coverage == Some("listed"), || {
                format!("its provider's coverage is {:?}", facts.coverage)
            });
        }
    }
    gate.finish();
}

/// The model a route names is the provider's: its binding's id (on fal, an endpoint's), or with
/// no binding the card's own id or its endpoint's `model`; and where the provider's endpoints are
/// recorded, the route sends to one of them.
#[test]
fn every_route_names_the_provider_s_ids() {
    let models = models();
    let mut gate = Gate::new("wire");
    for route in routes() {
        let Some(facts) = facts(&models, &route) else {
            continue;
        };
        let sent = sent_to(&route, &facts);
        if !facts.endpoints.is_empty() && sent.is_empty() {
            let ids: Vec<&Value> = facts.endpoints.iter().map(|e| &e["id"]).collect();
            gate.fail(
                &route,
                format!("sends to none of the provider's endpoints {ids:?}"),
            );
        }
        match facts.binding {
            Some(binding) if route.provider != "fal" => {
                if binding["id"] != route.model.as_str() {
                    gate.fail(&route, format!("the binding's id is {}", binding["id"]));
                }
            }
            Some(_) => {}
            None => {
                let named = parameter(&sent, "model").and_then(|p| p["enum"].as_array());
                let known = match named {
                    Some(models) => models.iter().any(|m| m == route.model.as_str()),
                    None => facts.card["id"] == route.model.as_str(),
                };
                if !known {
                    gate.fail(&route, "neither the card nor its endpoint names it".into());
                }
            }
        }
        if let Some(model) = parameter(&sent, "model").and_then(|p| p["enum"].as_array())
            && !model.iter().any(|m| m == route.model.as_str())
        {
            gate.fail(&route, format!("its endpoint's model is one of {model:?}"));
        }
    }
    gate.finish();
}

/// The GG 3D routes submit to an endpoint GG lists for the model: views to a `multi_image`
/// endpoint, a rig's model to an `asset` one, and the rig's check to the rig-check operation's;
/// Tripo's rig route checks at the operation's own path.
#[test]
fn the_3d_routes_submit_to_the_listed_endpoints() {
    let models = models();
    let mut gate = Gate::new("endpoint");
    let mut seen = 0;
    for route in routes() {
        let Some(facts) = facts(&models, &route) else {
            continue;
        };
        let adapter = route.contract["adapter"].as_str().unwrap_or("");
        let endpoint = |id: &str, among: &[&Value], input: &str| {
            among
                .iter()
                .any(|e| e["id"] == id && e["interface"] == "endpoints" && e["input"] == input)
        };
        let contract = |member: &str| route.contract[member].as_str().unwrap_or("").to_string();
        match adapter {
            "gg-mesh" => {
                seen += 1;
                if !endpoint(&contract("endpoint"), &facts.endpoints, "multi_image") {
                    gate.fail(
                        &route,
                        "its endpoint is not GG's multi_image endpoint".into(),
                    );
                }
            }
            "gg-rig" => {
                seen += 1;
                if !endpoint(&contract("endpoint"), &facts.endpoints, "asset") {
                    gate.fail(&route, "its endpoint is not GG's asset endpoint".into());
                }
                if !endpoint(&contract("check_endpoint"), &facts.check_endpoints, "asset") {
                    gate.fail(&route, "its check_endpoint is not GG's rig check".into());
                }
            }
            tripo::rig::ADAPTER => {
                seen += 1;
                if !facts
                    .check_endpoints
                    .iter()
                    .any(|e| e["id"] == tripo::CHECK_PATH)
                {
                    gate.fail(&route, "Tripo lists no rig check at its path".into());
                }
            }
            _ => {}
        }
    }
    assert_eq!(seen, 3, "the GG mesh and rig routes, and Tripo's rig route");
    gate.finish();
}

// Features --------------------------------------------------------------------------------------

/// Whether the provider documents `feature` for the route, from the endpoints it sends to (and
/// a chat binding's capabilities); `None` for a feature in [`UNMAPPED`]. An unknown fact is
/// `false`: it never enables a feature.
fn documented(route: &Route, facts: &Facts, feature: &str) -> Option<bool> {
    let sent = sent_to(route, facts);
    let flag = |name: &str| {
        facts
            .binding
            .is_some_and(|binding| binding["capabilities"][name] == true)
    };
    let chat = matches!(
        route.capability.as_str(),
        "structured.generate" | "agent.turn"
    );
    let fact = match feature {
        // Chat: the binding's capabilities.
        "image_input" if chat => flag("image_input"),
        "structured_output" => flag("structured_output"),
        "tool_use" => flag("tools"),
        // Images: the endpoint's inputs and parameters.
        "text_to_image" | "text_to_video" => input(&sent, &["text"]),
        "image_input" => {
            input(&sent, &["image", "multi_image"])
                || has(&sent, "image_urls")
                || parameter(&sent, "input_references")
                    .is_some_and(|p| p["maximum"].as_u64().unwrap_or(0) > 0)
        }
        "alpha" => takes(&sent, "background", "transparent"),
        "opaque_background" => takes(&sent, "background", "opaque"),
        "auto_background" => takes(&sent, "background", "auto"),
        "maximum_quality" => takes(&sent, "quality", "max"),
        "png_output" => takes(&sent, "output_format", "png"),
        "exact_size" | "custom_exact_size" => {
            has(&sent, "image_size.width") && has(&sent, "image_size.height")
        }
        "flexible_size" => {
            takes(&sent, "image_size", "auto") || takes(&sent, "aspect_ratio", "auto")
        }
        "mask" => ["mask", "mask_url", "mask_image_url"]
            .iter()
            .any(|name| has(&sent, name)),
        // Video: the end frame fal's adapter sends.
        "first_last_frame" => has(&sent, "end_image_url"),
        // 3D: the views, and the parameters the adapters send.
        "multiview" => {
            input(&sent, &["multi_image"])
                || parameter(&sent, "inputs").is_some_and(|p| p["type"] == "array")
        }
        "pbr" => has(&sent, "pbr"),
        "quad" => has(&sent, "quad"),
        "textured_mesh" => has(&sent, "texture"),
        "biped" => takes(&sent, "rig_type", "biped"),
        "rig_check" => !facts.check_endpoints.is_empty(),
        // Audio: the parameters ElevenLabs' adapters send.
        "exact_duration" => has(&sent, "duration_seconds"),
        "stability" => has(&sent, "voice_settings.stability"),
        _ => return None,
    };
    Some(fact)
}

const IMAGE_COMMON: &[&str] = &[
    "alpha",
    "opaque_background",
    "auto_background",
    "exact_size",
    "flexible_size",
    "png_output",
    "maximum_quality",
    "authored_prompt_passthrough",
];

/// The features an adapter honours for a capability, from its reference (src/<provider>/README.md),
/// whether or not a route declares them.
fn honoured(adapter: &str, capability: &str) -> BTreeSet<&'static str> {
    let mut features: BTreeSet<&str> = BTreeSet::new();
    if GPT_IMAGE.contains(&adapter) {
        features.extend(IMAGE_COMMON);
        if adapter != openrouter::image::CONTRACT_ADAPTER {
            features.insert("custom_exact_size");
        }
        if capability == GENERATE {
            features.insert("text_to_image");
        } else {
            features.insert("image_input");
            if ["fx-openai-image-v1", fal::image::CONTRACT_ADAPTER].contains(&adapter) {
                features.insert("mask");
            }
        }
        return features;
    }
    let own: &[&str] = match adapter {
        "gg-structured" | "openrouter-structured" => &["structured_output", "image_input"],
        "gg-tool-loop" | "openrouter-tool-loop" => &["tool_use", "image_input"],
        "gg-video" => &["text_to_video"],
        "fal-queue" => &["text_to_video", "first_last_frame"],
        "tripo-multiview" | "gg-mesh" => &["multiview", "pbr", "quad", "textured_mesh"],
        "tripo-rig" | "gg-rig" => &["biped", "glb_input", "rig_check"],
        "elevenlabs-sound-effect" => &["exact_duration"],
        "elevenlabs-speech" => &["audio_tags", "stability"],
        "openrouter-music" => &[],
        other => panic!("{other}: no adapter of the table honours features here"),
    };
    features.extend(own);
    features
}

/// R10 (docs/wg/provider-contract.md): a route's features are the provider's documented facts
/// that its adapter honours. So a feature the provider documents and the adapter honours is
/// declared; a declared feature is honoured; and a declared feature a fact could show is
/// documented, unless an exception says why Grida Models cannot show it.
#[test]
fn features_are_the_documented_ones_the_adapter_honours() {
    let models = models();
    let mut gate = Gate::new("feature");
    let mut unmapped_seen = BTreeSet::new();
    for route in routes() {
        let Some(facts) = facts(&models, &route) else {
            continue;
        };
        let adapter = route.contract["adapter"].as_str().unwrap();
        let honours = honoured(adapter, &route.capability);
        for feature in capability(&route.capability).unwrap().features {
            let declared = route.features.contains(*feature);
            if declared && !honours.contains(feature) {
                gate.fail(
                    &route,
                    format!("declares {feature}, which {adapter} does not honour"),
                );
            }
            let Some(documented) = documented(&route, &facts, feature) else {
                let listed = UNMAPPED
                    .iter()
                    .any(|(c, f, _)| *c == route.capability && f == feature);
                if listed {
                    unmapped_seen.insert((route.capability.clone(), feature.to_string()));
                } else {
                    gate.fail(
                        &route,
                        format!("{feature} has no fact mapped and is not UNMAPPED"),
                    );
                }
                continue;
            };
            if documented && honours.contains(feature) && !declared {
                gate.fail(
                    &route,
                    format!(
                        "the provider documents {feature} and {adapter} honours it: declare it"
                    ),
                );
            }
            gate.hold(
                &route,
                &format!("feature {feature}"),
                documented || !declared,
                || format!("declares {feature}, which the provider does not document"),
            );
        }
    }
    for (capability, feature, reason) in UNMAPPED {
        assert!(
            unmapped_seen.contains(&(capability.to_string(), feature.to_string())),
            "{capability} {feature} is UNMAPPED ({reason}) but no route needed it"
        );
    }
    gate.finish();
}

/// A route that asks for a reasoning effort runs on a model whose binding documents reasoning.
#[test]
fn a_reasoning_effort_runs_on_a_reasoning_model() {
    let models = models();
    let mut asked = 0;
    for route in routes() {
        let contract = &route.contract;
        let effort = contract.get("reasoning_effort").is_some()
            || contract["request_policy"]["reasoning"]
                .get("effort")
                .is_some();
        if !effort {
            continue;
        }
        asked += 1;
        let facts = facts(&models, &route).expect("a text route has its card");
        let reasoning = facts.binding.map(|b| &b["capabilities"]["reasoning"]);
        assert_eq!(
            reasoning,
            Some(&Value::Bool(true)),
            "{} {}",
            route.id(),
            route.capability
        );
    }
    assert_eq!(asked, 4, "the astra routes on GG and OpenRouter");
}

// Limits -----------------------------------------------------------------------------------------

/// The size facts of an endpoint: `image_constraints` (step, edges, pixels, aspect).
struct Constraints<'a>(&'a Value);

impl Constraints<'_> {
    fn number(&self, name: &str) -> Option<u64> {
        self.0[name].as_u64()
    }

    /// Whether `width` × `height` is within these constraints.
    fn takes(&self, width: u64, height: u64) -> bool {
        let (long, short) = (width.max(height), width.min(height));
        let step = self.number("step").unwrap_or(1);
        let aspect = self.0["aspect_ratio"]["max"].as_f64();
        width.is_multiple_of(step)
            && height.is_multiple_of(step)
            && self.number("max_edge").is_none_or(|max| long <= max)
            && self.number("min_edge").is_none_or(|min| short >= min)
            && self
                .number("min_pixels")
                .is_none_or(|min| width * height >= min)
            && self
                .number("max_pixels")
                .is_none_or(|max| width * height <= max)
            && aspect.is_none_or(|max| long as f64 <= max * short as f64)
    }

    /// Whether FX's envelope is within these constraints: every size it takes, they take.
    fn hold(&self, envelope: &wire::SizeEnvelope) -> bool {
        let step = self.number("step").unwrap_or(1);
        let aspect = self.0["aspect_ratio"]["max"].as_f64();
        (envelope.step as u64).is_multiple_of(step)
            && self
                .number("max_edge")
                .is_none_or(|max| envelope.max_edge as u64 <= max)
            && self.number("min_edge").is_none()
            && self
                .number("min_pixels")
                .is_none_or(|min| envelope.min_pixels >= min)
            && self
                .number("max_pixels")
                .is_none_or(|max| envelope.max_pixels <= max)
            && aspect.is_none_or(|max| envelope.max_aspect as f64 <= max)
    }
}

fn size(text: &str) -> (u64, u64) {
    let (w, h) = text.split_once('x').expect("a WxH tier");
    (w.parse().unwrap(), h.parse().unwrap())
}

/// FX's limits are within the provider's: the input pictures a route declares, the exact-size
/// envelope and the sizes a route serves or prices, fal's prompt length, Tripo's face limit and
/// a structured route's default `max_tokens` (an agent turn sends none by default).
#[test]
fn limits_are_within_the_provider_s() {
    let models = models();
    let mut gate = Gate::new("limit");
    for route in routes() {
        let Some(facts) = facts(&models, &route) else {
            continue;
        };
        let sent = sent_to(&route, &facts);
        let adapter = route.contract["adapter"].as_str().unwrap();
        if let Some(pictures) = route.max_input_pictures {
            let most = parameter(&sent, "input_references")
                .and_then(|p| p["maximum"].as_u64())
                .or_else(|| parameter(&sent, "image_urls").and_then(|p| p["max_items"].as_u64()));
            gate.hold(
                &route,
                "limit input pictures",
                most.is_some_and(|most| u64::from(pictures) <= most),
                || format!("takes {pictures} input pictures; the provider records {most:?}"),
            );
        }
        let constraints = sent
            .iter()
            .find_map(|e| e.get("image_constraints"))
            .map(Constraints);
        // The sizes a GPT Image route prices, and those its OpenRouter adapter serves.
        let mut sizes: Vec<(u64, u64)> = if GPT_IMAGE.contains(&adapter) {
            route.price.tiers.keys().map(|tier| size(tier)).collect()
        } else {
            Vec::new()
        };
        if adapter == openrouter::image::CONTRACT_ADAPTER {
            sizes.extend(
                openrouter::image::ALLOWED_SIZES
                    .iter()
                    .map(|text| size(text)),
            );
        }
        if !sizes.is_empty() {
            // FX's envelope, where FX holds an exact size to it: OpenRouter's adapter serves only
            // its sizes, and GG leaves sizes to GG (src/gg/README.md).
            let envelope = ["fx-openai-image-v1", fal::image::CONTRACT_ADAPTER].contains(&adapter);
            let within = constraints.as_ref().is_some_and(|c| {
                sizes.iter().all(|(w, h)| c.takes(*w, *h))
                    && (!envelope || c.hold(&wire::SIZE_ENVELOPE))
            });
            gate.hold(&route, "limit size envelope", within, || {
                "its sizes are not within the provider's image_constraints".into()
            });
            if envelope {
                for name in ["image_size.width", "image_size.height"] {
                    if let Some(edge) = parameter(&sent, name) {
                        let max = edge["maximum"].as_u64().unwrap_or(u64::MAX);
                        let step = edge["multiple_of"].as_u64().unwrap_or(1);
                        if wire::SIZE_ENVELOPE.max_edge as u64 > max
                            || !(wire::SIZE_ENVELOPE.step as u64).is_multiple_of(step)
                        {
                            gate.fail(&route, format!("its envelope exceeds {name}"));
                        }
                    }
                }
            }
        }
        if adapter == fal::image::CONTRACT_ADAPTER {
            let longest = parameter(&sent, "prompt").and_then(|p| p["max_length"].as_u64());
            let within =
                longest.is_some_and(|longest| fal::image::MAX_PROMPT_CHARS as u64 <= longest);
            if !within {
                gate.fail(
                    &route,
                    format!("its prompt limit exceeds fal's {longest:?}"),
                );
            }
        }
        if adapter == tripo::mesh::ADAPTER {
            let faces = parameter(&sent, "face_limit").expect("Tripo records face_limit");
            let (min, max) = (faces["minimum"].as_i64(), faces["maximum"].as_i64());
            let range = tripo::mesh::FACE_LIMIT;
            if !(min.is_some_and(|min| *range.start() >= min)
                && max.is_some_and(|max| *range.end() <= max))
            {
                gate.fail(
                    &route,
                    format!("face_limit {range:?} is not within {min:?}..{max:?}"),
                );
            }
        }
        // A structured call's default; an agent turn sends none (src/chat/mod.rs).
        if route.capability == "structured.generate" {
            let most = facts
                .binding
                .and_then(|b| b["limits"]["max_output_tokens"].as_u64());
            let within = most.is_some_and(|most| chat::DEFAULT_MAX_TOKENS <= most);
            if !within {
                gate.fail(
                    &route,
                    format!(
                        "max_tokens {} exceeds the model's {most:?}",
                        chat::DEFAULT_MAX_TOKENS
                    ),
                );
            }
        }
    }
    gate.finish();
}

// Prices ----------------------------------------------------------------------------------------

/// An exact amount of dollars, in units of 10^-18.
fn amount(text: &str) -> u128 {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    assert!(fraction.len() <= 18, "{text}: more than 18 decimal places");
    let padded = format!("{fraction:0<18}");
    whole.parse::<u128>().expect(text) * 10u128.pow(18) + padded.parse::<u128>().expect(text)
}

fn dollars(usd: Usd) -> u128 {
    u128::try_from(usd.0).unwrap() * 10u128.pow(12)
}

/// One published token rate: from `from` input tokens up to `to`, exclusive.
struct Rate {
    from: u64,
    to: Option<u64>,
    per_token: u128,
}

/// The route's published per-token rates of `meter` (`input` or `output`): OpenRouter's
/// `token_pricing` tiers, else its flat rate; GG's per-token components of the endpoint it sends
/// to, banded by `total_input_tokens`.
fn rates(route: &Route, facts: &Facts, meter: &str) -> Vec<Rate> {
    if let Some(pricing) = facts.binding.and_then(|b| b.get("token_pricing")) {
        let tiers: Vec<Rate> = list(pricing.get("tiers"))
            .into_iter()
            .filter(|tier| tier["meter"] == meter)
            .map(|tier| Rate {
                from: tier["minimum_input_tokens"].as_u64().unwrap(),
                to: tier["maximum_input_tokens"].as_u64(),
                per_token: amount(tier["usd_per_token"].as_str().unwrap()),
            })
            .collect();
        if !tiers.is_empty() {
            return tiers;
        }
        return pricing[meter]
            .as_str()
            .map(|flat| Rate {
                from: 0,
                to: None,
                per_token: amount(flat),
            })
            .into_iter()
            .collect();
    }
    let bound = |value: &Value| value.as_str().map(|text| text.parse::<u64>().unwrap());
    sent_to(route, facts)
        .into_iter()
        .flat_map(|endpoint| list(endpoint["published_pricing"].get("components")))
        .filter(|c| c["meter"] == format!("{meter}_tokens").as_str())
        .map(|c| {
            assert_eq!(
                (&c["unit"], &c["per_quantity"]),
                (&Value::from("token"), &Value::from("1"))
            );
            let range = &c["quantity_range"];
            assert!(
                range.is_null() || range["meter"] == "total_input_tokens",
                "{range}"
            );
            Rate {
                from: bound(&range["min"]).unwrap_or(0),
                to: bound(&range["max"]),
                per_token: amount(c["usd_per_unit"].as_str().unwrap()),
            }
        })
        .collect()
}

fn rate_at(rates: &[Rate], tokens: u64) -> Option<u128> {
    rates
        .iter()
        .find(|r| r.from <= tokens && r.to.is_none_or(|to| tokens < to))
        .map(|r| r.per_token)
}

/// Text prices against the published token rates, as bounds, never equality (spec/providers.md
/// §3): a `structured.generate` route's high holds the default `max_tokens` at the highest output
/// rate; and within one capability and provider, a model whose every rate is at least another's,
/// at every input length either publishes a band from, is priced at least as high, low and high.
#[test]
fn text_prices_are_bounds_over_the_published_token_rates() {
    let models = models();
    let text: Vec<Route> = routes()
        .into_iter()
        .filter(|r| r.capability == "structured.generate" || r.capability == "agent.turn")
        .collect();
    let rated = |route: &Route| {
        let facts = facts(&models, route).expect("a text route has its card");
        let input = rates(route, &facts, "input");
        let output = rates(route, &facts, "output");
        assert!(
            !input.is_empty() && !output.is_empty(),
            "{} {}: no token rates",
            route.id(),
            route.capability
        );
        (input, output)
    };
    for route in text
        .iter()
        .filter(|r| r.capability == "structured.generate")
    {
        let (_, output) = rated(route);
        let highest = output.iter().map(|r| r.per_token).max().unwrap();
        let needs = u128::from(chat::DEFAULT_MAX_TOKENS) * highest;
        assert!(
            dollars(route.price.high) >= needs,
            "{} {}: its high is below {} output tokens at the highest rate",
            route.id(),
            route.capability,
            chat::DEFAULT_MAX_TOKENS
        );
    }
    let mut compared = 0;
    for a in &text {
        for b in &text {
            if a.id() == b.id() || (&a.capability, &a.provider) != (&b.capability, &b.provider) {
                continue;
            }
            let (a_rates, b_rates) = (rated(a), rated(b));
            let bands: BTreeSet<u64> = [&a_rates.0, &a_rates.1, &b_rates.0, &b_rates.1]
                .iter()
                .flat_map(|rates| rates.iter().map(|r| r.from))
                .collect();
            let dominates = [(&a_rates.0, &b_rates.0), (&a_rates.1, &b_rates.1)]
                .iter()
                .all(|(mine, theirs)| {
                    bands
                        .iter()
                        .all(|&n| match (rate_at(mine, n), rate_at(theirs, n)) {
                            (Some(mine), Some(theirs)) => mine >= theirs,
                            _ => false,
                        })
                });
            if dominates {
                compared += 1;
                assert!(
                    a.price.low >= b.price.low && a.price.high >= b.price.high,
                    "{} {}: its every rate is at least {}'s, but its price is lower",
                    a.id(),
                    a.capability,
                    b.id()
                );
            }
        }
    }
    assert_eq!(
        compared, 4,
        "astra over sol, for both capabilities on GG and OpenRouter"
    );
}
