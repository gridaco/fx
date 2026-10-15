//! Provider declarations (spec/providers.md §1): for each provider FX sends requests to, the
//! variable its key comes from and the header the key travels in, its base URL and the variable
//! that may move it, and the hosts its results may be downloaded from.
//!
//! FX's own providers are [`built_in`], in the order `grida-fx doctor` lists them; Codex and Grok
//! Build are [`RUNTIMES`], not declarations. A project declares others under `providers:` in the
//! planning project's `fx.yaml` ([`parse`]); [`Declarations`] is the built-in ones, then the
//! project's. The keys, the `.env` reader, the base URLs, `doctor` and the variables Codex, Grok
//! Build, the background service and the browser opener never inherit all read them. A
//! declaration is configuration and is not hashed; only its `family` or its `adapter` reaches a
//! route's fingerprint, through the contract it completes ([`crate::routes::complete_contract`],
//! [`crate::routes::FAMILY_ADAPTERS`], a declared adapter's [`LoadedAdapter`]; spec/identity.md
//! §7). A declared adapter is read as `<path>#<attribute>` ([`DeclaredAdapter`]); what it serves
//! is learned only by loading it while planning ([`crate::adapters::load_declared`]).
//!
//! A declaration is refused while `fx.yaml` is read, `<file>: providers.<name>…: <sentence>`:
//! - a name FX ships: `<name> is a provider FX ships`;
//! - a family and an adapter both: `a provider has a family or an adapter, not both`;
//! - a variable already read, by a built-in declaration, an earlier one or the same one, compared
//!   in any letter case: `<VARIABLE> is already read for <provider>`;
//! - a variable FX or the programs it starts read for themselves ([`reserved`]):
//!   `<VARIABLE> is reserved for FX and the programs it starts`;
//! - a default base URL that does not normalize ([`normalize_base_url`]).

use crate::error::{Error, Result};
use crate::registry::SourceDigests;
use grida_fx_protocol::CallKind;
use indexmap::IndexMap;
use serde_json::Value;
use std::sync::{Arc, LazyLock};

/// The providers that are programs the user installed, not declarations: they bring their own
/// sign-in and network.
pub const RUNTIMES: [&str; 2] = ["codex", "grok"];

/// Where a provider's key comes from and how it is sent: `<header>: <scheme> <key>`, or
/// `<header>: <key>` with no scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub variable: String,
    /// Lowercase.
    pub header: String,
    /// `Bearer`, `Key`, or empty.
    pub scheme: String,
}

impl Credential {
    /// What precedes the key in its header: the scheme and a space, or nothing.
    pub fn prefix(&self) -> String {
        if self.scheme.is_empty() {
            String::new()
        } else {
            format!("{} ", self.scheme)
        }
    }
}

/// A provider's base URL: its default (normalized), and the variable whose value replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl {
    pub variable: Option<String>,
    pub default: String,
}

/// One provider's declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// The provider part of its routes (`<model>@<name>`).
    pub name: String,
    /// How base-URL refusals name it (`OpenAI base_url must …`): the name, for a declared one.
    pub label: String,
    /// None: the provider is sent no key and needs none.
    pub credential: Option<Credential>,
    pub base_url: BaseUrl,
    /// A host, or `*.` and a domain for every host below it.
    pub download_hosts: Vec<String>,
    /// The wire it speaks (`openai-compatible`), for a declared provider that names one.
    pub family: Option<String>,
    /// Its own adapter, for a declared provider that names one (spec/adapters.md §9).
    pub adapter: Option<DeclaredAdapter>,
}

/// A declaration's `adapter`: a project module's adapter (spec/adapters.md §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredAdapter {
    /// The module, POSIX, relative to the planning project's root, without `./` or `..`.
    pub path: String,
    /// The module-level name it is declared under.
    pub attribute: String,
    /// What loading it while planning learned ([`crate::adapters::load_declared`]); `None` until
    /// then, and then nothing serves the provider and no contract is completed from it.
    pub loaded: Option<LoadedAdapter>,
}

impl DeclaredAdapter {
    /// `<path>#<attribute>`: its name in a contract's `adapter` and in messages.
    pub fn name(&self) -> String {
        format!("{}#{}", self.path, self.attribute)
    }
}

