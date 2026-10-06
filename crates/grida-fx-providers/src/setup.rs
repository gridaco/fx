//! What every provider's adapters are built from: one [`Setup`] per invocation (the transport, the
//! keys, the endpoints and the clock), and the [`Client`] each adapter keeps for its provider.
//!
//! Each provider module has `pub fn register(adapters: &mut Adapters, setup: &Setup)`, which
//! registers its adapters for every capability it serves, **whether or not its key is present**: a
//! call without the key is refused before sending (`Sent::Refused`, "`<VARIABLE>` is not set",
//! spec/providers.md §5), and `grida-fx doctor` reports the route as not servable.

use crate::clock::Clock;
use crate::keys::{Environment, KeyName, Keys, Secret};
use crate::redact::Redactor;
use crate::transport::{Credential, HttpRequest, HttpResponse, Transport, TransportError};
use std::sync::Arc;

/// The base-URL variables a project may set (spec/providers.md §3). Tripo has none.
pub const BASE_URL_VARIABLES: [&str; 4] = [
    "OPENAI_BASE_URL",
    "OPENROUTER_BASE_URL",
    "FAL_BASE_URL",
    "ELEVENLABS_BASE_URL",
];

/// Each provider's API base, normalized ([`crate::wire::normalize_base_url`]): no trailing `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub openai: String,
    pub openrouter: String,
    /// fal's synchronous host (`FAL_BASE_URL`).
    pub fal_run: String,
    /// fal's queue host (not configurable).
    pub fal_queue: String,
    pub tripo: String,
    pub elevenlabs: String,
}

impl Default for Endpoints {
    fn default() -> Endpoints {
        Endpoints {
            openai: "https://api.openai.com/v1".into(),
            openrouter: "https://openrouter.ai/api/v1".into(),
            fal_run: "https://fal.run".into(),
            fal_queue: "https://queue.fal.run".into(),
            tripo: "https://openapi.tripo3d.ai/v3".into(),
            elevenlabs: "https://api.elevenlabs.io/v1".into(),
        }
    }
}

impl Endpoints {
    /// The defaults, with each of [`BASE_URL_VARIABLES`] the environment gives normalized
    /// ([`crate::wire::normalize_base_url`]) and put in its place. A value that does not normalize
    /// is refused with `<VARIABLE>: <sentence>`; a value containing any key's value is refused
    /// with `<VARIABLE> holds a credential` (never showing either value).
    pub fn from_environment(environment: &Environment, keys: &Keys) -> Result<Endpoints, String> {
        let mut endpoints = Endpoints::default();
        let secrets = keys.secrets();
        let slots: [(&str, &str, &mut String); 4] = [
            ("OPENAI_BASE_URL", "OpenAI", &mut endpoints.openai),
            (
                "OPENROUTER_BASE_URL",
                "OpenRouter",
                &mut endpoints.openrouter,
            ),
            // The run host only; the queue host is not configurable.
            ("FAL_BASE_URL", "fal", &mut endpoints.fal_run),
            (
                "ELEVENLABS_BASE_URL",
                "ElevenLabs",
                &mut endpoints.elevenlabs,
            ),
        ];
        for (variable, label, slot) in slots {
            let Some(value) = environment.get(variable) else {
                continue;
            };
            if secrets
                .iter()
                .any(|secret| !secret.expose().is_empty() && value.contains(secret.expose()))
            {
                return Err(format!("{variable} holds a credential"));
            }
            *slot = crate::wire::normalize_base_url(value, label)
                .map_err(|sentence| format!("{variable}: {sentence}"))?;
        }
        Ok(endpoints)
    }
}

/// One invocation's provider setup.
#[derive(Clone)]
pub struct Setup {
    pub transport: Arc<dyn Transport>,
    pub keys: Keys,
    pub endpoints: Endpoints,
    pub clock: Arc<dyn Clock>,
}

impl Setup {
    /// A redactor over every key of the invocation.
    pub fn redactor(&self) -> Redactor {
        Redactor::new(self.keys.secrets())
    }

    /// The client of one provider: its key (if any), its base URL and the shared transport.
    pub fn client(&self, key: KeyName, base: &str) -> Client {
        Client {
            transport: Arc::clone(&self.transport),
            key_name: key,
            key: self.keys.get(key).cloned(),
            base: base.trim_end_matches('/').to_string(),
            redactor: self.redactor(),
        }
    }
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("keys", &self.keys)
            .field("endpoints", &self.endpoints)
            .finish_non_exhaustive()
    }
}

/// What one provider's adapters share.
#[derive(Clone)]
pub struct Client {
    pub transport: Arc<dyn Transport>,
    pub key_name: KeyName,
    pub key: Option<Secret>,
    /// No trailing `/`.
    pub base: String,
    pub redactor: Redactor,
}

impl Client {
    /// The credential for the provider's API, or the refusal sentence `<VARIABLE> is not set`.
    pub fn credential(
        &self,
        header: &'static str,
        prefix: &'static str,
    ) -> Result<Credential, String> {
        match &self.key {
            Some(secret) if !secret.expose().trim().is_empty() => Ok(Credential {
                header,
                prefix,
                secret: secret.clone(),
            }),
            _ => Err(self.key_name.missing()),
        }
    }

    /// `<base>/<path>`.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
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
            .field("key_name", &self.key_name)
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
        Setup {
            transport: Arc::new(NoNetwork),
            keys,
            endpoints: Endpoints::default(),
            clock: Arc::new(SystemClock::new()),
        }
    }

    #[test]
    fn a_client_without_its_key_refuses_with_the_variable() {
        let client = setup(Keys::none()).client(KeyName::OpenAi, "https://api.openai.com/v1/");
        assert_eq!(
            client.credential("authorization", "Bearer ").unwrap_err(),
            "OPENAI_API_KEY is not set"
        );
        assert_eq!(
            client.url("/images/generations"),
            "https://api.openai.com/v1/images/generations"
        );
        let blank = setup(Keys::from_pairs(&[(KeyName::Tripo, "  ")]))
            .client(KeyName::Tripo, "https://x.test");
        assert_eq!(
            blank.credential("authorization", "Bearer ").unwrap_err(),
            "TRIPO_API_KEY is not set"
        );
    }

    #[test]
    fn endpoints_read_every_base_url_variable_and_only_those() {
        let pairs: Vec<(&str, String)> = BASE_URL_VARIABLES
            .iter()
            .enumerate()
            .map(|(i, name)| (*name, format!("https://proxy-{i}.example.test/")))
            .collect();
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let endpoints =
            Endpoints::from_environment(&Environment::from_pairs(&borrowed), &Keys::none())
                .expect("valid base URLs");
        assert_eq!(
            endpoints,
            Endpoints {
                openai: "https://proxy-0.example.test".into(),
                openrouter: "https://proxy-1.example.test".into(),
                fal_run: "https://proxy-2.example.test".into(),
                fal_queue: Endpoints::default().fal_queue,
                tripo: Endpoints::default().tripo,
                elevenlabs: "https://proxy-3.example.test".into(),
            }
        );
    }

    #[test]
    fn a_client_with_its_key_makes_the_credential_and_redacts_it() {
        let client = setup(Keys::from_pairs(&[(KeyName::Fal, "fal-secret")]))
            .client(KeyName::Fal, "https://fal.run");
        let credential = client.credential("authorization", "Key ").unwrap();
        assert_eq!(credential.header_value(), "Key fal-secret");
        assert_eq!(client.reason("bad fal-secret"), "bad [redacted]");
        assert!(!format!("{client:?}").contains("fal-secret"));
    }
}
