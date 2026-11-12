//! Explicit model import: verified public catalogue data becomes ordinary FX routes
//! ([model-selection.md](../../../spec/model-selection.md) §3). No I/O or provider calls.
//!
//! The initial admitted profile is GG's synchronous Images interface. Its operation schema must
//! accept what the existing adapter sends and describe its base64 image answer. Unknown fields
//! and model names never imply features; an explicit cap supplies the planning allowance.

use grida_fx_core::money::Usd;
use grida_fx_core::value::{parse_json, sha256_hex};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const REPOSITORY: &str = "https://github.com/gridaco/models";
pub const SCHEMA_ID: &str = "https://grida.co/schemas/ai-model-facts-v1.json";
pub const OPERATIONS_URL: &str = "https://gateway.grida.ai/schemas/operations.json";
pub const ASSETS: [&str; 4] = [
    "SHA256SUMS",
    "manifest.json",
    "models.v1.json",
    "models.v1.schema.json",
];
pub const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;

/// A release whose checksums, manifest and catalogue schema agree.
#[derive(Debug)]
pub struct Release {
    pub tag: String,
    pub models: Value,
    pub asset_sha256: String,
    pub schema_sha256: String,
}

/// A release tag, never a URL or a path. The v1 catalogue is the admitted format.
pub fn check_release(tag: &str) -> Result<(), String> {
    let parts: Vec<_> = tag.split('.').collect();
    if parts.len() != 3
        || parts[0] != "v1"
        || parts[1].len() != 8
        || !parts[1].bytes().all(|b| b.is_ascii_digit())
        || parts[2].is_empty()
        || !parts[2].bytes().all(|b| b.is_ascii_digit())
    {
        return Err("the Models release must be a v1.YYYYMMDD.N tag".into());
    }
    Ok(())
}

/// Verifies the same release assets as `tools/make_models.py`, entirely offline.
pub fn verify_release(tag: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<Release, String> {
    check_release(tag)?;
    let file = |name: &str| -> Result<&[u8], String> {
        let bytes = files
            .get(name)
            .ok_or_else(|| format!("the Models release is missing {name}"))?;
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(format!("{name} is larger than 32 MiB"));
        }
        Ok(bytes)
    };
    let sums = std::str::from_utf8(file("SHA256SUMS")?).map_err(|_| "SHA256SUMS is not UTF-8")?;
    let mut expected = BTreeMap::new();
    for line in sums.lines().filter(|line| !line.trim().is_empty()) {
        let (digest, name) = line
            .split_once(char::is_whitespace)
            .ok_or("SHA256SUMS has an invalid line")?;
        let name = name.trim().trim_start_matches('*');
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || name.is_empty()
            || expected.insert(name, digest).is_some()
        {
            return Err("SHA256SUMS has an invalid or repeated entry".into());
        }
    }
    for name in &ASSETS[1..] {
        if expected.get(name).copied() != Some(sha256_hex(file(name)?).as_str()) {
            return Err(format!("{name} does not match SHA256SUMS of {tag}"));
        }
    }
    let manifest = json_file(file("manifest.json")?, "manifest.json")?;
    if manifest["version"] != tag {
        return Err("the supplied manifest does not name the selected Models release".into());
    }
    for (name, path) in [
        ("models.v1.json", "models.json"),
        ("models.v1.schema.json", "schemas/models.schema.json"),
    ] {
        let entry = &manifest["files"][path];
        if entry["sha256"] != sha256_hex(file(name)?)
            || entry["bytes"].as_u64() != Some(file(name)?.len() as u64)
        {
            return Err(format!("manifest.json disagrees with {name}"));
        }
    }
    let schema = json_file(file("models.v1.schema.json")?, "models.v1.schema.json")?;
    if schema["$id"] != SCHEMA_ID {
        return Err("the Models schema has an unsupported $id".into());
    }
    local_references(&schema)?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|_| "the Models schema is not a valid draft 2020-12 schema")?;
    let models = json_file(file("models.v1.json")?, "models.v1.json")?;
    if models["$schema"] != SCHEMA_ID || !validator.is_valid(&models) {
        return Err("models.v1.json does not validate against the released Models schema".into());
    }
    Ok(Release {
        tag: tag.to_string(),
        models,
        asset_sha256: sha256_hex(file("models.v1.json")?),
        schema_sha256: sha256_hex(file("models.v1.schema.json")?),
    })
}

