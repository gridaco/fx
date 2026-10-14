//! The engine's checks before a send (spec/adapters.md §4 steps 1 and 2) against the adapters FX
//! ships: every adapter names the contract spec/providers.md §2 gives it, every built-in route
//! passes the engine's check, and for every call the engine refuses, the adapter called directly
//! refuses with the same sentence, so moving the checks to the engine changes no refusal and no
//! order.

use grida_fx_providers::adapter::{Adapter, Identity, RouteRef, Sent, Submitted};
use grida_fx_providers::capabilities::{MemberType, capability};
use grida_fx_providers::checks;
use grida_fx_providers::keys::{KeyName, Keys};
use grida_fx_providers::live::adapters;
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::routes::default_table;
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup};
use grida_fx_providers::transport::replay::NoNetwork;
use grida_fx_providers::{codex, grok};
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// FX's adapters as a live invocation registers them, with no keys, over a transport that fails
/// any send.
fn registry() -> Adapters {
    adapters(&setup(Arc::new(NoNetwork), Keys::none()))
}

fn route(capability: &str, provider: &str, contract: Value) -> RouteRef {
    RouteRef {
        capability: capability.into(),
        model: "m".into(),
        provider: provider.into(),
        contract,
        limits: Default::default(),
    }
}

/// The adapter serving a route of `capability` on `provider` that names `adapter`, or names none.
fn serving(
    registry: &Adapters,
    capability: &str,
    provider: &str,
    adapter: Option<&str>,
) -> Adapter {
    let contract = adapter.map_or(Value::Null, |adapter| json!({"adapter": adapter}));
    registry
        .serving(&route(capability, provider, contract))
        .unwrap_or_else(|| panic!("no adapter serves {capability} on {provider} ({adapter:?})"))
        .clone()
}

fn read_spec(name: &str) -> String {
    let path = format!("{}/../../spec/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// `(provider, capability, adapter, behaviour as JSON)` for each row of spec/providers.md §2's
/// two tables, the defaults first.
fn written_adapters() -> Vec<(String, String, String, String)> {
    let text = read_spec("providers.md");
    let start = text
        .find("## 2. The built-in adapters")
        .expect("providers.md §2");
    let end = text[start..]
        .find("\n## 3. ")
        .map_or(text.len(), |at| start + at);
    let code = |cell: &str| cell.trim_matches('`').to_string();
    text[start..end]
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split(" | ").map(str::trim).collect();
            (
                code(cells[0]),
                code(cells[1]),
                code(cells[2]),
                code(cells[3]),
            )
        })
        .collect()
}

#[test]
fn every_adapter_names_the_contract_spec_providers_2_gives_it() {
    let registry = registry();
    let written = written_adapters();
    assert_eq!(written.len(), registry.registered().len());
    for (provider, capability, adapter, behavior) in written {
        let identity = serving(&registry, &capability, &provider, Some(&adapter))
            .identity()
            .unwrap_or_else(|| panic!("{capability} on {provider} names no contract"));
        assert_eq!(identity.adapter, adapter, "{capability} on {provider}");
        assert_eq!(identity.behavior, behavior, "{capability} on {provider}");
    }
}

#[test]
fn every_built_in_route_passes_the_engines_contract_check() {
    let registry = registry();
    for route in default_table().unwrap().entries.values() {
        let adapter = registry
            .serving(&RouteRef::of(route))
            .unwrap_or_else(|| panic!("no adapter serves {}", route.id()))
            .clone();
        let call = CallBuilder::new(&route.capability, &route.id())
            .contract(route.contract.clone())
            .build();
        let identity = adapter.identity().unwrap();
        assert_eq!(
            checks::contract(&call.route, &identity),
            Ok(()),
            "{}",
            route.id()
        );
        assert_eq!(adapter.check_contract(&call), Ok(()), "{}", route.id());
    }
}

