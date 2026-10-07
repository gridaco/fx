//! Dispatching one instance: its events, its engine retries, its files (spec/protocol.md §4
//! "Retry", §5.3; spec/store.md §8).
//!
//! 1. emit the `node_started` event the scheduler built (`id, path, step, take, identity, uses,
//!    reads, routes, with`);
//! 2. make attempts with `executor::execute`: one, or under `retry: engine` up to six in all while
//!    an attempt is `retryable` (a `node_error`), emitting `node_retry {id, attempt, error}` (the
//!    failed attempt's number from 1, its error cut to 500 characters) before each new one. Calls
//!    an earlier attempt completed are answered by the call cache;
//! 3. a pick (`InstanceJob::picked`): a succeeded take whose first output file is not the picked
//!    digest fails with `take <n> of <path> no longer produces the picked result <digest>; pick a
//!    take again`. The first output file is the one `grida-fx pick` records: the value of the
//!    first port, in canonical member order, whose value is one file;
//! 4. succeeded: place each output file under `files/` (`folder::step_files`, `RunFolder::place`:
//!    the step's folder names its take when it has more than one) and emit `node_finished {id,
//!    path, cache, outputs: encoded, facts, duration_ms}`;
//!    skipped (a select with no candidate): `node_skipped {id, path, error, facts, duration_ms}`;
//!    failed: `node_failed {id, path, error, code, facts, duration_ms}`, `code` the attempt's
//!    (`executor::Attempt::code`; `null` for a pick that no longer holds). `duration_ms` covers every
//!    attempt.
//! 5. report [`Done`] to the loop. An attempt's `stop` is passed on; the loop stops the run.
//!
//! When `cancel` fires (the run is stopped, or the scheduler stops this one instance) the attempt
//! in flight is stopped by the executor; the dispatcher emits no terminal event for it (the run
//! emits `run_cancelled`, or the scheduler the instance's own), and starts no new attempt. An attempt
//! that succeeded (or a select that skipped) before it noticed is kept as usual: only a failed
//! attempt under a fired `cancel` counts as cancelled.
//!
//! Error texts are scrubbed of the engine's private paths ([`crate::engine::Engine::scrub`])
//! before they reach an event or the loop. A file that cannot be placed, or an event that cannot
//! be written, stops the run with a sentence (`cannot place <folder>/<path>: <reason>`, `cannot
//! write <folder>/events.jsonl: <reason>`); the instance then gets no terminal event.

use crate::engine::{Cancel, Services};
use crate::events::Event;
use crate::executor::{Attempt, CacheUse, InstanceJob};
use crate::folder::RunFolder;
use grida_fx_core::expand::{NodeResult, ResultStatus};
use grida_fx_core::spec::Retry;
use grida_fx_core::val::Val;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

/// The most runs of one body under `retry: engine` (spec/protocol.md §4 "Retry").
pub const MAX_RUNS: u32 = 6;

/// How much of a failed attempt's error a `node_retry` event keeps, in characters.
const RETRY_ERROR_CHARS: usize = 500;

/// What a dispatch reports to the loop.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub id: String,
    pub result: NodeResult,
    /// The run must stop, for this reason.
    pub stop: Option<String>,
    /// Cancelled before it finished: no result.
    pub cancelled: bool,
}

impl Done {
    fn cancelled(id: &str) -> Done {
        Done {
            id: id.to_string(),
            result: crate::executor::failed("cancelled"),
            stop: None,
            cancelled: true,
        }
    }

    fn stopped(id: &str, result: NodeResult, stop: String) -> Done {
        Done {
            id: id.to_string(),
            result,
            stop: Some(stop),
            cancelled: false,
        }
    }
}

