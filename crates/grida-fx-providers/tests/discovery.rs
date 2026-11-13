//! Selected-model discovery is replay-only. All IDs, metadata and keys here are made up.
use grida_fx_providers::discovery::{self, DEADLINE, MAX_RESPONSE_BYTES};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::testing::{block_on, setup, test_keys};
use grida_fx_providers::transport::replay::{Exchange, Expect, NoNetwork, ReplayTransport, Reply};
use grida_fx_providers::transport::{HttpResponse, Method};
use serde_json::{Value, json};
use std::sync::Arc;

const MODEL: &str = "acme/img-a";

fn gg_metadata() -> (Value, Value, Value) {
    let models = json!({"object":"list","revision":"acme-r1","data":[{
        "id":MODEL,"gg":{"kind":"image","interfaces":["images.generations","images.edits"],
            "capabilities":{"image_generation":true,"image_editing":true},
            "operation_ids":["acme.text","acme.edit"],
            "request_schemas":{"images.generations":{"$ref":"https://metadata.acme.test/schemas/operations.json#/$defs/Generate"},"images.edits":{"$ref":"#/$defs/Edit"}},
            "result_schemas":{"images.generations":{"$ref":"#/$defs/Answer"},"images.edits":{"$ref":"#/$defs/Answer"}},
            "pricing":{"currency":"USD","unit":"mill","components":[{"meter":"acme_usage","unit":"provider_unit","rate_mills":"1000","per_quantity":"1"}]},
            "reservation_policy":{"basis":"estimate","customer_ceiling_enforced":true}}}]});
    let endpoints = json!({"object":"list","revision":"acme-r1","data":[]});
    let properties = json!({
        "model":{"const":MODEL},"prompt":{"type":"string"},
        "n":{"type":"integer","minimum":1,"maximum":4},
        "size":{"$ref":"#/$defs/Size"},
        "background":{"enum":["auto","opaque","transparent"]},
        "output_format":{"enum":["png","jpeg"]},
        "image":{"type":"array","minItems":1,"maxItems":4}});
    let bundle = json!({"x-gg-operations":[
        {"model_id":MODEL,"operation_id":"acme.text","kind":"image","path":"/v1/images/generations","content_type":"application/json","request_view":"json","response_mode":"synchronous","delivery":"transient_base64","request_schema":{"$ref":"#/$defs/Generate"},"result_schema":{"$ref":"#/$defs/Answer"}},
        {"model_id":MODEL,"operation_id":"acme.edit","kind":"image","path":"/v1/images/edits","content_type":"multipart/form-data","request_view":"normalized_multipart","response_mode":"synchronous","delivery":"transient_base64","request_schema":{"$ref":"#/$defs/Edit"},"result_schema":{"$ref":"#/$defs/Answer"}}],
        "$defs":{"Size":{"enum":["auto","1024x1024"]},
        "Generate":{"type":"object","additionalProperties":false,"properties":properties,"required":["model","prompt"]},
        "Edit":{"type":"object","additionalProperties":false,"properties":properties,"required":["model","prompt","image"]},
        "Answer":{"type":"object","properties":{"data":{"type":"array","items":{"type":"object","properties":{"b64_json":{"type":"string"}}}}}}}});
    (models, endpoints, bundle)
}

fn gg_exchanges(models: Value, endpoints: Value, bundle: Value, base: &str) -> Vec<Exchange> {
    let prefix = base.strip_suffix("/v1").unwrap();
    vec![
        Expect::get(format!("{base}/models")).reply(HttpResponse::json(200, &models)),
        Expect::get(format!("{base}/endpoints")).reply(HttpResponse::json(200, &endpoints)),
        Expect::get(format!("{prefix}/schemas/operations.json"))
            .reply(HttpResponse::json(200, &bundle)),
    ]
}

fn or_endpoint(cost: Value) -> Value {
    json!({"provider_slug":"acme","supported_parameters":{
        "n":{"type":"range","min":1,"max":10},
        "input_references":{"type":"range","min":0,"max":4},
        "background":{"type":"enum","values":["auto","opaque","transparent"]},
        "size":{"type":"enum","values":["1024x1024"]},
        "resolution":{"type":"enum","values":["1K","2K"]}},
        "pricing":[{"billable":"output_image","unit":"image","cost_usd":cost},
            {"billable":"input_image","unit":"image","cost_usd":0}]})
}

