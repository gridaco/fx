//! The built-in route table (spec/providers.md §10) against what it must agree with: the
//! identities written down here, the features of spec/capabilities.md, the contract names of
//! spec/providers.md §9, and the adapters this crate registers.

use grida_fx_core::money::Usd;
use grida_fx_core::routes::{PriceUnit, Route, RouteTable};
use grida_fx_core::val::Val;
use grida_fx_core::value::{canon, digest};
use grida_fx_providers::adapter::{Adapter, RouteRef, Sent, Submitted};
use grida_fx_providers::capabilities::{CAPABILITIES, MemberType, Shape, capability};
use grida_fx_providers::keys::{KeyName, Keys};
use grida_fx_providers::live::adapters;
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::routes::{DEFAULT_ROUTES, default_table};
use grida_fx_providers::testing::{CallBuilder, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::NoNetwork;
use grida_fx_providers::{elevenlabs, fal, openai, openrouter, tripo};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

fn table() -> RouteTable {
    default_table().expect("the built-in route table parses")
}

fn registry() -> Adapters {
    adapters(&setup(Arc::new(NoNetwork), test_keys()))
}

fn usd(dollars: &str) -> Usd {
    Usd::parse(dollars).expect("a test amount")
}

/// The policy both `openai/gpt-6-astra` routes send (spec/providers.md §9.2).
fn astra_policy() -> Value {
    json!({
        "provider": {"require_parameters": true, "only": ["openai"], "allow_fallbacks": false},
        "reasoning": {"effort": "high"},
        "image_detail": "high"
    })
}

/// One route as written down by hand: what enters its fingerprint, and its price.
struct Expected {
    capability: &'static str,
    model: &'static str,
    provider: &'static str,
    contract: Value,
    low: &'static str,
    high: &'static str,
}

fn expected(
    capability: &'static str,
    route: &'static str,
    contract: Value,
    (low, high): (&'static str, &'static str),
) -> Expected {
    let (model, provider) = route.rsplit_once('@').expect("model@provider");
    Expected {
        capability,
        model,
        provider,
        contract,
        low,
        high,
    }
}

fn image_contract(adapter: &str, behavior: &str) -> Value {
    json!({"adapter": adapter, "adapter_behavior": behavior})
}

/// Every route of the table, in its order. Editing a contract here and in the table is a
/// deliberate change of every cache key the route serves (spec/identity.md §7).
fn expected_routes() -> Vec<Expected> {
    let tool_loop = json!({"adapter": "openrouter-tool-loop", "adapter_behavior": 1});
    let structured = json!({"adapter": "openrouter-structured", "adapter_behavior": 1});
    let mut astra_tool_loop = tool_loop.clone();
    astra_tool_loop["request_policy"] = astra_policy();
    let mut astra_structured = structured.clone();
    astra_structured["request_policy"] = astra_policy();
    astra_structured["pictures"] = json!("unchanged");
    vec![
        expected(
            "agent.turn",
            "openai/gpt-5.6-sol@openrouter",
            tool_loop,
            ("0.003", "0.1"),
        ),
        expected(
            "agent.turn",
            "openai/gpt-6-astra@openrouter",
            astra_tool_loop,
            ("0.01", "1.5"),
        ),
        expected(
            "image.edit",
            "gpt-image-2.5-sunburst@openai",
            image_contract("fx-openai-image-v1", "1"),
            ("0.18", "0.25"),
        ),
        expected(
            "image.edit",
            "openai/gpt-image-2.5-sunburst@openrouter",
            image_contract("fx-openrouter-image-v1", "3"),
            ("0.14", "0.3"),
        ),
        expected(
            "image.edit",
            "openai/gpt-image-2.5/sunburst@fal",
            image_contract("fx-fal-image-v1", "1"),
            ("0.16464", "0.5"),
        ),
        expected(
            "image.generate",
            "gpt-image-2.5-sunburst@openai",
            image_contract("fx-openai-image-v1", "1"),
            ("0.18", "0.25"),
        ),
        expected(
            "image.generate",
            "openai/gpt-image-2.5-sunburst@openrouter",
            image_contract("fx-openrouter-image-v1", "3"),
            ("0.14", "0.3"),
        ),
        expected(
            "image.generate",
            "openai/gpt-image-2.5/sunburst@fal",
            image_contract("fx-fal-image-v1", "1"),
            ("0.16464", "0.5"),
        ),
        expected(
            "mesh.generate",
            "P2-20260801@tripo",
            json!({"adapter": "tripo-multiview", "adapter_behavior": 1}),
            ("1.2", "2.5"),
        ),
        expected(
            "mesh.rig",
            "v1.0-20240301@tripo",
            json!({"adapter": "tripo-rig", "adapter_behavior": 1}),
            ("0.25", "0.5"),
        ),
        expected(
            "music.generate",
            "google/lyria-3-pro-preview@openrouter",
            json!({"adapter": "openrouter-music", "adapter_behavior": 1}),
            ("0.05", "0.5"),
        ),
        expected(
            "sound.generate",
            "eleven_text_to_sound_v2@elevenlabs",
            json!({"adapter": "elevenlabs-sound-effect", "adapter_behavior": 1}),
            ("0.001", "0.1"),
        ),
        expected(
            "speech.generate",
            "eleven_v3@elevenlabs",
            json!({"adapter": "elevenlabs-speech", "adapter_behavior": 1}),
            ("0.001", "0.05"),
        ),
        expected(
            "structured.generate",
            "openai/gpt-5.6-sol@openrouter",
            structured,
            ("0.02", "0.6"),
        ),
        expected(
            "structured.generate",
            "openai/gpt-6-astra@openrouter",
            astra_structured,
            ("0.05", "1.5"),
        ),
        expected(
            "video.generate",
            "google/gemini-omni-flash/v1.1/image-to-video@fal",
            json!({"adapter": "fal-queue", "adapter_behavior": 1}),
            ("0.03", "0.375"),
        ),
    ]
}

/// The contract each adapter serves, by capability and provider (spec/providers.md §9): its
/// `adapter`, and its `adapter_behavior` where §9 names one.
const CONTRACTS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "image.generate",
        "openai",
        "fx-openai-image-v1",
        Some("\"1\""),
    ),
    ("image.edit", "openai", "fx-openai-image-v1", Some("\"1\"")),
    (
        "image.generate",
        "openrouter",
        "fx-openrouter-image-v1",
        Some("\"3\""),
    ),
    (
        "image.edit",
        "openrouter",
        "fx-openrouter-image-v1",
        Some("\"3\""),
    ),
    (
        "structured.generate",
        "openrouter",
        "openrouter-structured",
        Some("1"),
    ),
    (
        "agent.turn",
        "openrouter",
        "openrouter-tool-loop",
        Some("1"),
    ),
    (
        "music.generate",
        "openrouter",
        "openrouter-music",
        Some("1"),
    ),
    ("image.generate", "fal", "fx-fal-image-v1", None),
    ("image.edit", "fal", "fx-fal-image-v1", None),
    ("video.generate", "fal", "fal-queue", None),
    ("background.remove", "fal", "fal-run-birefnet", None),
    ("mesh.generate", "tripo", "tripo-multiview", None),
    ("mesh.rig", "tripo", "tripo-rig", None),
    (
        "sound.generate",
        "elevenlabs",
        "elevenlabs-sound-effect",
        None,
    ),
    ("speech.generate", "elevenlabs", "elevenlabs-speech", None),
];