pub fn json_file(bytes: &[u8], name: &str) -> Result<Value, String> {
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(format!("{name} is larger than 32 MiB"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8"))?;
    parse_json(text).map_err(|_| format!("{name} is not valid FX JSON"))
}

/// Compiles only local schema references. Import never treats a schema as network authority.
fn local_references(schema: &Value) -> Result<(), String> {
    let mut pending = vec![schema];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(members) => {
                for key in ["$ref", "$dynamicRef", "$recursiveRef"] {
                    if let Some(reference) = members.get(key)
                        && !reference.as_str().is_some_and(|s| s.starts_with('#'))
                    {
                        return Err("import schemas must use local references only".into());
                    }
                }
                pending.extend(members.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    Ok(())
}

/// Materializes the canonical model's supported GG image operations, with no inherited price.
pub fn gg_image_table(
    release: &Release,
    operations_bytes: &[u8],
    model_id: &str,
    cap: Usd,
) -> Result<Value, String> {
    if cap <= Usd::ZERO {
        return Err("models import needs a positive per-call --cap-usd allowance".into());
    }
    if model_id.is_empty()
        || model_id.len() > 256
        || model_id.chars().any(|c| c.is_whitespace() || c == '@')
    {
        return Err("models import needs an unqualified canonical model identifier".into());
    }
    let card = &release.models["image.models"][model_id];
    let binding = &card["providers"]["grida"];
    let native = binding["id"]
        .as_str()
        .filter(|s| !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '@'))
        .ok_or("the selected Models release has no GG image binding for this model")?;
    if card["id"] != model_id || card["provider_coverage"]["grida"]["disposition"] != "listed" {
        return Err("the selected Models release does not list this GG image model".into());
    }
    let operations = json_file(operations_bytes, "gg.operations.json")?;
    if operations["$id"] != OPERATIONS_URL {
        return Err("gg.operations.json has an unsupported $id".into());
    }
    local_references(&operations)?;
    let endpoints = binding["endpoints"]
        .as_array()
        .ok_or("this GG binding declares no image interfaces")?;
    let mut routes = Vec::new();
    for (interface, capability, input) in [
        ("images.generations", "image.generate", "text"),
        ("images.edits", "image.edit", "multi_image"),
    ] {
        let found: Vec<_> = endpoints
            .iter()
            .filter(|e| e["interface"] == interface)
            .collect();
        let endpoint = match found.as_slice() {
            [] => continue,
            [endpoint] => *endpoint,
            _ => return Err(format!("the GG {interface} binding is ambiguous")),
        };
        if endpoint["input"] != input {
            return Err(format!(
                "the GG {interface} binding has an unsupported input"
            ));
        }
        routes.push(image_route(
            &operations,
            endpoint,
            model_id,
            native,
            capability,
            cap,
        )?);
    }
    if routes.is_empty() {
        return Err("this model has no GG image interface FX can import".into());
    }
    Ok(json!({
        "fx": "routes/v1",
        "model_source": {
            "repository": REPOSITORY,
            "release": release.tag,
            "asset_sha256": release.asset_sha256,
            "schema_sha256": release.schema_sha256,
            "operations_sha256": sha256_hex(operations_bytes),
        },
        "routes": routes,
    }))
}

fn image_route(
    operations: &Value,
    endpoint: &Value,
    model_id: &str,
    native: &str,
    capability: &str,
    cap: Usd,
) -> Result<Value, String> {
    let refused =
        || format!("the published {capability} operation does not fit FX's GG image adapter");
    let operation_id = endpoint["operation"].as_str().ok_or_else(refused)?;
    let matching: Vec<_> = operations["x-gg-operations"]
        .as_array()
        .ok_or_else(refused)?
        .iter()
        .filter(|o| o["operation_id"] == operation_id && o["model_id"] == native)
        .collect();
    let [operation] = matching.as_slice() else {
        return Err(refused());
    };
    let edit = capability == "image.edit";
    if operation["response_mode"] != "synchronous"
        || operation["delivery"] != "transient_base64"
        || operation["content_type"]
            != if edit {
                "multipart/form-data"
            } else {
                "application/json"
            }
        || operation["request_view"] != if edit { "normalized_multipart" } else { "json" }
        || operation["path"]
            != if edit {
                "/v1/images/edits"
            } else {
                "/v1/images/generations"
            }
    {
        return Err(refused());
    }
    let request = operation_schema(operations, endpoint, operation, "request")?;
    let result = operation_schema(operations, endpoint, operation, "result")?;
    let properties = &request["properties"];
    let result_item = &result["properties"]["data"]["items"];
    if request["type"] != "object"
        || !required(request, "model")
        || !required(request, "prompt")
        || properties["model"]["const"] != native
        || !properties["n"].is_object()
        || result["type"] != "object"
        || result["properties"]["data"]["type"] != "array"
        || result_item["type"] != "object"
        || result_item["properties"]["b64_json"]["type"] != "string"
        || result_item["properties"]["b64_json"]["contentEncoding"] != "base64"
        || !required(result, "data")
        || !required(result_item, "b64_json")
        || !takes(&result_item["properties"]["media_type"], "image/png")
    {
        return Err(refused());
    }
    // The adapter requests no output_format itself, so PNG must actually be the declared default.
    let png_default = properties["output_format"]["default"] == "png"
        || properties["output_format"]["const"] == "png"
        || request["x-gg-defaults"]["output_format"] == "png";
    if !png_default {
        return Err(refused());
    }
    let reference = operation["request_schema"]["$ref"]
        .as_str()
        .ok_or_else(refused)?;
    let validation_schema = json!({"$defs": operations["$defs"], "$ref": reference});
    let validator = jsonschema::draft202012::options()
        .build(&validation_schema)
        .map_err(|_| refused())?;
    let answer_schema =
        json!({"$defs": operations["$defs"], "$ref": operation["result_schema"]["$ref"]});
    let answer_validator = jsonschema::draft202012::options()
        .build(&answer_schema)
        .map_err(|_| refused())?;
    if !answer_validator.is_valid(&json!({
        "created": 0, "data": [{"b64_json": "AAAA", "media_type": "image/png"}],
    })) {
        return Err(refused());
    }
    let mut sent = json!({"model": native, "prompt": "acme preview", "n": 1});
    let quality_max = takes(&properties["quality"], "max");
    if quality_max {
        sent["quality"] = "max".into();
    }
    let mut max_pictures = None;
    if edit {
        let images = &properties["image"];
        let max = images["maxItems"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(refused)?;
        if images["type"] != "array" || images["minItems"].as_u64().unwrap_or(0) > 1 {
            return Err(refused());
        }
        max_pictures = Some(max.min(crate::gg::image::MAX_INPUT_IMAGES as u64));
        sent["image"] = json!([{
            "sha256": "a".repeat(64), "byte_length": 1000, "media_type": "image/png",
            "width": 1024, "height": 1024,
        }]);
    }
    if !validator.is_valid(&sent) {
        return Err(refused());
    }
    let accepts = |name: &str, value: Value| {
        let mut variant = sent.clone();
        variant[name] = value;
        validator.is_valid(&variant)
    };
    let mut features = vec!["authored_prompt_passthrough", "png_output"];
    features.push(if edit { "image_input" } else { "text_to_image" });
    if quality_max {
        features.push("maximum_quality");
    }
    for (value, feature) in [
        ("transparent", "alpha"),
        ("opaque", "opaque_background"),
        ("auto", "auto_background"),
    ] {
        if takes(&properties["background"], value) && accepts("background", value.into()) {
            features.push(feature);
        }
    }
    let size_auto = takes(&properties["size"], "auto")
        || properties["size"]["anyOf"]
            .as_array()
            .is_some_and(|forms| forms.iter().any(|form| takes(form, "auto")));
    if size_auto && accepts("size", "auto".into()) {
        features.push("flexible_size");
    }
    // Recognize the public Images pixel-pair contract; mere acceptance by a permissive schema
    // would not establish exact-size semantics.
    let exact = properties["size"]["anyOf"].as_array().is_some_and(|forms| {
        forms.iter().any(|f| {
            f["$ref"] == "#/$defs/positive_pixel_pair"
                && f["x-gg-pixel-limits"].is_object()
                && operations["$defs"]["positive_pixel_pair"]["x-gg-positive-pair"]
                    == "safe-integer"
        })
    });
    if exact && accepts("size", "1024x1024".into()) {
        features.extend(["exact_size", "custom_exact_size"]);
    }
    features.sort_unstable();
    let mut route = json!({
        "capability": capability,
        "route": format!("{native}@gg"),
        "model_id": model_id,
        "price": {"cap_usd": cap.to_value()},
        "features": features,
        "contract": {"adapter": crate::gg::image::ADAPTER, "adapter_behavior": "1"},
    });
    if quality_max {
        route["contract"]["quality"] = "max".into();
    }
    if let Some(max) = max_pictures {
        route["max_input_pictures"] = max.into();
    }
    Ok(route)
}

fn operation_schema<'a>(
    operations: &'a Value,
    endpoint: &Value,
    operation: &Value,
    side: &str,
) -> Result<&'a Value, String> {
    let reference = operation[format!("{side}_schema")]["$ref"]
        .as_str()
        .filter(|s| s.starts_with("#/$defs/"))
        .ok_or_else(|| format!("the GG operation has no supported {side} schema"))?;
    if endpoint[format!("{side}_schema_url")] != format!("{OPERATIONS_URL}{reference}") {
        return Err(format!(
            "the GG {side} schema differs from the released model binding"
        ));
    }
    operations
        .pointer(&reference[1..])
        .filter(|s| s.is_object())
        .ok_or_else(|| format!("the GG {side} schema reference cannot be resolved locally"))
}

fn required(schema: &Value, name: &str) -> bool {
    schema["required"]
        .as_array()
        .is_some_and(|a| a.iter().any(|n| n == name))
}

fn takes(schema: &Value, value: &str) -> bool {
    schema["const"] == value
        || schema["enum"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == value))
}

/// Original synthetic catalogue and operation assets for provider-free import acceptance.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::*;

    pub const TAG: &str = "v1.20260101.0";
    pub const MODEL: &str = "acme/img-a";

    pub fn files() -> BTreeMap<String, Vec<u8>> {
        let mut endpoints = Vec::new();
        let mut operations = Vec::new();
        let mut defs = serde_json::Map::new();
        defs.insert("positive_pixel_pair".into(), json!({
            "type": "string", "pattern": "^[0-9]+x[0-9]+$", "x-gg-positive-pair": "safe-integer",
        }));
        defs.insert(
            "image_result".into(),
            json!({
                "type": "object", "required": ["data"], "properties": {
                    "data": {"type": "array", "items": {
                        "type": "object", "required": ["b64_json", "media_type"], "properties": {
                            "b64_json": {"type": "string", "contentEncoding": "base64"},
                            "media_type": {"enum": ["image/png", "image/jpeg", "image/webp"]},
                        },
                    }},
                },
            }),
        );
        for (interface, input, variant, path) in [
            (
                "images.generations",
                "text",
                "text",
                "/v1/images/generations",
            ),
            (
                "images.edits",
                "multi_image",
                "references",
                "/v1/images/edits",
            ),
        ] {
            let id = format!("image.acme.img-a.{variant}");
            let request_ref = format!("#/$defs/{id}.input");
            let result_ref = "#/$defs/image_result";
            endpoints.push(json!({
                "id": id, "operation": id, "input": input, "interface": interface,
                "request_schema_url": format!("{OPERATIONS_URL}{request_ref}"),
                "result_schema_url": format!("{OPERATIONS_URL}{result_ref}"),
            }));
            operations.push(json!({
                "operation_id": id, "model_id": MODEL, "response_mode": "synchronous",
                "delivery": "transient_base64", "path": path,
                "content_type": if variant == "references" { "multipart/form-data" } else { "application/json" },
                "request_view": if variant == "references" { "normalized_multipart" } else { "json" },
                "request_schema": {"$ref": request_ref}, "result_schema": {"$ref": result_ref},
            }));
            let mut request = json!({
                "type": "object", "additionalProperties": false,
                "required": ["model", "prompt"], "properties": {
                    "model": {"const": MODEL}, "prompt": {"type": "string"},
                    "n": {"type": "integer", "minimum": 1, "maximum": 1},
                    "quality": {"enum": ["high", "max"]},
                    "background": {"enum": ["auto", "transparent", "opaque"]},
                    "output_format": {"enum": ["png", "webp"], "default": "png"},
                    "size": {"anyOf": [
                        {"$ref": "#/$defs/positive_pixel_pair", "x-gg-pixel-limits": {"multiple_of": 16}},
                        {"enum": ["auto"]},
                    ]},
                },
            });
            if variant == "references" {
                request["required"] = json!(["model", "prompt", "image"]);
                request["properties"]["image"] =
                    json!({"type": "array", "minItems": 1, "maxItems": 4});
            }
            defs.insert(format!("{id}.input"), request);
        }
        let models = json!({"$schema": SCHEMA_ID, "image.models": {MODEL: {
            "id": MODEL, "provider_coverage": {"grida": {"disposition": "listed"}},
            "providers": {"grida": {"id": MODEL, "endpoints": endpoints}},
        }}});
        let schema = json!({"$id": SCHEMA_ID, "type": "object", "required": ["image.models"]});
        let ops = json!({"$id": OPERATIONS_URL, "$defs": defs, "x-gg-operations": operations});
        let mut files = BTreeMap::from([
            (
                "models.v1.json".into(),
                serde_json::to_vec(&models).unwrap(),
            ),
            (
                "models.v1.schema.json".into(),
                serde_json::to_vec(&schema).unwrap(),
            ),
            (
                "gg.operations.json".into(),
                serde_json::to_vec(&ops).unwrap(),
            ),
        ]);
        refresh(&mut files);
        files
    }

    pub fn refresh(files: &mut BTreeMap<String, Vec<u8>>) {
        let mut manifest = json!({"version": TAG, "files": {}});
        for (name, path) in [
            ("models.v1.json", "models.json"),
            ("models.v1.schema.json", "schemas/models.schema.json"),
        ] {
            manifest["files"][path] =
                json!({"sha256": sha256_hex(&files[name]), "bytes": files[name].len()});
        }
        files.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        let sums: String = ASSETS[1..]
            .iter()
            .map(|name| format!("{}  {name}\n", sha256_hex(&files[*name])))
            .collect();
        files.insert("SHA256SUMS".into(), sums.into_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(files: &BTreeMap<String, Vec<u8>>) -> Result<Value, String> {
        let release = verify_release(testing::TAG, files)?;
        gg_image_table(
            &release,
            &files["gg.operations.json"],
            testing::MODEL,
            Usd(3_000_000),
        )
    }

    #[test]
    fn a_verified_new_model_becomes_concrete_capped_routes() {
        let files = testing::files();
        let imported = table(&files).unwrap();
        let generate = &imported["routes"][0];
        let edit = &imported["routes"][1];
        assert_eq!(generate["route"], "acme/img-a@gg");
        assert_eq!(generate["model_id"], testing::MODEL);
        assert_eq!(generate["price"], json!({"cap_usd": 3}));
        assert_eq!(
            generate["contract"],
            json!({"adapter": "gg-image", "adapter_behavior": "1", "quality": "max"})
        );
        assert_eq!(edit["max_input_pictures"], 4);
        assert!(
            generate["features"]
                .as_array()
                .unwrap()
                .contains(&json!("alpha"))
        );
        assert!(
            generate["features"]
                .as_array()
                .unwrap()
                .contains(&json!("exact_size"))
        );
        assert_eq!(
            imported["model_source"]["operations_sha256"],
            sha256_hex(&files["gg.operations.json"])
        );
    }

    #[test]
    fn release_integrity_and_schema_are_checked_before_model_selection() {
        let original = testing::files();
        let mut files = original.clone();
        files.get_mut("models.v1.json").unwrap().push(b' ');
        assert!(
            verify_release(testing::TAG, &files)
                .unwrap_err()
                .contains("SHA256SUMS")
        );
        files = original.clone();
        let schema = json!({"$id": SCHEMA_ID, "$ref": "https://acme.test/schema"});
        files.insert(
            "models.v1.schema.json".into(),
            serde_json::to_vec(&schema).unwrap(),
        );
        testing::refresh(&mut files);
        assert!(
            verify_release(testing::TAG, &files)
                .unwrap_err()
                .contains("local references")
        );
        files = original;
        files.insert("models.v1.json".into(), b"{}".to_vec());
        testing::refresh(&mut files);
        assert!(
            verify_release(testing::TAG, &files)
                .unwrap_err()
                .contains("does not validate")
        );
    }

    #[test]
    fn incompatible_required_inputs_and_result_envelopes_are_refused() {
        for changed in ["required", "result", "content_type", "request_view"] {
            let mut files = testing::files();
            let mut operations: Value =
                serde_json::from_slice(&files["gg.operations.json"]).unwrap();
            if changed == "result" {
                operations["$defs"]["image_result"]["properties"]["data"]["items"]["properties"]
                    ["b64_json"] = json!({"type": "string"});
            } else if changed == "required" {
                operations["$defs"]["image.acme.img-a.text.input"]["required"] =
                    json!(["model", "prompt", "seed"]);
            } else {
                operations["x-gg-operations"][0][changed] = "unsupported".into();
            }
            files.insert(
                "gg.operations.json".into(),
                serde_json::to_vec(&operations).unwrap(),
            );
            assert!(table(&files).unwrap_err().contains("does not fit"));
        }
    }

    #[test]
    fn absent_facts_do_not_enable_features_or_old_model_contracts() {
        let mut files = testing::files();
        let mut operations: Value = serde_json::from_slice(&files["gg.operations.json"]).unwrap();
        let properties = operations["$defs"]["image.acme.img-a.text.input"]["properties"]
            .as_object_mut()
            .unwrap();
        properties.remove("background");
        properties.remove("quality");
        properties.remove("size");
        files.insert(
            "gg.operations.json".into(),
            serde_json::to_vec(&operations).unwrap(),
        );
        let imported = table(&files).unwrap();
        assert_eq!(
            imported["routes"][0]["features"],
            json!(["authored_prompt_passthrough", "png_output", "text_to_image"])
        );
        assert!(imported["routes"][0]["contract"].get("quality").is_none());
    }
}