fn or_transport(endpoints: Vec<Value>) -> Arc<ReplayTransport> {
    Arc::new(ReplayTransport::new(vec![
        Expect::get("https://openrouter.ai/api/v1/images/models/acme/img-a/endpoints")
            .credential("authorization", "Bearer test-openrouter-key")
            .reply(HttpResponse::json(
                200,
                &json!({"id":MODEL,"endpoints":endpoints}),
            )),
    ]))
}

fn has(route: &Value, feature: &str) -> bool {
    route["features"]
        .as_array()
        .unwrap()
        .contains(&json!(feature))
}

#[test]
fn gg_new_model_uses_anonymous_metadata_and_the_existing_wire_at_a_configured_origin() {
    let (models, endpoints, bundle) = gg_metadata();
    let base = "https://proxy.acme.test/gg/v1";
    let transport = Arc::new(ReplayTransport::new(gg_exchanges(
        models, endpoints, bundle, base,
    )));
    let mut setup = setup(transport.clone(), test_keys());
    setup.endpoints = setup.endpoints.with_base("gg", base);
    let route = block_on(discovery::resolve(&setup, "image.edit", MODEL, "gg")).unwrap();
    transport.assert_done();
    assert_eq!(route["route"], "acme/img-a@gg");
    assert_eq!(route["model_id"], MODEL);
    assert!(route.get("price").is_none());
    assert_eq!(route["max_input_pictures"], 4);
    assert!(has(&route, "image_input") && has(&route, "exact_size") && has(&route, "alpha"));
    assert!(!has(&route, "maximum_quality"));
    assert_eq!(
        route["contract"],
        json!({"adapter":"gg-image","adapter_behavior":"1","provider_options":{"gg":{"output_format":"png"}}})
    );
    for request in transport.requests() {
        assert_eq!(request.method, Method::Get);
        assert!(request.credential.is_none());
        assert_eq!(request.timeout, DEADLINE);
        assert_eq!(request.max_response_bytes, MAX_RESPONSE_BYTES);
    }
}

#[test]
fn unknown_gg_control_facts_enable_no_features_and_meter_rates_are_not_quotes() {
    let (models, endpoints, mut bundle) = gg_metadata();
    bundle["$defs"]["Generate"]["properties"] =
        json!({"model":{"const":MODEL},"prompt":{"type":"string"},"n":{"const":1}});
    let transport = Arc::new(ReplayTransport::new(gg_exchanges(
        models,
        endpoints,
        bundle,
        "https://gateway.grida.ai/v1",
    )));
    let route = block_on(discovery::resolve(
        &setup(transport.clone(), Keys::default()),
        "image.generate",
        MODEL,
        "gg",
    ))
    .unwrap();
    transport.assert_done();
    for absent in [
        "alpha",
        "opaque_background",
        "exact_size",
        "image_input",
        "maximum_quality",
        "png_output",
    ] {
        assert!(!has(&route, absent), "{absent}");
    }
    assert!(
        route.get("price").is_none(),
        "usage-meter tariffs are not a request quote"
    );
}

#[test]
fn inconsistent_gg_revisions_are_not_transient_and_do_not_fetch_a_schema() {
    let (models, mut endpoints, _) = gg_metadata();
    endpoints["revision"] = json!("acme-r2");
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get("https://gateway.grida.ai/v1/models").reply(HttpResponse::json(200, &models)),
        Expect::get("https://gateway.grida.ai/v1/endpoints")
            .reply(HttpResponse::json(200, &endpoints)),
    ]));
    let reason = block_on(discovery::resolve(
        &setup(transport.clone(), test_keys()),
        "image.generate",
        MODEL,
        "gg",
    ))
    .unwrap_err();
    transport.assert_done();
    assert!(reason.contains("invalid or incompatible"));
    assert!(!reason.contains("unavailable"));
}