/// Runs one instance to its end (module doc).
pub async fn dispatch(
    services: Arc<Services>,
    job: Arc<InstanceJob>,
    started: Event,
    folder: Arc<RunFolder>,
    cancel: Cancel,
) -> Done {
    let emit = |event: &Event| -> Result<(), String> {
        match &services.events {
            Some(log) => log.emit(event).map_err(|error| {
                format!(
                    "cannot write {}: {}",
                    super::in_folder(&folder.label, "events.jsonl"),
                    grida_fx_core::error::io_reason(&error)
                )
            }),
            None => Ok(()),
        }
    };
    let scrub = |text: &str| services.engine.scrub(text);
    let display = started.node_display();
    if let Err(stop) = emit(&started) {
        return Done::stopped(&job.id, crate::executor::failed(&stop), stop);
    }
    let begin = Instant::now();
    let attempt = match attempts(
        &job,
        &cancel,
        || crate::executor::execute(Arc::clone(&services), Arc::clone(&job), cancel.child()),
        &emit,
        &scrub,
    )
    .await
    {
        Attempted::Made(attempt) => attempt,
        Attempted::Cancelled => return Done::cancelled(&job.id),
        Attempted::Stopped(stop) => {
            return Done::stopped(&job.id, crate::executor::failed(&stop), stop);
        }
    };
    let duration_ms = u64::try_from(begin.elapsed().as_millis()).unwrap_or(u64::MAX);
    let Attempt {
        result,
        cache,
        stop,
        code,
        ..
    } = attempt;
    let mut code = code;
    let mut result = scrubbed(result, &scrub);
    if let Some(error) = picked_mismatch(&job, &result) {
        code = None;
        result = NodeResult {
            status: ResultStatus::Failed,
            outputs: Default::default(),
            facts: result.facts,
            error: Some(error),
        };
    }
    let terminal = match result.status {
        ResultStatus::Succeeded => {
            let placed = crate::folder::step_files(&job.path, &job.takes, &result.outputs);
            for (relative, digest) in placed {
                let placed = super::place_file(&services.engine.store, &folder, &relative, &digest);
                if let Err(sentence) = placed {
                    return Done::stopped(&job.id, result, sentence);
                }
            }
            Event::NodeFinished {
                id: job.id.clone(),
                path: job.path.clone(),
                cache_hit: cache == CacheUse::Hit,
                outputs: result
                    .outputs
                    .iter()
                    .map(|(port, value)| (port.clone(), crate::events::encode(value)))
                    .collect(),
                facts: result.facts.clone(),
                duration_ms,
            }
        }
        ResultStatus::Skipped => Event::NodeSkipped {
            id: job.id.clone(),
            path: job.path.clone(),
            reason: None,
            blocked: false,
            error: result.error.clone(),
            facts: Some(result.facts.clone()),
            duration_ms: Some(duration_ms),
            display,
        },
        ResultStatus::Failed => Event::NodeFailed {
            id: job.id.clone(),
            path: job.path.clone(),
            error: result.error.clone(),
            code: code.map(|code| code.name().to_string()),
            facts: Some(result.facts.clone()),
            duration_ms: Some(duration_ms),
            display,
        },
    };
    if let Err(sentence) = emit(&terminal) {
        return Done::stopped(&job.id, result, sentence);
    }
    Done {
        id: job.id.clone(),
        result,
        stop: stop.map(|text| scrub(&text)),
        cancelled: false,
    }
}

/// How the attempts of one dispatch ended.
#[derive(Debug, PartialEq)]
enum Attempted {
    /// The last attempt made.
    Made(Attempt),
    /// `cancel` fired: no result.
    Cancelled,
    /// A `node_retry` event could not be written.
    Stopped(String),
}

/// Makes the attempts of `job` (module doc, step 2). `execute` makes one attempt; `emit` writes an
/// event; `scrub` cleans an error text.
async fn attempts<F, Fut>(
    job: &InstanceJob,
    cancel: &Cancel,
    mut execute: F,
    emit: &(dyn Fn(&Event) -> Result<(), String> + Send + Sync),
    scrub: &(dyn Fn(&str) -> String + Send + Sync),
) -> Attempted
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Attempt>,
{
    let runs = if job.retry == Retry::Engine {
        MAX_RUNS
    } else {
        1
    };
    let mut number = 1;
    loop {
        if cancel.is_cancelled() {
            return Attempted::Cancelled;
        }
        let attempt = execute().await;
        let failed = attempt.result.status == ResultStatus::Failed;
        if failed && cancel.is_cancelled() {
            return Attempted::Cancelled;
        }
        if !(failed && attempt.retryable && attempt.stop.is_none() && number < runs) {
            return Attempted::Made(attempt);
        }
        let error = scrub(attempt.result.error.as_deref().unwrap_or_default());
        let retry = Event::NodeRetry {
            id: job.id.clone(),
            attempt: number,
            error: error.chars().take(RETRY_ERROR_CHARS).collect(),
        };
        if let Err(stop) = emit(&retry) {
            return Attempted::Stopped(stop);
        }
        number += 1;
    }
}

