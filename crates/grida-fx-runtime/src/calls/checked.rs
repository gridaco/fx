//! The engine's checks around an adapter that names its identity, one FX ships or a project
//! adapter (spec/adapters.md §3.4, §4, §9; spec/capabilities.md §1): [`checked`] wraps the
//! adapter the registry hands the call path.
//!
//! - **Before a send or a submit**, the route's contract against the adapter's identity, the
//!   adapter's own judgement of its other contract members, and the request against its capability
//!   (`grida_fx_providers::checks::before_send`). A refusal is `Sent::Refused` (or
//!   `Submitted::Refused`) with the sentence, made where the adapter's own refusal would be: inside
//!   the attempt, after the route's slot, the hold and a long job's `submitting` record, so the
//!   events, the $0 settlement and the job record are the same whoever refuses. The retry owner
//!   redacts the sentence with the adapter's redactor, as every reason it reports.
//!   A request that would be refused skips optional credential preparation.
//! - **After the adapter's check**, the capability's every-route check
//!   (`grida_fx_providers::checks::every_route_with`), with the adapter's redactor. Its refusal
//!   fails the attempt as billed, as the adapter's check's would.
//!
//! Collecting a job is not checked first: the job's call key already fixes its route and request
//! (spec/adapters.md §4). An adapter that names no identity (a test's fake) is handed back as it
//! is, and none of this applies to it.

use grida_fx_providers::adapter::{Billing, Identity};
use grida_fx_providers::checks;
use grida_fx_providers::redact::Redactor;
use grida_fx_providers::{
    Adapter, Answer, BoxFuture, CallRequest, Collected, LongJob, RequestAdapter, Sent, Submitted,
};
use serde_json::Value;
use std::sync::Arc;

/// `adapter` with the engine's checks around it (module doc); as it is when it names no identity.
pub fn checked(adapter: Adapter) -> Adapter {
    let Some(identity) = adapter.identity() else {
        return adapter;
    };
    match adapter {
        Adapter::Request(inner) => Adapter::Request(Arc::new(Checked { inner, identity })),
        Adapter::Job(inner) => Adapter::Job(Arc::new(Checked { inner, identity })),
    }
}

/// An adapter and the identity the engine checks its calls against.
struct Checked<A: ?Sized> {
    inner: Arc<A>,
    identity: Identity,
}

impl RequestAdapter for Checked<dyn RequestAdapter> {
    fn prepare<'a>(&'a self, call: &'a CallRequest) -> Option<BoxFuture<'a, Result<(), String>>> {
        validation(call, &self.identity, || self.inner.check_contract(call)).ok()?;
        self.inner.prepare(call)
    }

    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        match validation(call, &self.identity, || self.inner.check_contract(call)) {
            Ok(()) => self.inner.send(call),
            Err((source, reason)) => Box::pin(async move {
                let diagnostic = validation_diagnostic(call, source, &reason);
                Sent::Refused { reason }.with_diagnostic(diagnostic)
            }),
        }
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        self.inner.check(call, answer)?;
        checks::every_route_with(call, answer, &self.inner.redactor())
    }

    fn identity(&self) -> Option<Identity> {
        Some(self.identity)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        self.inner.check_contract(call)
    }

    fn billing(&self) -> Billing {
        self.inner.billing()
    }

    fn redactor(&self) -> Redactor {
        self.inner.redactor()
    }
}

impl LongJob for Checked<dyn LongJob> {
    fn prepare<'a>(&'a self, call: &'a CallRequest) -> Option<BoxFuture<'a, Result<(), String>>> {
        validation(call, &self.identity, || self.inner.check_contract(call)).ok()?;
        self.inner.prepare(call)
    }

