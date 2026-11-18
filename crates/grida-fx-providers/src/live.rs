//! A live invocation's adapters (spec/adapters.md §1): the only place the default transport is
//! built. `grida-fx run --live` (and an `at: plan` step of a live run) calls [`live_setup`] and
//! [`adapters`]; without `--live` nothing here is constructed and the engine gets an empty
//! [`Adapters`], so every uncached paid call is refused before anything could leave.
//!
//! [`adapters`] registers every provider's adapters on whatever [`Setup`] it is given, so tests
//! build the same registry over a `ReplayTransport` or `NoNetwork`. [`servable`] is what
//! `grida-fx doctor` reports per route, over a registry built on [`offline_setup`] and the
//! programs [`installed`] finds: doctor never constructs anything that could reach the network.

use crate::BoxFuture;
use crate::adapter::{
    Adapter, Answer, Billing, CallRequest, Collected, Identity, LongJob, RequestAdapter, RouteRef,
    Sent, Submitted,
};
use crate::clock::SystemClock;
use crate::installed::{Program, Version};
use crate::keys::Keys;
use crate::redact::Redactor;
use crate::registry::Adapters;
use crate::setup::{Endpoints, Setup};
use crate::transport::http::{HttpConfig, HttpTransport};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// Whether a live run may reach the network ([`crate::transport::NETWORK_VARIABLE`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    On,
    /// Every exchange is refused before it leaves ([`crate::transport::Offline`]).
    Off,
}

impl Network {
    /// `Off` when `GRIDA_FX_NETWORK` is `off` (trimmed, in any case), else `On`.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Network {
        match env(crate::transport::NETWORK_VARIABLE) {
            Some(value) if value.trim().eq_ignore_ascii_case("off") => Network::Off,
            _ => Network::On,
        }
    }
}

/// The setup of a live run: the default transport (or [`crate::transport::Offline`] when the
/// network is off), the system clock, and the user's Codex and Grok Build as the process
/// environment finds them ([`crate::codex::find_program`], [`crate::grok::find_program`]; none when
/// the network is off). This is the only place either is looked for, so no other setup can start
/// one.
pub fn live_setup(keys: Keys, endpoints: Endpoints, network: Network) -> Result<Setup, String> {
    let (transport, codex, grok): (Arc<dyn crate::transport::Transport>, _, _) = match network {
        Network::On => {
            let env = |name: &str| std::env::var(name).ok();
            let codex = codex_program(&env).ok_or_else(|| crate::codex::NOT_INSTALLED.to_string());
            let grok = crate::grok::find_program(&env)
                .map(|program| program.path)
                .ok_or_else(|| crate::grok::NOT_INSTALLED.to_string());
            (
                Arc::new(HttpTransport::new(HttpConfig::default())?),
                codex,
                grok,
            )
        }
        Network::Off => (
            Arc::new(crate::transport::Offline),
            Err(crate::codex::NETWORK_OFF.to_string()),
            Err(crate::grok::NETWORK_OFF.to_string()),
        ),
    };
    Ok(Setup {
        endpoints,
        codex,
        grok,
        ..Setup::new(transport, keys, Arc::new(SystemClock::new()))
    })
}

/// A setup that can send nothing: the [`crate::transport::Offline`] transport (every exchange
/// refused before it leaves), the default endpoints and the system clock. `grida-fx doctor`
/// builds its registry on it to say which routes a live run could serve.
pub fn offline_setup(keys: Keys) -> Setup {
    Setup::new(
        Arc::new(crate::transport::Offline),
        keys,
        Arc::new(SystemClock::new()),
    )
}