/// What a declared adapter told planning, and the digest of its source (spec/adapters.md §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedAdapter {
    /// Its version; `None` for one whose behaviour is its source.
    pub version: Option<u64>,
    /// Each capability it serves, in its order, and whether it is a request or a job.
    pub capabilities: IndexMap<String, CallKind>,
    /// Its source closure's file digests, with no resources (spec/identity.md §6).
    pub source: SourceDigests,
    /// A versioned adapter whose source no longer matches fx.lock: the problem every step on a
    /// route it serves gets.
    pub drift: Option<String>,
}

impl LoadedAdapter {
    /// Its contract's `adapter_behavior` (spec/identity.md §7): the version, an integer, or
    /// `source:<digest>`.
    pub fn behavior(&self) -> Value {
        match self.version {
            Some(version) => Value::from(version),
            None => Value::String(format!("source:{}", self.source.digest())),
        }
    }

    /// Whether it serves `capability`.
    pub fn serves(&self, capability: &str) -> bool {
        self.capabilities.contains_key(capability)
    }
}

impl Declaration {
    /// The variables it reads: its key's, then its base URL's.
    pub fn variables(&self) -> impl Iterator<Item = &str> {
        let key = self.credential.as_ref().map(|c| c.variable.as_str());
        key.into_iter().chain(self.base_url.variable.as_deref())
    }
}

/// One built-in declaration (spec/providers.md §1).
#[allow(clippy::too_many_arguments)]
fn built(
    name: &str,
    label: &str,
    variable: &str,
    header: &str,
    scheme: &str,
    base_variable: Option<&str>,
    default: &str,
    download_hosts: &[&str],
) -> Declaration {
    Declaration {
        name: name.into(),
        label: label.into(),
        credential: Some(Credential {
            variable: variable.into(),
            header: header.into(),
            scheme: scheme.into(),
        }),
        base_url: BaseUrl {
            variable: base_variable.map(String::from),
            default: default.into(),
        },
        download_hosts: download_hosts.iter().map(|host| host.to_string()).collect(),
        family: None,
        adapter: None,
    }
}

/// FX's providers, as spec/providers.md §1 lists them: Grida Gateway first.
static BUILT_IN: LazyLock<Declarations> = LazyLock::new(|| {
    let (authorization, bearer) = ("authorization", "Bearer");
    let all = vec![
        built(
            "gg",
            "Grida Gateway",
            "GG_API_KEY",
            authorization,
            bearer,
            Some("GG_BASE_URL"),
            "https://gateway.grida.ai/v1",
            &[],
        ),
        built(
            "openai",
            "OpenAI",
            "OPENAI_API_KEY",
            authorization,
            bearer,
            Some("OPENAI_BASE_URL"),
            "https://api.openai.com/v1",
            &[],
        ),
        built(
            "openrouter",
            "OpenRouter",
            "OPENROUTER_API_KEY",
            authorization,
            bearer,
            Some("OPENROUTER_BASE_URL"),
            "https://openrouter.ai/api/v1",
            &[],
        ),
        // The run host only: the queue host, https://queue.fal.run, is not configurable.
        built(
            "fal",
            "fal",
            "FAL_KEY",
            authorization,
            "Key",
            Some("FAL_BASE_URL"),
            "https://fal.run",
            &["fal.media", "*.fal.media"],
        ),
        built(
            "tripo",
            "Tripo",
            "TRIPO_API_KEY",
            authorization,
            bearer,
            None,
            "https://openapi.tripo3d.ai/v3",
            &[
                "tripo3d.ai",
                "*.tripo3d.ai",
                "tripo-data.rg1.data.tripo3d.com",
            ],
        ),
        built(
            "elevenlabs",
            "ElevenLabs",
            "ELEVENLABS_API_KEY",
            "xi-api-key",
            "",
            Some("ELEVENLABS_BASE_URL"),
            "https://api.elevenlabs.io/v1",
            &[],
        ),
    ];
    let built_in = all.len();
    Declarations {
        all: Arc::from(all),
        built_in,
    }
});

/// FX's own declarations, in their order.
pub fn built_in() -> &'static [Declaration] {
    &BUILT_IN.all
}