    fn prepare_collect<'a>(
        &'a self,
        call: &'a CallRequest,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        self.inner.prepare_collect(call)
    }

    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        match validation(call, &self.identity, || self.inner.check_contract(call)) {
            Ok(()) => self.inner.submit(call),
            Err((source, reason)) => Box::pin(async move {
                let diagnostic = validation_diagnostic(call, source, &reason);
                Submitted::Refused { reason }.with_diagnostic(diagnostic)
            }),
        }
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        self.inner.collect(call, handle)
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        self.inner.check(call, answer)?;
        checks::every_route_with(call, answer, &self.inner.redactor())
    }

    fn identity(&self) -> Option<Identity> {
        Some(self.identity)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        self.inner.check_contract(call)
    }

    fn billing(&self) -> Billing {
        self.inner.billing()
    }

    fn redactor(&self) -> Redactor {
        self.inner.redactor()
    }
}

fn validation_diagnostic(
    call: &CallRequest,
    source: grida_fx_protocol::DiagnosticSource,
    reason: &str,
) -> grida_fx_protocol::AdapterDiagnostic {
    use grida_fx_protocol::{AdapterDiagnostic, DiagnosticDetail, DiagnosticPhase};
    let mut diagnostic = AdapterDiagnostic::new(source, DiagnosticPhase::Validate);
    diagnostic.operation = Some(call.route.capability.clone());
    diagnostic.message = Some(reason.to_string());
    diagnostic.detail = DiagnosticDetail::Available;
    diagnostic
}

fn validation(
    call: &CallRequest,
    identity: &Identity,
    own: impl FnOnce() -> Result<(), String>,
) -> Result<(), (grida_fx_protocol::DiagnosticSource, String)> {
    use grida_fx_protocol::DiagnosticSource;
    checks::contract(&call.route, identity).map_err(|reason| (DiagnosticSource::Engine, reason))?;
    own().map_err(|reason| (DiagnosticSource::Adapter, reason))?;
    checks::request(&call.route.capability, &call.request)
        .map_err(|reason| (DiagnosticSource::Engine, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_providers::testing::{CallBuilder, block_on};
    use serde_json::json;

    struct Acme;

    const IDENTITY: Identity = Identity {
        adapter: "acme-image",
        behavior: "1",
    };

    impl RequestAdapter for Acme {
        fn prepare<'a>(&'a self, _: &'a CallRequest) -> Option<BoxFuture<'a, Result<(), String>>> {
            panic!("preparation received an invalid request");
        }

        fn send<'a>(&'a self, _: &'a CallRequest) -> BoxFuture<'a, Sent> {
            panic!("send received an invalid request");
        }

        fn identity(&self) -> Option<Identity> {
            Some(IDENTITY)
        }
    }

    impl LongJob for Acme {
        fn prepare<'a>(&'a self, _: &'a CallRequest) -> Option<BoxFuture<'a, Result<(), String>>> {
            panic!("preparation received an invalid job");
        }

        fn submit<'a>(&'a self, _: &'a CallRequest) -> BoxFuture<'a, Submitted> {
            panic!("submit received an invalid job");
        }

        fn collect<'a>(&'a self, _: &'a CallRequest, _: &'a Value) -> BoxFuture<'a, Collected> {
            panic!("no job was submitted");
        }

        fn identity(&self) -> Option<Identity> {
            Some(IDENTITY)
        }
    }

    #[test]
    fn a_bad_contract_is_refused_without_invoking_preparation() {
        let call = CallBuilder::new("image.generate", "img-a@acme")
            .contract(json!({"adapter": IDENTITY.adapter, "adapter_behavior": 2}))
            .request(json!({"prompt": "a kite"}))
            .build();
        let Adapter::Request(request) = checked(Adapter::Request(Arc::new(Acme))) else {
            unreachable!("the wrapped adapter is a request");
        };
        assert!(request.prepare(&call).is_none());
        assert!(matches!(
            block_on(request.send(&call)).into_report().0,
            Sent::Refused { .. }
        ));

        let Adapter::Job(job) = checked(Adapter::Job(Arc::new(Acme))) else {
            unreachable!("the wrapped adapter is a job");
        };
        assert!(job.prepare(&call).is_none());
        assert!(matches!(
            block_on(job.submit(&call)).into_report().0,
            Submitted::Refused { .. }
        ));
    }
}
