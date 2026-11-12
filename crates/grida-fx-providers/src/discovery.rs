//! Nonspending metadata lookup for a selected image model, through the invocation's transport.
//!
//! A successful lookup returns one ordinary `fx: routes/v1` entry. It does not select a model,
//! persist a cache, grant permission to spend, or perform a model request. Remote descriptions,
//! URLs and error bodies never become diagnostics or route contracts. Missing facts do not
//! enable features; tariffs that do not define a computable one-image amount need an allowance.

use crate::keys::KeyName;
use crate::setup::{Client, Setup};
use crate::transport::{HttpRequest, Lane, Method, TransportErrorKind};
use grida_fx_core::money::Usd;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::time::Duration;

/// The response bound of each metadata GET, including the shared operation bundle.
pub const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// Metadata reads have a shorter deadline than generation requests and are never retried here.
pub const DEADLINE: Duration = Duration::from_secs(15);

/// Resolve a selected GG or OpenRouter image model into a route-table entry.
///
/// `allowance` is a reusable, positive per-attempt USD allowance, not a quote or authorization
/// to spend. The caller owns discovery policy, cache freshness and the later financial approval.
pub async fn resolve(
    setup: &Setup,
    capability: &str,
    model: &str,
    provider: &str,
    allowance: Option<Usd>,
) -> Result<Value, String> {
    if !matches!(capability, "image.generate" | "image.edit")
        || !matches!(provider, "gg" | "openrouter")
    {
        return Err("no bundled model resolver serves this capability and provider".into());
    }
    if model.is_empty()
        || model.len() > 512
        || model
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '@')
    {
        return Err(
            "model discovery needs a nonempty native model identifier without whitespace or @"
                .into(),
        );
    }
    if allowance.is_some_and(|amount| amount <= Usd::ZERO) {
        return Err(format!(
            "model_allowances.{provider}.{capability}: an allowance is more than 0"
        ));
    }
    match provider {
        "gg" => gg(setup, capability, model, allowance).await,
        "openrouter" => openrouter(setup, capability, model, allowance).await,
        _ => unreachable!(),
    }
}

fn unavailable(provider: &str) -> String {
    format!(
        "{provider} model metadata is unavailable; use cached facts or provide an authored route"
    )
}

fn invalid(provider: &str) -> String {
    format!(
        "{provider} model metadata is invalid or incompatible; provide an authored route instead"
    )
}

fn not_found(provider: &str) -> String {
    format!(
        "{provider} model was not found in provider metadata; provide an authored route instead"
    )
}

fn needs_allowance(provider: &str, capability: &str) -> String {
    format!(
        "{provider} model metadata does not provide a computable per-call price: set \
         model_allowances.{provider}.{capability} or provide an authored route price"
    )
}

async fn get(client: &Client, url: String, authenticate: bool) -> Result<Value, String> {
    let mut request = HttpRequest::new(Method::Get, url, Lane::Provider)
        .header("accept", "application/json, application/schema+json")
        .timeout(DEADLINE)
        .max_response_bytes(MAX_RESPONSE_BYTES);
    if authenticate && client.key.is_some() {
        // The configured provider origin is the only destination, never a metadata-supplied URL.
        request.credential = Some(client.credential().map_err(|_| invalid(&client.provider))?);
    }
    let response = client.send(request).await.map_err(|error| {
        if error.kind == TransportErrorKind::TooLarge {
            invalid(&client.provider)
        } else {
            unavailable(&client.provider)
        }
    })?;
    if response.status == 429 || response.status >= 500 {
        return Err(unavailable(&client.provider));
    }
    if response.status == 404 {
        return Err(not_found(&client.provider));
    }
    if response.status != 200 {
        return Err(format!(
            "{} model metadata request was refused (HTTP {})",
            client.provider, response.status
        ));
    }
    if response.body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(invalid(&client.provider));
    }
    let content_type = response
        .header("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if !matches!(content_type, "application/json" | "application/schema+json") {
        return Err(invalid(&client.provider));
    }
    serde_json::from_slice::<Value>(&response.body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| invalid(&client.provider))
}

fn entry(
    capability: &str,
    model: &str,
    provider: &str,
    price: Value,
    features: BTreeSet<&str>,
    contract: Value,
) -> Value {
    json!({"capability": capability, "route": format!("{model}@{provider}"),
        "model_id": model, "price": price, "features": features, "contract": contract})
}

