//! What every provider's adapters are built from: one [`Setup`] per invocation (the transport, the
//! keys, the endpoints and the clock), and the [`Client`] each adapter keeps for its provider.
//!
//! Each provider module has `pub fn register(adapters: &mut Adapters, setup: &Setup)`, which
//! registers its adapters for every capability it serves, **whether or not its key is present**: a
//! call without the key is refused before sending (`Sent::Refused`, "`<VARIABLE>` is not set",
//! spec/adapters.md §4), and `grida-fx doctor` reports the route as not servable.

use crate::clock::Clock;
use crate::keys::{Environment, KeyName, Keys, Secret};
use crate::redact::Redactor;
use crate::transport::{Credential, HttpRequest, HttpResponse, Transport, TransportError};
use grida_fx_core::providers::{Declaration, Declarations};
use std::collections::BTreeMap;
use std::sync::Arc;

/// fal's queue host: not configurable, so not a declared base URL (spec/providers.md §1).
pub const FAL_QUEUE: &str = "https://queue.fal.run";

/// Each declared provider's API base, normalized ([`crate::wire::normalize_base_url`]): no
/// trailing `/`. fal's is its synchronous host (`FAL_BASE_URL`); its queue host is apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    bases: BTreeMap<String, String>,
    /// fal's queue host ([`FAL_QUEUE`]).
    pub fal_queue: String,
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
                    endpoints.bases.insert(declaration.name.clone(), base);
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
        }
    }

    /// A redactor over every key of the invocation.
    pub fn redactor(&self) -> Redactor {
        Redactor::new(self.keys.secrets())
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
        let declaration = key.declaration();
        let base = self.endpoints.base(key.provider());
        let base = base.unwrap_or(&declaration.base_url.default).to_string();
        self.client_for(declaration, &base)
    }

    /// The client of one of FX's own providers at `base` (fal's queue host).
    pub fn client_at(&self, key: KeyName, base: &str) -> Client {
        self.client_for(key.declaration(), base)
    }

    /// The client of a provider the planning project declares, at its base URL: its key (if it
    /// is declared with one and the key is set), its declaration and the shared transport.
    pub fn declared_client(&self, declaration: &Declaration) -> Client {
        let base = self.endpoints.base(&declaration.name);
        let base = base.unwrap_or(&declaration.base_url.default).to_string();
        self.client_for(declaration, &base)
    }

    fn client_for(&self, declaration: &Declaration, base: &str) -> Client {
        Client {
            transport: Arc::clone(&self.transport),
            provider: declaration.name.clone(),
            declared: declaration.credential.clone(),
            key: self.keys.of(&declaration.name).cloned(),
            base: base.trim_end_matches('/').to_string(),
            download_hosts: declaration.download_hosts.clone(),
            redactor: self.redactor(),
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
}

impl Client {
    /// The credential for the provider's API, sent as its declaration says, or the refusal
    /// sentence `<VARIABLE> is not set`.
    pub fn credential(&self) -> Result<Credential, String> {
        let Some(declared) = &self.declared else {
            return Err(format!("{} is declared without a key", self.provider));
        };
        match &self.key {
            Some(secret) if !secret.expose().trim().is_empty() => Ok(Credential {
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
            .field("base", &self.base)
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