#[test]
fn a_live_registry_keeps_each_adapters_own_contract_judgement() {
    let registry = registry();
    let call = CallBuilder::new("structured.generate", "acme/text-a@openrouter")
        .contract(json!({"pictures": "small"}))
        .build();
    assert_eq!(
        serving(&registry, "structured.generate", "openrouter", None).check_contract(&call),
        Err(concat!(
            "acme/text-a@openrouter is not a structured.generate route this adapter serves: ",
            r#"pictures is "unchanged" or absent"#
        )
        .into())
    );
    let call = CallBuilder::new("mesh.generate", "acme/mesh-a@gg").build();
    assert_eq!(
        serving(&registry, "mesh.generate", "gg", None).check_contract(&call),
        Err(concat!(
            "acme/mesh-a@gg is not a mesh.generate route this adapter serves: ",
            "the contract names no GG endpoint"
        )
        .into())
    );
}

/// A value of `ty` that passes the capability check: files are stored PNGs.
fn sample(builder: &mut CallBuilder, ty: MemberType) -> Value {
    let mut file = || builder.file("image/png", &media::png(2, 2, None));
    match ty {
        MemberType::Text => json!("text"),
        MemberType::Number | MemberType::Integer => json!(1),
        MemberType::Boolean => json!(false),
        MemberType::Object => json!({}),
        MemberType::List => json!([]),
        MemberType::File => file(),
        MemberType::Files => json!([file()]),
        MemberType::FilesByName => json!({"front": file()}),
        MemberType::FileOrObject => json!({"type": "object"}),
    }
}

/// A value that is not of `ty`.
fn wrong(ty: MemberType) -> Value {
    match ty {
        MemberType::Text | MemberType::Boolean => json!(1),
        MemberType::Number => json!("1"),
        MemberType::Integer => json!(1.5),
        MemberType::Object => json!([]),
        MemberType::List | MemberType::FilesByName => json!("x"),
        MemberType::File | MemberType::Files => json!({"file": 1}),
        MemberType::FileOrObject => json!("{}"),
    }
}

/// The requests the matrix sends on `capability`: one that fits, and one for each kind of
/// refusal of spec/capabilities.md §1.
fn requests(builder: &mut CallBuilder, capability_name: &str) -> Vec<Value> {
    let members = capability(capability_name).unwrap().members;
    let mut fits = Map::new();
    for member in members.iter().filter(|m| m.required) {
        let value = sample(builder, member.ty);
        fits.insert(member.name.into(), value);
    }
    let mut requests = vec![Value::Object(fits.clone()), json!({}), json!([])];
    let mut unknown = fits.clone();
    unknown.insert("zz_unknown".into(), json!(1));
    requests.push(Value::Object(unknown));
    for member in members {
        let mut typed = fits.clone();
        typed.insert(member.name.into(), wrong(member.ty));
        requests.push(Value::Object(typed));
    }
    requests
}

/// The route contracts the matrix sends to the adapter of `identity` on `capability` and
/// `provider`: the built-in routes', and ones each of FX's adapters judges in its own way.
fn contracts(identity: &Identity, capability: &str, provider: &str) -> Vec<Value> {
    let built_in: Vec<Value> = default_table()
        .unwrap()
        .entries
        .values()
        .filter(|route| route.capability == capability && route.provider == provider)
        .map(|route| route.contract.clone())
        .collect();
    let mut contracts = built_in.clone();
    // Each built-in route's contract with another adapter, or another behaviour.
    for contract in built_in {
        for (member, value) in [
            ("adapter", json!("acme-other")),
            ("adapter_behavior", json!(9)),
        ] {
            let mut changed = contract.clone();
            changed[member] = value;
            contracts.push(changed);
        }
    }
    let own = identity.adapter;
    contracts.extend([
        json!({}),
        Value::Null,
        json!([]),
        json!("fx"),
        json!({"adapter": own}),
        json!({"adapter": null, "adapter_behavior": null}),
        json!({"adapter": "acme-other"}),
        json!({"adapter": 1}),
        json!({"extra": 1}),
        json!({"quality": "max"}),
        json!({"quality": 1}),
        json!({"quality": " "}),
        json!({"image_detail": "high", "reasoning_effort": "high"}),
        json!({"image_detail": "x"}),
        json!({"reasoning_effort": "x"}),
        json!({"pictures": "unchanged"}),
        json!({"pictures": "small"}),
        json!({"endpoint": "three-d.acme.mesh-v1"}),
        json!({"endpoint": "not an id"}),
        json!({"endpoint": "three-d.acme.rig-v1", "check_endpoint": 1}),
        json!({"request_policy": {"reasoning": {"effort": "high"}}}),
        json!({"request_policy": {"x": 1}}),
    ]);
    for behavior in [
        json!("1"),
        json!("2"),
        json!("3"),
        json!(1),
        json!(1.0),
        json!(2),
        json!(true),
    ] {
        contracts.push(json!({"adapter": own, "adapter_behavior": behavior}));
        contracts.push(json!({"adapter_behavior": behavior}));
    }
    contracts
}