#[test]
fn openrouter_uses_the_intersection_of_endpoint_controls_and_the_full_price_range() {
    let first = or_endpoint(json!(0.04));
    let mut second = or_endpoint(json!(0.07));
    second["supported_parameters"]["background"]["values"] = json!(["auto", "opaque"]);
    second["supported_parameters"]["input_references"]["max"] = json!(2);
    let transport = or_transport(vec![first, second]);
    let route = block_on(discovery::resolve(
        &setup(transport.clone(), test_keys()),
        "image.generate",
        MODEL,
        "openrouter",
    ))
    .unwrap();
    transport.assert_done();
    assert_eq!(
        route["price"],
        json!({"low_usd":0.04,"high_usd":0.07}),
        "published unconditional prices cover every eligible endpoint"
    );
    assert_eq!(route["max_input_pictures"], 2);
    assert!(
        has(&route, "image_input") && has(&route, "exact_size") && has(&route, "opaque_background")
    );
    assert!(!has(&route, "alpha"));
    assert_eq!(
        route["contract"],
        json!({"adapter":"openrouter-image-discovered-v1","adapter_behavior":"1","image_controls":{"background":["auto","opaque"],"size":["1024x1024"]}})
    );
}

#[test]
fn unknown_endpoint_prices_are_absent_and_resolution_is_not_exact_pixels() {
    let mut endpoint = or_endpoint(json!(0.04));
    endpoint["supported_parameters"]
        .as_object_mut()
        .unwrap()
        .remove("size");
    endpoint["pricing"][1]["cost_usd"] = json!(0.02);
    let transport = or_transport(vec![endpoint]);
    let route = block_on(discovery::resolve(
        &setup(transport.clone(), test_keys()),
        "image.edit",
        MODEL,
        "openrouter",
    ))
    .unwrap();
    transport.assert_done();
    assert!(route.get("price").is_none());
    assert!(!has(&route, "exact_size"));
}

#[test]
fn a_single_unknown_endpoint_prevents_an_underpriced_range_and_explicit_free_is_known() {
    let mut unknown = or_endpoint(json!(0.01));
    unknown["pricing"][0]["when"] = json!({"size":"large"});
    let transport = or_transport(vec![or_endpoint(json!(0.04)), unknown]);
    let unpriced = block_on(discovery::resolve(
        &setup(transport.clone(), test_keys()),
        "image.generate",
        MODEL,
        "openrouter",
    ))
    .unwrap();
    assert!(unpriced.get("price").is_none());
    transport.assert_done();
    let transport = or_transport(vec![or_endpoint(json!(0))]);
    let route = block_on(discovery::resolve(
        &setup(transport.clone(), test_keys()),
        "image.generate",
        MODEL,
        "openrouter",
    ))
    .unwrap();
    transport.assert_done();
    assert_eq!(route["price"], json!({"low_usd":0,"high_usd":0}));
}

#[test]
fn status_diagnostics_are_fixed_and_only_transient_failures_can_use_stale_facts() {
    for (status, transient) in [
        (404, false),
        (401, false),
        (302, false),
        (429, true),
        (503, true),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![Expect::get("https://openrouter.ai/api/v1/images/models/acme/img-a/endpoints")
            .reply(HttpResponse::json(status,&json!({"error":{"message":"test-openrouter-key https://private.acme.test/signed?token=secret"}})))]));
        let reason = block_on(discovery::resolve(
            &setup(transport.clone(), Keys::default()),
            "image.generate",
            MODEL,
            "openrouter",
        ))
        .unwrap_err();
        transport.assert_done();
        assert_eq!(reason.contains("unavailable"), transient, "{reason}");
        assert!(
            !reason.contains("test-openrouter-key")
                && !reason.contains("https://")
                && !reason.contains("secret")
        );
    }
}

#[test]
fn unsupported_lookups_do_not_send() {
    let setup = setup(Arc::new(NoNetwork), Keys::default());
    assert!(block_on(discovery::resolve(&setup, "video.generate", MODEL, "gg")).is_err());
    assert!(block_on(discovery::resolve(&setup, "image.generate", MODEL, "acme")).is_err());
}

