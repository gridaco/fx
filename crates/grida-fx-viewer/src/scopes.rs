//! Validate recorded display metadata without loading workflow definitions.

use crate::read::Node;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

fn schema() -> &'static jsonschema::Validator {
    static SCHEMA: OnceLock<jsonschema::Validator> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let document: Value = serde_json::from_str(include_str!(
            "../../../spec/schemas/fx-viewer-run-v1.schema.json"
        ))
        .expect("embedded viewer schema is JSON");
        jsonschema::validator_for(&json!({
            "$defs": document["$defs"],
            "type":"object", "required":["scopes", "bindings"],
            "properties":{
                "scopes":{"type":"array","items":{"$ref":"#/$defs/scope"}},
                "bindings":{"type":"object","additionalProperties":{
                    "type":"array","uniqueItems":true,"items":{"$ref":"#/$defs/interface_binding"}
                }}
            }
        }))
        .expect("embedded scope schema compiles")
    })
}

fn string<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().unwrap_or_default()
}

fn validate(
    scopes: &Value,
    bindings: &Value,
    nodes: &BTreeSet<String>,
    pending: Option<&BTreeSet<String>>,
) -> Result<(), &'static str> {
    if !schema().is_valid(&json!({"scopes":scopes,"bindings":bindings})) {
        return Err("Scope metadata has an unsupported shape.");
    }
    let entries = scopes.as_array().expect("validated scope list");
    let by_id: BTreeMap<_, _> = entries
        .iter()
        .map(|scope| (string(scope, "id"), scope))
        .collect();
    if entries.len() != by_id.len() {
        return Err("Scope metadata repeats a scope id.");
    }
    let mut members = BTreeSet::new();
    for scope in entries {
        let mut current = scope;
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(string(current, "id")) {
                return Err("Scope metadata contains a parent cycle.");
            }
            let Some(parent) = current["parent"].as_str() else {
                break;
            };
            current = by_id
                .get(parent)
                .ok_or("Scope metadata has an unknown parent.")?;
        }
        if let Some(source) = scope["source"].as_str()
            && (std::path::Path::new(source).is_absolute()
                || source.contains('\\')
                || source.as_bytes().get(1) == Some(&b':'))
        {
            return Err("Scope metadata has a non-portable source.");
        }
        for node in scope["nodes"].as_array().expect("validated members") {
            let id = node.as_str().expect("validated member id");
            if !nodes.contains(id) || !members.insert(id) {
                return Err("Scope metadata has an unknown or repeated node member.");
            }
        }
        if let Some(pending) = pending {
            for member in scope["pending"]
                .as_array()
                .expect("validated pending members")
            {
                if !pending.contains(member.as_str().expect("validated pending path")) {
                    return Err("Scope metadata has an unknown pending repeat.");
                }
            }
        }
        for (field, ports) in [("input_bindings", "inputs"), ("output_bindings", "outputs")] {
            for binding in scope[field].as_array().expect("validated bindings") {
                let target = string(binding, "target_port");
                if !has_port(scope, ports, target) {
                    return Err("Scope metadata binds an undeclared boundary port.");
                }
                validate_source(binding, &by_id, nodes)?;
            }
        }
        if scope["kind"] == "group"
            && (scope["source"] != Value::Null
                || !scope["ports"]["inputs"]
                    .as_object()
                    .expect("validated inputs")
                    .is_empty()
                || !scope["ports"]["outputs"]
                    .as_array()
                    .expect("validated outputs")
                    .is_empty())
        {
            return Err("Inline group metadata declares a workflow interface.");
        }
        if scope["kind"] == "workflow" && scope["source"].as_str().is_none() {
            return Err("Imported workflow metadata has no source.");
        }
    }
    for (id, bindings) in bindings.as_object().expect("validated node bindings") {
        if !nodes.contains(id) {
            return Err("Interface metadata belongs to an unknown node.");
        }
        for binding in bindings.as_array().expect("validated interface bindings") {
            validate_source(binding, &by_id, nodes)?;
        }
    }
    Ok(())
}

fn has_port(scope: &Value, direction: &str, port: &str) -> bool {
    if direction == "inputs" {
        scope["ports"][direction].get(port).is_some()
    } else {
        scope["ports"][direction]
            .as_array()
            .is_some_and(|ports| ports.iter().any(|p| p == port))
    }
}

fn validate_source(
    binding: &Value,
    scopes: &BTreeMap<&str, &Value>,
    nodes: &BTreeSet<String>,
) -> Result<(), &'static str> {
    let source = string(binding, "source");
    match string(binding, "source_kind") {
        "scope_input" | "scope_output" => {
            let scope = scopes
                .get(source)
                .ok_or("Interface metadata names an unknown scope.")?;
            let direction = if binding["source_kind"] == "scope_input" {
                "inputs"
            } else {
                "outputs"
            };
            if !has_port(scope, direction, string(binding, "source_port")) {
                return Err("Interface metadata names an undeclared source port.");
            }
        }
        _ if !nodes.contains(source) => return Err("Interface metadata names an unknown node."),
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_plan(plan: &Value) -> Result<(), &'static str> {
    let Some(scopes) = plan.get("scopes") else {
        return Ok(());
    };
    let instances = plan["instances"]
        .as_array()
        .expect("validated graph instances");
    let nodes = instances
        .iter()
        .map(|v| string(v, "id").to_string())
        .collect();
    let bindings: BTreeMap<_, _> = instances
        .iter()
        .filter_map(|v| v.get("interface_bindings").map(|b| (string(v, "id"), b)))
        .collect();
    let pending = plan["pending"]
        .as_array()
        .expect("validated graph pending")
        .iter()
        .map(|v| string(v, "path").to_string())
        .collect();
    validate(scopes, &json!(bindings), &nodes, Some(&pending))
}

pub(crate) fn project_run(
    scopes: Option<Value>,
    recorded_bindings: Option<&Value>,
    plan: &Value,
    nodes: &mut [Node],
    warnings: &mut Vec<String>,
) -> Option<Value> {
    let Some(mut scopes) = scopes else {
        for node in nodes {
            node.interface_bindings = None;
        }
        return None;
    };
    let mut known: BTreeSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    known.extend(
        plan["instances"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["id"].as_str().map(str::to_string)),
    );
    let bindings = recorded_bindings.cloned().unwrap_or_else(|| {
        let map: BTreeMap<_, _> = nodes
            .iter()
            .filter_map(|n| n.interface_bindings.as_ref().map(|b| (&n.id, b)))
            .collect();
        json!(map)
    });
    // A complete snapshot can precede a dynamic node's first execution event.
    if let Some(bindings) = recorded_bindings.and_then(Value::as_object) {
        known.extend(bindings.keys().cloned());
    }
    if let Err(reason) = validate(&scopes, &bindings, &known, None) {
        warnings.push(format!("{reason} The run is shown without scope metadata."));
        for node in nodes {
            node.interface_bindings = None;
        }
        return None;
    }
    let visible: BTreeSet<_> = nodes.iter().map(|n| n.id.as_str()).collect();
    for scope in scopes.as_array_mut().expect("validated scopes") {
        // The run model omits absent planned nodes; retain only its visible members.
        scope["nodes"]
            .as_array_mut()
            .expect("validated scope nodes")
            .retain(|node| node.as_str().is_some_and(|id| visible.contains(id)));
    }
    Some(scopes)
}
