//! A scripted adapter for the engine's tests (feature `testing`). It never talks to a provider:
//! each send, submit or collect takes the next outcome of its script and records the request it
//! was given, so a test can assert every attempt the retry owner made, in order.
//!
//! - [`FakeAdapter::plain`]: a [`RequestAdapter`] playing [`Sent`] outcomes;
//! - [`FakeAdapter::long_job`]: a [`LongJob`] playing [`Submitted`] then [`Collected`] outcomes;
//! - [`FakeAdapter::with_check`]: refuse answers whose `data` holds `{"bad": true}` (any check
//!   the test gives, [`refuse_bad`] being that one), to pin "checks after an answer fail the
//!   attempt as billed". Without a check every answer passes.
//!
//! A script that runs out answers `Sent::Failed { reason: "the fake adapter's script ran out",
//! cost: None, retryable: false }` (or the long-job equivalents), so a test that sends more than
//! it scripted fails loudly instead of hanging.
//!
//! Clones share one script and one log: register a clone ([`FakeAdapter::as_request`],
//! [`FakeAdapter::as_job`]) and read the log from the original.

use crate::BoxFuture;
use crate::adapter::{
    Adapter, Answer, CallRequest, Collected, LongJob, RequestAdapter, Sent, Submitted,
};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// What a script that ran out answers.
pub const SCRIPT_RAN_OUT: &str = "the fake adapter's script ran out";

/// What a fake adapter was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeCall {
    Send(CallRequest),
    Submit(CallRequest),
    Collect(CallRequest, Value),
}

impl FakeCall {
    /// The request the call carried.
    pub fn request(&self) -> &CallRequest {
        match self {
            FakeCall::Send(call) | FakeCall::Submit(call) | FakeCall::Collect(call, _) => call,
        }
    }
}

/// A check over an answer.
pub type FakeCheck = Arc<dyn Fn(&Answer) -> Result<(), String> + Send + Sync>;

/// The check the module doc names: refuses an answer whose `data` holds `"bad": true`.
pub fn refuse_bad() -> FakeCheck {
    Arc::new(|answer: &Answer| {
        if answer.data.get("bad") == Some(&Value::Bool(true)) {
            Err("the answer is bad".to_string())
        } else {
            Ok(())
        }
    })
}

/// A scripted adapter (module doc).
#[derive(Clone, Default)]
pub struct FakeAdapter {
    sends: Arc<Mutex<VecDeque<Sent>>>,
    submits: Arc<Mutex<VecDeque<Submitted>>>,
    collects: Arc<Mutex<VecDeque<Collected>>>,
    check: Option<FakeCheck>,
    /// Every call made, in order.
    pub calls: Arc<Mutex<Vec<FakeCall>>>,
}

impl std::fmt::Debug for FakeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeAdapter")
            .field("sends", &*lock(&self.sends))
            .field("submits", &*lock(&self.submits))
            .field("collects", &*lock(&self.collects))
            .field("check", &self.check.is_some())
            .field("calls", &lock(&self.calls).len())
            .finish()
    }
}