fn allowance_price(
    allowance: Option<Usd>,
    provider: &str,
    capability: &str,
) -> Result<Value, String> {
    allowance
        .map(|amount| json!({"cap_usd": amount.to_value()}))
        .ok_or_else(|| needs_allowance(provider, capability))
}

fn array<'a>(
    object: &'a Value,
    key: &str,
    provider: &str,
    max: usize,
) -> Result<&'a Vec<Value>, String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .filter(|items| items.len() <= max)
        .ok_or_else(|| invalid(provider))
}

fn lists(value: &Value, text: &str) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(text)))
}

async fn gg(
    setup: &Setup,
    capability: &str,
    model: &str,
    allowance: Option<Usd>,
) -> Result<Value, String> {
    let client = setup.client(KeyName::GridaGateway);
    let models = get(&client, client.url("models"), false).await?;
    let endpoints = get(&client, client.url("endpoints"), false).await?;
    let Some(revision) = models
        .get("revision")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Err(invalid("gg"));
    };
    if endpoints.get("revision").and_then(Value::as_str) != Some(revision) {
        return Err(invalid("gg"));
    }
    array(&endpoints, "data", "gg", 2048)?;
    let mut schema_base = url::Url::parse(&client.base).map_err(|_| invalid("gg"))?;
    // GG's schemas sit beside v1. Keep an explicitly configured proxy path prefix.
    let prefix = schema_base
        .path()
        .strip_suffix("/v1")
        .ok_or_else(|| invalid("gg"))?
        .to_string();
    schema_base.set_path(&format!("{prefix}/schemas/operations.json"));
    let bundle = get(&client, schema_base.to_string(), false).await?;
    let matching: Vec<_> = array(&models, "data", "gg", 2048)?
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) == Some(model))
        .collect();
    if matching.is_empty() {
        return Err(not_found("gg"));
    }
    let [offered] = matching.as_slice() else {
        return Err(invalid("gg"));
    };
    let facts = &offered["gg"];
    let (interface, path, content_type, request_view) = if capability == "image.generate" {
        (
            "images.generations",
            "/v1/images/generations",
            "application/json",
            "json",
        )
    } else {
        (
            "images.edits",
            "/v1/images/edits",
            "multipart/form-data",
            "normalized_multipart",
        )
    };
    let enabled = if capability == "image.generate" {
        "image_generation"
    } else {
        "image_editing"
    };
    if facts["kind"] != "image"
        || !lists(&facts["interfaces"], interface)
        || facts["capabilities"][enabled] != true
    {
        return Err(invalid("gg"));
    }
    let operations: Vec<_> = array(&bundle, "x-gg-operations", "gg", 4096)?
        .iter()
        .filter(|op| {
            op["model_id"] == model
                && op["kind"] == "image"
                && op["path"] == path
                && op["content_type"] == content_type
                && op["request_view"] == request_view
                && op["response_mode"] == "synchronous"
                && op["delivery"] == "transient_base64"
                && op
                    .get("operation_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| lists(&facts["operation_ids"], id))
        })
        .collect();
    let [operation] = operations.as_slice() else {
        return Err(invalid("gg"));
    };
    if !same_schema_reference(
        &facts["request_schemas"][interface],
        &operation["request_schema"],
    ) || !same_schema_reference(
        &facts["result_schemas"][interface],
        &operation["result_schema"],
    ) {
        return Err(invalid("gg"));
    }
    let schema =
        local_schema(&bundle, &operation["request_schema"]).ok_or_else(|| invalid("gg"))?;
    let result = local_schema(&bundle, &operation["result_schema"]).ok_or_else(|| invalid("gg"))?;
    if schema["properties"]["model"]["const"] != model
        || result["properties"]["data"]["type"] != "array"
        || result["properties"]["data"]["items"]["properties"]["b64_json"]["type"] != "string"
        || !schema["properties"]["prompt"].is_object()
    {
        return Err(invalid("gg"));
    }
    let mut features = BTreeSet::from(["authored_prompt_passthrough"]);
    if capability == "image.generate" {
        features.insert("text_to_image");
    }
    if !lists(&schema["required"], "size")
        || accepts(&bundle, &schema["properties"]["size"], json!("auto"))
    {
        features.insert("flexible_size");
    }
    if accepts(&bundle, &schema["properties"]["size"], json!("1024x1024")) {
        features.insert("exact_size");
    }
    let mut contract = json!({"adapter": "gg-image", "adapter_behavior": "1"});
    if accepts(
        &bundle,
        &schema["properties"]["output_format"],
        json!("png"),
    ) {
        contract["provider_options"] = json!({"gg": {"output_format": "png"}});
        features.insert("png_output");
    }
    // Schemas describe a known executable interface, not arbitrary executable code. Refuse a
    // contract whose required fields or single-output choice this wire cannot represent.
    let mut sample = json!({"model":model,"prompt":"An original object.","n":1});
    if features.contains("png_output") {
        sample["output_format"] = json!("png");
    }
    if capability == "image.edit" {
        sample["image"] = json!([{"sha256":"0".repeat(64),"byte_length":1024,
            "media_type":"image/png","width":1024,"height":1024}]);
    }
    let mut input_schema = schema.clone();
    if let Some(definitions) = bundle.get("$defs") {
        input_schema["$defs"] = definitions.clone();
    }
    if !crate::checks::schema_validator(&input_schema)
        .is_ok_and(|validator| validator.is_valid(&sample))
    {
        return Err(invalid("gg"));
    }
    for (value, feature) in [
        ("auto", "auto_background"),
        ("opaque", "opaque_background"),
        ("transparent", "alpha"),
    ] {
        if accepts(&bundle, &schema["properties"]["background"], json!(value))
            && (value != "transparent" || features.contains("png_output"))
        {
            features.insert(feature);
        }
    }
    let mut route = entry(
        capability,
        model,
        "gg",
        allowance_price(allowance, "gg", capability)?,
        features,
        contract,
    );
    if capability == "image.edit" {
        let images = &schema["properties"]["image"];
        if images["type"] != "array" {
            return Err(invalid("gg"));
        }
        route["features"]
            .as_array_mut()
            .expect("features are an array")
            .push(json!("image_input"));
        if let Some(max) = images["maxItems"]
            .as_u64()
            .filter(|max| *max > 0 && *max <= u32::MAX as u64)
        {
            route["max_input_pictures"] = json!(max);
        }
    }
    // GG's tariffs name provider usage meters; neither their identifier nor unit defines
    // the request's quantity. A reservation policy is not an estimate or a quoted bound.
    Ok(route)
}

