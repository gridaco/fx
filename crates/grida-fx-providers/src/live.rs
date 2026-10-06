//! A live invocation's adapters (spec/providers.md §1): the only place the default transport is
//! built. `grida-fx run --live` (and an `at: plan` step of a live run) calls [`live_setup`] and
//! [`adapters`]; without `--live` nothing here is constructed and the engine gets an empty
//! [`Adapters`], so every uncached paid call is refused before anything could leave.
//!
//! [`adapters`] registers every provider's adapters on whatever [`Setup`] it is given, so tests
//! build the same registry over a `ReplayTransport` or `NoNetwork`. [`servable`] is what
//! `grida-fx doctor` reports per route, over a registry built on [`offline_setup`]: doctor never
//! constructs anything that could reach the network.

use crate::clock::SystemClock;
use crate::keys::{KeyName, Keys};
use crate::registry::Adapters;
use crate::setup::{Endpoints, Setup};
use crate::transport::http::{HttpConfig, HttpTransport};
use std::sync::Arc;

/// The providers this crate serves, in the order `doctor` lists them.
pub const PROVIDERS: [&str; 5] = ["openai", "openrouter", "fal", "tripo", "elevenlabs"];

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
/// network is off) and the system clock.
pub fn live_setup(keys: Keys, endpoints: Endpoints, network: Network) -> Result<Setup, String> {
    let transport: Arc<dyn crate::transport::Transport> = match network {
        Network::On => Arc::new(HttpTransport::new(HttpConfig::default())?),
        Network::Off => Arc::new(crate::transport::Offline),
    };
    Ok(Setup {
        transport,
        keys,
        endpoints,
        clock: Arc::new(SystemClock::new()),
    })
}

/// A setup that can send nothing: the [`crate::transport::Offline`] transport (every exchange
/// refused before it leaves), the default endpoints and the system clock. `grida-fx doctor`
/// builds its registry on it to say which routes a live run could serve.
pub fn offline_setup(keys: Keys) -> Setup {
    Setup {
        transport: Arc::new(crate::transport::Offline),
        keys,
        endpoints: Endpoints::default(),
        clock: Arc::new(SystemClock::new()),
    }
}

/// Every provider's adapters over `setup`.
pub fn adapters(setup: &Setup) -> Adapters {
    let mut adapters = Adapters::new();
    crate::openai::register(&mut adapters, setup);
    crate::openrouter::register(&mut adapters, setup);
    crate::fal::register(&mut adapters, setup);
    crate::tripo::register(&mut adapters, setup);
    crate::elevenlabs::register(&mut adapters, setup);
    adapters
}

/// Whether a route can be sent (doctor's `routes` lines).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Servable {
    Yes,
    /// No adapter of this crate serves the capability on the provider.
    NoAdapter,
    /// An adapter serves it, but the provider's key is missing.
    NoKey(KeyName),
}

/// Whether `capability` on `provider` is servable with `keys` by `adapters`.
pub fn servable(adapters: &Adapters, keys: &Keys, capability: &str, provider: &str) -> Servable {
    if !adapters.serves(capability, provider) {
        return Servable::NoAdapter;
    }
    match KeyName::of_provider(provider) {
        Some(key) if keys.get(key).is_none() => Servable::NoKey(key),
        _ => Servable::Yes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::transport::replay::NoNetwork;

    fn setup(keys: Keys) -> Setup {
        Setup {
            transport: Arc::new(NoNetwork),
            keys,
            endpoints: Endpoints::default(),
            clock: Arc::new(FakeClock::new()),
        }
    }

    #[test]
    fn every_default_route_has_an_adapter() {
        let adapters = adapters(&setup(Keys::none()));
        let table = crate::routes::default_table().unwrap();
        for ((capability, id), route) in &table.entries {
            assert!(
                adapters.serves(capability, &route.provider),
                "no adapter serves {id} for {capability}"
            );
        }
        assert!(adapters.serves("background.remove", "fal"));
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

    #[test]
    fn a_route_without_its_key_is_not_servable() {
        let keys = Keys::from_pairs(&[(KeyName::Fal, "fal-test-key")]);
        let adapters = adapters(&setup(keys.clone()));
        assert_eq!(
            servable(&adapters, &keys, "video.generate", "fal"),
            Servable::Yes
        );
        assert_eq!(
            servable(&adapters, &keys, "mesh.rig", "tripo"),
            Servable::NoKey(KeyName::Tripo)
        );
        assert_eq!(
            servable(&adapters, &keys, "image.generate", "acme"),
            Servable::NoAdapter
        );
        assert_eq!(
            servable(&adapters, &keys, "video.generate", "openai"),
            Servable::NoAdapter
        );
    }
}
