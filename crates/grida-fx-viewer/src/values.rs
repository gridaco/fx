//! Recorded value presentation. Evaluation and source attribution belong to the engine.

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::OnceLock;

const SCHEMA: &str = include_str!("../../../spec/schemas/fx-viewer-run-v1.schema.json");

fn schema() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let document: Value = serde_json::from_str(SCHEMA).expect("embedded viewer schema");
        jsonschema::validator_for(&json!({
            "$defs": document["$defs"], "type":"object",
            "required":["value_views","value_links"],
            "properties": {
                "value_views":document["properties"]["value_views"],
                "value_links":document["properties"]["value_links"],
            }
        }))
        .expect("embedded value schema compiles")
    })
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn bounded(value: &Value, depth: usize) -> bool {
    depth <= 64
        && match value {
            Value::Array(items) => items.iter().all(|item| bounded(item, depth + 1)),
            Value::Object(items) => items.values().all(|item| bounded(item, depth + 1)),
            _ => true,
        }
}

fn shape(document: &Value) -> bool {
    let (Some(views), Some(links)) = (document.get("value_views"), document.get("value_links"))
    else {
        return false;
    };
    // The capture limit applies to encoded data, not the surrounding array/view envelope.
    if list(views)
        .iter()
        .any(|view| view.get("data").is_some_and(|data| !bounded(data, 0)))
        || !bounded(links, 0)
        || !schema().is_valid(document)
    {
        return false;
    }
    let mut ids = BTreeSet::new();
    // Context is an opaque semantic label, not a filesystem path. Quoted repeat keys
    // legitimately contain escaped apostrophes, backslashes and other user characters.
    list(views)
        .iter()
        .all(|view| ids.insert(view["id"].as_str().unwrap_or_default()))
        && list(links).iter().all(|link| {
            // Only a producer output/fact can be an instance source. A target names a with field.
            (link["source"]["kind"] != "instance" || link["source"].get("source_kind").is_some())
                && link["target"].get("source_kind").is_none()
                && link["source"] != link["target"]
        })
}

fn owned(view: &Value, scopes: Option<&Value>) -> bool {
    view["scope_id"].is_null()
        || scopes.is_some_and(|scopes| {
            list(scopes)
                .iter()
                .any(|scope| scope["id"] == view["scope_id"])
        })
}

fn endpoint(endpoint: &Value, values: &BTreeSet<&str>, nodes: &BTreeSet<&str>) -> bool {
    match endpoint["kind"].as_str() {
        Some("value") => endpoint["value_id"]
            .as_str()
            .is_some_and(|id| values.contains(id)),
        Some("instance") => endpoint["instance_id"]
            .as_str()
            .is_some_and(|id| nodes.contains(id)),
        _ => false,
    }
}

/// Materialized plans have a complete reference set and reject invented owners/endpoints.
pub(crate) fn validate_plan(plan: &Value) -> Result<(), &'static str> {
    if plan.get("value_views").is_none() && plan.get("value_links").is_none() {
        return Ok(());
    }
    if !shape(plan) {
        return Err("Value presentation metadata has an unsupported shape.");
    }
    let values: BTreeSet<_> = list(&plan["value_views"])
        .iter()
        .filter_map(|view| view["id"].as_str())
        .collect();
    let nodes: BTreeSet<_> = list(&plan["instances"])
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    if list(&plan["value_views"])
        .iter()
        .any(|view| !owned(view, plan.get("scopes")))
        || list(&plan["value_links"]).iter().any(|link| {
            !endpoint(&link["source"], &values, &nodes)
                || !endpoint(&link["target"], &values, &nodes)
        })
    {
        return Err("Value presentation metadata has an unknown owner or endpoint.");
    }
    Ok(())
}

/// An engine snapshot replaces the preceding values, so pending results can become known.
/// Absence in legacy events retains the newest recorded evidence; invalid snapshots cannot
/// replace a previously valid one.
#[derive(Default)]
pub(crate) struct Captured {
    views: Option<Value>,
    links: Option<Value>,
}

impl Captured {
    pub(crate) fn from_plan(plan: &Value, warnings: &mut Vec<String>) -> Self {
        let mut captured = Self::default();
        captured.capture(plan, warnings);
        captured
    }

    pub(crate) fn capture(&mut self, document: &Value, warnings: &mut Vec<String>) {
        if document.get("value_views").is_none() && document.get("value_links").is_none() {
            return;
        }
        if !shape(document) {
            warnings.push(
                "Recorded value presentation metadata has an unsupported shape and was ignored."
                    .into(),
            );
            return;
        }
        self.views = document.get("value_views").cloned();
        self.links = document.get("value_links").cloned();
    }

