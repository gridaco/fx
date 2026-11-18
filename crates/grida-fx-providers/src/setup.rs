//! What every provider's adapters are built from: one [`Setup`] per invocation (the transport, the
//! keys, the endpoints and the clock), and the [`Client`] each adapter keeps for its provider.
//!
//! Each provider module has `pub fn register(adapters: &mut Adapters, setup: &Setup)`, which
//! registers its adapters for every capability it serves, **whether or not its key is present**: a
//! call without the key is refused before sending (`Sent::Refused`, "`<VARIABLE>` is not set",
//! spec/adapters.md §4), and `grida-fx doctor` reports the route as not servable.

use crate::clock::Clock;
use crate::keys::{Environment, KeyName, KeySource, Keys, Secret, StoredCredentials};
use crate::redact::Redactor;
use crate::transport::{
    Credential, HttpRequest, HttpResponse, Transport, TransportError, TransportErrorKind,
};
use grida_fx_core::providers::{Declaration, Declarations};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// fal's queue host: not configurable, so not a declared base URL (spec/providers.md §1).
pub const FAL_QUEUE: &str = "https://queue.fal.run";

/// Each declared provider's API base, normalized ([`crate::wire::normalize_base_url`]): no
/// trailing `/`. fal's is its synchronous host (`FAL_BASE_URL`); its queue host is apart.
#[derive(Clone)]
pub struct Endpoints {
    bases: BTreeMap<String, String>,
    configured: BTreeMap<String, ConfiguredBase>,
    /// fal's queue host ([`FAL_QUEUE`]).
    pub fal_queue: String,
}

impl PartialEq for Endpoints {
    fn eq(&self, other: &Self) -> bool {
        self.bases == other.bases && self.fal_queue == other.fal_queue
    }
}

impl Eq for Endpoints {}

impl std::fmt::Debug for Endpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoints")
            .field("providers", &self.bases.keys())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct ConfiguredBase {
    variable: String,
    raw: Secret,
}

impl Default for Endpoints {
    /// The built-in declarations' defaults.
    fn default() -> Endpoints {
        Endpoints::of(&Declarations::built_in())
    }
}

impl Endpoints {
    /// The defaults of `declarations`.
    pub fn of(declarations: &Declarations) -> Endpoints {
        Endpoints {
            bases: declarations
                .iter()
                .map(|d| (d.name.clone(), d.base_url.default.clone()))
                .collect(),
            configured: BTreeMap::new(),
            fal_queue: FAL_QUEUE.into(),
        }
    }

    /// The defaults of the environment's declarations, with each base-URL variable the
    /// environment gives normalized ([`crate::wire::normalize_base_url`]) and put in its place, in
    /// the declarations' order. A value that does not normalize is refused with
    /// `<VARIABLE>: <sentence>`; a value containing any key's value is refused with
    /// `<VARIABLE> holds a credential` (never showing either value). The first refusal is the
    /// error.
    pub fn from_environment(environment: &Environment, keys: &Keys) -> Result<Endpoints, String> {
        let (endpoints, mut refused) = Endpoints::each_from_environment(environment, keys);
        if refused.is_empty() {
            Ok(endpoints)
        } else {
            Err(refused.swap_remove(0))
        }
    }

    /// [`Endpoints::from_environment`] one declaration at a time: a refused variable leaves its
    /// provider at its default, and its refusal is listed, in the declarations' order.
    pub fn each_from_environment(
        environment: &Environment,
        keys: &Keys,
    ) -> (Endpoints, Vec<String>) {
        let declarations = environment.declarations();
        let mut endpoints = Endpoints::of(declarations);
        let mut refused = Vec::new();
        let secrets = keys.secrets();
        for declaration in declarations.iter() {
            let Some(variable) = &declaration.base_url.variable else {
                continue;
            };
            let Some(value) = environment.get(variable) else {
                continue;
            };
            if secrets
                .iter()
                .any(|secret| !secret.expose().is_empty() && value.contains(secret.expose()))
            {
                refused.push(format!("{variable} holds a credential"));
                continue;
            }
            match crate::wire::normalize_base_url(value, &declaration.label) {
                Ok(base) => {
                    if secrets
                        .iter()
                        .any(|secret| !secret.expose().is_empty() && base.contains(secret.expose()))
                    {
                        refused.push(format!("{variable} holds a credential"));
                        continue;
                    }
                    endpoints.bases.insert(declaration.name.clone(), base);
                    endpoints.configured.insert(
                        declaration.name.clone(),
                        ConfiguredBase {
                            variable: variable.clone(),
                            raw: Secret::new(value),
                        },
                    );
                }
                Err(sentence) => refused.push(format!("{variable}: {sentence}")),
            }
        }
        (endpoints, refused)
    }

