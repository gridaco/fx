//! Selected-model preparation outside the offline core. Sources yield ordinary route entries;
//! authored routes always win. Only sanitized entries are cached, never provider responses.

use grida_fx_core::docs::project::Project;
use grida_fx_core::host::NodeHost;
use grida_fx_core::project::Planner;
use grida_fx_core::routes::{RouteTable, parse_route_id};
use grida_fx_core::value::{digest, parse_json};
use grida_fx_core::{Error, ErrorKind};
use grida_fx_providers::clock::SystemClock;
use grida_fx_providers::discovery;
use grida_fx_providers::keys::{KeyName, KeySource};
use grida_fx_providers::live::Network;
use grida_fx_providers::setup::Setup;
use grida_fx_providers::transport::http::HttpTransport;
use grida_fx_providers::transport::{Offline, Transport};
use grida_fx_providers::{Endpoints, Keys};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const FRESH_SECONDS: u64 = 3600;

pub(crate) fn empty() -> Value {
    json!({"fx": "routes/v1", "routes": []})
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
    if table.entries.len() > MAX_ENTRIES {
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
        if let Some(price) = entry.get("price") {
            let fields = price
                .as_object()
                .ok_or_else(|| Error::usage("resolved model facts contain invalid pricing"))?;
            if fields
                .keys()
                .any(|key| !["usd", "low_usd", "high_usd"].contains(&key.as_str()))
            {
                return Err(Error::usage(
                    "resolved model facts contain unsupported pricing members",
                ));
            }
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
/// Stand-in runs disable stored keys independently of metadata networking.
pub(crate) fn prepare(
    planner: &mut Planner,
    host: &mut dyn NodeHost,
    snapshot: Option<&Value>,
    isolated: bool,
    allow_stored_keys: bool,
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
        MetadataCredentials::Environment {
            env: &crate::engine::process_env,
            allow_stored_keys,
        },
    )
}

pub(crate) enum MetadataCredentials<'a> {
    Environment {
        env: &'a dyn Fn(&str) -> Option<String>,
        allow_stored_keys: bool,
    },
    Diagnosed {
        settings: Result<(&'a Keys, &'a Endpoints), &'a str>,
        openrouter_store_error: Option<&'a str>,
        network: Network,
    },
}

pub(crate) fn prepare_used(
    project: &Project,
    catalog: &mut RouteTable,
    used: &crate::verbs::doctor::UsedRoutes,
    snapshot: Option<&Value>,
    isolated: bool,
    credentials: MetadataCredentials<'_>,
) -> Result<Value, Error> {
    prepare_with_transport(
        project,
        catalog,
        used,
        snapshot,
        isolated,
        credentials,
        |network| match network {
            Network::Off => Ok(Arc::new(Offline)),
            Network::On => Ok(Arc::new(HttpTransport::new(Default::default()).map_err(
                |_| Error::new(ErrorKind::Internal, "cannot start model metadata transport"),
            )?)),
        },
    )
}

fn prepare_with_transport(
    project: &Project,
    catalog: &mut RouteTable,
    used: &crate::verbs::doctor::UsedRoutes,
    snapshot: Option<&Value>,
    isolated: bool,
    credentials: MetadataCredentials<'_>,
    make_transport: impl Fn(Network) -> Result<Arc<dyn Transport>, Error>,
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
    let mut store = None;
    let mut runtime = None;
    let mut online = false;
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
        if resolved.len() >= MAX_ENTRIES {
            return Err(Error::usage(
                "too many distinct model bindings for one plan; reduce the number of distinct image models",
            ));
        }
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
        } else if let Err(reason) = discovery::validate_selection(capability, &model, &provider) {
            Err(reason)
        } else {
            if setup.is_none() {
                let rt = crate::engine::runtime()?;
                let (keys, endpoints, network) = match credentials {
                    MetadataCredentials::Environment {
                        env,
                        allow_stored_keys,
                    } => {
                        let (loaded, keys, endpoints) =
                            crate::engine::provider_variables(project, env)?;
                        let network = Network::from_env(env);
                        if allow_stored_keys && matches!(network, Network::On) {
                            store = Some((env, loaded));
                        }
                        (keys, endpoints, network)
                    }
                    MetadataCredentials::Diagnosed {
                        settings, network, ..
                    } => {
                        let (keys, endpoints) = settings.map_err(Error::usage)?;
                        (keys.clone(), endpoints.clone(), network)
                    }
                };
                online = matches!(network, Network::On);
                let entered = rt.enter();
                let transport = make_transport(network)?;
                let mut metadata_setup = Setup::new(transport, keys, Arc::new(SystemClock::new()));
                metadata_setup.endpoints = endpoints;
                setup = Some(metadata_setup);
                drop(entered);
                runtime = Some(rt);
            }
            let setup = setup.as_mut().expect("metadata setup");
            // Only account-dependent online metadata may select a stored key. Public GG
            // metadata and offline cached facts need no user-wide credential access.
            if online
                && provider == "openrouter"
                && let MetadataCredentials::Diagnosed {
                    openrouter_store_error: Some(reason),
                    ..
                } = credentials
            {
                return Err(Error::usage(reason));
            }
            if provider == "openrouter"
                && let Some((env, environment)) = store.take()
            {
                crate::engine::stored_keys(
                    env,
                    &mut setup.keys,
                    &environment,
                    &setup.endpoints,
                    &[KeyName::OpenRouter],
                )
                .map_err(Error::usage)?;
                // Stored secrets join the same endpoint-value checks as explicit keys.
                setup.endpoints =
                    Endpoints::from_environment(&environment, &setup.keys).map_err(Error::usage)?;
            }
            let rt = runtime.as_ref().expect("metadata runtime");
            let scope = digest(
                &json!({"provider":provider,"model":model,"capability":capability,"base":setup.endpoints.base(&provider)}),
            );
            let account = setup
                .keys
                .of(&provider)
                .filter(|_| {
                    setup.keys.source_of(&provider) != Some(KeySource::GridaStore)
                        || (online && provider == "openrouter")
                })
                .map(|key| digest(&json!({"credential":key.expose()})));
            let cache = project
                .root
                .join(".fx/models")
                .join(format!("{scope}.json"));
            let cached = cached(&cache, capability, &id, account.as_deref());
            match cached {
                Some((entry, at)) if !online || now().saturating_sub(at) < FRESH_SECONDS => {
                    Ok(entry)
                }
                old => {
                    if !online {
                        Err("model facts are not available locally and networking is disabled; supply an authored route or resolve online once".into())
                    } else {
                        let found =
                            rt.block_on(discovery::resolve(setup, capability, &model, &provider));
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

fn cached(path: &Path, capability: &str, id: &str, account: Option<&str>) -> Option<(Value, u64)> {
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
    let entry = doc["routes"]
        .as_array()?
        .iter()
        .find(|entry| entry["capability"] == capability && entry["route"] == id)?
        .clone();

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

    fn used_openrouter() -> crate::verbs::doctor::UsedRoutes {
        used_model("acme/img-a@openrouter", None)
    }

    fn used_model(selector: &str, provider: Option<&str>) -> crate::verbs::doctor::UsedRoutes {
        use grida_fx_core::routes::{RouteSelection, RoutingSource};

        [(
            (
                "image.generate".into(),
                Some(RouteSelection {
                    selector: selector.into(),
                    provider: provider.map(str::to_owned),
                    source: RoutingSource::Step,
                    provider_source: None,
                }),
            ),
            1,
        )]
        .into_iter()
        .collect()
    }

    fn write_cached_model(project: &Project, account: &str) {
        write_cached_entry(project, account, entry());
    }

    fn write_cached_entry(project: &Project, account: &str, facts: Value) {
        let scope = digest(&json!({
            "provider":"openrouter",
            "model":facts["model_id"],
            "capability":facts["capability"],
            "base":"https://openrouter.ai/api/v1",
        }));
        let path = project
            .root
            .join(".fx/models")
            .join(format!("{scope}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if path.exists() {
            // Metadata refresh publishes a read-only cache file.
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::write(
            path,
            serde_json::to_vec(&json!({
                "kind":"fx-model-cache-v1",
                "fetched_at":now(),
                "account_scope":account,
                "models":{"fx":"routes/v1","routes":[facts]},
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn authored_bindings_snapshots_and_isolated_routes_never_read_credentials() {
        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        let snapshot = json!({"fx":"routes/v1","routes":[entry()]});
        let used = used_openrouter();
        let no_environment = |name: &str| panic!("unexpected environment read: {name}");
        let mut authored = RouteTable::from_document(&snapshot, "fixture").unwrap();
        assert_eq!(
            prepare_used(
                &project,
                &mut authored,
                &used,
                None,
                false,
                MetadataCredentials::Environment {
                    env: &no_environment,
                    allow_stored_keys: true,
                },
            )
            .unwrap(),
            empty(),
        );
        assert_eq!(
            prepare_used(
                &project,
                &mut RouteTable::default(),
                &used,
                Some(&snapshot),
                false,
                MetadataCredentials::Environment {
                    env: &no_environment,
                    allow_stored_keys: true,
                },
            )
            .unwrap(),
            snapshot,
        );
        assert_eq!(
            prepare_used(
                &project,
                &mut RouteTable::default(),
                &used,
                None,
                true,
                MetadataCredentials::Environment {
                    env: &no_environment,
                    allow_stored_keys: true,
                },
            )
            .unwrap(),
            empty(),
        );
    }

    #[test]
    fn invalid_openrouter_selection_does_not_read_provider_settings() {
        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        let mut catalog = RouteTable::default();
        let used = used_model("img-a", Some("openrouter"));
        let prepared = prepare_used(
            &project,
            &mut catalog,
            &used,
            None,
            false,
            MetadataCredentials::Environment {
                env: &|name| panic!("unexpected environment read: {name}"),
                allow_stored_keys: true,
            },
        )
        .unwrap();
        assert_eq!(prepared, empty());
        assert!(
            catalog
                .resolve_selector("image.generate", "img-a", Some("openrouter"))
                .unwrap_err()
                .contains("model metadata is invalid or incompatible")
        );
    }

    #[test]
    fn offline_and_stand_in_metadata_use_cached_facts_without_the_store() {
        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        write_cached_model(&project, "a-previous-account");
        for (offline, allow_stored_keys) in [(true, true), (false, false)] {
            let env = |name: &str| match name {
                "HOME" | "GRIDA_HOME" => panic!("unexpected credential home read"),
                "GRIDA_FX_DISABLE_DOTENV" => Some("1".into()),
                "GRIDA_FX_NETWORK" if offline => Some("off".into()),
                _ => None,
            };
            let prepared = prepare_used(
                &project,
                &mut RouteTable::default(),
                &used_openrouter(),
                None,
                false,
                MetadataCredentials::Environment {
                    env: &env,
                    allow_stored_keys,
                },
            )
            .unwrap();
            assert_eq!(prepared["routes"], json!([entry()]));
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn selected_online_metadata_refuses_invalid_custody_before_a_request() {
        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        write_cached_model(&project, "a-previous-account");
        let env = |name: &str| match name {
            "GRIDA_HOME" => Some("private-relative-credential-home".into()),
            "GRIDA_FX_DISABLE_DOTENV" => Some("1".into()),
            _ => None,
        };
        let error = prepare_used(
            &project,
            &mut RouteTable::default(),
            &used_openrouter(),
            None,
            false,
            MetadataCredentials::Environment {
                env: &env,
                allow_stored_keys: true,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Usage);
        assert!(!error.message.contains("private-relative-credential-home"));
        assert!(error.message.contains("credential"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn stored_openrouter_key_scopes_online_cached_metadata() {
        use grida_fx_providers::transport::HttpResponse;
        use grida_fx_providers::transport::replay::{Expect, ReplayTransport};
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        let home = root.path().canonicalize().unwrap().join("grida-home");
        let providers = home.join("providers");
        std::fs::create_dir_all(&providers).unwrap();
        for directory in [&home, &providers] {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let secret = format!("sk-or-v1-{}", "a".repeat(64));
        let path = providers.join("credentials.toml");
        std::fs::write(
            &path,
            format!(
                "version = 1\n[migration]\nstate = \"unstarted\"\nremoved = []\n[providers.openrouter]\napi_key = \"{secret}\"\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let account = digest(&json!({"credential": secret}));
        write_cached_model(&project, &account);
        let env = |name: &str| match name {
            "GRIDA_HOME" => Some(home.to_str().unwrap().into()),
            "GRIDA_FX_DISABLE_DOTENV" => Some("1".into()),
            _ => None,
        };
        let environment = || MetadataCredentials::Environment {
            env: &env,
            allow_stored_keys: true,
        };
        let prepare = |credentials, replay: Arc<ReplayTransport>| {
            prepare_with_transport(
                &project,
                &mut RouteTable::default(),
                &used_openrouter(),
                None,
                false,
                credentials,
                |_| Ok(replay.clone()),
            )
            .unwrap()
        };
        let hit = Arc::new(ReplayTransport::new(vec![]));
        let prepared = prepare(environment(), hit.clone());
        assert_eq!(prepared["routes"], json!([entry()]));
        assert!(hit.requests().is_empty());

        let endpoints = Endpoints::default();
        let mut keys = Keys::default();
        keys.insert_stored(
            KeyName::OpenRouter,
            grida_fx_providers::keys::Secret::new(&secret),
        );
        for credentials in [
            environment(),
            MetadataCredentials::Diagnosed {
                settings: Ok((&keys, &endpoints)),
                openrouter_store_error: None,
                network: Network::On,
            },
        ] {
            write_cached_model(&project, "other-account");
            let refresh = Arc::new(ReplayTransport::new(vec![
                Expect::get("https://openrouter.ai/api/v1/images/models/acme/img-a/endpoints")
                    .credential("authorization", &format!("Bearer {secret}"))
                    .reply(HttpResponse::json(
                        200,
                        &json!({
                            "id":"acme/img-a", "endpoints":[{"supported_parameters":{}}]
                        }),
                    )),
            ]));
            let prepared = prepare(credentials, refresh.clone());
            refresh.assert_done();
            assert_eq!(prepared["routes"][0]["route"], "acme/img-a@openrouter");
            assert!(prepared["routes"][0].get("price").is_none());
            assert!(!prepared.to_string().contains(&secret));
            assert!(!prepared.to_string().contains(home.to_str().unwrap()));
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn absent_stored_metadata_key_is_checked_once_for_multiple_bindings() {
        use std::cell::Cell;

        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        let missing_home = root.path().canonicalize().unwrap().join("missing-home");
        write_cached_model(&project, "old-account");
        let mut other = entry();
        other["route"] = json!("acme/img-b@openrouter");
        other["model_id"] = json!("acme/img-b");
        write_cached_entry(&project, "old-account", other);
        let mut used = used_openrouter();
        let ((capability, selection), count) = used.first_key_value().unwrap();
        let mut selection = selection.clone().unwrap();
        selection.selector = "acme/img-b@openrouter".into();
        used.insert((capability.clone(), Some(selection)), *count);
        let reads = Cell::new(0);
        let env = |name: &str| match name {
            "GRIDA_HOME" => {
                reads.set(reads.get() + 1);
                Some(missing_home.to_str().unwrap().into())
            }
            "GRIDA_FX_DISABLE_DOTENV" => Some("1".into()),
            _ => None,
        };
        let prepared = prepare_used(
            &project,
            &mut RouteTable::default(),
            &used,
            None,
            false,
            MetadataCredentials::Environment {
                env: &env,
                allow_stored_keys: true,
            },
        )
        .unwrap();
        assert_eq!(prepared["routes"].as_array().unwrap().len(), 2);
        assert_eq!(reads.get(), 1);
        assert!(!missing_home.exists());
    }

    #[test]
    fn prepared_models_share_the_reader_entry_limit() {
        use grida_fx_core::routes::{RouteSelection, RoutingSource};

        let root = tempfile::tempdir().unwrap();
        let project = Project::find(root.path()).unwrap();
        let entries: Vec<Value> = (0..MAX_ENTRIES)
            .map(|index| {
                let model = format!("acme/image-{index:03}");
                json!({"capability":"image.generate","route":format!("{model}@openrouter"),"model_id":model,"price":{"usd":1},"features":["text_to_image"],"contract":{"adapter":"openrouter-image-discovered-v1","adapter_behavior":"1","image_controls":{}}})
            })
            .collect();
        let snapshot = json!({"fx":"routes/v1","routes":entries});
        let mut used = crate::verbs::doctor::UsedRoutes::new();
        for index in 0..=MAX_ENTRIES {
            if index == MAX_ENTRIES {
                let prepared = prepare_used(
                    &project,
                    &mut RouteTable::default(),
                    &used,
                    Some(&snapshot),
                    false,
                    MetadataCredentials::Environment {
                        env: &|_| None,
                        allow_stored_keys: true,
                    },
                )
                .unwrap();
                assert_eq!(prepared["routes"].as_array().unwrap().len(), MAX_ENTRIES);
                let path = root.path().join("prepared.json");
                std::fs::write(&path, serde_json::to_vec(&prepared).unwrap()).unwrap();
                assert_eq!(read(&path).unwrap(), prepared);
            }
            used.insert(
                (
                    "image.generate".into(),
                    Some(RouteSelection {
                        selector: format!("acme/image-{index:03}@openrouter"),
                        provider: None,
                        source: RoutingSource::Step,
                        provider_source: None,
                    }),
                ),
                1,
            );
        }
        let error = prepare_used(
            &project,
            &mut RouteTable::default(),
            &used,
            Some(&snapshot),
            false,
            MetadataCredentials::Environment {
                env: &|_| None,
                allow_stored_keys: true,
            },
        )
        .unwrap_err();
        assert!(error.message.contains("too many distinct model bindings"));
    }

    fn entry() -> Value {
        json!({"capability":"image.generate","route":"acme/img-a@openrouter","model_id":"acme/img-a","price":{"usd":1},"features":["text_to_image"],"contract":{"adapter":"openrouter-image-discovered-v1","adapter_behavior":"1","image_controls":{}}})
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
    fn cached_prices_preserve_binding_and_require_the_same_account_when_given() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("model.json");
        std::fs::write(&path, serde_json::to_vec(&json!({"kind":"fx-model-cache-v1","fetched_at":1,"account_scope":"account-a","models":{"fx":"routes/v1","routes":[entry()]}})).unwrap()).unwrap();
        let (facts, at) = cached(
            &path,
            "image.generate",
            "acme/img-a@openrouter",
            Some("account-a"),
        )
        .unwrap();
        assert_eq!(facts, entry());
        assert_eq!(at, 1);
        assert!(
            cached(
                &path,
                "image.generate",
                "acme/img-a@openrouter",
                Some("account-b")
            )
            .is_none()
        );
        assert!(cached(&path, "image.generate", "acme/img-a@openrouter", None).is_some());
    }
}
