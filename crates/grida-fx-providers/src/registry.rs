//! Which adapter serves a route: one per capability and provider. A route whose provider has no
//! adapter for its capability is refused with `no_route` when a call on it must be sent
//! (spec/protocol.md §6.1, §7).

use crate::adapter::{Adapter, RouteRef};
use indexmap::IndexMap;

/// The adapters of one invocation.
#[derive(Debug, Clone, Default)]
pub struct Adapters {
    by_route: IndexMap<(String, String), Adapter>,
}

impl Adapters {
    /// No adapters: every uncached live call is `no_route`.
    pub fn new() -> Adapters {
        Adapters::default()
    }

    /// Registers `adapter` for `capability` on `provider`; a later registration replaces an
    /// earlier one.
    pub fn register(&mut self, capability: &str, provider: &str, adapter: Adapter) {
        self.by_route
            .insert((capability.to_string(), provider.to_string()), adapter);
    }

    /// The adapter serving `route`, if any.
    pub fn serving(&self, route: &RouteRef) -> Option<&Adapter> {
        self.by_route
            .get(&(route.capability.clone(), route.provider.clone()))
    }

    /// Whether no adapter is registered.
    pub fn is_empty(&self) -> bool {
        self.by_route.is_empty()
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

    fn route(capability: &str, model: &str, provider: &str) -> RouteRef {
        RouteRef {
            capability: capability.into(),
            model: model.into(),
            provider: provider.into(),
            contract: json!({}),
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
