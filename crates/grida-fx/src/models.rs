//! Selected-model preparation outside the offline core. Sources yield ordinary route entries;
//! authored routes always win. Only sanitized entries are cached, never provider responses.

use grida_fx_core::docs::project::Project;
use grida_fx_core::host::NodeHost;
use grida_fx_core::money::Usd;
use grida_fx_core::project::Planner;
use grida_fx_core::routes::{RouteTable, parse_route_id};
use grida_fx_core::value::{digest, parse_json};
use grida_fx_core::{Error, ErrorKind};
use grida_fx_providers::clock::SystemClock;
use grida_fx_providers::live::Network;
use grida_fx_providers::setup::Setup;
use grida_fx_providers::transport::http::HttpTransport;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const FRESH_SECONDS: u64 = 3600;

pub(crate) fn empty() -> Value {
    json!({"fx": "routes/v1", "routes": []})
}

/// A resumed CLI invocation uses its current allowance policy, as authored prices do.
pub(crate) fn refresh_allowances(document: &mut Value, project: &Project) {
    for entry in document["routes"].as_array_mut().into_iter().flatten() {
        if entry["price"].get("cap_usd").is_none() {
            continue;
        }
        let Some(capability) = entry["capability"].as_str() else {
            continue;
        };
        let Some(provider) = entry["route"]
            .as_str()
            .and_then(|id| id.rsplit_once('@'))
            .map(|(_, provider)| provider)
        else {
            continue;
        };
        if let Some(allowance) = project
            .document
            .model_allowances
            .get(provider)
            .and_then(|caps| caps.get(capability))
        {
            entry["price"] = json!({"cap_usd":allowance.to_value()});
        }
    }
}

/// A supplied SDK snapshot freezes discovery, including when it contains no entries.
pub(crate) fn read(path: &Path) -> Result<Value, Error> {
    let refuse = || Error::usage("resolved model facts are unreadable, too large or invalid");
    let metadata = std::fs::symlink_metadata(path).map_err(|_| refuse())?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(refuse());
    }
    let bytes = std::fs::read(path).map_err(|_| refuse())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| refuse())?;
    let document = parse_json(text).map_err(|_| refuse())?;
    validate(&document)?;
    Ok(document)
}

fn validate(document: &Value) -> Result<(), Error> {
    let table = RouteTable::from_document(document, "resolved model facts")
        .map_err(|_| Error::usage("resolved model facts fail route validation"))?;
    if table.entries.len() > 128 {
        return Err(Error::usage(
            "resolved model facts contain too many entries",
        ));
    }
    for route in table.entries.values() {
        if !eligible(&route.capability, &route.provider) || !safe_model(&route.model) {
            return Err(Error::usage(
                "resolved model facts name an unsupported binding",
            ));
        }
        let features = grida_fx_providers::capabilities::capability(&route.capability)
            .expect("managed image capability")
            .features;
        if route
            .features
            .iter()
            .any(|feature| !features.contains(&feature.as_str()))
        {
            return Err(Error::usage(
                "resolved model facts contain unsupported features",
            ));
        }
    }
    // No arbitrary contract members: these facts are portable and never contain credentials,
    // private connection addresses, provider error bodies or executable adapter paths.
    for entry in document["routes"].as_array().into_iter().flatten() {
        let allowed = [
            "capability",
            "route",
            "model_id",
            "price",
            "features",
            "max_input_pictures",
            "concurrency",
            "requests_per_minute",
            "contract",
        ];
        if entry
            .as_object()
            .is_none_or(|fields| fields.keys().any(|k| !allowed.contains(&k.as_str())))
        {
            return Err(Error::usage(
                "resolved model facts contain unsupported members",
            ));
        }
        if entry
            .get("model_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !safe_model(id))
        {
            return Err(Error::usage(
                "resolved model facts contain an invalid canonical model",
            ));
        }
        let price = entry
            .get("price")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::usage("resolved model facts need a price"))?;
        if price
            .keys()
            .any(|key| !["usd", "low_usd", "high_usd", "cap_usd"].contains(&key.as_str()))
        {
            return Err(Error::usage(
                "resolved model facts contain unsupported pricing members",
            ));
        }
        if let Some(contract) = entry.get("contract") {
            if contract.as_object().is_none_or(|fields| {
                fields.keys().any(|k| {
                    ![
                        "adapter",
                        "adapter_behavior",
                        "image_controls",
                        "provider_options",
                    ]
                    .contains(&k.as_str())
                })
            }) {
                return Err(Error::usage(
                    "resolved model facts contain unsupported contract members",
                ));
            }
            if let Some(options) = contract.get("provider_options")
                && (options != &json!({"gg":{"output_format":"png"}})
                    || !entry["route"].as_str().is_some_and(|r| r.ends_with("@gg")))
            {
                return Err(Error::usage(
                    "resolved model facts contain unsupported provider options",
                ));
            }
            if let Some(controls) = contract.get("image_controls") {
                let controls = controls
                    .as_object()
                    .ok_or_else(|| Error::usage("invalid image controls"))?;
                for (key, values) in controls {
                    let values = values
                        .as_array()
                        .filter(|values| values.len() <= 256)
                        .ok_or_else(|| Error::usage("invalid image controls"))?;
                    for value in values {
                        let text = value
                            .as_str()
                            .ok_or_else(|| Error::usage("invalid image control value"))?;
                        let valid = match key.as_str() {
                            "background" => ["auto", "opaque", "transparent"].contains(&text),
                            "size" => text.split_once('x').is_some_and(|(w, h)| {
                                w.parse::<u32>().is_ok_and(|w| w > 0)
                                    && h.parse::<u32>().is_ok_and(|h| h > 0)
                            }),
                            _ => false,
                        };
                        if !valid {
                            return Err(Error::usage("invalid image control value"));
                        }
                    }
                }
            }
            let expected = if entry["route"].as_str().is_some_and(|r| r.ends_with("@gg")) {
                "gg-image"
            } else {
                "openrouter-image-discovered-v1"
            };
            if contract["adapter"] != expected || contract["adapter_behavior"] != "1" {
                return Err(Error::usage(
                    "resolved model facts name an unsupported adapter contract",
                ));
            }
        } else {
            return Err(Error::usage(
                "resolved model facts need an adapter contract",
            ));
        }
    }
    Ok(())
}