    /// The base of `provider`, when it is declared.
    pub fn base(&self, provider: &str) -> Option<&str> {
        self.bases.get(provider).map(String::as_str)
    }

    /// These endpoints with `provider`'s base set to `base` (tests point a provider at a stand-in
    /// server this way).
    pub fn with_base(mut self, provider: &str, base: &str) -> Endpoints {
        self.bases.insert(provider.into(), base.into());
        self.configured.remove(provider);
        self
    }
}

/// One invocation's provider setup.
#[derive(Clone)]
pub struct Setup {
    pub transport: Arc<dyn Transport>,
    /// The keys, and the declarations they were read for.
    pub keys: Keys,
    pub endpoints: Endpoints,
    pub clock: Arc<dyn Clock>,
    /// The user's Codex program, or why there is none (the refusal of every Codex call). Only
    /// [`crate::live::live_setup`] looks for one.
    pub codex: Result<crate::installed::Program, String>,
    /// The user's Grok Build program, or why there is none (the refusal of every Grok call). Only
    /// [`crate::live::live_setup`] looks for one.
    pub grok: Result<std::path::PathBuf, String>,
    pub(crate) stored: Option<Arc<StoredCredentials>>,
}

impl Setup {
    /// A setup over `transport` with `keys`, the default endpoints of their declarations and
    /// `clock`, and no program the user installed: only [`crate::live::live_setup`] looks for
    /// those.
    pub fn new(transport: Arc<dyn Transport>, keys: Keys, clock: Arc<dyn Clock>) -> Setup {
        Setup {
            transport,
            endpoints: Endpoints::of(keys.declarations()),
            keys,
            clock,
            codex: Err(crate::codex::NOT_INSTALLED.into()),
            grok: Err(crate::grok::NOT_INSTALLED.into()),
            stored: None,
        }
    }

    /// Enable deferred fallback for eligible built-in providers in a live invocation.
    pub fn with_stored_credentials(
        mut self,
        home: Result<PathBuf, String>,
        providers: &[KeyName],
    ) -> Setup {
        let eligible: Vec<_> = providers
            .iter()
            .copied()
            .filter(|name| {
                KeyName::STORED.contains(name)
                    && self.keys.get(*name).is_none()
                    && self.endpoints.base(name.provider())
                        == Some(name.declaration().base_url.default.as_str())
            })
            .collect();
        if !eligible.is_empty() {
            self.stored = Some(Arc::new(StoredCredentials::new(home, &eligible)));
        }
        self
    }

    /// A redactor over every key of the invocation.
    pub fn redactor(&self) -> Redactor {
        Redactor::new(self.keys.secrets()).with_stored(self.stored.clone())
    }

    /// The declarations of the invocation: FX's, then the planning project's.
    pub fn declarations(&self) -> &Declarations {
        self.keys.declarations()
    }

    /// Every declared variable, which Codex and Grok Build never inherit: each declaration's key
    /// and base URL (spec/providers.md §1).
    pub fn withheld(&self) -> Arc<[String]> {
        Arc::from(self.declarations().variables())
    }

    /// The client of one of FX's own providers at its base URL: its key (if any), its
    /// declaration and the shared transport.
    pub fn client(&self, key: KeyName) -> Client {
        self.declared_client(key.declaration())
    }

    /// The client of one of FX's own providers at `base` (fal's queue host).
    pub fn client_at(&self, key: KeyName, base: &str) -> Client {
        let declaration = key.declaration();
        let trusted_base = if key == KeyName::Fal && base.trim_end_matches('/') == FAL_QUEUE {
            FAL_QUEUE
        } else {
            &declaration.base_url.default
        };
        self.client_for(declaration, base, Some(trusted_base), None)
    }

    pub(crate) fn fal_queue_client(&self) -> Client {
        self.client_for(
            KeyName::Fal.declaration(),
            &self.endpoints.fal_queue,
            Some(FAL_QUEUE),
            None,
        )
    }