/// Every provider's adapters over `setup`, in registration order, those of a family last, for each
/// provider `setup`'s declarations declare with it ([`crate::openai_compatible`]), each answering
/// [`RequestAdapter::redactor`] (or [`LongJob::redactor`]) with [`Setup::redactor`]: every key of
/// the invocation. The engine's retry owner redacts each reason it reports with it, so a check's
/// sentence, or the engine's own, never shows a key whichever adapter it came from
/// (spec/adapters.md §7).
pub fn adapters(setup: &Setup) -> Adapters {
    let mut registered = Adapters::new();
    crate::gg::register(&mut registered, setup);
    crate::openai::register(&mut registered, setup);
    crate::openrouter::register(&mut registered, setup);
    crate::fal::register(&mut registered, setup);
    crate::tripo::register(&mut registered, setup);
    crate::elevenlabs::register(&mut registered, setup);
    crate::codex::register(&mut registered, setup);
    crate::grok::register(&mut registered, setup);
    crate::openai_compatible::register(&mut registered, setup);
    let redactor = setup.redactor();
    let setup = Arc::new(setup.clone());
    registered.map(|adapter| match adapter {
        Adapter::Request(inner) => Adapter::Request(Arc::new(Redacting {
            inner: Arc::clone(inner),
            redactor: redactor.clone(),
            setup: Arc::clone(&setup),
        })),
        Adapter::Job(inner) => Adapter::Job(Arc::new(Redacting {
            inner: Arc::clone(inner),
            redactor: redactor.clone(),
            setup: Arc::clone(&setup),
        })),
    })
}

/// An adapter with the invocation's redactor and deferred credential preparation. Its identity
/// and contract judgement are unchanged for the engine's checks.
struct Redacting<A: ?Sized> {
    inner: Arc<A>,
    redactor: Redactor,
    setup: Arc<Setup>,
}

impl<A: ?Sized> Redacting<A> {
    fn preparation(&self, call: &CallRequest) -> Option<BoxFuture<'_, Result<(), String>>> {
        self.setup.stored.as_ref()?;
        let route = &call.route;
        // Video is the only fal adapter that sends through the queue client.
        let client = if route.provider == "fal" && route.capability == "video.generate" {
            self.setup.fal_queue_client()
        } else {
            let declaration = self.setup.declarations().get(&route.provider)?;
            self.setup.declared_client(declaration)
        };
        client
            .needs_preparation()
            .then(|| Box::pin(async move { client.prepare().await }) as BoxFuture<_>)
    }
}

impl RequestAdapter for Redacting<dyn RequestAdapter> {
    fn prepare<'a>(&'a self, call: &'a CallRequest) -> Option<BoxFuture<'a, Result<(), String>>> {
        self.preparation(call)
    }

    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            if let Some(prepare) = self.preparation(call)
                && let Err(reason) = prepare.await
            {
                return Sent::Refused { reason };
            }
            self.inner.send(call).await
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        self.inner.check(call, answer)
    }

    fn identity(&self) -> Option<Identity> {
        self.inner.identity()
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        self.inner.check_contract(call)
    }

    fn billing(&self) -> Billing {
        self.inner.billing()
    }

    fn redactor(&self) -> Redactor {
        self.redactor.clone()
    }
}

