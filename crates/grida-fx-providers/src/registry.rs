//! Which adapter serves a route (spec/adapters.md §1; spec/providers.md §2): by capability,
//! provider and the contract `adapter` the adapter's [`Identity`] names, and per capability and
//! provider a default, which serves a route whose contract names no `adapter` (absent, `null` or
//! not a string). A route whose contract names an adapter none is registered under, or whose
//! provider has no adapter for its capability, is refused with `no_route` when a call on it must
//! be sent (spec/protocol.md §6.1, §7).

use crate::BoxFuture;
use crate::adapter::{Adapter, Billing, Identity, RouteRef, Runtime};
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The adapters of one invocation, and the runtimes they share.
#[derive(Clone, Default)]
pub struct Adapters {
    by_route: IndexMap<(String, String), Served>,
    runtimes: Vec<Arc<dyn Runtime>>,
}

/// The adapters of one capability and provider.
#[derive(Clone, Default, Debug)]
struct Served {
    /// The adapter of a route whose contract names none.
    default: Option<Adapter>,
    /// Each adapter by the contract `adapter` its identity names, in registration order.
    named: IndexMap<&'static str, Adapter>,
}

/// One adapter as registered ([`Adapters::registered`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registered<'a> {
    pub capability: &'a str,
    pub provider: &'a str,
    /// The contract it serves.
    pub identity: Identity,
    /// Whether it is its capability and provider's default.
    pub default: bool,
}

impl std::fmt::Debug for Adapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adapters")
            .field("by_route", &self.by_route)
            .field("runtimes", &self.runtimes.len())
            .finish()
    }
}

impl Adapters {
    /// No adapters: every uncached live call is `no_route`.
    pub fn new() -> Adapters {
        Adapters::default()
    }

    /// Registers `adapter` as the default for `capability` on `provider`, and under the contract
    /// `adapter` its identity names, if any. A later registration replaces an earlier one: the
    /// default, and the adapter of the same name.
    pub fn register(&mut self, capability: &str, provider: &str, adapter: Adapter) {
        let served = self.served_mut(capability, provider);
        if let Some(identity) = adapter.identity() {
            served.named.insert(identity.adapter, adapter.clone());
        }
        served.default = Some(adapter);
    }

    /// Registers `adapter` for `capability` on `provider` under the contract `adapter` its
    /// identity names, for a route whose contract names it only: the pair's default is unchanged.
    /// Panics on an adapter that names no identity, which no route could name.
    pub fn register_named(&mut self, capability: &str, provider: &str, adapter: Adapter) {
        let identity = adapter
            .identity()
            .expect("an adapter registered by name names its contract");
        self.served_mut(capability, provider)
            .named
            .insert(identity.adapter, adapter);
    }

    fn served_mut(&mut self, capability: &str, provider: &str) -> &mut Served {
        self.by_route
            .entry((capability.to_string(), provider.to_string()))
            .or_default()
    }

    /// The adapter serving `route`, if any (module doc).
    pub fn serving(&self, route: &RouteRef) -> Option<&Adapter> {
        self.serving_contract(&route.capability, &route.provider, &route.contract)
    }

    /// The adapter serving a route of `capability` on `provider` whose contract is `contract`:
    /// the one its `adapter` names, else the default.
    pub fn serving_contract(
        &self,
        capability: &str,
        provider: &str,
        contract: &Value,
    ) -> Option<&Adapter> {
        let served = self
            .by_route
            .get(&(capability.to_string(), provider.to_string()))?;
        match contract.get("adapter").and_then(Value::as_str) {
            Some(name) => served.named.get(name),
            None => served.default.as_ref(),
        }
    }

    /// Whether an adapter serves `capability` on `provider`.
    pub fn serves(&self, capability: &str, provider: &str) -> bool {
        self.by_route
            .contains_key(&(capability.to_string(), provider.to_string()))
    }

    /// Every `(capability, provider)` served, in registration order.
    pub fn served(&self) -> Vec<(String, String)> {
        self.by_route.keys().cloned().collect()
    }