/// A result with its error scrubbed.
fn scrubbed(mut result: NodeResult, scrub: &dyn Fn(&str) -> String) -> NodeResult {
    result.error = result.error.map(|error| scrub(&error));
    result
}

/// The pick check (module doc, step 3): the error when a succeeded take of a picked instance no
/// longer produces the picked result. `pick` reads the take's `node_finished` outputs, whose
/// members are in canonical order (UTF-16 code units, spec/identity.md §2), so the ports are
/// compared in that order here, never in declaration order.
fn picked_mismatch(job: &InstanceJob, result: &NodeResult) -> Option<String> {
    let picked = job.picked.as_deref()?;
    if result.status != ResultStatus::Succeeded {
        return None;
    }
    let first = result
        .outputs
        .iter()
        .filter_map(|(port, value)| match value {
            Val::File(file) => Some((port, file.digest.as_str())),
            _ => None,
        })
        .min_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()))
        .map(|(_, digest)| digest);
    if first == Some(picked) {
        return None;
    }
    let take = job.takes.last().copied().unwrap_or(1);
    Some(format!(
        "take {take} of {} no longer produces the picked result {picked}; pick a take again",
        job.path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ReadSet;
    use grida_fx_core::spec::{NodeSpec, Port};
    use grida_fx_core::val::FileValue;
    use indexmap::IndexMap;
    use std::sync::Mutex;

    fn spec(outputs: &[&str], retry: Retry) -> NodeSpec {
        NodeSpec {
            name: "draw".into(),
            description: None,
            inputs: IndexMap::new(),
            params: IndexMap::new(),
            outputs: outputs
                .iter()
                .map(|name| (name.to_string(), Port::parse("image").unwrap()))
                .collect(),
            judge: false,
            capability: None,
            calls: IndexMap::new(),
            resources: Vec::new(),
            tools: Vec::new(),
            view: None,
            version: Some(1),
            retry,
        }
    }

    fn job(retry: Retry, outputs: &[&str], picked: Option<&str>) -> InstanceJob {
        InstanceJob {
            id: "draw['a']#1.2".into(),
            path: "draw['a']".into(),
            step: "draw".into(),
            key: Some("a".into()),
            takes: vec![1, 2],
            uses: "./nodes/draw.py#draw".into(),
            type_identity: "0".repeat(64),
            spec: Arc::new(spec(outputs, retry)),
            body: crate::executor::JobBody::Project {
                path: "nodes/draw.py".into(),
                attribute: "draw".into(),
            },
            with: IndexMap::new(),
            identity: None,
            read: ReadSet::new(),
            calls: IndexMap::new(),
            routes: IndexMap::new(),
            limits: IndexMap::new(),
            scopes: Vec::new(),
            resources: IndexMap::new(),
            tools: IndexMap::new(),
            timeout_s: None,
            retry,
            picked: picked.map(str::to_string),
        }
    }

    fn attempt(status: ResultStatus, error: Option<&str>, retryable: bool) -> Attempt {
        Attempt {
            result: NodeResult {
                status,
                outputs: IndexMap::new(),
                facts: IndexMap::new(),
                error: error.map(str::to_string),
            },
            cache: CacheUse::Miss,
            retryable,
            stop: None,
            code: None,
        }
    }

    fn node_error(text: &str) -> Attempt {
        attempt(ResultStatus::Failed, Some(text), true)
    }

    /// Runs `attempts` over a script of attempts; returns what it ended with, the events it
    /// emitted and how many attempts it made.
    fn run_script(
        job: &InstanceJob,
        cancel: &Cancel,
        script: Vec<Attempt>,
        cancel_after: Option<usize>,
    ) -> (Attempted, Vec<Event>, usize) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let script = Mutex::new(script.into_iter());
        let made = Mutex::new(0usize);
        let events = Mutex::new(Vec::new());
        let emit = |event: &Event| -> Result<(), String> {
            events.lock().unwrap().push(event.clone());
            Ok(())
        };
        let scrub = |text: &str| text.replace("/private/acme/", "");
        let ended = runtime.block_on(attempts(
            job,
            cancel,
            || {
                let mut made = made.lock().unwrap();
                *made += 1;
                if cancel_after == Some(*made) {
                    cancel.cancel();
                }
                let next = script.lock().unwrap().next().expect("no attempt left");
                async move { next }
            },
            &emit,
            &scrub,
        ));
        let made = *made.lock().unwrap();
        (ended, events.into_inner().unwrap(), made)
    }

    #[test]
    fn retry_engine_runs_a_node_error_at_most_six_times() {
        let job = job(Retry::Engine, &["image"], None);
        let script: Vec<Attempt> = (1..=7)
            .map(|n| node_error(&format!("RuntimeError: kaboom {n}")))
            .collect();
        let (ended, events, made) = run_script(&job, &Cancel::new(), script, None);
        assert_eq!(made, 6);
        assert_eq!(ended, Attempted::Made(node_error("RuntimeError: kaboom 6")));
        let expected: Vec<Event> = (1..=5)
            .map(|n| Event::NodeRetry {
                id: "draw['a']#1.2".into(),
                attempt: n,
                error: format!("RuntimeError: kaboom {n}"),
            })
            .collect();
        assert_eq!(events, expected);
    }

    #[test]
    fn a_later_success_ends_the_retries() {
        let job = job(Retry::Engine, &["image"], None);
        let script = vec![
            node_error("OSError: busy"),
            attempt(ResultStatus::Succeeded, None, false),
            node_error("never made"),
        ];
        let (ended, events, made) = run_script(&job, &Cancel::new(), script, None);
        assert_eq!(made, 2);
        assert_eq!(
            ended,
            Attempted::Made(attempt(ResultStatus::Succeeded, None, false))
        );
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn only_retryable_failures_under_retry_engine_run_again() {
        // retry: service runs once, even after a node_error.
        let (ended, events, made) = run_script(
            &job(Retry::Service, &["image"], None),
            &Cancel::new(),
            vec![node_error("x"), node_error("y")],
            None,
        );
        assert_eq!((made, events.len()), (1, 0));
        assert_eq!(ended, Attempted::Made(node_error("x")));
        // A node_failure, a timeout or an engine error is not retryable.
        let refused = attempt(ResultStatus::Failed, Some("ran past 1.5 seconds"), false);
        let (ended, events, made) = run_script(
            &job(Retry::Engine, &["image"], None),
            &Cancel::new(),
            vec![refused.clone(), node_error("y")],
            None,
        );
        assert_eq!((made, events.len()), (1, 0));
        assert_eq!(ended, Attempted::Made(refused));
        // A stop is passed on at once.
        let mut stopping = node_error("z");
        stopping.stop = Some("the job record jobs/k.json is unreadable: bad".into());
        let (ended, _, made) = run_script(
            &job(Retry::Engine, &["image"], None),
            &Cancel::new(),
            vec![stopping.clone(), node_error("y")],
            None,
        );
        assert_eq!(made, 1);
        assert_eq!(ended, Attempted::Made(stopping));
    }

    #[test]
    fn retry_errors_are_scrubbed_and_cut_to_500_characters() {
        let long = format!("ValueError: /private/acme/nodes/x.py {}", "é".repeat(600));
        let (_, events, _) = run_script(
            &job(Retry::Engine, &["image"], None),
            &Cancel::new(),
            vec![
                node_error(&long),
                attempt(ResultStatus::Succeeded, None, false),
            ],
            None,
        );
        let Event::NodeRetry { error, attempt, .. } = &events[0] else {
            panic!("{events:?}");
        };
        assert_eq!(*attempt, 1);
        assert_eq!(error.chars().count(), 500);
        assert!(error.starts_with("ValueError: nodes/x.py éé"), "{error}");
    }

    #[test]
    fn cancelling_stops_the_attempts_without_a_result() {
        // Cancelled before anything started.
        let cancel = Cancel::new();
        cancel.cancel();
        let (ended, _, made) = run_script(
            &job(Retry::Engine, &["image"], None),
            &cancel,
            vec![node_error("x")],
            None,
        );
        assert_eq!((ended, made), (Attempted::Cancelled, 0));
        // A failure while cancelled is the cancellation, never retried.
        let (ended, events, made) = run_script(
            &job(Retry::Engine, &["image"], None),
            &Cancel::new(),
            vec![node_error("x"), node_error("y")],
            Some(1),
        );
        assert_eq!((ended, made, events.len()), (Attempted::Cancelled, 1, 0));
        // A success that finished anyway is kept.
        let (ended, _, _) = run_script(
            &job(Retry::Engine, &["image"], None),
            &Cancel::new(),
            vec![attempt(ResultStatus::Succeeded, None, false)],
            Some(1),
        );
        assert_eq!(
            ended,
            Attempted::Made(attempt(ResultStatus::Succeeded, None, false))
        );
    }

    #[test]
    fn a_failed_retry_event_stops_the_attempts() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let job = job(Retry::Engine, &["image"], None);
        let emit = |_: &Event| -> Result<(), String> {
            Err("cannot write runs/one/events.jsonl: full".into())
        };
        let scrub = |text: &str| text.to_string();
        let ended = runtime.block_on(attempts(
            &job,
            &Cancel::new(),
            || async { node_error("x") },
            &emit,
            &scrub,
        ));
        assert_eq!(
            ended,
            Attempted::Stopped("cannot write runs/one/events.jsonl: full".into())
        );
    }

    fn file(digest: &str) -> Val {
        Val::File(Box::new(FileValue {
            digest: digest.into(),
            kind: "image/png".into(),
            name: "draw/image".into(),
            size: 70,
            key: None,
            content: None,
            location: None,
        }))
    }

    fn succeeded(outputs: &[(&str, Val)]) -> NodeResult {
        NodeResult {
            status: ResultStatus::Succeeded,
            outputs: outputs
                .iter()
                .map(|(port, value)| (port.to_string(), value.clone()))
                .collect(),
            facts: IndexMap::new(),
            error: None,
        }
    }

    #[test]
    fn a_pick_checks_the_first_one_file_port_in_canonical_order() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        // Declared `sheet, mask, image`; `pick` read the log, where `image` comes first and
        // `sheet` holds a list, so `image` is the port it recorded.
        let picked = job(Retry::Service, &["sheet", "mask", "image"], Some(&a));
        let result = succeeded(&[
            ("sheet", Val::List(vec![file(&b)])),
            ("mask", file(&b)),
            ("image", file(&a)),
        ]);
        assert_eq!(picked_mismatch(&picked, &result), None);
        let result = succeeded(&[("mask", file(&a)), ("image", file(&b))]);
        assert_eq!(
            picked_mismatch(&picked, &result).as_deref(),
            Some(
                format!(
                    "take 2 of draw['a'] no longer produces the picked result {a}; pick a take \
                     again"
                )
                .as_str()
            )
        );
        // No port with one file: it no longer produces the picked result either.
        let result = succeeded(&[("sheet", Val::List(vec![file(&a)]))]);
        assert!(picked_mismatch(&picked, &result).is_some());
        // Not a pick, or not a success: nothing to check.
        let plain = job(Retry::Service, &["image"], None);
        assert_eq!(
            picked_mismatch(&plain, &succeeded(&[("image", file(&b))])),
            None
        );
        let failed = crate::executor::failed("x");
        assert_eq!(picked_mismatch(&picked, &failed), None);
    }
}