    pub(crate) fn project(
        self,
        scopes: Option<&Value>,
        nodes: &[super::read::Node],
    ) -> (Option<Value>, Option<Value>) {
        let Some(views) = self.views else {
            return (None, None);
        };
        let views: Vec<_> = list(&views)
            .iter()
            .filter(|view| owned(view, scopes))
            .cloned()
            .collect();
        let values: BTreeSet<_> = views
            .iter()
            .filter_map(|view| view["id"].as_str())
            .collect();
        let nodes: BTreeSet<_> = nodes.iter().map(|node| node.id.as_str()).collect();
        let links = list(self.links.as_ref().unwrap_or(&Value::Null))
            .iter()
            .filter(|link| {
                endpoint(&link["source"], &values, &nodes)
                    && endpoint(&link["target"], &values, &nodes)
            })
            .cloned()
            .collect::<Vec<_>>();
        (Some(Value::Array(views)), Some(Value::Array(links)))
    }
}

fn route_schema(field: &str) -> jsonschema::Validator {
    let document: Value = serde_json::from_str(SCHEMA).expect("embedded viewer schema");
    jsonschema::validator_for(&document["$defs"]["node"]["properties"][field])
        .expect("embedded route schema compiles")
}

/// Normalize planned fingerprints and runtime route ids to one positive projection.
pub(crate) fn routes(value: Option<&Value>) -> Option<Value> {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    let entries = value?.as_object()?;
    let value = Value::Object(
        entries
            .iter()
            .map(|(name, value)| {
                let value = if value.is_string() {
                    json!({"route":value})
                } else {
                    let mut projected = serde_json::Map::new();
                    for field in ["route", "fingerprint"] {
                        if let Some(value) = value.get(field) {
                            projected.insert(field.into(), value.clone());
                        }
                    }
                    Value::Object(projected)
                };
                (name.clone(), value)
            })
            .collect(),
    );
    VALIDATOR
        .get_or_init(|| route_schema("routes"))
        .is_valid(&value)
        .then_some(value)
}

