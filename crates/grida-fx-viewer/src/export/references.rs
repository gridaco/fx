//! Reference closure of the frozen inspection projection, beyond JSON Schema's shapes.

use super::{ExportError, invalid};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_default()
}

fn indexed<'a>(
    values: &'a [Value],
    field: &str,
) -> Result<BTreeMap<&'a str, &'a Value>, ExportError> {
    let by_id: BTreeMap<_, _> = values
        .iter()
        .map(|value| (text(value, field), value))
        .collect();
    if by_id.len() != values.len() {
        return Err(invalid());
    }
    Ok(by_id)
}

fn has_port(ports: &Value, port: &str) -> bool {
    ports
        .as_object()
        .is_some_and(|ports| ports.contains_key(port))
        || list(ports).iter().any(|name| name.as_str() == Some(port))
}

fn binding(
    value: &Value,
    nodes: &BTreeMap<&str, &Value>,
    scopes: &BTreeMap<&str, &Value>,
) -> Result<(), ExportError> {
    let source = text(value, "source");
    let port = text(value, "source_port");
    let valid = match text(value, "source_kind") {
        "scope_input" | "scope_output" => scopes.get(source).is_some_and(|scope| {
            let side = if value["source_kind"] == "scope_input" {
                "inputs"
            } else {
                "outputs"
            };
            has_port(&scope["ports"][side], port)
        }),
        "output" => nodes.get(source).is_some_and(|node| {
            node.get("ports").is_none() || has_port(&node["ports"]["outputs"], port)
        }),
        "fact" => nodes.contains_key(source),
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid()) }
}

fn file_references(value: &Value, inventory: &BTreeMap<&str, &Value>) -> Result<(), ExportError> {
    if let Some(file) = value["file"].as_object() {
        if !file
            .get("digest")
            .and_then(Value::as_str)
            .is_some_and(|digest| inventory.contains_key(digest))
            || !file.get("kind").is_some_and(Value::is_string)
            || !file.get("name").is_some_and(Value::is_string)
            || file.get("size").and_then(Value::as_u64).is_none()
        {
            return Err(invalid());
        }
    } else if let Some(values) = value["list"].as_array() {
        for value in values {
            file_references(value, inventory)?;
        }
    } else if let Some(values) = value["collection"].as_array() {
        for entry in values {
            file_references(&entry[1], inventory)?;
        }
    }
    Ok(())
}

/// Called only after frozen-schema validation; unknown optional fields stay ignored.
pub(super) fn validate(manifest: &Value) -> Result<(), ExportError> {
    let view = &manifest["view"];
    let nodes = indexed(list(&view["nodes"]), "id")?;
    let scopes = indexed(list(&view["scopes"]), "id")?;
    let pending = indexed(list(&view["pending"]), "path")?;
    let inventory = indexed(list(&manifest["artifacts"]), "digest")?;
    if pending.values().any(|repeat| {
        list(&repeat["waiting_on"])
            .iter()
            .any(|id| !nodes.contains_key(id.as_str().unwrap_or_default()))
    }) {
        return Err(invalid());
    }
    let mut members = BTreeSet::new();
    let mut pending_members = BTreeSet::new();
    for (id, scope) in &scopes {
        let mut visited = BTreeSet::from([*id]);
        let mut parent = scope["parent"].as_str();
        while let Some(id) = parent {
            if !visited.insert(id) {
                return Err(invalid());
            }
            parent = scopes.get(id).ok_or_else(invalid)?["parent"].as_str();
        }
        for member in list(&scope["nodes"]) {
            let id = member.as_str().unwrap_or_default();
            if !nodes.contains_key(id) || !members.insert(id) {
                return Err(invalid());
            }
        }
        for member in list(&scope["pending"]) {
            let id = member.as_str().unwrap_or_default();
            if !pending.contains_key(id) || !pending_members.insert(id) {
                return Err(invalid());
            }
        }
        for (field, side) in [("input_bindings", "inputs"), ("output_bindings", "outputs")] {
            for value in list(&scope[field]) {
                binding(value, &nodes, &scopes)?;
                if !has_port(&scope["ports"][side], text(value, "target_port")) {
                    return Err(invalid());
                }
            }
        }
    }
    for node in nodes.values() {
        for field in ["reads", "needs"] {
            if list(&node[field])
                .iter()
                .any(|id| !nodes.contains_key(id.as_str().unwrap_or_default()))
            {
                return Err(invalid());
            }
        }
        if node["judges"]
            .as_str()
            .is_some_and(|id| !nodes.contains_key(id))
        {
            return Err(invalid());
        }
        for field in ["bindings", "interface_bindings"] {
            for value in list(&node[field]) {
                binding(value, &nodes, &scopes)?;
                if let Some(ports) = node.get("ports") {
                    let target = text(value, "target_port");
                    if !has_port(&ports["inputs"], target) && !has_port(&ports["params"], target) {
                        return Err(invalid());
                    }
                }
            }
        }
    }
    for values in [&view["inputs"], &view["outputs"]].into_iter().chain(
        nodes
            .values()
            .flat_map(|node| [&node["with"], &node["outputs"]]),
    ) {
        for value in values
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, value)| value)
        {
            file_references(value, &inventory)?;
        }
    }
    let layout = &manifest["layout"];
    for value in nodes
        .values()
        .chain(scopes.values())
        .chain(pending.values())
    {
        let address = value["step"]
            .as_str()
            .unwrap_or_else(|| text(value, "path"));
        if layout["cells"].get(address).is_none() {
            return Err(invalid());
        }
    }
    let order: BTreeSet<_> = list(&layout["order"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if order.len() != list(&layout["order"]).len()
        || order
            .iter()
            .any(|id| !nodes.contains_key(id) && !scopes.contains_key(id))
        || scopes.keys().any(|id| !order.contains(id))
    {
        return Err(invalid());
    }
    for (id, node) in nodes {
        let unused = node["state"] == "skipped"
            && node["cache"].is_null()
            && node["duration_ms"].is_null()
            && node.get("started_at").is_none()
            && node["outputs"]
                .as_object()
                .is_some_and(|outputs| outputs.is_empty());
        if !unused && !order.contains(id) {
            return Err(invalid());
        }
    }
    Ok(())
}
