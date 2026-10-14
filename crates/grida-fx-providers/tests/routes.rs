//! The built-in route table (spec/providers.md §3) against what it must agree with: the
//! identities written down here, the features of spec/capabilities.md, the contract names of
//! spec/providers.md §2, and the adapters this crate registers.

use grida_fx_core::money::Usd;
use grida_fx_core::routes::{
    BUILT_IN_ADAPTERS, PriceUnit, Route, RouteTable, built_in_adapter, built_in_adapter_named,
};
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
use grida_fx_providers::{codex, elevenlabs, fal, gg, grok, openai, openrouter, tripo};
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

/// The policy both `openai/gpt-6-astra` routes send (src/openrouter/README.md).
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
    let gg_image = json!({"adapter": "gg-image", "adapter_behavior": "1", "quality": "max"});
    let gg_astra_tool_loop = json!({"adapter": "gg-tool-loop", "adapter_behavior": 1,
                                    "image_detail": "high", "reasoning_effort": "high"});
    let gg_astra_structured = json!({"adapter": "gg-structured", "adapter_behavior": 1,
                                     "image_detail": "high", "pictures": "unchanged",
                                     "reasoning_effort": "high"});
    vec![
        expected(
            "agent.turn",
            "openai/gpt-5.6-sol@gg",
            json!({"adapter": "gg-tool-loop", "adapter_behavior": 1}),
            ("0.003", "0.1"),
        ),
        expected(
            "agent.turn",
            "openai/gpt-5.6-sol@openrouter",
            tool_loop,
            ("0.003", "0.1"),
        ),
        expected(
            "agent.turn",
            "openai/gpt-6-astra@gg",
            gg_astra_tool_loop,
            ("0.01", "1.5"),
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
            ("0.09", "0.85"),
        ),
        expected(
            "image.edit",
            "imagegen@codex",
            image_contract("fx-codex-image-v1", "2"),
            ("0", "0"),
        ),
        expected(
            "image.edit",
            "imagine@grok",
            image_contract("fx-grok-image-v1", "1"),
            ("0", "0"),
        ),
        expected(
            "image.edit",
            "openai/gpt-image-2.5-sunburst@gg",
            gg_image.clone(),
            ("0.09", "0.85"),
        ),
        expected(
            "image.edit",
            "openai/gpt-image-2.5-sunburst@openrouter",
            image_contract("fx-openrouter-image-v1", "3"),
            ("0.13", "0.85"),
        ),
        expected(
            "image.edit",
            "openai/gpt-image-2.5/sunburst@fal",
            image_contract("fx-fal-image-v1", "1"),
            ("0.09", "0.85"),
        ),
        expected(
            "image.generate",
            "gpt-image-2.5-sunburst@openai",
            image_contract("fx-openai-image-v1", "1"),
            ("0.09", "0.7"),
        ),
        expected(
            "image.generate",
            "imagegen@codex",
            image_contract("fx-codex-image-v1", "2"),
            ("0", "0"),
        ),
        expected(
            "image.generate",
            "imagine@grok",
            image_contract("fx-grok-image-v1", "1"),
            ("0", "0"),
        ),
        expected(
            "image.generate",
            "openai/gpt-image-2.5-sunburst@gg",
            gg_image,
            ("0.09", "0.7"),
        ),
        expected(
            "image.generate",
            "openai/gpt-image-2.5-sunburst@openrouter",
            image_contract("fx-openrouter-image-v1", "3"),
            ("0.13", "0.7"),
        ),
        expected(
            "image.generate",
            "openai/gpt-image-2.5/sunburst@fal",
            image_contract("fx-fal-image-v1", "1"),
            ("0.09", "0.7"),
        ),
        expected(
            "mesh.generate",
            "P2-20260801@tripo",
            json!({"adapter": "tripo-multiview", "adapter_behavior": 1}),
            ("1.2", "2.5"),
        ),
        expected(
            "mesh.generate",
            "tripo/p2@gg",
            json!({"adapter": "gg-mesh", "adapter_behavior": 1,
                   "endpoint": "three-d.tripo.p2.multiview"}),
            ("1.2", "2.5"),
        ),
        expected(
            "mesh.rig",
            "tripo/rig-v1.0@gg",
            json!({"adapter": "gg-rig", "adapter_behavior": 1,
                   "check_endpoint": "three-d.tripo.rig-check",
                   "endpoint": "three-d.tripo.rig-v1.0.mesh"}),
            ("0.25", "0.5"),
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
            "openai/gpt-5.6-sol@gg",
            json!({"adapter": "gg-structured", "adapter_behavior": 1}),
            ("0.02", "0.6"),
        ),
        expected(
            "structured.generate",
            "openai/gpt-5.6-sol@openrouter",
            structured,
            ("0.02", "0.6"),
        ),
        expected(
            "structured.generate",
            "openai/gpt-6-astra@gg",
            gg_astra_structured,
            ("0.05", "1.5"),
        ),
        expected(
            "structured.generate",
            "openai/gpt-6-astra@openrouter",
            astra_structured,
            ("0.05", "1.5"),
        ),
        expected(
            "video.generate",
            "alibaba/wan-3.0@gg",
            json!({"adapter": "gg-video", "adapter_behavior": 1}),
            ("0.05", "0.2"),
        ),
        expected(
            "video.generate",
            "google/gemini-omni-flash/v1.1/image-to-video@fal",
            json!({"adapter": "fal-queue", "adapter_behavior": 1}),
            ("0.03", "0.375"),
        ),
    ]
}