    /// The client of a provider the planning project declares, at its base URL: its key (if it
    /// is declared with one and the key is set), its declaration and the shared transport.
    pub fn declared_client(&self, declaration: &Declaration) -> Client {
        let base = self.endpoints.base(&declaration.name);
        let trusted_base = KeyName::of_provider(&declaration.name)
            .map(|key| key.declaration().base_url.default.as_str());
        self.client_for(
            declaration,
            base.unwrap_or(&declaration.base_url.default),
            trusted_base,
            self.endpoints.configured.get(&declaration.name).cloned(),
        )
    }

    fn client_for(
        &self,
        declaration: &Declaration,
        base: &str,
        trusted_base: Option<&str>,
        configured: Option<ConfiguredBase>,
    ) -> Client {
        let base = base.trim_end_matches('/');
        let store_allowed = trusted_base == Some(base)
            && KeyName::of_provider(&declaration.name)
                .is_some_and(|key| KeyName::STORED.contains(&key));
        let stored = self
            .stored
            .as_ref()
            .filter(|stored| store_allowed && stored.eligible(&declaration.name))
            .cloned();
        let key = self
            .keys
            .of(&declaration.name)
            .filter(|_| {
                store_allowed
                    || self.keys.source_of(&declaration.name) != Some(KeySource::GridaStore)
            })
            .cloned();
        Client {
            transport: Arc::clone(&self.transport),
            provider: declaration.name.clone(),
            declared: declaration.credential.clone(),
            key,
            base: base.to_string(),
            download_hosts: declaration.download_hosts.clone(),
            redactor: self.redactor(),
            configured,
            stored,
        }
    }
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("keys", &self.keys)
            .field("endpoints", &self.endpoints)
            .field("codex", &self.codex)
            .field("grok", &self.grok)
            .finish_non_exhaustive()
    }
}

/// What one provider's adapters share.
#[derive(Clone)]
pub struct Client {
    pub transport: Arc<dyn Transport>,
    /// The provider's name.
    pub provider: String,
    /// Its declared credential: where its key comes from and how it is sent.
    pub declared: Option<grida_fx_core::providers::Credential>,
    pub key: Option<Secret>,
    /// No trailing `/`.
    pub base: String,
    /// The hosts its results may be downloaded from ([`crate::wire::download_url`]).
    pub download_hosts: Vec<String>,
    pub redactor: Redactor,
    configured: Option<ConfiguredBase>,
    stored: Option<Arc<StoredCredentials>>,
}

impl Client {
    pub(crate) fn needs_preparation(&self) -> bool {
        self.stored.is_some() || self.configured.is_some()
    }

    pub(crate) async fn prepare(&self) -> Result<(), String> {
        if let Some(stored) = &self.stored {
            stored.prepare(&self.provider).await?;
        }
        self.checked_base().map(|_| ())
    }

    /// The base URL, or a refusal if a key loaded since setup matches its configured value.
    pub fn checked_base(&self) -> Result<&str, String> {
        if let Some(configured) = &self.configured
            && (self
                .redactor
                .contains_stored_secret(configured.raw.expose())
                || self.redactor.contains_stored_secret(&self.base))
        {
            return Err(format!("{} holds a credential", configured.variable));
        }
        Ok(&self.base)
    }

    /// The credential for the provider's API, sent as its declaration says, or the refusal
    /// sentence `<VARIABLE> is not set`.
    pub fn credential(&self) -> Result<Credential, String> {
        let Some(declared) = &self.declared else {
            return Err(format!("{} is declared without a key", self.provider));
        };
        let key = self
            .key
            .as_ref()
            .filter(|secret| !secret.expose().trim().is_empty())
            .or_else(|| self.stored.as_ref()?.key(&self.provider));
        match key {
            Some(secret) => Ok(Credential {
                header: declared.header.clone(),
                prefix: declared.prefix(),
                secret: secret.clone(),
            }),
            _ => Err(crate::keys::not_set(&declared.variable)),
        }
    }

    /// [`Client::credential`], or none for a provider declared without one, which is sent no
    /// key (spec/providers.md §1).
    pub fn optional_credential(&self) -> Result<Option<Credential>, String> {
        match &self.declared {
            None => Ok(None),
            Some(_) => self.credential().map(Some),
        }
    }

    /// `<base>/<path>`.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// A result's URL when it is on the provider's download hosts ([`crate::wire::download_url`]).
    pub fn download_url(&self, url: &str) -> Result<url::Url, crate::wire::OffHost> {
        crate::wire::download_url(url, &self.download_hosts)
    }