/// The declarations of an invocation: the built-in ones, then the planning project's, in the
/// order its `fx.yaml` lists them. Cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declarations {
    all: Arc<[Declaration]>,
    built_in: usize,
}

impl Default for Declarations {
    fn default() -> Declarations {
        Declarations::built_in()
    }
}

impl Declarations {
    /// FX's own alone.
    pub fn built_in() -> Declarations {
        BUILT_IN.clone()
    }

    /// FX's own, then `declared`, which [`parse`] has checked.
    pub fn with(declared: &[Declaration]) -> Declarations {
        if declared.is_empty() {
            return Declarations::built_in();
        }
        let all: Vec<Declaration> = built_in().iter().chain(declared).cloned().collect();
        Declarations {
            all: Arc::from(all),
            built_in: BUILT_IN.built_in,
        }
    }

    /// Every declaration, the built-in ones first.
    pub fn iter(&self) -> std::slice::Iter<'_, Declaration> {
        self.all.iter()
    }

    /// The project's declarations alone.
    pub fn declared(&self) -> &[Declaration] {
        &self.all[self.built_in..]
    }

    /// The declaration of `provider`, if any.
    pub fn get(&self, provider: &str) -> Option<&Declaration> {
        self.all
            .iter()
            .find(|declaration| declaration.name == provider)
    }

    /// Every variable the declarations read: the keys' in order, then the base URLs'. The
    /// built-in ones alone are spec/providers.md §1's eleven.
    pub fn variables(&self) -> Vec<String> {
        let keys = self
            .all
            .iter()
            .filter_map(|d| d.credential.as_ref().map(|c| c.variable.clone()));
        let bases = self.all.iter().filter_map(|d| d.base_url.variable.clone());
        keys.chain(bases).collect()
    }
}

/// Whether FX or the programs it starts read `variable` for themselves (module doc): a
/// `GRIDA_FX_` variable, a proxy or certificate variable the transport reads, `PATH` or `HOME`,
/// in any letter case.
pub fn reserved(variable: &str) -> bool {
    const NAMES: [&str; 8] = [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "PATH",
        "HOME",
    ];
    let upper = variable.to_ascii_uppercase();
    upper.starts_with("GRIDA_FX_") || NAMES.contains(&upper.as_str())
}

/// Types fx.yaml's `providers:` (`value`, already validated against fx-project-v1), refusing what
/// the module doc lists. `label` names the file.
pub(crate) fn parse(value: Option<&Value>, label: &str) -> Result<Vec<Declaration>> {
    let Some(Value::Object(entries)) = value else {
        return Ok(Vec::new());
    };
    let refuse = |where_: &str, sentence: &str| {
        Error::document(format!("{label}: providers.{where_}: {sentence}"))
    };
    // Every variable read so far (upper case), and the provider that reads it.
    let mut read: Vec<(String, String)> = built_in()
        .iter()
        .flat_map(|d| {
            d.variables()
                .map(|v| (v.to_ascii_uppercase(), d.name.clone()))
        })
        .collect();
    let mut declared = Vec::new();
    for (name, entry) in entries {
        let ships = built_in().iter().any(|d| &d.name == name) || RUNTIMES.contains(&name.as_str());
        if ships {
            return Err(refuse(name, &format!("{name} is a provider FX ships")));
        }
        let text = |value: Option<&Value>| value.and_then(Value::as_str).map(String::from);
        let family = text(entry.get("family"));
        let adapter = text(entry.get("adapter"));
        if family.is_some() && adapter.is_some() {
            return Err(refuse(
                name,
                "a provider has a family or an adapter, not both",
            ));
        }
        let mut claim = |where_: &str, variable: &str| -> Result<()> {
            if reserved(variable) {
                return Err(refuse(
                    where_,
                    &format!("{variable} is reserved for FX and the programs it starts"),
                ));
            }
            let upper = variable.to_ascii_uppercase();
            if let Some((_, owner)) = read.iter().find(|(seen, _)| *seen == upper) {
                return Err(refuse(
                    where_,
                    &format!("{variable} is already read for {owner}"),
                ));
            }
            read.push((upper, name.clone()));
            Ok(())
        };
        let credential = match entry.get("credential") {
            None | Some(Value::Null) => None,
            Some(credential) => {
                let variable = text(credential.get("variable")).unwrap_or_default();
                claim(&format!("{name}.credential.variable"), &variable)?;
                Some(Credential {
                    variable,
                    header: text(credential.get("header")).unwrap_or("authorization".into()),
                    scheme: text(credential.get("scheme")).unwrap_or("Bearer".into()),
                })
            }
        };
        let base = entry.get("base_url");
        let variable = text(base.and_then(|base| base.get("variable")));
        if let Some(variable) = &variable {
            claim(&format!("{name}.base_url.variable"), variable)?;
        }
        let default = text(base.and_then(|base| base.get("default"))).unwrap_or_default();
        let default = normalize_base_url(&default, name)
            .map_err(|sentence| refuse(&format!("{name}.base_url.default"), &sentence))?;
        let download_hosts = match entry.get("download_hosts") {
            Some(Value::Array(hosts)) => hosts.iter().filter_map(|h| text(Some(h))).collect(),
            _ => Vec::new(),
        };
        declared.push(Declaration {
            name: name.clone(),
            label: name.clone(),
            credential,
            base_url: BaseUrl { variable, default },
            download_hosts,
            family,
            // The schema holds its shape: `<path>#<attribute>`, the attribute after the last `#`.
            adapter: adapter.and_then(|written| {
                let (path, attribute) = written.rsplit_once('#')?;
                Some(DeclaredAdapter {
                    path: path.into(),
                    attribute: attribute.into(),
                    loaded: None,
                })
            }),
        });
    }
    Ok(declared)
}