/// The contract the default adapter of `capability` on `provider` serves, its `adapter` and its
/// `adapter_behavior` as JSON: core's table of FX's adapters, which completes a route's contract
/// (spec/identity.md §7) and which `core_s_table_is_every_adapter_s_identity` holds to the
/// adapters.
fn contract_for(capability: &str, provider: &str) -> Option<(&'static str, &'static str)> {
    built_in_adapter(capability, provider).map(|served| (served.adapter, served.behavior))
}

/// The behaviour, as JSON, of the adapter of `capability` on `provider` named `adapter`, default
/// or not.
fn behavior_of(capability: &str, provider: &str, adapter: &str) -> Option<&'static str> {
    built_in_adapter_named(capability, provider, adapter).map(|served| served.behavior)
}

fn route_ref(route: &Route) -> RouteRef {
    RouteRef::of(route)
}

fn read_spec(name: &str) -> String {
    let path = format!("{}/../../spec/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// One row of spec/providers.md §2's tables of built-in adapters.
struct Written {
    capability: String,
    provider: String,
    adapter: String,
    /// The behaviour as the table writes it in JSON.
    behavior: String,
    /// The adapter's reference, relative to spec/.
    reference: String,
    /// Whether the row is in the first table, its pair's default.
    default: bool,
}

/// The rows of spec/providers.md §2's tables of built-in adapters: the defaults, then the
/// adapters a route names.
fn written_adapters() -> Vec<Written> {
    let text = read_spec("providers.md");
    let start = text
        .find("## 2. The built-in adapters")
        .expect("providers.md §2");
    let end = text[start..]
        .find("\n## 3. ")
        .map_or(text.len(), |at| start + at);
    let section = &text[start..end];
    let code = |cell: &str| cell.trim_matches('`').to_string();
    let mut tables = 0;
    section
        .lines()
        .filter_map(|line| {
            tables += usize::from(line.starts_with("| Provider | Capability |"));
            line.starts_with("| `").then_some((line, tables))
        })
        .map(|(line, tables)| {
            let cells: Vec<&str> = line.trim_matches('|').split(" | ").map(str::trim).collect();
            assert_eq!(cells.len(), 5, "{line}");
            let reference = cells[4].split_once("](").expect("a link").1;
            Written {
                provider: code(cells[0]),
                capability: code(cells[1]),
                adapter: code(cells[2]),
                behavior: code(cells[3]),
                reference: reference.trim_end_matches(')').to_string(),
                default: tables == 1,
            }
        })
        .collect()
}

#[test]
fn the_table_holds_29_routes_over_10_capabilities() {
    let table = table();
    assert_eq!(table.entries.len(), 29);
    let capabilities: BTreeSet<&str> = table.entries.keys().map(|(c, _)| c.as_str()).collect();
    assert_eq!(capabilities.len(), 10);
    assert!(
        read_spec("providers.md").contains("It holds 29 routes over 10 capabilities."),
        "spec/providers.md §3 states the table's size"
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

/// Every built-in route's fingerprint, as literal digests, in the table's order: what every cache
/// key a built-in route serves is made from. Filling in a contract (spec/identity.md §7) moved
/// none of them, and nothing else may.
const FINGERPRINTS: [(&str, &str, &str); 29] = [
    (
        "agent.turn",
        "openai/gpt-5.6-sol@gg",
        "fae706d0d52419b2d3177bf3bb8634011c8c76b5b530955102bd35627ed14b81",
    ),
    (
        "agent.turn",
        "openai/gpt-5.6-sol@openrouter",
        "e2afda9cdf14b34034d12a304e459979801551ed7305b5f26bca20f10fbf2d5b",
    ),
    (
        "agent.turn",
        "openai/gpt-6-astra@gg",
        "939609e3a5cf5b56a15ca16a87fe05e067de494f7f5f1be272054b7a29e04c50",
    ),
    (
        "agent.turn",
        "openai/gpt-6-astra@openrouter",
        "c30b6b8de0fd741497fd5607fd41ef90dc377c218a9a0d2b1936a1f217b9c512",
    ),
    (
        "image.edit",
        "gpt-image-2.5-sunburst@openai",
        "c456b1963f44d5f035e41b27cce8b7002164f3476073cf517a436260e8899de5",
    ),
    (
        "image.edit",
        "imagegen@codex",
        "e39d67200ddf392c3ac296da47a08b3392aa80cefb400e8835c345c22ea54375",
    ),
    (
        "image.edit",
        "imagine@grok",
        "2789bb47a0b99c2fd47cc84520b381cecdecfae1b405fe6bae282a5b32b95146",
    ),
    (
        "image.edit",
        "openai/gpt-image-2.5-sunburst@gg",
        "d6e0f4120166ff3d981d4e6b192eb849cf5bc0c8d898787f363eef44402b7480",
    ),
    (
        "image.edit",
        "openai/gpt-image-2.5-sunburst@openrouter",
        "a5eabba76d5f2f9050a921604945a82b5b941b5ebd1800addf8d11c908bfbc6c",
    ),
    (
        "image.edit",
        "openai/gpt-image-2.5/sunburst@fal",
        "73ff8b2a07903cf80f5b42e1f934286605506ae99bebe70bd84e5a42666d19b4",
    ),
    (
        "image.generate",
        "gpt-image-2.5-sunburst@openai",
        "dda66fe634f878cd82b88bc74d74eb7f571540113cd5006cb591547cf29f4236",
    ),
    (
        "image.generate",
        "imagegen@codex",
        "dbb0a7d6073422b1dcf2f9cb0fd12a3b586a8a22e63159e0e6143dd0813cec7c",
    ),
    (
        "image.generate",
        "imagine@grok",
        "075c3cee74d095200087f251fe912544e0bb4864ca22ae39f920175570008bbd",
    ),
    (
        "image.generate",
        "openai/gpt-image-2.5-sunburst@gg",
        "ecb54b4cc9d53094f25276cd8ba478ecde1e520b28b73815cea014f6b786fe49",
    ),
    (
        "image.generate",
        "openai/gpt-image-2.5-sunburst@openrouter",
        "472dbf587d1a20f7ed495e9e923e372f5380eb82173203f524e09602f5548ee6",
    ),
    (
        "image.generate",
        "openai/gpt-image-2.5/sunburst@fal",
        "f44fdcd62fdb65f5e46190ec7e87a498cd8584136c829705c2ec350e5987cb5f",
    ),
    (
        "mesh.generate",
        "P2-20260801@tripo",
        "2f38aab5c2e1e9a7a38b7a169f5fdfd1731678d2edd517b1d560629b69031e7a",
    ),
    (
        "mesh.generate",
        "tripo/p2@gg",
        "87a940723d51b345e53f66234d0f4ec047197f0be0380f77943654647d7df5bf",
    ),
    (
        "mesh.rig",
        "tripo/rig-v1.0@gg",
        "fa7536bf483efdcc38f31f4778c41e82f010f201fa8497efb9240fbbdf4d8318",
    ),
    (
        "mesh.rig",
        "v1.0-20240301@tripo",
        "4d81220039a7c3b2a13514b85643763d5c5b951258a98fe46e4f80d62fb8f602",
    ),
    (
        "music.generate",
        "google/lyria-3-pro-preview@openrouter",
        "59420dcc03ca6b761770613d948da6897c0e243fb35ac3345bcc24c840a61702",
    ),
    (
        "sound.generate",
        "eleven_text_to_sound_v2@elevenlabs",
        "b49122935dffdcb64bfa37f0bfc82210d2fa15cfb5a9b2c678ebc81db722d01b",
    ),
    (
        "speech.generate",
        "eleven_v3@elevenlabs",
        "630cbb2737943f0faaa6bd018e40b2eb1ecdc870e9d4696a7b80f8390146543c",
    ),
    (
        "structured.generate",
        "openai/gpt-5.6-sol@gg",
        "ce4e944398eafc9aec5055abc09f7f18813d4bbeea26b32a239c27ff845b08a6",
    ),
    (
        "structured.generate",
        "openai/gpt-5.6-sol@openrouter",
        "a405c28fb5be2b3e74e58a62a0b943ddcde92f77f7aafbb5ceafa90ec0563461",
    ),
    (
        "structured.generate",
        "openai/gpt-6-astra@gg",
        "79fc40ba6f22797a3162a93b442fa747218065d94fcc8440a58dc3bb5ac95015",
    ),
    (
        "structured.generate",
        "openai/gpt-6-astra@openrouter",
        "849f6d6cc549969e37b91b4ba028edb7113c52bdd9b6fe170479ab9dd3c0c718",
    ),
    (
        "video.generate",
        "alibaba/wan-3.0@gg",
        "2760d5dee58f8bf06ea59d7ce750bd0cc8d67c92947e1027a81e51506b29d8e4",
    ),
    (
        "video.generate",
        "google/gemini-omni-flash/v1.1/image-to-video@fal",
        "8aed505f280948fe25cc885d9e9dabe770f6b523d7a6b0aa08d74f1b99756226",
    ),
];

#[test]
fn no_built_in_fingerprint_moves() {
    let table = table();
    let found: Vec<(&str, String, String)> = table
        .entries
        .values()
        .map(|route| (route.capability.as_str(), route.id(), route.fingerprint()))
        .collect();
    let pinned: Vec<(&str, String, String)> = FINGERPRINTS
        .iter()
        .map(|(capability, id, fingerprint)| (*capability, id.to_string(), fingerprint.to_string()))
        .collect();
    assert_eq!(found, pinned);
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
    for route in table().entries.values() {
        let id = route.id();
        let adapter = route.contract["adapter"]
            .as_str()
            .unwrap_or_else(|| panic!("{} {id} names no adapter", route.capability));
        let behavior = behavior_of(&route.capability, &route.provider, adapter)
            .unwrap_or_else(|| panic!("no adapter {adapter} serves {} on {id}", route.capability));
        assert_eq!(
            canon(&route.contract["adapter_behavior"]),
            behavior,
            "{} {id}",
            route.capability
        );
    }
    // The OpenRouter and fal image routes name GPT Image's adapters, not their generic defaults.
    for route in table().entries.values() {
        let gpt = match (route.capability.as_str(), route.provider.as_str()) {
            ("image.generate" | "image.edit", "openrouter") => openrouter::image::CONTRACT_ADAPTER,
            ("image.generate" | "image.edit", "fal") => fal::image::CONTRACT_ADAPTER,
            _ => continue,
        };
        assert_eq!(route.contract["adapter"], json!(gpt), "{}", route.id());
    }
}

#[test]
fn the_contracts_are_spec_providers_2_and_each_reference_heads_with_its_own() {
    let written = written_adapters();
    let rows: Vec<(&str, &str, &str, &str, bool)> = written
        .iter()
        .map(|w| {
            (
                w.capability.as_str(),
                w.provider.as_str(),
                w.adapter.as_str(),
                w.behavior.as_str(),
                w.default,
            )
        })
        .collect();
    // Core's table lists the rows of spec/providers.md §2, in its order, defaults first.
    let ours: Vec<(&str, &str, &str, &str, bool)> = BUILT_IN_ADAPTERS
        .iter()
        .map(|served| {
            (
                served.capability,
                served.provider,
                served.adapter,
                served.behavior,
                served.default,
            )
        })
        .collect();
    assert_eq!(rows, ours, "spec/providers.md §2");
    for w in &written {
        let path = format!("{}/../../spec/{}", env!("CARGO_MANIFEST_DIR"), w.reference);
        let reference = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let row = format!("| `{}` | `{}` |", w.adapter, w.behavior);
        assert!(
            reference
                .lines()
                .any(|line| line.ends_with(&row) && line.contains(&format!("`{}`", w.capability))),
            "{}: no row for {} {row}",
            w.reference,
            w.capability
        );
    }
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
fn the_adapters_serve_exactly_the_contracts_of_spec_providers_2() {
    let registry = registry();
    let served: BTreeSet<(String, String)> = registry.served().into_iter().collect();
    let expected: BTreeSet<(String, String)> = BUILT_IN_ADAPTERS
        .iter()
        .map(|served| (served.capability.to_string(), served.provider.to_string()))
        .collect();
    assert_eq!(served, expected);
    // Each adapter under its contract `adapter`, and the defaults, as the table lists them.
    let registered: BTreeSet<(&str, &str, &str, bool)> = registry
        .registered()
        .iter()
        .map(|r| (r.capability, r.provider, r.identity.adapter, r.default))
        .collect();
    let listed: BTreeSet<(&str, &str, &str, bool)> = BUILT_IN_ADAPTERS
        .iter()
        .map(|served| {
            (
                served.capability,
                served.provider,
                served.adapter,
                served.default,
            )
        })
        .collect();
    assert_eq!(registered, listed);
    for (capability, provider) in &served {
        assert!(
            capability_is_shipped(capability),
            "{capability} on {provider} is not in CAPABILITIES"
        );
    }
    // Served but routed nowhere: background.remove (spec/providers.md §3).
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

/// Core completes a route's contract from its table of FX's adapters when a table is read
/// (spec/identity.md §7), knowing nothing of this crate: every adapter registered here names the
/// contract that table gives its capability and provider, behaviour included, so a completed
/// contract is always one its adapter serves.
#[test]
fn core_s_table_is_every_adapter_s_identity() {
    let registry = registry();
    assert_eq!(registry.registered().len(), BUILT_IN_ADAPTERS.len());
    let read = |capability: &str, provider: &str, contract: Value| {
        let table = RouteTable::from_document(
            &json!({"fx": "routes/v1", "routes": [
                {"capability": capability, "route": format!("m@{provider}"), "price": {"usd": 0},
                 "contract": contract},
            ]}),
            "routes.yaml",
        )
        .unwrap();
        table.entries[0].contract.clone()
    };
    for registered in registry.registered() {
        let (capability, provider, identity) = (
            registered.capability,
            registered.provider,
            registered.identity,
        );
        let served =
            built_in_adapter_named(capability, provider, identity.adapter).unwrap_or_else(|| {
                panic!(
                    "core knows no adapter {} of {capability} on {provider}",
                    identity.adapter
                )
            });
        assert_eq!(
            (served.behavior, served.default),
            (identity.behavior, registered.default),
            "{} for {capability} on {provider}",
            identity.adapter
        );
        // A route that names the adapter alone is completed to its behaviour.
        let contract = read(capability, provider, json!({"adapter": identity.adapter}));
        assert!(
            identity.behaves(&contract["adapter_behavior"]),
            "{} for {capability} on {provider}",
            identity.adapter
        );
        if !registered.default {
            continue;
        }
        // A route declared with no contract is completed to one its default serves, and the
        // registry hands it that default.
        let contract = read(capability, provider, json!({}));
        assert_eq!(contract["adapter"], json!(identity.adapter));
        assert!(
            identity.behaves(&contract["adapter_behavior"]),
            "{capability} on {provider}"
        );
        let route = RouteRef {
            capability: capability.into(),
            model: "m".into(),
            provider: provider.into(),
            contract,
            limits: Default::default(),
        };
        assert_eq!(
            registry.serving(&route).and_then(Adapter::identity),
            Some(identity),
            "{capability} on {provider}"
        );
    }
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
    let gg = "openai/gpt-image-2.5-sunburst@gg";
    let openai = "gpt-image-2.5-sunburst@openai";
    let openrouter = "openai/gpt-image-2.5-sunburst@openrouter";
    let fal = "openai/gpt-image-2.5/sunburst@fal";
    // OpenRouter refuses a mask and a transparent background (src/openrouter/README.md); GG takes
    // a transparent background and refuses a mask (src/gg/README.md).
    for capability in ["image.generate", "image.edit"] {
        assert!(has(capability, openai, "alpha") && has(capability, fal, "alpha"));
        assert!(has(capability, gg, "alpha"));
        assert!(!has(capability, openrouter, "alpha"));
    }
    // Codex makes every background and draws from pictures, but takes no exact size and no
    // mask (src/codex/README.md).
    let codex = "imagegen@codex";
    for capability in ["image.generate", "image.edit"] {
        for feature in [
            "flexible_size",
            "alpha",
            "auto_background",
            "opaque_background",
            "image_input",
        ] {
            assert!(has(capability, codex, feature), "{capability} {feature}");
        }
        for feature in ["exact_size", "custom_exact_size", "mask"] {
            assert!(!has(capability, codex, feature), "{capability} {feature}");
        }
    }
    assert!(has("image.generate", codex, "text_to_image"));
    assert!(!has("image.edit", codex, "text_to_image"));
    // Grok Build draws opaque JPEGs from pictures, sent as PNGs, and takes no exact size, no
    // transparency and no mask (src/grok/README.md).
    let grok = "imagine@grok";
    for capability in ["image.generate", "image.edit"] {
        for feature in [
            "flexible_size",
            "auto_background",
            "opaque_background",
            "image_input",
            "png_output",
        ] {
            assert!(has(capability, grok, feature), "{capability} {feature}");
        }
        for feature in ["exact_size", "custom_exact_size", "mask", "alpha"] {
            assert!(!has(capability, grok, feature), "{capability} {feature}");
        }
    }
    assert!(has("image.generate", grok, "text_to_image"));
    assert!(!has("image.edit", grok, "text_to_image"));
    assert!(has("image.edit", openai, "mask") && has("image.edit", fal, "mask"));
    assert!(!has("image.edit", openrouter, "mask") && !has("image.edit", gg, "mask"));
    // Every edit route takes input pictures; every other generate route refuses references.
    for id in [gg, openai, openrouter, fal] {
        assert!(has("image.edit", id, "image_input"), "{id}");
        assert!(!has("image.generate", id, "image_input"), "{id}");
        assert!(has("image.generate", id, "text_to_image"), "{id}");
        assert!(!has("image.edit", id, "text_to_image"), "{id}");
    }
    // No route keeps a renamed feature (spec/providers.md §4).
    for alias in [
        "masked_edit",
        "transparent_background",
        "reference_images",
        "data_url_reference_input",
    ] {
        assert!(!DEFAULT_ROUTES.contains(alias), "{alias}");
    }
}

/// routes-caps F4: an example's own route table (examples/<name>/routes.yaml) plans offline, so a
/// feature it declares that FX's adapter refuses would plan a step that a live run refuses. Its
/// routes on providers FX serves declare only names their capability defines, and only what the
/// adapter honours; `structured.review` has no features (spec/capabilities.md §13).
#[test]
fn every_example_table_declares_only_what_fx_honours() {
    let examples = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut tables: Vec<_> = std::fs::read_dir(&examples)
        .unwrap_or_else(|e| panic!("{examples}: {e}"))
        .map(|entry| entry.expect("an example folder").path().join("routes.yaml"))
        .filter(|path| path.is_file())
        .collect();
    tables.sort();
    assert!(!tables.is_empty(), "no example has a routes.yaml");
    let registry = registry();
    for path in tables {
        let label = path.display().to_string();
        let text = std::fs::read(&path).unwrap_or_else(|e| panic!("{label}: {e}"));
        let document = grida_fx_core::yaml::load(&text, "routes.yaml").expect("the table loads");
        let table = RouteTable::from_document(&document, "routes.yaml").expect("the table");
        for route in table.entries.values() {
            let id = route.id();
            if let Some(capability) = capability(&route.capability) {
                for feature in &route.features {
                    assert!(
                        capability.features.contains(&feature.as_str()),
                        "{label}: {} {id}: {feature} is not one of its features",
                        route.capability
                    );
                }
            }
            if route.capability == "structured.review" {
                assert!(route.features.is_empty(), "{label}: {id}");
            }
            if !registry.serves(&route.capability, &route.provider) {
                continue;
            }
            // Every image.generate adapter of FX refuses references but Codex's, Grok Build's and
            // OpenRouter's generic one (spec/providers.md §2), so the adapter that serves the
            // route decides.
            if route.capability == "image.generate" {
                let serving = registry
                    .serving_contract(&route.capability, &route.provider, &route.contract)
                    .and_then(Adapter::identity)
                    .map(|identity| identity.adapter);
                let takes_references = [
                    codex::ADAPTER,
                    grok::ADAPTER,
                    openrouter::generic_image::CONTRACT_ADAPTER,
                ];
                if !serving.is_some_and(|adapter| takes_references.contains(&adapter)) {
                    assert!(!route.features.contains("image_input"), "{label}: {id}");
                }
            }
            // OpenRouter's image adapter refuses a mask and a transparent background
            // (src/openrouter/README.md).
            if route.provider == "openrouter" && route.capability.starts_with("image.") {
                assert!(!route.features.contains("alpha"), "{label}: {id}");
                assert!(!route.features.contains("mask"), "{label}: {id}");
            }
        }
    }
}

#[test]
fn pacing_is_declared_where_spec_providers_3_says() {
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
            // One Codex draws several images at once (spec/providers.md §3).
            ("image.generate" | "image.edit", "imagegen@codex") => Some(3),
            // So does one Grok Build (spec/providers.md §3).
            ("image.generate" | "image.edit", "imagine@grok") => Some(3),
            // GG keeps one unsettled GPT Image call per organization (spec/providers.md §3).
            ("image.generate" | "image.edit", "openai/gpt-image-2.5-sunburst@gg") => Some(1),
            ("structured.generate", _) => Some(4),
            ("agent.turn", "openai/gpt-6-astra@gg" | "openai/gpt-6-astra@openrouter") => Some(1),
            _ => None,
        };
        assert_eq!(route.concurrency, concurrency, "{} {id}", route.capability);
    }
}

#[test]
fn prices_and_tiers_parse() {
    let plan = registry().plan_providers();
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
            let expected = match route.provider.as_str() {
                "gg" => vec![
                    ("1080p", usd("0.2"), usd("0.2")),
                    ("480p", usd("0.05"), usd("0.05")),
                    ("720p", usd("0.1"), usd("0.2")),
                ],
                _ => vec![
                    ("1080p", usd("0.15"), usd("0.1875")),
                    ("360p", usd("0.03"), usd("0.0375")),
                    ("4k", usd("0.3"), usd("0.375")),
                    ("720p", usd("0.1"), usd("0.125")),
                ],
            };
            assert_eq!(tiers, expected, "{}", route.id());
        } else if plan.contains(&route.provider) {
            // Free to FX (spec/providers.md §3).
            assert_eq!((price.low, price.high), (Usd::ZERO, Usd::ZERO));
            assert_eq!(price.unit, PriceUnit::Call);
            assert!(price.tiers.is_empty() && price.by.is_none());
        } else if route.capability.starts_with("image.") {
            assert_eq!(price.unit, PriceUnit::Call, "{}", route.id());
            assert_eq!(price.by.as_deref(), Some("size"), "{}", route.id());
            let tiers: Vec<(&str, Usd, Usd)> = price
                .tiers
                .iter()
                .map(|(name, (low, high))| (name.as_str(), *low, *high))
                .collect();
            assert_eq!(
                tiers,
                image_tiers(&route.capability, &route.provider),
                "{} {}",
                route.capability,
                route.id()
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

/// The size tiers of the OpenAI and fal image routes (one model, one envelope): the size, the
/// low price, the high price of a generate, and of an edit. A low is the output at quality max,
/// measured or derived from a measured bill, rounded down to the cent; a high adds $0.07 for a
/// generate's prompt, or $0.21 for an edit's 16 input pictures and long prompt, rounded up
/// (spec/providers.md §3).
const IMAGE_TIERS: &[(&str, &str, &str, &str)] = &[
    ("1024x1024", "0.21", "0.29", "0.43"),
    ("1024x1536", "0.16", "0.24", "0.38"),
    ("1152x1024", "0.19", "0.27", "0.41"),
    ("1152x2496", "0.14", "0.22", "0.36"),
    ("1536x1024", "0.16", "0.24", "0.38"),
    ("1536x1536", "0.30", "0.38", "0.52"),
    ("1712x2560", "0.28", "0.36", "0.50"),
    ("2064x1008", "0.13", "0.21", "0.35"),
    ("2464x3328", "0.52", "0.60", "0.74"),
    ("2496x1152", "0.14", "0.22", "0.36"),
    ("2560x1440", "0.22", "0.30", "0.44"),
    ("2560x1712", "0.28", "0.36", "0.50"),
    ("2880x960", "0.10", "0.18", "0.32"),
];

/// The OpenRouter image routes' tiers: only the sizes it serves, with highs that cover twice the
/// output (spec/providers.md §3).
const OPENROUTER_IMAGE_TIERS: &[(&str, &str, &str, &str)] = &[
    ("1024x1024", "0.21", "0.50", "0.64"),
    ("1152x2496", "0.14", "0.37", "0.51"),
    ("1712x2560", "0.28", "0.65", "0.79"),
    ("2064x1008", "0.13", "0.34", "0.48"),
    ("2496x1152", "0.14", "0.37", "0.51"),
    ("2560x1440", "0.22", "0.52", "0.66"),
    ("2560x1712", "0.28", "0.65", "0.79"),
];

/// An image route's tiers as `(size, low, high)`, in the table's order.
fn image_tiers(capability: &str, provider: &str) -> Vec<(&'static str, Usd, Usd)> {
    let tiers = match provider {
        "openrouter" => OPENROUTER_IMAGE_TIERS,
        _ => IMAGE_TIERS,
    };
    tiers
        .iter()
        .map(|(size, low, generate, edit)| {
            let high = if capability == "image.edit" {
                edit
            } else {
                generate
            };
            (*size, usd(low), usd(high))
        })
        .collect()
}

#[test]
fn image_tiers_name_sizes_their_routes_serve() {
    let table = table();
    let sizes = |id: &str| -> BTreeSet<String> {
        table.entries[&("image.generate".to_string(), id.to_string())]
            .price
            .tiers
            .keys()
            .cloned()
            .collect()
    };
    let served: BTreeSet<String> = openrouter::image::ALLOWED_SIZES
        .iter()
        .map(|size| size.to_string())
        .collect();
    assert_eq!(sizes("openai/gpt-image-2.5-sunburst@openrouter"), served);
    for id in [
        "openai/gpt-image-2.5-sunburst@gg",
        "gpt-image-2.5-sunburst@openai",
        "openai/gpt-image-2.5/sunburst@fal",
    ] {
        for size in sizes(id) {
            let (width, height) = size.split_once('x').expect("WxH");
            let (width, height) = (width.parse().unwrap(), height.parse().unwrap());
            assert_eq!(
                grida_fx_providers::wire::check_size_envelope(width, height, "OpenAI"),
                Ok(()),
                "{id} {size}"
            );
        }
    }
}

#[test]
fn the_image_routes_price_by_size() {
    let table = table();
    let plan = registry().plan_providers();
    let with = |size: Option<&str>| -> IndexMap<String, Val> {
        let mut with = IndexMap::new();
        with.insert("prompt".to_string(), Val::Str("a kite".to_string()));
        if let Some(size) = size {
            with.insert("size".to_string(), Val::Str(size.to_string()));
        }
        with
    };
    let mut checked = 0;
    for route in table.entries.values() {
        if !route.capability.starts_with("image.") || plan.contains(&route.provider) {
            continue;
        }
        let id = route.id();
        let (_, low, high) = image_tiers(&route.capability, &route.provider)
            .into_iter()
            .find(|(size, _, _)| *size == "1024x1024")
            .expect("every image route has a 1024x1024 tier");
        assert_eq!(
            route.cost(&with(Some("1024x1024"))),
            (low, high),
            "{} {id}",
            route.capability
        );
        // A size the tiers do not list, `auto`, or none: the whole range.
        let whole = (route.price.low, route.price.high);
        for size in [Some("2048x2048"), Some("auto"), None] {
            assert_eq!(
                route.cost(&with(size)),
                whole,
                "{} {id} {size:?}",
                route.capability
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 8);
    let openai = |capability: &str, size: &str| {
        table
            .resolve(capability, "gpt-image-2.5-sunburst@openai")
            .unwrap()
            .cost(&with(Some(size)))
    };
    assert_eq!(
        openai("image.generate", "1024x1024"),
        (usd("0.21"), usd("0.29"))
    );
    assert_eq!(
        openai("image.edit", "1024x1024"),
        (usd("0.21"), usd("0.43"))
    );
    assert_eq!(
        openai("image.generate", "2048x2048"),
        (usd("0.09"), usd("0.70"))
    );
    assert_eq!(openai("image.edit", "auto"), (usd("0.09"), usd("0.85")));
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
    let constants: [(&str, &str, &str); 24] = [
        ("image.generate", "gg", gg::image::ADAPTER),
        ("image.edit", "gg", gg::image::ADAPTER),
        ("structured.generate", "gg", gg::chat::STRUCTURED_ADAPTER),
        ("agent.turn", "gg", gg::chat::TURN_ADAPTER),
        ("video.generate", "gg", gg::video::ADAPTER),
        ("mesh.generate", "gg", gg::mesh::MESH_ADAPTER),
        ("mesh.rig", "gg", gg::mesh::RIG_ADAPTER),
        ("image.generate", "openai", openai::ADAPTER),
        ("image.edit", "openai", openai::ADAPTER),
        (
            "image.generate",
            "openrouter",
            openrouter::generic_image::CONTRACT_ADAPTER,
        ),
        (
            "image.edit",
            "openrouter",
            openrouter::generic_image::CONTRACT_ADAPTER,
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
        (
            "image.generate",
            "fal",
            fal::generic_image::CONTRACT_ADAPTER,
        ),
        ("image.generate", "codex", codex::ADAPTER),
        ("image.edit", "codex", codex::ADAPTER),
        ("image.generate", "grok", grok::ADAPTER),
        ("image.edit", "grok", grok::ADAPTER),
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
    // The contracts the engine checks: each behaviour is the table's, as JSON text.
    let identities = [
        ("image.generate", "gg", gg::image::IDENTITY),
        ("structured.generate", "gg", gg::chat::STRUCTURED_IDENTITY),
        ("agent.turn", "gg", gg::chat::TURN_IDENTITY),
        ("video.generate", "gg", gg::video::IDENTITY),
        ("mesh.generate", "gg", gg::mesh::MESH_IDENTITY),
        ("mesh.rig", "gg", gg::mesh::RIG_IDENTITY),
        ("image.generate", "openai", openai::IDENTITY),
        (
            "image.generate",
            "openrouter",
            openrouter::generic_image::IDENTITY,
        ),
        (
            "structured.generate",
            "openrouter",
            openrouter::structured::IDENTITY,
        ),
        ("agent.turn", "openrouter", openrouter::agent::IDENTITY),
        ("music.generate", "openrouter", openrouter::music::IDENTITY),
        ("image.generate", "fal", fal::generic_image::IDENTITY),
        ("video.generate", "fal", fal::video::IDENTITY),
        ("background.remove", "fal", fal::background::IDENTITY),
        ("mesh.generate", "tripo", tripo::mesh::IDENTITY),
        ("mesh.rig", "tripo", tripo::rig::IDENTITY),
        ("image.generate", "codex", codex::IDENTITY),
        ("image.generate", "grok", grok::IDENTITY),
        (
            "sound.generate",
            "elevenlabs",
            elevenlabs::identity(elevenlabs::sound::ADAPTER),
        ),
        (
            "speech.generate",
            "elevenlabs",
            elevenlabs::identity(elevenlabs::speech::ADAPTER),
        ),
    ];
    for (capability, provider, identity) in identities {
        assert_eq!(
            contract_for(capability, provider),
            Some((identity.adapter, identity.behavior)),
            "{capability} on {provider}"
        );
    }
    // The adapters a route names: GPT Image's, on both capabilities.
    for (provider, identity) in [
        ("openrouter", openrouter::image::IDENTITY),
        ("fal", fal::image::IDENTITY),
    ] {
        for capability in ["image.generate", "image.edit"] {
            let served = built_in_adapter_named(capability, provider, identity.adapter).unwrap();
            assert_eq!(
                served.behavior, identity.behavior,
                "{capability} on {provider}"
            );
            assert!(!served.default, "{capability} on {provider}");
        }
    }
}

/// spec/providers.md §3: a route that takes input pictures declares the most its adapter takes:
/// the adapter's own limit, or for GG, which holds an edit to its route's, GPT Image's; a route
/// that takes none declares nothing.
#[test]
fn input_picture_limits_are_the_adapters_own() {
    for route in table().entries.values() {
        let id = route.id();
        let limit = match (route.capability.as_str(), route.provider.as_str()) {
            ("image.generate" | "image.edit", "codex") => Some(codex::MAX_PICTURES),
            ("image.generate" | "image.edit", "grok") => Some(grok::MAX_PICTURES),
            ("image.edit", "openai") => Some(openai::MAX_INPUT_IMAGES),
            ("image.edit", "openrouter") => Some(openrouter::image::MAX_PICTURES),
            ("image.edit", "fal") => Some(fal::image::MAX_PICTURES),
            ("image.edit", "gg") => Some(gg::image::MAX_INPUT_IMAGES),
            _ => None,
        };
        assert_eq!(
            route.max_input_pictures.map(|n| n as usize),
            limit,
            "{} {id}",
            route.capability
        );
    }
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
/// (spec/adapters.md §4 steps 1–2): with no keys, each call is refused for its key (step 3), or
/// the Codex and Grok Build routes for their programs, which no test setup has, so no table entry
/// points a model at a wire its adapter refuses.
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
        if route.provider == "codex" {
            assert_eq!(reason, codex::NOT_INSTALLED, "{id}");
            continue;
        }
        if route.provider == "grok" {
            assert_eq!(reason, grok::NOT_INSTALLED, "{id}");
            continue;
        }
        let key = KeyName::of_provider(&route.provider)
            .unwrap_or_else(|| panic!("{id}: no key names provider {}", route.provider));
        assert_eq!(reason, key.missing(), "{id} {}", route.capability);
    }
}