fn safe_model(model: &str) -> bool {
    model.len() <= 256
        && model
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && model
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/:-".contains(&c))
        && !model.contains("://")
        && !model.split('/').any(|s| s == ".." || s.is_empty())
}

fn eligible(capability: &str, provider: &str) -> bool {
    matches!(capability, "image.generate" | "image.edit") && matches!(provider, "gg" | "openrouter")
}

/// Resolve missing choices before core planning. The snapshot is returned for SDK execution
/// and the run's independent recovery record. This does not evaluate workflow bodies.
pub(crate) fn prepare(
    planner: &mut Planner,
    host: &mut dyn NodeHost,
    snapshot: Option<&Value>,
    isolated: bool,
) -> Result<Value, Error> {
    let mut used = crate::verbs::doctor::UsedRoutes::new();
    crate::verbs::doctor::used_routes(
        &planner.workflow.workflow.steps,
        &mut planner.registry,
        host,
        &mut Vec::new(),
        &mut used,
    );
    prepare_used(
        &planner.project,
        &mut planner.routes,
        &used,
        snapshot,
        isolated,
        &|name| std::env::var(name).ok(),
    )
}

pub(crate) fn prepare_used(
    project: &Project,
    catalog: &mut RouteTable,
    used: &crate::verbs::doctor::UsedRoutes,
    snapshot: Option<&Value>,
    isolated: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Value, Error> {
    if isolated {
        return Ok(empty());
    }
    if let Some(snapshot) = snapshot {
        validate(snapshot)?;
    }
    let mut resolved = Vec::new();
    // Construct a metadata transport lazily: ordinary authored/shipped selections stay local.
    let mut setup = None;
    let mut runtime = None;
    for (capability, selection) in used.keys() {
        let Some(selection) = selection else { continue };
        if catalog
            .resolve_selector(
                capability,
                &selection.selector,
                selection.provider.as_deref(),
            )
            .is_ok()
        {
            continue;
        }
        let (model, provider) = if selection.is_qualified() {
            match parse_route_id(&selection.selector) {
                Ok(pair) => pair,
                Err(_) => continue,
            }
        } else {
            let Some(provider) = &selection.provider else {
                continue;
            };
            // Preserve ambiguity, rather than trying to repair it through discovery.
            if catalog
                .routes_for(capability)
                .iter()
                .any(|r| r.provider == *provider && r.canonical_model() == selection.selector)
            {
                continue;
            }
            (selection.selector.clone(), provider.clone())
        };
        if !eligible(capability, &provider) || !safe_model(&model) {
            continue;
        }
        let id = format!("{model}@{provider}");
        // A different authored canonical ID deliberately hides its native spelling from bare
        // selection. Discovery must never overwrite that entry to repair the selector.
        if catalog
            .entries
            .contains_key(&(capability.clone(), id.clone()))
        {
            continue;
        }
        let allowance = project
            .document
            .model_allowances
            .get(&provider)
            .and_then(|caps| caps.get(capability))
            .copied();
        let result = if let Some(snapshot) = snapshot {
            snapshot["routes"]
                .as_array()
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| entry["capability"] == *capability && entry["route"] == id)
                })
                .cloned()
                .ok_or_else(|| {
                    "the prepared model facts do not include this binding; plan again".to_string()
                })
                .and_then(|entry| {
                    if let Some(cap) = entry["price"].get("cap_usd")
                        && Usd::from_value(cap).ok() != allowance
                    {
                        return Err("the model allowance changed since planning; plan again".into());
                    }
                    Ok(entry)
                })
        } else {
            if setup.is_none() {
                let rt = crate::engine::runtime()?;
                let (_, keys, endpoints) = crate::engine::provider_variables(project, env)?;
                let entered = rt.enter();
                let transport: Arc<dyn grida_fx_providers::transport::Transport> =
                    match Network::from_env(env) {
                        Network::Off => Arc::new(grida_fx_providers::transport::Offline),
                        Network::On => {
                            Arc::new(HttpTransport::new(Default::default()).map_err(|_| {
                                Error::new(
                                    ErrorKind::Internal,
                                    "cannot start model metadata transport",
                                )
                            })?)
                        }
                    };
                let mut metadata_setup = Setup::new(transport, keys, Arc::new(SystemClock::new()));
                metadata_setup.endpoints = endpoints;
                setup = Some(metadata_setup);
                drop(entered);
                runtime = Some(rt);
            }
            let setup = setup.as_ref().expect("metadata setup");
            let rt = runtime.as_ref().expect("metadata runtime");
            let network = !matches!(Network::from_env(env), Network::Off);
            let scope = digest(
                &json!({"provider":provider,"model":model,"capability":capability,"base":setup.endpoints.base(&provider)}),
            );
            let account = setup
                .keys
                .of(&provider)
                .map(|key| digest(&json!({"credential":key.expose()})));
            let cache = project
                .root
                .join(".fx/models")
                .join(format!("{scope}.json"));
            let cached = cached(&cache, capability, &id, allowance, account.as_deref());
            match cached {
                Some((entry, at)) if !network || now().saturating_sub(at) < FRESH_SECONDS => {
                    Ok(entry)
                }
                old => {
                    if !network {
                        Err("model facts are not available locally and networking is disabled; supply an authored route or resolve online once".into())
                    } else {
                        let found = rt.block_on(grida_fx_providers::discovery::resolve(
                            setup, capability, &model, &provider, allowance,
                        ));
                        match found {
                            Ok(entry) => {
                                let doc = json!({"fx":"routes/v1","routes":[entry.clone()]});
                                validate(&doc)?;
                                let cached = json!({"kind":"fx-model-cache-v1","fetched_at":now(),"account_scope":account,"models":doc});
                                // Optional cache failure never changes the resolved binding.
                                if std::fs::create_dir_all(cache.parent().expect("cache parent"))
                                    .is_ok()
                                {
                                    let _ = grida_fx_runtime::store::atomic_write(
                                        &cache,
                                        &serde_json::to_vec(&cached).expect("JSON"),
                                        true,
                                    );
                                }
                                Ok(entry)
                            }
                            Err(reason) if transient(&reason) && old.is_some() => {
                                Ok(old.expect("cached fallback").0)
                            }
                            Err(reason) => Err(reason),
                        }
                    }
                }
            }
        };
        match result {
            Ok(entry) => {
                let doc = json!({"fx":"routes/v1","routes":[entry.clone()]});
                let table = RouteTable::from_document(&doc, "resolved model facts")?;
                catalog.overlay(table);
                resolved.push(entry);
            }
            Err(reason) => catalog.resolution_error(
                capability,
                &id,
                format!("model discovery for {id} ({capability}): {reason}"),
            ),
        }
    }
    Ok(json!({"fx":"routes/v1","routes":resolved}))
}