impl LongJob for Redacting<dyn LongJob> {
    fn prepare<'a>(&'a self, call: &'a CallRequest) -> Option<BoxFuture<'a, Result<(), String>>> {
        self.preparation(call)
    }

    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(async move {
            if let Some(prepare) = self.preparation(call)
                && let Err(reason) = prepare.await
            {
                return Submitted::Refused { reason };
            }
            self.inner.submit(call).await
        })
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(async move {
            if let Some(prepare) = self.preparation(call)
                && let Err(reason) = prepare.await
            {
                return Collected::Unreachable { reason };
            }
            self.inner.collect(call, handle).await
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        self.inner.check(call, answer)
    }

    fn identity(&self) -> Option<Identity> {
        self.inner.identity()
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        self.inner.check_contract(call)
    }

    fn billing(&self) -> Billing {
        self.inner.billing()
    }

    fn redactor(&self) -> Redactor {
        self.redactor.clone()
    }
}

/// Whether a route can be sent (doctor's `routes` lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Servable {
    Yes,
    /// No adapter of this crate serves the route: none of its capability on its provider, or
    /// none under the `adapter` its contract names.
    NoAdapter,
    /// An adapter serves it, but the provider's key is missing: the variable it is read from.
    NoKey(String),
    /// An adapter serves it through a program that is not installed (`codex`, `grok`).
    NoProgram(&'static str),
    /// An adapter serves it through a program older than a live run starts.
    TooOld {
        program: &'static str,
        version: String,
        minimum: &'static str,
    },
    /// An adapter serves it through a program that names no version a live run reads.
    NoVersion(&'static str),
}

/// Where the environment finds a program.
type Finder = fn(&dyn Fn(&str) -> Option<String>) -> Option<Program>;

/// How a program is asked its version before a live run starts it: the program, a folder it may
/// run in, and the declared variables it never inherits.
type Asker = for<'a> fn(&'a Path, &'a Path, &'a [String]) -> BoxFuture<'a, Result<Version, String>>;

/// The providers that are programs the user installed: how each is found, and how it is asked
/// its version before it starts, when it is (Grok Build names its own once started).
const PROGRAMS: [(&str, Finder, Option<Asker>); 2] = [
    ("codex", codex_program, Some(crate::codex::version)),
    ("grok", crate::grok::find_program, None),
];

/// The user's Codex: [`crate::codex::find_program`], with the macOS app's.
fn codex_program(env: &dyn Fn(&str) -> Option<String>) -> Option<Program> {
    let app = Path::new(crate::codex::MACOS_APP_PROGRAM);
    crate::codex::find_program(env, app)
}

/// Codex or Grok Build, as a live run would find it (doctor's `program` lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// `codex` or `grok`.
    pub name: &'static str,
    /// The program, when found.
    pub program: Option<Program>,
    /// Its version, for a program asked before it starts (a program that cannot be started names
    /// none).
    pub version: Option<Version>,
}

/// Codex and Grok Build, as the process environment finds them, each asked its version as a live
/// run asks it (in the system's temporary folder, without `withheld`, the declared variables:
/// [`Setup::withheld`]).
pub async fn installed(withheld: &[String]) -> Vec<Installed> {
    let env = |name: &str| std::env::var(name).ok();
    let folder = std::env::temp_dir();
    let mut found = Vec::new();
    for (name, find, ask) in PROGRAMS {
        let program = find(&env);
        let version = match (&program, ask) {
            (Some(program), Some(ask)) => Some(
                ask(&program.path, &folder, withheld)
                    .await
                    .unwrap_or(Version::Unnamed),
            ),
            _ => None,
        };
        found.push(Installed {
            name,
            program,
            version,
        });
    }
    found
}