pub(crate) fn routing(value: Option<&Value>) -> Option<Value> {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    let value = value?;
    VALIDATOR
        .get_or_init(|| route_schema("routing"))
        .is_valid(value)
        .then(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(byte: &str, state: &str) -> Value {
        let mut view = json!({"id":format!("value:{}",byte.repeat(64)),"scope_id":null,"kind":"input","name":"caption","state":state});
        if state == "known" {
            view["data"] = json!({"value":null});
        }
        view
    }

    #[test]
    fn snapshots_preserve_null_absence_and_pending_then_replace_with_known_evidence() {
        let mut warnings = Vec::new();
        let mut captured = Captured::from_plan(
            &json!({
                "value_views":[view("a", "pending"),view("b", "absent"),view("c", "known")],"value_links":[]
            }),
            &mut warnings,
        );
        assert_eq!(captured.views.as_ref().unwrap()[0]["state"], "pending");
        assert_eq!(captured.views.as_ref().unwrap()[1]["state"], "absent");
        assert_eq!(
            captured.views.as_ref().unwrap()[2]["data"],
            json!({"value":null})
        );
        captured.capture(
            &json!({"value_views":[view("a", "known")],"value_links":[]}),
            &mut warnings,
        );
        captured.capture(&json!({"event":"legacy_snapshot"}), &mut warnings);
        let (views, links) = captured.project(None, &[]);
        assert_eq!(views.unwrap(), json!([view("a", "known")]));
        assert_eq!(links.unwrap(), json!([]));
        assert!(warnings.is_empty());
    }

    #[test]
    fn contextual_repeat_labels_are_not_mistaken_for_filesystem_paths() {
        for context in [
            r"items['it\'s'].draw#1",
            r"items['a\\b'].draw#1",
            "items['line\nnext'].draw#1",
            &"🌿".repeat(256),
        ] {
            let mut value = view("a", "known");
            value["kind"] = json!("let");
            value["context"] = json!(context);
            assert!(validate_plan(&json!({"value_views":[value],"value_links":[]})).is_ok());
        }
    }

    #[test]
    fn data_depth_limit_excludes_the_snapshot_envelope() {
        let mut nested = json!(null);
        for _ in 0..63 {
            nested = json!({"nested":nested});
        }
        let mut captured = view("a", "known");
        captured["data"] = json!({"value":nested});
        let mut snapshot = json!({"value_views":[captured],"value_links":[]});
        assert!(
            shape(&snapshot),
            "64 encoded-data levels remain valid inside their view envelope"
        );
        snapshot["value_views"][0]["data"] = json!({"value":{"one_more":nested}});
        assert!(
            !shape(&snapshot),
            "65 encoded-data levels exceed the capture contract"
        );
    }

    #[test]
    fn malformed_snapshot_cannot_replace_recorded_values_or_invent_an_owner() {
        let valid = view("a", "known");
        let mut warnings = Vec::new();
        let mut captured = Captured::from_plan(
            &json!({"value_views":[valid.clone()],"value_links":[]}),
            &mut warnings,
        );
        let mut invalid = valid.clone();
        invalid["source"] = json!("/private/workflow.yaml");
        captured.capture(
            &json!({"value_views":[invalid],"value_links":[]}),
            &mut warnings,
        );
        assert_eq!(captured.project(None, &[]).0.unwrap(), json!([valid]));
        assert_eq!(warnings.len(), 1);
        let mut orphan = view("b", "pending");
        orphan["scope_id"] = json!("scope:unknown#1");
        assert!(
            validate_plan(&json!({"value_views":[orphan],"value_links":[],"instances":[]}))
                .is_err()
        );
        assert!(
            validate_plan(
                &json!({"value_views":[view("a","known"),view("a","known")],"value_links":[]})
            )
            .is_err()
        );
    }

    #[test]
    fn lineage_endpoints_are_recorded_references_not_renderer_ids() {
        let value = view("a", "known");
        let mut plan = json!({"value_views":[value],"value_links":[{
            "source":{"kind":"value","value_id":format!("value:{}","a".repeat(64))},
            "target":{"kind":"instance","instance_id":"draw#1","port":"prompt"}
        }],"instances":[{"id":"draw#1"}]});
        assert!(validate_plan(&plan).is_ok());
        plan["value_links"][0]["target"]["instance_id"] = json!("renderer-card-42");
        assert!(validate_plan(&plan).is_err());
        plan["value_links"][0]["source"] =
            json!({"kind":"instance","instance_id":"draw#1","port":"image"});
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn explicit_value_files_join_the_inventory_but_file_shaped_json_stays_opaque() {
        let fixture = crate::tests::Fixture::new("image/png");
        let path = fixture.root().join("plan.json");
        let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut file = view("a", "known");
        file["data"] = json!({"file":{"digest":fixture.digest,"kind":"image/png","name":"reference.png","size":27}});
        // Match the recorded artifact's size exactly; only its existing placed bytes may serve it.
        file["data"]["file"]["size"] = json!(b"a synthetic offline artifact".len());
        let mut ordinary = view("b", "known");
        ordinary["data"] = json!({"value":{"file":{"digest":"c".repeat(64),"kind":"image/png","name":"plain.json","size":7}}});
        plan["value_views"] = json!([file, ordinary]);
        plan["value_links"] = json!([]);
        std::fs::write(path, plan.to_string()).unwrap();
        let snapshot = crate::read::read_run(&fixture.root()).unwrap();
        assert_eq!(
            snapshot.document.value_views.as_ref().unwrap()[1]["data"]["value"]["file"]["name"],
            "plain.json"
        );
        assert!(
            snapshot
                .document
                .artifacts
                .iter()
                .any(|artifact| artifact.digest == fixture.digest && artifact.available)
        );
        assert!(
            !snapshot
                .document
                .artifacts
                .iter()
                .any(|artifact| artifact.digest == "c".repeat(64))
        );
    }

    #[test]
    fn recorded_routes_normalize_without_source_or_transport_extensions() {
        assert_eq!(
            routes(Some(&json!({"image.generate":"img-a@acme"}))).unwrap(),
            json!({"image.generate":{"route":"img-a@acme"}})
        );
        assert_eq!(routes(Some(&json!({"image.generate":{"route":"img-a@acme","fingerprint":"a".repeat(64),"private":"/private/path"}}))).unwrap(), json!({"image.generate":{"route":"img-a@acme","fingerprint":"a".repeat(64)}}));
        assert!(routes(Some(&json!({"image.generate":"/private/path"}))).is_none());
        assert!(routing(Some(&json!({"image.generate":{"model":"img-a","model_source":"step","provider_preference":"acme","provider_source":"workflow_home","private":"path"}}))).is_none());
    }
}

#[cfg(test)]
mod input_snapshots {
    use super::*;
    use std::io::Read;

    #[test]
    fn transformed_outputs_do_not_hide_the_original_recorded_input_bytes() {
        let fixture = crate::tests::Fixture::new("image/png");
        let bytes = b"an original synthetic reference, different from every output";
        let digest = grida_fx_core::value::file_digest(bytes);
        assert_ne!(digest, fixture.digest);
        let directory = fixture.root().join("values");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join(&digest), bytes).unwrap();
        let path = fixture.root().join("plan.json");
        let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        plan["value_views"] = json!([{
            "id":format!("value:{}", "a".repeat(64)),"scope_id":null,"kind":"input","name":"reference","state":"known",
            "data":{"file":{"digest":digest,"kind":"image/png","name":"reference.png","size":bytes.len()}}
        }]);
        plan["value_links"] = json!([]);
        std::fs::write(path, plan.to_string()).unwrap();
        let snapshot = crate::read::read_run(&fixture.root()).unwrap();
        let input = snapshot
            .document
            .artifacts
            .iter()
            .find(|artifact| artifact.digest == digest)
            .unwrap();
        assert!(input.available);
        let (mut file, artifact, path) = snapshot
            .open_artifact_with_path(&fixture.root(), &digest)
            .unwrap();
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert_eq!(artifact.name, "reference.png");
        assert_eq!(path, directory.join(&digest));
        // No other project file or cache path can replace the optional recorded snapshot.
        std::fs::remove_file(directory.join(&digest)).unwrap();
        let snapshot = crate::read::read_run(&fixture.root()).unwrap();
        assert!(
            !snapshot
                .document
                .artifacts
                .iter()
                .find(|artifact| artifact.digest == digest)
                .unwrap()
                .available
        );
        assert!(snapshot.open_artifact(&fixture.root(), &digest).is_err());
    }
}