    /// One exchange through the shared transport.
    pub async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.checked_base()
            .map_err(|reason| TransportError::not_sent(TransportErrorKind::Refused, reason))?;
        self.transport.send(request).await
    }

    /// A reason, redacted and bounded.
    pub fn reason(&self, text: &str) -> String {
        self.redactor.reason(text)
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("provider", &self.provider)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use crate::transport::replay::NoNetwork;

    fn setup(keys: Keys) -> Setup {
        Setup::new(Arc::new(NoNetwork), keys, Arc::new(SystemClock::new()))
    }

    #[test]
    fn a_client_without_its_key_refuses_with_the_variable() {
        let client = setup(Keys::none()).client_at(KeyName::OpenAi, "https://api.openai.com/v1/");
        assert_eq!(
            client.credential().unwrap_err(),
            "OPENAI_API_KEY is not set"
        );
        assert_eq!(
            client.url("/images/generations"),
            "https://api.openai.com/v1/images/generations"
        );
        let blank = setup(Keys::from_pairs(&[(KeyName::Tripo, "  ")])).client(KeyName::Tripo);
        assert_eq!(blank.credential().unwrap_err(), "TRIPO_API_KEY is not set");
        assert_eq!(blank.base, "https://openapi.tripo3d.ai/v3");
    }

    #[test]
    fn an_eager_store_key_authenticates_only_at_the_built_in_base() {
        let mut keys = Keys::none();
        keys.insert_stored(KeyName::OpenRouter, Secret::new("test-saved-key"));
        let origin = setup(keys.clone()).client(KeyName::OpenRouter);
        assert_eq!(
            origin.credential().unwrap().header_value(),
            "Bearer test-saved-key"
        );

        let proxy = "https://proxy.example.test/v1";
        let mut saved = setup(keys);
        saved.endpoints = saved.endpoints.with_base("openrouter", proxy);
        let saved = saved.client(KeyName::OpenRouter);
        assert_eq!(saved.base, proxy);
        assert_eq!(
            saved.credential().unwrap_err(),
            KeyName::OpenRouter.missing()
        );

        let mut explicit = setup(Keys::from_pairs(&[(
            KeyName::OpenRouter,
            "test-explicit-key",
        )]));
        explicit.endpoints = explicit.endpoints.with_base("openrouter", proxy);
        assert_eq!(
            explicit
                .client(KeyName::OpenRouter)
                .credential()
                .unwrap()
                .header_value(),
            "Bearer test-explicit-key"
        );
    }

    #[test]
    fn endpoints_read_every_base_url_variable_and_only_those() {
        let declarations = Declarations::built_in();
        let variables: Vec<&str> = declarations
            .iter()
            .filter_map(|d| d.base_url.variable.as_deref())
            .collect();
        let pairs: Vec<(&str, String)> = variables
            .iter()
            .enumerate()
            .map(|(i, name)| (*name, format!("https://proxy-{i}.example.test/")))
            .collect();
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let endpoints =
            Endpoints::from_environment(&Environment::from_pairs(&borrowed), &Keys::none())
                .expect("valid base URLs");
        let expected = Endpoints::default()
            .with_base("gg", "https://proxy-0.example.test")
            .with_base("openai", "https://proxy-1.example.test")
            .with_base("openrouter", "https://proxy-2.example.test")
            .with_base("fal", "https://proxy-3.example.test")
            .with_base("elevenlabs", "https://proxy-4.example.test");
        assert_eq!(endpoints, expected);
        assert_eq!(
            endpoints.base("tripo"),
            Some("https://openapi.tripo3d.ai/v3")
        );
        assert_eq!(endpoints.fal_queue, "https://queue.fal.run");
    }

    #[test]
    fn a_client_with_its_key_makes_the_credential_and_redacts_it() {
        let client = setup(Keys::from_pairs(&[(KeyName::Fal, "fal-secret")])).client(KeyName::Fal);
        assert_eq!(client.base, "https://fal.run");
        let credential = client.credential().unwrap();
        assert_eq!(credential.header, "authorization");
        assert_eq!(credential.header_value(), "Key fal-secret");
        assert_eq!(client.reason("bad fal-secret"), "bad [redacted]");
        assert!(!format!("{client:?}").contains("fal-secret"));
        let eleven = setup(Keys::from_pairs(&[(KeyName::ElevenLabs, "el-secret")]))
            .client(KeyName::ElevenLabs);
        let credential = eleven.credential().unwrap();
        assert_eq!(
            (credential.header.as_str(), credential.header_value()),
            ("xi-api-key", "el-secret".to_string())
        );
    }
}