fn transient(reason: &str) -> bool {
    reason.contains("unavailable")
        || reason.contains("transport")
        || reason.contains("HTTP 5")
        || reason.contains("HTTP 429")
}

fn cached(
    path: &Path,
    capability: &str,
    id: &str,
    allowance: Option<Usd>,
    account: Option<&str>,
) -> Option<(Value, u64)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return None;
    }
    let cache = parse_json(std::str::from_utf8(&std::fs::read(path).ok()?).ok()?).ok()?;
    if cache["kind"] != "fx-model-cache-v1" {
        return None;
    }
    // A changed credential must refresh account-dependent facts. With no credential, local
    // facts remain usable for paid-result replay; no uncached paid call can be sent.
    if account.is_some_and(|account| cache["account_scope"].as_str() != Some(account)) {
        return None;
    }
    let doc = &cache["models"];
    validate(doc).ok()?;
    let mut entry = doc["routes"]
        .as_array()?
        .iter()
        .find(|entry| entry["capability"] == capability && entry["route"] == id)?
        .clone();
    if entry["price"].get("cap_usd").is_some() {
        entry["price"] = json!({"cap_usd": allowance?.to_value()});
    }
    Some((entry, cache["fetched_at"].as_u64()?))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Value {
        json!({"capability":"image.generate","route":"acme/img-a@openrouter","model_id":"acme/img-a","price":{"cap_usd":1},"features":["text_to_image"],"contract":{"adapter":"openrouter-image-discovered-v1","adapter_behavior":"1","image_controls":{}}})
    }

    #[test]
    fn snapshots_and_cache_reject_executable_or_private_metadata() {
        assert!(safe_model("acme/img-a:variant"));
        assert!(!safe_model("https://private.test/model"));
        let valid = entry();
        validate(&json!({"fx":"routes/v1","routes":[valid.clone()]})).unwrap();
        let mut bad = Vec::new();
        let mut route = valid.clone();
        route["contract"]["adapter"] = json!("/private/provider.py#adapter");
        bad.push(route);
        let mut route = valid.clone();
        route["contract"]["provider_options"] = json!({"openrouter":{"key":"secret-value"}});
        bad.push(route);
        let mut route = valid.clone();
        route["features"] = json!(["https://private.test?token=secret-value"]);
        bad.push(route);
        let mut route = valid.clone();
        route["model_id"] = json!("https://private.test?token=secret-value");
        bad.push(route);
        let mut route = valid.clone();
        route["contract"]["image_controls"] =
            json!({"size":["https://private.test?token=secret-value"]});
        bad.push(route);
        let mut route = valid;
        route["source_url"] = json!("https://private.test?token=secret-value");
        bad.push(route);
        for route in bad {
            let error = validate(&json!({"fx":"routes/v1","routes":[route]})).unwrap_err();
            assert!(!error.message.contains("secret-value"));
            assert!(!error.message.contains("/private/"));
        }
    }

    #[test]
    fn cached_allowances_follow_current_policy_and_preserve_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("model.json");
        std::fs::write(&path, serde_json::to_vec(&json!({"kind":"fx-model-cache-v1","fetched_at":1,"models":{"fx":"routes/v1","routes":[entry()]}})).unwrap()).unwrap();
        let (one, _) = cached(
            &path,
            "image.generate",
            "acme/img-a@openrouter",
            Some(Usd(1_000_000)),
            None,
        )
        .unwrap();
        let (two, _) = cached(
            &path,
            "image.generate",
            "acme/img-a@openrouter",
            Some(Usd(2_000_000)),
            None,
        )
        .unwrap();
        assert_eq!(two["price"]["cap_usd"], 2);
        assert!(cached(&path, "image.generate", "acme/img-a@openrouter", None, None).is_none());
        let fingerprint = |entry| {
            RouteTable::from_document(&json!({"fx":"routes/v1","routes":[entry]}), "test")
                .unwrap()
                .entries
                .values()
                .next()
                .unwrap()
                .fingerprint()
        };
        assert_eq!(fingerprint(one), fingerprint(two));
    }
}