#[test]
fn incompatible_required_controls_counts_and_external_refs_are_never_executed() {
    for change in 0..3 {
        let (models, endpoints, mut bundle) = gg_metadata();
        match change {
            0 => bundle["$defs"]["Generate"]["required"]
                .as_array_mut()
                .unwrap()
                .push(json!("unimplemented")),
            1 => bundle["$defs"]["Generate"]["properties"]["n"] = json!({"const":2}),
            _ => {
                bundle["$defs"]["Generate"]["properties"]["size"] =
                    json!({"$ref":"https://private.acme.test/schema"})
            }
        }
        let transport = Arc::new(ReplayTransport::new(gg_exchanges(
            models,
            endpoints,
            bundle,
            "https://gateway.grida.ai/v1",
        )));
        let reason = block_on(discovery::resolve(
            &setup(transport.clone(), Keys::default()),
            "image.generate",
            MODEL,
            "gg",
        ))
        .unwrap_err();
        transport.assert_done();
        assert!(reason.contains("invalid or incompatible"));
        assert!(!reason.contains("private.acme.test"));
    }
    let mut endpoint = or_endpoint(json!(0.04));
    endpoint["supported_parameters"]["n"]["min"] = json!(2);
    let transport = or_transport(vec![endpoint]);
    assert!(
        block_on(discovery::resolve(
            &setup(transport.clone(), test_keys()),
            "image.generate",
            MODEL,
            "openrouter"
        ))
        .unwrap_err()
        .contains("invalid or incompatible")
    );
    transport.assert_done();
}

#[test]
fn absent_gg_models_and_oversized_metadata_do_not_enable_stale_fallback() {
    let (mut models, _, _) = gg_metadata();
    models["data"] = json!([]);
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get("https://gateway.grida.ai/v1/models").reply(HttpResponse::json(200, &models)),
    ]));
    let reason = block_on(discovery::resolve(
        &setup(transport.clone(), Keys::default()),
        "image.generate",
        MODEL,
        "gg",
    ))
    .unwrap_err();
    transport.assert_done();
    assert!(reason.contains("not found") && !reason.contains("unavailable"));
    let transport = Arc::new(ReplayTransport::new(vec![Exchange {
        expect: Expect::get("https://openrouter.ai/api/v1/images/models/acme/img-a/endpoints"),
        reply: Reply::Zeros {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            len: MAX_RESPONSE_BYTES + 1,
        },
    }]));
    let reason = block_on(discovery::resolve(
        &setup(transport.clone(), Keys::default()),
        "image.generate",
        MODEL,
        "openrouter",
    ))
    .unwrap_err();
    transport.assert_done();
    assert!(reason.contains("invalid or incompatible") && !reason.contains("unavailable"));
}

#[test]
fn openrouter_edits_requiring_multiple_initial_images_need_an_authored_contract() {
    let mut endpoint = or_endpoint(json!(0.04));
    endpoint["supported_parameters"]["input_references"]["min"] = json!(2);
    let transport = or_transport(vec![endpoint]);
    let reason = block_on(discovery::resolve(
        &setup(transport.clone(), test_keys()),
        "image.edit",
        MODEL,
        "openrouter",
    ))
    .unwrap_err();
    transport.assert_done();
    assert!(reason.contains("invalid or incompatible"));
}

#[test]
fn reordered_or_duplicate_size_enums_do_not_change_the_execution_contract() {
    let mut routes = Vec::new();
    for sizes in [
        json!(["1024x1024", "512x512", "1024x1024"]),
        json!(["512x512", "1024x1024"]),
    ] {
        let mut endpoint = or_endpoint(json!(0.04));
        endpoint["supported_parameters"]["size"]["values"] = sizes;
        let transport = or_transport(vec![endpoint]);
        routes.push(
            block_on(discovery::resolve(
                &setup(transport.clone(), test_keys()),
                "image.generate",
                MODEL,
                "openrouter",
            ))
            .unwrap(),
        );
        transport.assert_done();
    }
    assert_eq!(routes[0]["contract"], routes[1]["contract"]);
}