/// Locks a shared part, recovering from a test thread that panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeAdapter {
    /// A plain adapter playing `script`.
    pub fn plain(script: Vec<Sent>) -> FakeAdapter {
        FakeAdapter {
            sends: Arc::new(Mutex::new(script.into())),
            ..FakeAdapter::default()
        }
    }

    /// A long-job adapter playing `submits` and `collects`.
    pub fn long_job(submits: Vec<Submitted>, collects: Vec<Collected>) -> FakeAdapter {
        FakeAdapter {
            submits: Arc::new(Mutex::new(submits.into())),
            collects: Arc::new(Mutex::new(collects.into())),
            ..FakeAdapter::default()
        }
    }

    /// The same adapter with a check over answers (it replaces an earlier one). The script and
    /// the log stay shared with `self`'s clones.
    pub fn with_check(self, check: FakeCheck) -> FakeAdapter {
        FakeAdapter {
            check: Some(check),
            ..self
        }
    }

    /// This adapter, registered as a plain one.
    pub fn as_request(&self) -> Adapter {
        Adapter::Request(Arc::new(self.clone()))
    }

    /// This adapter, registered as a long-job one.
    pub fn as_job(&self) -> Adapter {
        Adapter::Job(Arc::new(self.clone()))
    }

    /// What was asked of it, in order.
    pub fn log(&self) -> Vec<FakeCall> {
        lock(&self.calls).clone()
    }

    /// How many scripted outcomes are left: `(sends, submits, collects)`.
    pub fn left(&self) -> (usize, usize, usize) {
        (
            lock(&self.sends).len(),
            lock(&self.submits).len(),
            lock(&self.collects).len(),
        )
    }

    fn record(&self, call: FakeCall) {
        lock(&self.calls).push(call);
    }

    fn apply_check(&self, answer: &Answer) -> Result<(), String> {
        match &self.check {
            Some(check) => check(answer),
            None => Ok(()),
        }
    }

    fn next_send(&self) -> Sent {
        lock(&self.sends)
            .pop_front()
            .unwrap_or_else(|| Sent::Failed {
                reason: SCRIPT_RAN_OUT.into(),
                cost: None,
                retryable: false,
            })
    }

    fn next_submit(&self) -> Submitted {
        lock(&self.submits)
            .pop_front()
            .unwrap_or_else(|| Submitted::Failed {
                reason: SCRIPT_RAN_OUT.into(),
                cost: None,
                retryable: false,
            })
    }

    fn next_collect(&self) -> Collected {
        lock(&self.collects)
            .pop_front()
            .unwrap_or_else(|| Collected::Ended {
                reason: SCRIPT_RAN_OUT.into(),
            })
    }
}

impl RequestAdapter for FakeAdapter {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            self.record(FakeCall::Send(call.clone()));
            self.next_send()
        })
    }

    fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
        self.apply_check(answer)
    }
}