/// What the adapter, called directly, refuses `call` with. Every call the matrix makes is refused
/// before anything leaves: there is no key and no program.
fn refusal(adapter: &Adapter, call: &TestCall) -> String {
    match adapter {
        Adapter::Request(adapter) => match block_on(adapter.send(call)) {
            Sent::Refused { reason } => reason,
            other => panic!("{}: {other:?}", call.route.id()),
        },
        Adapter::Job(adapter) => match block_on(adapter.submit(call)) {
            Submitted::Refused { reason } => reason,
            other => panic!("{}: {other:?}", call.route.id()),
        },
    }
}

/// The refusals that come after the engine's (spec/adapters.md §4 step 3): the provider's key,
/// or the program of Codex and Grok Build.
fn later_refusal(provider: &str) -> String {
    match provider {
        "codex" => codex::NOT_INSTALLED.into(),
        "grok" => grok::NOT_INSTALLED.into(),
        _ => KeyName::of_provider(provider)
            .unwrap_or_else(|| panic!("no key names provider {provider}"))
            .missing(),
    }
}

#[test]
fn the_engine_refuses_what_each_adapter_refuses_with_the_same_sentence_first() {
    let registry = registry();
    let mut refused = 0;
    let mut passed = 0;
    for registered in registry.registered() {
        let (capability_name, provider) = (registered.capability, registered.provider);
        let adapter = serving(
            &registry,
            capability_name,
            provider,
            Some(registered.identity.adapter),
        );
        let identity = adapter.identity().unwrap();
        let redactor = match &adapter {
            Adapter::Request(adapter) => adapter.redactor(),
            Adapter::Job(adapter) => adapter.redactor(),
        };
        let mut pictures = CallBuilder::new(capability_name, &format!("acme/m-a@{provider}"));
        let requests = requests(&mut pictures, capability_name);
        for contract in contracts(&identity, capability_name, provider) {
            for request in &requests {
                let mut builder =
                    CallBuilder::new(capability_name, &format!("acme/m-a@{provider}"))
                        .contract(contract.clone())
                        .request(request.clone());
                // The one picture every file value of the requests names, stored for this call.
                builder.file("image/png", &media::png(2, 2, None));
                let call = builder.build();
                let engine =
                    checks::before_send(&call, &identity, || adapter.check_contract(&call));
                let direct = refusal(&adapter, &call);
                let at = format!("{capability_name} on {provider}: {contract} {request}");
                match engine {
                    Err(sentence) => {
                        // What the retry owner reports: each passes through the redactor.
                        assert_eq!(redactor.reason(&direct), redactor.reason(&sentence), "{at}");
                        refused += 1;
                    }
                    Ok(()) => {
                        assert_eq!(direct, later_refusal(provider), "{at}");
                        passed += 1;
                    }
                }
            }
        }
    }
    assert!(
        refused > 1000 && passed > 50,
        "{refused} refused, {passed} passed"
    );
}