fn same_schema_reference(first: &Value, second: &Value) -> bool {
    let fragment = |value: &Value| {
        value
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| {
                reference
                    .split_once('#')
                    .map(|(_, fragment)| fragment.to_string())
            })
    };
    matches!((fragment(first), fragment(second)), (Some(first), Some(second)) if first == second)
}

fn local_schema<'a>(bundle: &'a Value, schema: &'a Value) -> Option<&'a Value> {
    if let Some(reference) = schema.get("$ref") {
        let reference = reference.as_str()?.strip_prefix("#/$defs/")?;
        if reference.contains('/') || reference.contains('~') {
            return None;
        }
        bundle
            .get("$defs")?
            .get(reference)
            .filter(|schema| schema.is_object())
    } else {
        schema.as_object().map(|_| schema)
    }
}

fn accepts(bundle: &Value, schema: &Value, value: Value) -> bool {
    if !schema.is_object() || schema.as_object().is_none_or(Map::is_empty) {
        return false;
    }
    let mut schema = schema.clone();
    if let Some(definitions) = bundle.get("$defs") {
        schema["$defs"] = definitions.clone();
    }
    crate::checks::schema_validator(&schema).is_ok_and(|validator| validator.is_valid(&value))
}

async fn openrouter(
    setup: &Setup,
    capability: &str,
    model: &str,
    allowance: Option<Usd>,
) -> Result<Value, String> {
    let client = setup.client(KeyName::OpenRouter);
    let (author, slug) = model
        .split_once('/')
        .filter(|(author, slug)| !author.is_empty() && !slug.is_empty())
        .ok_or_else(|| invalid("openrouter"))?;
    let mut url = url::Url::parse(&client.base).map_err(|_| invalid("openrouter"))?;
    url.path_segments_mut()
        .map_err(|_| invalid("openrouter"))?
        .pop_if_empty()
        .extend(["images", "models", author, slug, "endpoints"]);
    let metadata = get(&client, url.to_string(), true).await?;
    if metadata["id"] != model {
        return Err(invalid("openrouter"));
    }
    let endpoints = array(&metadata, "endpoints", "openrouter", 256)?;
    if endpoints.is_empty()
        || endpoints
            .iter()
            .any(|endpoint| !endpoint["supported_parameters"].is_object())
    {
        return Err(invalid("openrouter"));
    }
    if endpoints.iter().any(|endpoint| {
        endpoint["supported_parameters"]
            .get("n")
            .is_some_and(|count| {
                count["type"] != "range"
                    || count["min"].as_f64().is_none_or(|min| min > 1.0)
                    || count["max"].as_f64().is_none_or(|max| max < 1.0)
            })
    }) {
        return Err(invalid("openrouter"));
    }
    let values = |parameter: &str, value: &str| {
        endpoints.iter().all(|endpoint| {
            endpoint["supported_parameters"][parameter]["type"] == "enum"
                && lists(
                    &endpoint["supported_parameters"][parameter]["values"],
                    value,
                )
        })
    };
    let image_input = endpoints.iter().all(|endpoint| {
        endpoint["supported_parameters"]["input_references"]["max"]
            .as_u64()
            .is_some_and(|max| max > 0)
    });
    if capability == "image.edit" && !image_input {
        return Err(invalid("openrouter"));
    }
    let mut features =
        BTreeSet::from(["authored_prompt_passthrough", "flexible_size", "png_output"]);
    if capability == "image.generate"
        && endpoints.iter().all(|endpoint| {
            endpoint["supported_parameters"]
                .get("input_references")
                .is_none()
                || endpoint["supported_parameters"]["input_references"]["min"].as_u64() == Some(0)
        })
    {
        features.insert("text_to_image");
    }
    if image_input {
        features.insert("image_input");
    }
    let mut controls = Map::new();
    let background: Vec<_> = ["auto", "opaque", "transparent"]
        .into_iter()
        .filter(|value| values("background", value))
        .collect();
    for (value, feature) in [
        ("auto", "auto_background"),
        ("opaque", "opaque_background"),
        ("transparent", "alpha"),
    ] {
        if values("background", value) {
            features.insert(feature);
        }
    }
    if !background.is_empty() {
        controls.insert("background".into(), json!(background));
    }
    // A published size control is distinct from aspect_ratio or resolution. Those cannot
    // promise exact pixels, so they never enable exact_size.
    let sizes: BTreeSet<_> = endpoints[0]["supported_parameters"]["size"]["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|size| {
            crate::wire::parse_size(Some(&json!(size)))
                .ok()
                .flatten()
                .is_some()
                && values("size", size)
        })
        .collect();
    if !sizes.is_empty() {
        controls.insert("size".into(), json!(sizes));
        features.insert("exact_size");
    }
    let price = match openrouter_price(endpoints) {
        Some(price) => price,
        None => allowance_price(allowance, "openrouter", capability)?,
    };
    let contract = json!({"adapter": "openrouter-image-discovered-v1", "adapter_behavior": "1", "image_controls": controls});
    let mut route = entry(capability, model, "openrouter", price, features, contract);
    if image_input
        && let Some(max) = endpoints
            .iter()
            .filter_map(|endpoint| {
                endpoint["supported_parameters"]["input_references"]["max"].as_u64()
            })
            .min()
            .filter(|max| *max <= u32::MAX as u64)
    {
        route["max_input_pictures"] = json!(max);
    }
    Ok(route)
}

fn openrouter_price(endpoints: &[Value]) -> Option<Value> {
    let mut prices = Vec::new();
    for endpoint in endpoints {
        let lines = endpoint.get("pricing")?.as_array()?;
        if lines.is_empty() || lines.len() > 256 {
            return None;
        }
        let mut price = Usd::ZERO;
        let mut output = false;
        for line in lines {
            // All nonzero components must be unconditional per-output-image USD amounts.
            // Unknown tier/condition fields stay unknown, rather than being discarded.
            let object = line.as_object()?;
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "billable" | "unit" | "cost_usd"))
            {
                return None;
            }
            let amount = match line.get("cost_usd")? {
                Value::String(text) => Usd::parse(text).ok()?,
                number => Usd::from_value(number).ok()?,
            };
            if line["billable"] == "output_image" && line["unit"] == "image" {
                output = true;
                price = price + amount;
            } else if amount != Usd::ZERO {
                return None;
            }
        }
        if !output {
            return None;
        }
        prices.push(price);
    }
    Some(
        json!({"low_usd": prices.iter().min()?.to_value(), "high_usd": prices.iter().max()?.to_value()}),
    )
}