impl LongJob for FakeAdapter {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(async move {
            self.record(FakeCall::Submit(call.clone()));
            self.next_submit()
        })
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(async move {
            self.record(FakeCall::Collect(call.clone(), handle.clone()));
            self.next_collect()
        })
    }

    fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
        self.apply_check(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AnsweredFile, RouteRef};
    use grida_fx_core::money::Usd;
    use indexmap::IndexMap;
    use serde_json::json;

    fn request(attempt: u32) -> CallRequest {
        CallRequest {
            route: RouteRef {
                capability: "image.generate".into(),
                model: "img-a".into(),
                provider: "acme".into(),
                contract: json!({}),
            },
            request: json!({"prompt": "a lantern"}),
            files: IndexMap::new(),
            take: vec![1],
            key: "0".repeat(64),
            attempt,
        }
    }

    fn answer(data: Value, cost: Option<Usd>) -> Answer {
        Answer {
            files: IndexMap::new(),
            data,
            cost,
        }
    }

    #[tokio::test]
    async fn plain_plays_its_script_in_order_and_logs_every_send() {
        let fake = FakeAdapter::plain(vec![
            Sent::NotReceived {
                reason: "reset".into(),
            },
            Sent::Answered(answer(json!({"n": 1}), Some(Usd(20_000)))),
        ]);
        let adapter: &dyn RequestAdapter = &fake;
        assert_eq!(
            adapter.send(&request(1)).await,
            Sent::NotReceived {
                reason: "reset".into()
            }
        );
        assert_eq!(
            adapter.send(&request(1)).await,
            Sent::Answered(answer(json!({"n": 1}), Some(Usd(20_000))))
        );
        assert_eq!(
            fake.log(),
            vec![FakeCall::Send(request(1)), FakeCall::Send(request(1))]
        );
        assert_eq!(fake.left(), (0, 0, 0));
    }

    #[tokio::test]
    async fn a_script_that_runs_out_fails_loudly() {
        let fake = FakeAdapter::default();
        assert_eq!(
            RequestAdapter::send(&fake, &request(1)).await,
            Sent::Failed {
                reason: SCRIPT_RAN_OUT.into(),
                cost: None,
                retryable: false,
            }
        );
        assert_eq!(
            LongJob::submit(&fake, &request(1)).await,
            Submitted::Failed {
                reason: SCRIPT_RAN_OUT.into(),
                cost: None,
                retryable: false,
            }
        );
        assert_eq!(
            LongJob::collect(&fake, &request(1), &json!({"id": "j1"})).await,
            Collected::Ended {
                reason: SCRIPT_RAN_OUT.into()
            }
        );
        assert_eq!(fake.log().len(), 3);
    }

    #[tokio::test]
    async fn long_job_plays_submits_and_collects_and_logs_the_handle() {
        let fake = FakeAdapter::long_job(
            vec![Submitted::Accepted {
                handle: json!({"id": "j1"}),
            }],
            vec![Collected::Unreachable {
                reason: "timeout".into(),
            }],
        );
        let job: &dyn LongJob = &fake;
        assert_eq!(
            job.submit(&request(2)).await,
            Submitted::Accepted {
                handle: json!({"id": "j1"})
            }
        );
        assert_eq!(
            job.collect(&request(2), &json!({"id": "j1"})).await,
            Collected::Unreachable {
                reason: "timeout".into()
            }
        );
        assert_eq!(
            fake.log(),
            vec![
                FakeCall::Submit(request(2)),
                FakeCall::Collect(request(2), json!({"id": "j1"})),
            ]
        );
        assert_eq!(fake.log()[1].request().attempt, 2);
    }

    #[tokio::test]
    async fn clones_share_the_script_and_the_log() {
        let fake = FakeAdapter::plain(vec![
            Sent::Refused {
                reason: "no key".into(),
            },
            Sent::Refused {
                reason: "second".into(),
            },
        ]);
        let Adapter::Request(registered) = fake.as_request() else {
            panic!("as_request registers a plain adapter");
        };
        assert_eq!(
            registered.send(&request(1)).await,
            Sent::Refused {
                reason: "no key".into()
            }
        );
        assert_eq!(
            RequestAdapter::send(&fake, &request(1)).await,
            Sent::Refused {
                reason: "second".into()
            }
        );
        assert_eq!(fake.log().len(), 2);
        assert!(matches!(fake.as_job(), Adapter::Job(_)));
    }

    #[test]
    fn checks_pass_by_default_and_apply_the_given_check() {
        let plain = FakeAdapter::plain(Vec::new());
        let bad = answer(json!({"bad": true}), None);
        let good = answer(json!({"bad": false}), None);
        assert_eq!(RequestAdapter::check(&plain, &request(1), &bad), Ok(()));
        assert_eq!(LongJob::check(&plain, &request(1), &bad), Ok(()));

        let checked = plain.clone().with_check(refuse_bad());
        assert_eq!(
            RequestAdapter::check(&checked, &request(1), &bad),
            Err("the answer is bad".into())
        );
        assert_eq!(
            LongJob::check(&checked, &request(1), &bad),
            Err("the answer is bad".into())
        );
        assert_eq!(RequestAdapter::check(&checked, &request(1), &good), Ok(()));

        let custom = plain.with_check(Arc::new(|answer: &Answer| {
            if answer.files.contains_key("image") {
                Ok(())
            } else {
                Err("no image".into())
            }
        }));
        let mut with_image = good.clone();
        with_image.files.insert(
            "image".into(),
            AnsweredFile {
                kind: "image/png".into(),
                bytes: vec![1, 2, 3],
            },
        );
        assert_eq!(
            RequestAdapter::check(&custom, &request(1), &good),
            Err("no image".into())
        );
        assert_eq!(
            RequestAdapter::check(&custom, &request(1), &with_image),
            Ok(())
        );
    }

    #[test]
    fn with_check_keeps_the_shared_script() {
        let fake = FakeAdapter::plain(vec![Sent::Refused { reason: "x".into() }]);
        let checked = fake.clone().with_check(refuse_bad());
        assert_eq!(checked.left(), (1, 0, 0));
        assert!(format!("{checked:?}").contains("check: true"));
        drop(checked);
        assert_eq!(fake.left(), (1, 0, 0));
    }
}