    /// Every adapter registered under its contract `adapter`, by capability and provider in
    /// registration order, then in the order its names were registered. A default that names no
    /// identity (a test's double) is not listed.
    pub fn registered(&self) -> Vec<Registered<'_>> {
        let mut found = Vec::new();
        for ((capability, provider), served) in &self.by_route {
            let default = served
                .default
                .as_ref()
                .and_then(Adapter::identity)
                .map(|identity| identity.adapter);
            for (name, adapter) in &served.named {
                let Some(identity) = adapter.identity() else {
                    continue;
                };
                found.push(Registered {
                    capability,
                    provider,
                    identity,
                    default: default == Some(*name),
                });
            }
        }
        found
    }

    /// The same registry with every adapter replaced by `wrap` of it: defaults, names and order
    /// kept. The runtimes are kept too.
    pub fn map(&self, wrap: impl Fn(&Adapter) -> Adapter) -> Adapters {
        let by_route = self
            .by_route
            .iter()
            .map(|(pair, served)| {
                let served = Served {
                    default: served.default.as_ref().map(&wrap),
                    named: served
                        .named
                        .iter()
                        .map(|(name, adapter)| (*name, wrap(adapter)))
                        .collect(),
                };
                (pair.clone(), served)
            })
            .collect();
        Adapters {
            by_route,
            runtimes: self.runtimes.clone(),
        }
    }

    /// Whether no adapter is registered.
    pub fn is_empty(&self) -> bool {
        self.by_route.is_empty()
    }

    /// Keeps a runtime the registered adapters share, to shut it down with the invocation.
    pub fn keep_runtime(&mut self, runtime: Arc<dyn Runtime>) {
        self.runtimes.push(runtime);
    }

    /// The runtimes kept, in order.
    pub fn runtimes(&self) -> &[Arc<dyn Runtime>] {
        &self.runtimes
    }

    /// The providers whose adapters bill the user's own plan ([`Billing::Plan`]). Only an
    /// adapter makes a provider one, never a route table: the registry serves a provider by name.
    pub fn plan_providers(&self) -> BTreeSet<String> {
        self.by_route
            .iter()
            .filter(|(_, served)| {
                served
                    .default
                    .iter()
                    .chain(served.named.values())
                    .any(|adapter| adapter.billing() == Billing::Plan)
            })
            .map(|((_, provider), _)| provider.clone())
            .collect()
    }

    /// How calls on `capability` and `provider` are billed: by the pair's default, else in
    /// dollars.
    pub fn billing(&self, capability: &str, provider: &str) -> Billing {
        self.by_route
            .get(&(capability.to_string(), provider.to_string()))
            .and_then(|served| served.default.as_ref())
            .map_or(Billing::Dollars, Adapter::billing)
    }

    /// Shuts every kept runtime down. Whether every one ended verifiably.
    pub fn shutdown(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let mut verified = true;
            for runtime in &self.runtimes {
                verified &= runtime.shutdown().await;
            }
            verified
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxFuture;
    use crate::adapter::{CallRequest, RequestAdapter, Sent};
    use serde_json::json;
    use std::sync::Arc;

    struct Named(&'static str);

    impl RequestAdapter for Named {
        fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
            Box::pin(async move {
                Sent::Refused {
                    reason: self.0.to_string(),
                }
            })
        }
    }

    /// An adapter that names its contract: the adapter `acme-<name>`.
    struct Identified(&'static str, &'static str);

    impl RequestAdapter for Identified {
        fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
            Box::pin(async move {
                Sent::Refused {
                    reason: self.1.to_string(),
                }
            })
        }

        fn identity(&self) -> Option<Identity> {
            Some(Identity {
                adapter: self.0,
                behavior: r#""1""#,
            })
        }
    }

    fn naming(capability: &str, provider: &str, contract: Value) -> RouteRef {
        RouteRef {
            contract,
            ..route(capability, "img-a", provider)
        }
    }

    fn route(capability: &str, model: &str, provider: &str) -> RouteRef {
        RouteRef {
            capability: capability.into(),
            model: model.into(),
            provider: provider.into(),
            contract: json!({}),
            limits: Default::default(),
        }
    }

    fn name_of(adapter: Option<&Adapter>) -> Option<String> {
        let Some(Adapter::Request(adapter)) = adapter else {
            return None;
        };
        let call = CallRequest {
            route: route("x", "m", "p"),
            request: json!({}),
            files: indexmap::IndexMap::new(),
            take: vec![1],
            key: String::new(),
            attempt: 1,
        };
        let sent = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime")
            .block_on(adapter.send(&call));
        match sent {
            Sent::Refused { reason } => Some(reason),
            _ => None,
        }
    }

    #[test]
    fn no_adapters_serve_nothing() {
        let adapters = Adapters::new();
        assert!(adapters.is_empty());
        assert!(
            adapters
                .serving(&route("image.generate", "img-a", "acme"))
                .is_none()
        );
    }

    #[test]
    fn adapters_are_keyed_by_capability_and_provider_not_model() {
        let mut adapters = Adapters::new();
        adapters.register(
            "image.generate",
            "acme",
            Adapter::Request(Arc::new(Named("a"))),
        );
        adapters.register("agent.turn", "acme", Adapter::Request(Arc::new(Named("b"))));
        assert!(!adapters.is_empty());
        assert_eq!(
            name_of(adapters.serving(&route("image.generate", "img-a", "acme"))),
            Some("a".into())
        );
        assert_eq!(
            name_of(adapters.serving(&route("image.generate", "img-b", "acme"))),
            Some("a".into())
        );
        assert_eq!(
            name_of(adapters.serving(&route("agent.turn", "chat-a", "acme"))),
            Some("b".into())
        );
        assert!(
            adapters
                .serving(&route("image.generate", "img-a", "other"))
                .is_none()
        );
        assert!(
            adapters
                .serving(&route("video.generate", "img-a", "acme"))
                .is_none()
        );
    }

    #[test]
    fn a_contract_s_adapter_picks_among_a_pair_s_adapters() {
        let mut adapters = Adapters::new();
        let request = |adapter: Identified| Adapter::Request(Arc::new(adapter));
        adapters.register_named("image.edit", "acme", request(Identified("acme-gpt", "gpt")));
        adapters.register("image.edit", "acme", request(Identified("acme-any", "any")));
        let served =
            |contract: Value| name_of(adapters.serving(&naming("image.edit", "acme", contract)));
        // A contract that names none gets the default; one that names an adapter gets it.
        for none in [
            json!({}),
            json!({"adapter": null}),
            json!({"adapter": 1}),
            Value::Null,
        ] {
            assert_eq!(served(none.clone()), Some("any".into()), "{none}");
        }
        assert_eq!(served(json!({"adapter": "acme-gpt"})), Some("gpt".into()));
        assert_eq!(served(json!({"adapter": "acme-any"})), Some("any".into()));
        // An adapter no entry has serves nothing, even beside a default.
        assert_eq!(served(json!({"adapter": "acme-other"})), None);
        assert!(
            adapters
                .serving(&naming(
                    "image.generate",
                    "acme",
                    json!({"adapter": "acme-gpt"})
                ))
                .is_none()
        );
        assert_eq!(
            adapters.served(),
            [("image.edit".to_string(), "acme".to_string())]
        );
        let registered: Vec<(&str, bool)> = adapters
            .registered()
            .iter()
            .map(|r| (r.identity.adapter, r.default))
            .collect();
        assert_eq!(registered, [("acme-gpt", false), ("acme-any", true)]);
        // Wrapping keeps defaults and names.
        let wrapped = adapters.map(Adapter::clone);
        assert_eq!(
            name_of(wrapped.serving(&naming(
                "image.edit",
                "acme",
                json!({"adapter": "acme-gpt"})
            ))),
            Some("gpt".into())
        );
        assert_eq!(
            name_of(wrapped.serving(&naming("image.edit", "acme", json!({})))),
            Some("any".into())
        );
        // A pair with only adapters a route names has no default.
        let mut only = Adapters::new();
        only.register_named("image.edit", "acme", request(Identified("acme-gpt", "gpt")));
        assert!(only.serves("image.edit", "acme"));
        assert!(
            only.serving(&naming("image.edit", "acme", json!({})))
                .is_none()
        );
    }

    #[test]
    fn a_double_that_names_no_contract_serves_only_routes_that_name_none() {
        let mut adapters = Adapters::new();
        adapters.register(
            "image.generate",
            "acme",
            Adapter::Request(Arc::new(Named("a"))),
        );
        let served = |contract: Value| {
            name_of(adapters.serving(&naming("image.generate", "acme", contract)))
        };
        assert_eq!(served(json!({})), Some("a".into()));
        assert_eq!(served(json!({"adapter": "acme-image"})), None);
        assert!(adapters.registered().is_empty());
    }

    #[test]
    fn a_later_registration_replaces_an_earlier_one() {
        let mut adapters = Adapters::new();
        adapters.register(
            "image.generate",
            "acme",
            Adapter::Request(Arc::new(Named("a"))),
        );
        adapters.register(
            "image.generate",
            "acme",
            Adapter::Request(Arc::new(Named("c"))),
        );
        assert_eq!(
            name_of(adapters.serving(&route("image.generate", "img-a", "acme"))),
            Some("c".into())
        );
    }
}