fn contract_for(capability: &str, provider: &str) -> Option<(&'static str, Option<&'static str>)> {
    CONTRACTS
        .iter()
        .find(|(c, p, _, _)| *c == capability && *p == provider)
        .map(|(_, _, adapter, behavior)| (*adapter, *behavior))
}

fn route_ref(route: &Route) -> RouteRef {
    RouteRef {
        capability: route.capability.clone(),
        model: route.model.clone(),
        provider: route.provider.clone(),
        contract: route.contract.clone(),
    }
}

fn read_spec(name: &str) -> String {
    let path = format!("{}/../../spec/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn the_table_holds_16_routes_over_10_capabilities() {
    let table = table();
    assert_eq!(table.entries.len(), 16);
    let capabilities: BTreeSet<&str> = table.entries.keys().map(|(c, _)| c.as_str()).collect();
    assert_eq!(capabilities.len(), 10);
    assert!(
        read_spec("providers.md").contains("It holds 16 routes over 10 capabilities."),
        "spec/providers.md §10 states the table's size"
    );
}

#[test]
fn every_fingerprint_input_is_as_written_down() {
    let table = table();
    let expected = expected_routes();
    let ids: Vec<String> = table.entries.values().map(Route::id).collect();
    let expected_ids: Vec<String> = expected
        .iter()
        .map(|e| format!("{}@{}", e.model, e.provider))
        .collect();
    assert_eq!(ids, expected_ids);
    for (route, want) in table.entries.values().zip(&expected) {
        let id = route.id();
        assert_eq!(route.capability, want.capability, "{id}");
        assert_eq!(
            (route.model.as_str(), route.provider.as_str()),
            (want.model, want.provider),
            "{id}"
        );
        // JCS distinguishes the string "1" from the integer 1, as the fingerprint does.
        assert_eq!(
            canon(&route.contract),
            canon(&want.contract),
            "{} {id}",
            want.capability
        );
        let input = json!({
            "kind": "fx-route-v1",
            "capability": want.capability,
            "model": want.model,
            "provider": want.provider,
            "contract": want.contract,
        });
        assert_eq!(
            route.fingerprint(),
            digest(&input),
            "{} {id}",
            want.capability
        );
        assert_eq!(
            (route.price.low, route.price.high),
            (usd(want.low), usd(want.high)),
            "{} {id}",
            want.capability
        );
    }
}

#[test]
fn image_contracts_carry_their_behaviour_as_a_string_and_the_rest_as_an_integer() {
    for route in table().entries.values() {
        let behavior = &route.contract["adapter_behavior"];
        if route.capability.starts_with("image.") {
            assert!(behavior.is_string(), "{}", route.id());
        } else {
            assert_eq!(canon(behavior), "1", "{} {}", route.capability, route.id());
            assert!(behavior.is_number(), "{}", route.id());
        }
    }
}

#[test]
fn every_contract_names_the_adapter_that_serves_its_route() {
    let providers_md = read_spec("providers.md");
    for route in table().entries.values() {
        let id = route.id();
        let (adapter, behavior) = contract_for(&route.capability, &route.provider)
            .unwrap_or_else(|| panic!("no adapter serves {} on {id}", route.capability));
        assert_eq!(
            route.contract["adapter"],
            json!(adapter),
            "{} {id}",
            route.capability
        );
        if let Some(behavior) = behavior {
            assert_eq!(
                canon(&route.contract["adapter_behavior"]),
                behavior,
                "{} {id}",
                route.capability
            );
        }
    }
    for (capability, _, adapter, _) in CONTRACTS {
        assert!(
            providers_md.contains(&format!("Contract `adapter`: `{adapter}`")),
            "spec/providers.md §9 names no adapter {adapter} ({capability})"
        );
    }
    assert!(
        !DEFAULT_ROUTES.contains("gnode") && !providers_md.contains("`gnode-"),
        "contracts use FX's adapter names"
    );
}

#[test]
fn every_route_is_served_by_an_adapter_of_its_capability_s_shape() {
    let registry = registry();
    for route in table().entries.values() {
        let id = route.id();
        let shape = capability(&route.capability)
            .unwrap_or_else(|| panic!("{id}: {} is not in CAPABILITIES", route.capability))
            .shape;
        match (registry.serving(&route_ref(route)), shape) {
            (Some(Adapter::Request(_)), Shape::Request)
            | (Some(Adapter::Job(_)), Shape::LongJob) => {}
            (Some(other), shape) => {
                panic!(
                    "{id}: {} is served by {other:?}, not a {shape:?}",
                    route.capability
                )
            }
            (None, _) => panic!("{id}: no adapter serves {}", route.capability),
        }
    }
}

#[test]
fn the_adapters_serve_exactly_the_contracts_of_spec_providers_9() {
    let served: BTreeSet<(String, String)> = registry().served().into_iter().collect();
    let expected: BTreeSet<(String, String)> = CONTRACTS
        .iter()
        .map(|(c, p, _, _)| (c.to_string(), p.to_string()))
        .collect();
    assert_eq!(served, expected);
    for (capability, provider) in &served {
        assert!(
            capability_is_shipped(capability),
            "{capability} on {provider} is not in CAPABILITIES"
        );
    }
    // Served but routed nowhere: background.remove (spec/providers.md §10).
    let routed: BTreeSet<(String, String)> = table()
        .entries
        .values()
        .map(|r| (r.capability.clone(), r.provider.clone()))
        .collect();
    let unrouted: Vec<&(String, String)> = served.difference(&routed).collect();
    assert_eq!(
        unrouted,
        vec![&("background.remove".to_string(), "fal".to_string())]
    );
}

fn capability_is_shipped(name: &str) -> bool {
    CAPABILITIES.iter().any(|c| c.name == name)
}

#[test]
fn every_feature_is_one_its_capability_defines() {
    for route in table().entries.values() {
        let vocabulary = capability(&route.capability).unwrap().features;
        for feature in &route.features {
            assert!(
                vocabulary.contains(&feature.as_str()),
                "{} {}: {feature} is not a feature spec/capabilities.md lists for it",
                route.capability,
                route.id()
            );
        }
    }
}

#[test]
fn features_say_what_the_adapters_honour() {
    let table = table();
    let has = |capability: &str, id: &str, feature: &str| {
        table.entries[&(capability.to_string(), id.to_string())]
            .features
            .contains(feature)
    };
    let openai = "gpt-image-2.5-sunburst@openai";
    let openrouter = "openai/gpt-image-2.5-sunburst@openrouter";
    let fal = "openai/gpt-image-2.5/sunburst@fal";
    // OpenRouter refuses a mask and a transparent background (spec/providers.md §9.2).
    for capability in ["image.generate", "image.edit"] {
        assert!(has(capability, openai, "alpha") && has(capability, fal, "alpha"));
        assert!(!has(capability, openrouter, "alpha"));
    }
    assert!(has("image.edit", openai, "mask") && has("image.edit", fal, "mask"));
    assert!(!has("image.edit", openrouter, "mask"));
    // Every edit route takes input pictures; every generate route refuses references.
    for id in [openai, openrouter, fal] {
        assert!(has("image.edit", id, "image_input"), "{id}");
        assert!(!has("image.generate", id, "image_input"), "{id}");
        assert!(
            !has("image.generate", id, "hosted_url_reference_input"),
            "{id}"
        );
        assert!(has("image.generate", id, "text_to_image"), "{id}");
        assert!(!has("image.edit", id, "text_to_image"), "{id}");
    }
    // No route keeps a name spec/capabilities.md does not list (the predecessor's aliases).
    for alias in [
        "masked_edit",
        "transparent_background",
        "reference_images",
        "data_url_reference_input",
    ] {
        assert!(!DEFAULT_ROUTES.contains(alias), "{alias}");
    }
}

/// routes-caps F4: the guide's example table (docs/guide/examples/routes.yaml) plans offline, so
/// a feature it declares that FX's adapter refuses would plan a step that a live run refuses. Its
/// routes on providers FX serves declare only names their capability defines, and only what the
/// adapter honours; `structured.review` has no features (spec/capabilities.md §13).
#[test]
fn the_guide_s_example_table_declares_only_what_fx_honours() {
    let path = format!(
        "{}/../../docs/guide/examples/routes.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let document =
        grida_fx_core::yaml::load(&text, "routes.yaml").expect("the guide's table loads");
    let guide = RouteTable::from_document(&document, "routes.yaml").expect("the guide's table");
    let registry = registry();
    for route in guide.entries.values() {
        let id = route.id();
        if let Some(capability) = capability(&route.capability) {
            for feature in &route.features {
                assert!(
                    capability.features.contains(&feature.as_str()),
                    "{} {id}: {feature} is not one of its features",
                    route.capability
                );
            }
        }
        if !registry.serves(&route.capability, &route.provider) {
            continue;
        }
        // Every image.generate adapter of FX refuses references (spec/providers.md §9).
        if route.capability == "image.generate" {
            assert!(!route.features.contains("image_input"), "{id}");
        }
        // OpenRouter's image adapter refuses a mask and a transparent background (§9.2).
        if route.provider == "openrouter" && route.capability.starts_with("image.") {
            assert!(!route.features.contains("alpha"), "{id}");
            assert!(!route.features.contains("mask"), "{id}");
        }
    }
    for route in guide.entries.values() {
        if route.capability == "structured.review" {
            assert!(route.features.is_empty(), "{}", route.id());
        }
    }
}

#[test]
fn pacing_is_declared_where_spec_providers_10_says() {
    for route in table().entries.values() {
        let id = route.id();
        let image_paced = route.capability.starts_with("image.")
            && (route.provider == "openai" || route.provider == "openrouter");
        assert_eq!(
            route.requests_per_minute,
            image_paced.then_some(150),
            "{} {id}",
            route.capability
        );
        let concurrency = match (route.capability.as_str(), id.as_str()) {
            ("mesh.generate" | "mesh.rig" | "video.generate", _) => Some(1),
            ("structured.generate", _) => Some(4),
            ("agent.turn", "openai/gpt-6-astra@openrouter") => Some(1),
            _ => None,
        };
        assert_eq!(route.concurrency, concurrency, "{} {id}", route.capability);
    }
}

#[test]
fn prices_and_tiers_parse() {
    for route in table().entries.values() {
        let price = &route.price;
        assert!(price.low <= price.high, "{}", route.id());
        if route.capability == "video.generate" {
            assert_eq!(price.unit, PriceUnit::Second);
            assert_eq!(price.by.as_deref(), Some("resolution"));
            let tiers: Vec<(&str, Usd, Usd)> = price
                .tiers
                .iter()
                .map(|(name, (low, high))| (name.as_str(), *low, *high))
                .collect();
            assert_eq!(
                tiers,
                vec![
                    ("1080p", usd("0.15"), usd("0.1875")),
                    ("360p", usd("0.03"), usd("0.0375")),
                    ("4k", usd("0.3"), usd("0.375")),
                    ("720p", usd("0.1"), usd("0.125")),
                ]
            );
        } else {
            assert_eq!(price.unit, PriceUnit::Call, "{}", route.id());
            assert!(
                price.tiers.is_empty() && price.by.is_none(),
                "{}",
                route.id()
            );
        }
    }
}

#[test]
fn the_video_route_prices_by_second_and_tier() {
    let table = table();
    let video = table
        .resolve(
            "video.generate",
            "google/gemini-omni-flash/v1.1/image-to-video@fal",
        )
        .unwrap();
    let with = |pairs: &[(&str, Val)]| -> IndexMap<String, Val> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    };
    let tier = |name: &str| Val::Str(name.to_string());
    assert_eq!(
        video.cost(&with(&[
            ("duration", Val::Number(8.0)),
            ("resolution", tier("720p"))
        ])),
        (usd("0.80"), usd("1.00"))
    );
    // Duration unknown: the high price covers max_units (10) seconds, the low price nothing.
    assert_eq!(
        video.cost(&with(&[("resolution", tier("4k"))])),
        (Usd::ZERO, usd("3.75"))
    );
    // No tier: the whole range.
    assert_eq!(
        video.cost(&with(&[("duration", Val::Number(8.0))])),
        (usd("0.24"), usd("3.0"))
    );
    // Longer than max_units: the high price is capped, the low price is not.
    assert_eq!(
        video.cost(&with(&[
            ("duration", Val::Number(12.0)),
            ("resolution", tier("360p"))
        ])),
        (usd("0.36"), usd("0.375"))
    );
}

#[test]
fn the_adapters_name_their_contracts_as_the_table_does() {
    let constants: [(&str, &str, &str); 13] = [
        ("image.generate", "openai", openai::ADAPTER),
        ("image.edit", "openai", openai::ADAPTER),
        (
            "image.generate",
            "openrouter",
            openrouter::image::CONTRACT_ADAPTER,
        ),
        (
            "image.edit",
            "openrouter",
            openrouter::image::CONTRACT_ADAPTER,
        ),
        (
            "structured.generate",
            "openrouter",
            openrouter::structured::CONTRACT_ADAPTER,
        ),
        (
            "agent.turn",
            "openrouter",
            openrouter::agent::CONTRACT_ADAPTER,
        ),
        (
            "music.generate",
            "openrouter",
            openrouter::music::CONTRACT_ADAPTER,
        ),
        ("image.generate", "fal", fal::image::CONTRACT_ADAPTER),
        ("video.generate", "fal", fal::video::CONTRACT_ADAPTER),
        (
            "background.remove",
            "fal",
            fal::background::CONTRACT_ADAPTER,
        ),
        ("mesh.generate", "tripo", tripo::mesh::ADAPTER),
        ("mesh.rig", "tripo", tripo::rig::ADAPTER),
        ("sound.generate", "elevenlabs", elevenlabs::sound::ADAPTER),
    ];
    for (capability, provider, constant) in constants {
        let (adapter, _) = contract_for(capability, provider).unwrap();
        assert_eq!(constant, adapter, "{capability} on {provider}");
    }
    assert_eq!(
        contract_for("speech.generate", "elevenlabs").unwrap().0,
        elevenlabs::speech::ADAPTER
    );
    assert_eq!(openai::ADAPTER_BEHAVIOR, "1");
    assert_eq!(openrouter::image::CONTRACT_BEHAVIOR, "3");
}

/// A value of `ty` for a request that only has to pass the capability check (spec/capabilities.md
/// §1): files are stored PNGs.
fn sample(builder: &mut CallBuilder, ty: MemberType) -> Value {
    let mut file = || builder.file("image/png", &media::png(2, 2, None));
    match ty {
        MemberType::Text => json!("text"),
        MemberType::Number => json!(1),
        MemberType::Integer => json!(1),
        MemberType::Boolean => json!(false),
        MemberType::Object => json!({}),
        MemberType::List => json!([]),
        MemberType::File => file(),
        MemberType::Files => json!([file()]),
        MemberType::FilesByName => json!({"front": file()}),
        MemberType::FileOrObject => json!({"type": "object"}),
    }
}

/// Every built-in route gets past its adapter's contract check and the capability check
/// (spec/providers.md §5 steps 1–2): with no keys, each call is refused for its key (step 3), so
/// no table entry points a model at a wire its adapter refuses.
#[test]
fn every_route_passes_its_adapter_s_contract_and_capability_checks() {
    let registry = adapters(&setup(Arc::new(NoNetwork), Keys::none()));
    for route in table().entries.values() {
        let id = route.id();
        let members = capability(&route.capability)
            .unwrap_or_else(|| panic!("{id}: {} is not in CAPABILITIES", route.capability))
            .members;
        let mut builder = CallBuilder::new(&route.capability, &id).contract(route.contract.clone());
        let mut request = serde_json::Map::new();
        for member in members.iter().filter(|m| m.required) {
            let value = sample(&mut builder, member.ty);
            request.insert(member.name.to_string(), value);
        }
        let call = builder.request(Value::Object(request)).build();
        let reason = match registry.serving(&route_ref(route)) {
            Some(Adapter::Request(adapter)) => match block_on(adapter.send(&call)) {
                Sent::Refused { reason } => reason,
                other => panic!("{id} {}: {other:?}", route.capability),
            },
            Some(Adapter::Job(adapter)) => match block_on(adapter.submit(&call)) {
                Submitted::Refused { reason } => reason,
                other => panic!("{id} {}: {other:?}", route.capability),
            },
            None => panic!("{id}: no adapter serves {}", route.capability),
        };
        let key = KeyName::of_provider(&route.provider)
            .unwrap_or_else(|| panic!("{id}: no key names provider {}", route.provider));
        assert_eq!(reason, key.missing(), "{id} {}", route.capability);
    }
}