/// Normalizes a provider base URL (spec/providers.md §1): trimmed, trailing `/` removed; an
/// `http`/`https` URL with a host, no userinfo, no query, no fragment and a valid port; `http`
/// only for a loopback host (`localhost`, or a loopback IP). `label` names the provider in the
/// refusal (`OpenAI base_url must …`).
pub fn normalize_base_url(value: &str, label: &str) -> std::result::Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    let shape = || {
        format!("{label} base_url must be an HTTP(S) URL without credentials, query, or fragment")
    };
    let url = match url::Url::parse(trimmed) {
        Ok(url) => url,
        Err(url::ParseError::InvalidPort) => {
            return Err(format!("{label} base_url must use a valid network port"));
        }
        Err(_) => return Err(shape()),
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || trimmed.contains('?')
        || trimmed.contains('#')
    {
        return Err(shape());
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(format!(
            "{label} base_url must use HTTPS unless it targets a loopback host"
        ));
    }
    Ok(trimmed.to_string())
}

/// Whether a URL's host is this machine: `localhost`, or a loopback IP.
pub fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// A base URL's host as `doctor` shows it: the host, and its port when the URL names one.
pub fn shown_host(base: &str) -> String {
    match url::Url::parse(base) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            (None, _) => String::new(),
        },
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(providers: Value) -> std::result::Result<Vec<Declaration>, String> {
        parse(Some(&providers), "fx.yaml").map_err(|error| error.message)
    }

    #[test]
    fn the_built_in_declarations_read_the_eleven_variables() {
        let names: Vec<&str> = built_in().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            ["gg", "openai", "openrouter", "fal", "tripo", "elevenlabs"]
        );
        assert_eq!(
            Declarations::built_in().variables(),
            [
                "GG_API_KEY",
                "OPENAI_API_KEY",
                "OPENROUTER_API_KEY",
                "FAL_KEY",
                "TRIPO_API_KEY",
                "ELEVENLABS_API_KEY",
                "GG_BASE_URL",
                "OPENAI_BASE_URL",
                "OPENROUTER_BASE_URL",
                "FAL_BASE_URL",
                "ELEVENLABS_BASE_URL",
            ]
        );
        let prefixes: Vec<String> = built_in()
            .iter()
            .map(|d| d.credential.as_ref().unwrap().prefix())
            .collect();
        assert_eq!(
            prefixes,
            ["Bearer ", "Bearer ", "Bearer ", "Key ", "Bearer ", ""]
        );
        for declaration in built_in() {
            assert_eq!(
                normalize_base_url(&declaration.base_url.default, &declaration.label).as_deref(),
                Ok(declaration.base_url.default.as_str())
            );
        }
        assert!(Declarations::built_in().declared().is_empty());
    }

    #[test]
    fn a_declaration_is_typed_with_its_defaults() {
        let declared = parsed(json!({
            "acme": {
                "credential": {"variable": "ACME_API_KEY"},
                "base_url": {"variable": "ACME_BASE_URL", "default": " https://api.acme.test/v1/ "},
                "download_hosts": ["cdn.acme.test", "*.acme.test"],
                "family": "openai-compatible",
            },
            "local": {"base_url": {"default": "http://127.0.0.1:8080/v1"}},
            "bare": {
                "credential": {"variable": "BARE_TOKEN", "header": "x-api-key", "scheme": ""},
                "base_url": {"default": "https://bare.example.test"},
            },
        }))
        .unwrap();
        assert_eq!(
            declared[0],
            Declaration {
                name: "acme".into(),
                label: "acme".into(),
                credential: Some(Credential {
                    variable: "ACME_API_KEY".into(),
                    header: "authorization".into(),
                    scheme: "Bearer".into(),
                }),
                base_url: BaseUrl {
                    variable: Some("ACME_BASE_URL".into()),
                    default: "https://api.acme.test/v1".into(),
                },
                download_hosts: vec!["cdn.acme.test".into(), "*.acme.test".into()],
                family: Some("openai-compatible".into()),
                adapter: None,
            }
        );
        assert_eq!(declared[1].credential, None);
        assert_eq!(declared[1].base_url.variable, None);
        assert_eq!(declared[2].credential.as_ref().unwrap().prefix(), "");
        let all = Declarations::with(&declared);
        assert_eq!(all.declared(), &declared[..]);
        assert_eq!(all.get("local"), Some(&declared[1]));
        assert_eq!(all.get("fal").map(|d| d.label.as_str()), Some("fal"));
        let variables = all.variables();
        assert_eq!(variables[..6], Declarations::built_in().variables()[..6]);
        assert_eq!(variables[6..8], ["ACME_API_KEY", "BARE_TOKEN"]);
        assert_eq!(variables.last().map(String::as_str), Some("ACME_BASE_URL"));
        assert_eq!(variables.len(), 11 + 3);
    }

    #[test]
    fn declarations_are_refused_by_name_and_variable() {
        let base = json!({"default": "https://api.acme.test"});
        for name in [
            "gg",
            "openai",
            "openrouter",
            "fal",
            "tripo",
            "elevenlabs",
            "codex",
            "grok",
        ] {
            assert_eq!(
                parsed(json!({name: {"base_url": base}})),
                Err(format!(
                    "fx.yaml: providers.{name}: {name} is a provider FX ships"
                ))
            );
        }
        let with_key = |variable: &str| json!({"acme": {"credential": {"variable": variable}, "base_url": base}});
        for (variable, owner) in [("FAL_KEY", "fal"), ("openai_base_url", "openai")] {
            assert_eq!(
                parsed(with_key(variable)),
                Err(format!(
                    "fx.yaml: providers.acme.credential.variable: {variable} is already read for \
                     {owner}"
                ))
            );
        }
        for variable in [
            "GRIDA_FX_NETWORK",
            "grida_fx_x",
            "HTTPS_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "no_proxy",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "PATH",
            "HOME",
        ] {
            assert_eq!(
                parsed(with_key(variable)),
                Err(format!(
                    "fx.yaml: providers.acme.credential.variable: {variable} is reserved for FX \
                     and the programs it starts"
                ))
            );
        }
        // Codex's and Grok Build's withheld variables are not read by FX.
        assert!(parsed(with_key("XAI_API_KEY")).is_ok());
        assert!(parsed(with_key("CODEX_API_KEY")).is_ok());
        // One declaration's key read again: by its own base URL, or by a later declaration.
        let twice = json!({"acme": {
            "credential": {"variable": "ACME_KEY"},
            "base_url": {"variable": "ACME_KEY", "default": "https://api.acme.test"},
        }});
        assert_eq!(
            parsed(twice),
            Err(
                "fx.yaml: providers.acme.base_url.variable: ACME_KEY is already read for acme"
                    .into()
            )
        );
        let later = json!({
            "acme": {"credential": {"variable": "ACME_KEY"}, "base_url": base},
            "other": {"credential": {"variable": "Acme_Key"}, "base_url": base},
        });
        assert_eq!(
            parsed(later),
            Err(
                "fx.yaml: providers.other.credential.variable: Acme_Key is already read for acme"
                    .into()
            )
        );
        for (default, sentence) in [
            (
                "http://api.acme.test",
                "acme base_url must use HTTPS unless it targets a loopback host",
            ),
            (
                "https://user:pw@api.acme.test",
                "acme base_url must be an HTTP(S) URL without credentials, query, or fragment",
            ),
            (
                "https://api.acme.test:99999",
                "acme base_url must use a valid network port",
            ),
        ] {
            assert_eq!(
                parsed(json!({"acme": {"base_url": {"default": default}}})),
                Err(format!(
                    "fx.yaml: providers.acme.base_url.default: {sentence}"
                ))
            );
        }
        assert!(
            parsed(json!({"acme": {"base_url": {"default": "http://localhost:8080"}}})).is_ok()
        );
    }

    #[test]
    fn a_declaration_names_a_family_or_an_adapter() {
        let base = json!({"default": "https://media.acme.test/v2"});
        let declared = parsed(json!({
            "acme-media": {"base_url": base, "adapter": "adapters/media.v2/acme.py#acme_media"},
        }))
        .unwrap();
        let adapter = declared[0].adapter.as_ref().unwrap();
        assert_eq!(adapter.path, "adapters/media.v2/acme.py");
        assert_eq!(adapter.attribute, "acme_media");
        assert_eq!(adapter.name(), "adapters/media.v2/acme.py#acme_media");
        assert_eq!(adapter.loaded, None);
        assert_eq!(declared[0].family, None);
        assert_eq!(
            parsed(json!({"acme": {
                "base_url": base,
                "family": "openai-compatible",
                "adapter": "adapters/acme.py#acme",
            }})),
            Err("fx.yaml: providers.acme: a provider has a family or an adapter, not both".into())
        );
    }

    #[test]
    fn a_loaded_adapters_behaviour_is_its_version_or_its_source() {
        let mut loaded = LoadedAdapter {
            version: Some(3),
            capabilities: [("image.generate".to_string(), CallKind::Request)]
                .into_iter()
                .collect(),
            source: SourceDigests::default(),
            drift: None,
        };
        assert_eq!(loaded.behavior(), json!(3));
        assert!(loaded.serves("image.generate") && !loaded.serves("video.generate"));
        loaded.version = None;
        assert_eq!(
            loaded.behavior(),
            json!(format!("source:{}", SourceDigests::default().digest()))
        );
    }

    /// The backticked words of a table cell.
    fn ticked(cell: &str) -> Vec<String> {
        cell.split('`')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_built_in_declarations_are_the_specs() {
        let spec = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/providers.md"),
        )
        .unwrap();
        let section = spec
            .split("## 1. ")
            .nth(1)
            .and_then(|rest| rest.split("## 2. ").next())
            .unwrap();
        let rows: Vec<Vec<&str>> = section
            .lines()
            .filter(|line| line.starts_with("| `"))
            .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
            .collect();
        assert_eq!(rows.len(), built_in().len());
        for (row, declaration) in rows.iter().zip(built_in()) {
            let credential = declaration.credential.as_ref().unwrap();
            assert_eq!(ticked(row[0]), [declaration.name.as_str()]);
            assert_eq!(ticked(row[1]), [credential.variable.as_str()]);
            assert_eq!(
                ticked(row[2]),
                [format!(
                    "{}: {}<key>",
                    credential.header,
                    credential.prefix()
                )]
            );
            let base: Vec<String> = declaration.base_url.variable.iter().cloned().collect();
            assert_eq!(ticked(row[3]), base, "{}", declaration.name);
            assert_eq!(ticked(row[4])[0], declaration.base_url.default);
            assert_eq!(ticked(row[5]), declaration.download_hosts);
        }
    }

    #[test]
    fn doctor_shows_a_base_urls_host_and_its_port() {
        assert_eq!(shown_host("https://api.acme.test/v1"), "api.acme.test");
        assert_eq!(shown_host("http://127.0.0.1:8080/v1"), "127.0.0.1:8080");
        assert_eq!(shown_host("https://api.acme.test:443/v1"), "api.acme.test");
    }
}