/// Whether `route` is servable with `keys` by `adapters`: an adapter serves it
/// ([`Adapters::serving`], by its contract's `adapter`), and the key its provider's declaration
/// names, if any, is present. The route of a program the user installed is servable when
/// `installed` holds it found, and of a version a live run starts.
pub fn servable(
    adapters: &Adapters,
    keys: &Keys,
    installed: &[Installed],
    route: &RouteRef,
) -> Servable {
    if adapters.serving(route).is_none() {
        return Servable::NoAdapter;
    }
    let provider = route.provider.as_str();
    if let Some(program) = installed.iter().find(|program| program.name == provider) {
        return match (&program.program, &program.version) {
            (None, _) => Servable::NoProgram(program.name),
            (Some(_), Some(Version::Older { version, minimum })) => Servable::TooOld {
                program: program.name,
                version: version.clone(),
                minimum,
            },
            (Some(_), Some(Version::Unnamed)) => Servable::NoVersion(program.name),
            (Some(_), _) => Servable::Yes,
        };
    }
    let declared = keys.declarations().get(provider);
    match declared.and_then(|declaration| declaration.credential.as_ref()) {
        Some(credential) if keys.of(provider).is_none() => {
            Servable::NoKey(credential.variable.clone())
        }
        _ => Servable::Yes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::keys::KeyName;
    use crate::transport::replay::NoNetwork;
    use grida_fx_core::providers::{BaseUrl, Credential, Declaration, Declarations};

    fn setup(keys: Keys) -> Setup {
        Setup::new(Arc::new(NoNetwork), keys, Arc::new(FakeClock::new()))
    }

    #[test]
    fn fal_video_checks_the_queue_clients_store_eligibility() {
        use crate::testing::{CallBuilder, block_on};

        let mut setup = setup(Keys::none())
            .with_stored_credentials(Err("cannot read test store".into()), &[KeyName::Fal]);
        let call = CallBuilder::new("video.generate", "vid-a@fal").build();
        let result = |setup: &Setup| {
            let adapters = adapters(setup);
            let Some(Adapter::Job(adapter)) = adapters.serving(&call.route) else {
                panic!("no fal video adapter");
            };
            adapter.prepare(&call).map(block_on)
        };
        assert_eq!(result(&setup), Some(Err("cannot read test store".into())));
        for base in ["https://queue.example.test", "https://fal.run"] {
            setup.endpoints.fal_queue = base.into();
            assert_eq!(result(&setup), None);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_configured_base_is_refused_once_another_providers_saved_key_matches() {
        use crate::keys::Environment;
        use crate::testing::{CallBuilder, block_on};
        use crate::transport::{HttpRequest, Lane, Method, TransportErrorKind};
        use std::fs::{DirBuilder, OpenOptions};
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().canonicalize().unwrap();
        let providers = home.join("providers");
        DirBuilder::new().mode(0o700).create(&providers).unwrap();
        writeln!(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(providers.join("credentials.toml"))
                .unwrap(),
            "version = 1\n[migration]\nstate = 'unstarted'\nremoved = []\n\
             [providers.gg]\napi_key = 'ACME-STORE-KEY'"
        )
        .unwrap();
        let environment = Environment::from_pairs(&[
            ("OPENAI_API_KEY", "acme-test-key"),
            ("OPENAI_BASE_URL", "https://ACME-STORE-KEY.example.test/v1"),
        ]);
        let mut setup = setup(Keys::from_environment(&environment));
        setup.endpoints = Endpoints::from_environment(&environment, &setup.keys).unwrap();
        let setup = setup.with_stored_credentials(Ok(home), &[KeyName::GridaGateway]);
        let adapters = adapters(&setup);
        let prepare = |call: &CallRequest| {
            let Some(Adapter::Request(adapter)) = adapters.serving(&call.route) else {
                panic!("no image adapter");
            };
            block_on(adapter.prepare(call).unwrap())
        };

        let other = CallBuilder::new("image.generate", "img-a@openai").build();
        assert_eq!(prepare(&other), Ok(()));
        let gg = CallBuilder::new("image.generate", "img-a@gg").build();
        assert_eq!(prepare(&gg), Ok(()));
        assert_eq!(
            prepare(&other),
            Err("OPENAI_BASE_URL holds a credential".into())
        );

        let client = setup.client(KeyName::OpenAi);
        let request = HttpRequest::new(
            Method::Post,
            client.url("images/generations"),
            Lane::Provider,
        );
        let error = block_on(client.send(request)).unwrap_err();
        assert_eq!(error.kind, TransportErrorKind::Refused);
        assert_eq!(error.reason, "OPENAI_BASE_URL holds a credential");
    }

    #[test]
    fn every_default_route_has_an_adapter() {
        let adapters = adapters(&setup(Keys::none()));
        let table = crate::routes::default_table().unwrap();
        for ((capability, id), route) in &table.entries {
            assert!(
                adapters.serving(&RouteRef::of(route)).is_some(),
                "no adapter serves {id} for {capability}"
            );
        }
        assert!(adapters.serves("background.remove", "fal"));
    }

    /// The built-in declarations and `acme`, a project's, with a key.
    fn declared() -> Declarations {
        Declarations::with(&[Declaration {
            name: "acme".into(),
            label: "acme".into(),
            credential: Some(Credential {
                variable: "ACME_API_KEY".into(),
                header: "authorization".into(),
                scheme: "Bearer".into(),
            }),
            base_url: BaseUrl {
                variable: None,
                default: "https://api.acme.test/v1".into(),
            },
            download_hosts: Vec::new(),
            family: Some("openai-compatible".into()),
            adapter: None,
        }])
    }

    #[test]
    fn every_adapter_redacts_with_every_key_of_the_invocation() {
        let environment = crate::keys::Environment::declared(
            &[
                ("OPENAI_API_KEY", "openai-test-key-0123"),
                ("TRIPO_API_KEY", "tripo-test-key-0123"),
                ("ACME_API_KEY", "acme-test-key-0123"),
            ],
            &declared(),
        );
        let keys = Keys::from_environment(&environment);
        let plain = adapters(&setup(Keys::none()));
        let adapters = adapters(&setup(keys));
        // FX's routes, in order, then the declared family's on acme.
        let mut served = plain.served();
        served.extend([
            ("structured.generate".to_string(), "acme".to_string()),
            ("agent.turn".to_string(), "acme".to_string()),
        ]);
        assert_eq!(adapters.served(), served, "the same routes, in order");
        let registered = adapters.registered();
        assert_eq!(registered[..plain.registered().len()], plain.registered());
        let declared: Vec<(&str, &str, bool)> = registered[plain.registered().len()..]
            .iter()
            .map(|r| (r.provider, r.identity.adapter, r.default))
            .collect();
        assert_eq!(
            declared,
            [
                ("acme", "openai-compatible-structured", true),
                ("acme", "openai-compatible-tool-loop", true),
            ]
        );
        for registered in adapters.registered() {
            let (capability, provider) = (registered.capability, registered.provider);
            let route = RouteRef {
                capability: capability.into(),
                model: "m".into(),
                provider: provider.into(),
                contract: serde_json::json!({"adapter": registered.identity.adapter}),
                limits: Default::default(),
            };
            let redactor = match adapters.serving(&route) {
                Some(Adapter::Request(adapter)) => adapter.redactor(),
                Some(Adapter::Job(adapter)) => adapter.redactor(),
                None => panic!("{capability} on {provider} is served"),
            };
            assert_eq!(
                redactor.reason("x openai-test-key-0123 tripo-test-key-0123 acme-test-key-0123"),
                "x [redacted] [redacted] [redacted]",
                "{} for {capability} on {provider}",
                registered.identity.adapter
            );
        }
    }

    #[test]
    fn the_network_can_be_turned_off() {
        let off = |name: &str| (name == "GRIDA_FX_NETWORK").then(|| "off".to_string());
        assert_eq!(Network::from_env(&off), Network::Off);
        assert_eq!(Network::from_env(&|_| Some(" OFF ".into())), Network::Off);
        assert_eq!(Network::from_env(&|_| None), Network::On);
        let setup = live_setup(Keys::none(), Endpoints::default(), Network::Off).unwrap();
        let request = crate::transport::HttpRequest::new(
            crate::transport::Method::Get,
            "https://api.example.test/x",
            crate::transport::Lane::Provider,
        );
        let error = crate::testing::block_on(setup.transport.send(request)).unwrap_err();
        assert_eq!(error.kind, crate::transport::TransportErrorKind::Refused);
    }

    #[test]
    fn the_offline_setup_sends_nothing() {
        let keys = Keys::from_pairs(&[(KeyName::OpenAi, "openai-test-key")]);
        let off = offline_setup(keys.clone());
        assert_eq!(off.keys, keys);
        assert_eq!(off.endpoints, Endpoints::default());
        let request = crate::transport::HttpRequest::new(
            crate::transport::Method::Post,
            "https://api.example.test/x",
            crate::transport::Lane::Provider,
        );
        let error = crate::testing::block_on(off.transport.send(request)).unwrap_err();
        assert_eq!(error.kind, crate::transport::TransportErrorKind::Refused);
        assert!(
            error.reason.contains("GRIDA_FX_NETWORK"),
            "{}",
            error.reason
        );
        // The same registry as a live run's.
        let offline = adapters(&off);
        let replayed = adapters(&setup(keys));
        assert_eq!(offline.served(), replayed.served());
        assert!(!offline.is_empty());
    }

    fn route(capability: &str, id: &str, contract: Value) -> RouteRef {
        let (model, provider) = grida_fx_core::routes::parse_route_id(id).unwrap();
        RouteRef {
            capability: capability.into(),
            model,
            provider,
            contract,
            limits: Default::default(),
        }
    }

    #[test]
    fn a_route_without_its_key_is_not_servable() {
        let keys = Keys::from_pairs(&[(KeyName::Fal, "fal-test-key")]);
        let adapters = adapters(&setup(keys.clone()));
        let servable = |capability, id, contract| {
            servable(&adapters, &keys, &[], &route(capability, id, contract))
        };
        let none = Value::Null;
        assert_eq!(
            servable("video.generate", "vid-a@fal", none.clone()),
            Servable::Yes
        );
        assert_eq!(
            servable("mesh.rig", "rig-a@tripo", none.clone()),
            Servable::NoKey("TRIPO_API_KEY".into())
        );
        assert_eq!(
            servable("image.generate", "img-a@acme", none.clone()),
            Servable::NoAdapter
        );
        // A provider declared with the family is served on its chat capabilities: with its key
        // only, and on no other capability.
        let declared_servable = |pairs: &[(&str, &str)], capability, id| {
            let environment = crate::keys::Environment::declared(pairs, &declared());
            let keys = Keys::from_environment(&environment);
            let adapters = super::adapters(&setup(keys.clone()));
            super::servable(&adapters, &keys, &[], &route(capability, id, Value::Null))
        };
        for capability in ["structured.generate", "agent.turn"] {
            assert_eq!(
                declared_servable(&[], capability, "chat-a@acme"),
                Servable::NoKey("ACME_API_KEY".into())
            );
            assert_eq!(
                declared_servable(&[("ACME_API_KEY", "k")], capability, "chat-a@acme"),
                Servable::Yes
            );
        }
        assert_eq!(
            declared_servable(&[("ACME_API_KEY", "k")], "image.generate", "img-a@acme"),
            Servable::NoAdapter
        );
        // Undeclared, the same route has none.
        let chat = route("structured.generate", "chat-a@acme", none.clone());
        assert_eq!(
            super::servable(&adapters, &keys, &[], &chat),
            Servable::NoAdapter
        );
        assert_eq!(
            servable("video.generate", "vid-a@openai", none),
            Servable::NoAdapter
        );
        // The contract's adapter picks among the pair's adapters: a name none has is no adapter.
        let named = |adapter: &str| serde_json::json!({"adapter": adapter});
        for adapter in ["fal-image", "fx-fal-image-v1"] {
            assert_eq!(
                servable("image.edit", "acme/img-a@fal", named(adapter)),
                Servable::Yes,
                "{adapter}"
            );
        }
        assert_eq!(
            servable("image.edit", "acme/img-a@fal", named("acme-image")),
            Servable::NoAdapter
        );
        assert_eq!(
            servable("video.generate", "vid-a@fal", named("fal-image")),
            Servable::NoAdapter
        );
    }

    #[test]
    fn a_program_is_servable_when_found_and_of_a_version_a_live_run_starts() {
        let keys = Keys::none();
        let adapters = adapters(&setup(keys.clone()));
        let found = |name, version| Installed {
            name,
            program: Some(Program {
                path: "/opt/acme/bin/x".into(),
                origin: "PATH",
            }),
            version,
        };
        let servable = |installed: &[Installed], provider: &str| {
            let id = format!("img-a@{provider}");
            servable(
                &adapters,
                &keys,
                installed,
                &route("image.generate", &id, Value::Null),
            )
        };
        let older = Version::Older {
            version: "0.150.0".into(),
            minimum: "0.160.1",
        };
        let codex = |version| [found("codex", Some(version))];
        assert_eq!(
            servable(&codex(Version::Supported("0.160.1".into())), "codex"),
            Servable::Yes
        );
        assert_eq!(
            servable(&codex(older), "codex"),
            Servable::TooOld {
                program: "codex",
                version: "0.150.0".into(),
                minimum: "0.160.1"
            }
        );
        assert_eq!(
            servable(&codex(Version::Unnamed), "codex"),
            Servable::NoVersion("codex")
        );
        // Grok Build names its version once started: found is servable.
        assert_eq!(servable(&[found("grok", None)], "grok"), Servable::Yes);
        let missing = Installed {
            name: "grok",
            program: None,
            version: None,
        };
        assert_eq!(servable(&[missing], "grok"), Servable::NoProgram("grok"));
    }
}
