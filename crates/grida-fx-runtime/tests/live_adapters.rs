//! The retry owner (`calls::retry`) driving real provider adapters over a replay transport: what
//! the ratified rule (spec/protocol.md §6.1 step 7; spec/providers.md §4) bills when an actual
//! adapter classifies actual provider answers, attempt by attempt.
//!
//! The adapters are built exactly as a live run builds them (`<provider>::register` over a
//! `Setup`), but the transport is a `ReplayTransport` (every request asserted, nothing leaves the
//! process) or `NoNetwork` (any send panics); the default transport is never constructed. Holds,
//! job records and pacing are the same fakes as `tests/retry.rs`, on paused time.

use grida_fx_core::money::Usd;
use grida_fx_providers::elevenlabs;
use grida_fx_providers::testing::media::{MP3, MP3_FRAME};
use grida_fx_providers::testing::{setup, test_keys};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Offline, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::{
    Adapter, Adapters, Answer, BoxFuture, CallRequest, Keys, RequestAdapter, RouteRef,
};
use grida_fx_runtime::calls::engine_check;
use grida_fx_runtime::calls::pacing::{Admission, Slot};
use grida_fx_runtime::calls::retry::{
    Attempts, Backoff, HoldBook, JobBook, MAX_SENDS, Outcome, send_plain,
};
use grida_fx_runtime::engine::Cancel;
use grida_fx_runtime::ledger::{Hold, NotReserved, Scopes};
use grida_fx_runtime::store::StoreError;
use grida_fx_runtime::store::records::JobRecord;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::time::{Duration, Instant};

const INSTANCE: &str = "sfx";
const KEY: &str = "test-elevenlabs-key";

/// The built-in sound route's high price (spec/providers.md §10): what each attempt reserves.
const SOUND_HOLD: Usd = Usd(100_000);
/// The built-in speech route's high price.
const SPEECH_HOLD: Usd = Usd(50_000);

const SOUND_URL: &str = "https://api.elevenlabs.io/v1/sound-generation?output_format=mp3_44100_192";
const SOUND_BODY: &str = r#"{"text":"wooden door opens","model_id":"eleven_text_to_sound_v2","loop":false,"duration_seconds":0.6}"#;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Fakes (as in tests/retry.rs)

/// A hold book that records every reservation and settlement.
#[derive(Default)]
struct Book {
    reserved: Mutex<Vec<(String, Usd)>>,
    /// `(hold, reported, charged)`.
    settled: Mutex<Vec<(String, Option<Usd>, Usd)>>,
}

impl HoldBook for Book {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, NotReserved> {
        lock(&self.reserved).push((node_id.clone(), amount));
        Ok(Hold {
            node_id,
            amount,
            scopes: scopes.iter().map(|(owner, _)| owner.clone()).collect(),
        })
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        let charged = reported.unwrap_or(hold.amount);
        lock(&self.settled).push((hold.node_id, reported, charged));
        charged
    }
}

impl Book {
    fn holds(&self) -> Vec<String> {
        lock(&self.reserved)
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// `(hold, reported)` of every settlement, in order.
    fn settlements(&self) -> Vec<(String, Option<Usd>)> {
        lock(&self.settled)
            .iter()
            .map(|(name, reported, _)| (name.clone(), *reported))
            .collect()
    }

    /// What every settlement charged, added up.
    fn charged(&self) -> Usd {
        Usd(lock(&self.settled).iter().map(|(_, _, c)| c.0).sum())
    }

    /// Every hold reserved was settled exactly once.
    fn assert_all_settled(&self) {
        let mut reserved = self.holds();
        let mut settled: Vec<String> = self.settlements().into_iter().map(|(n, _)| n).collect();
        reserved.sort();
        settled.sort();
        assert_eq!(reserved, settled, "every hold is settled exactly once");
    }
}

/// A job book a plain call must never touch.
#[derive(Default)]
struct Jobs {
    touched: Mutex<u32>,
}

impl JobBook for Jobs {
    fn save_job(&self, _record: &JobRecord) -> Result<(), StoreError> {
        *lock(&self.touched) += 1;
        Ok(())
    }

    fn remove_job(&self, _key: &str) -> Result<(), StoreError> {
        *lock(&self.touched) += 1;
        Ok(())
    }
}

/// An admission that hands out free slots, noting when each admit happened.
#[derive(Default)]
struct Gate {
    admits: Mutex<Vec<(String, Instant)>>,
}

impl Admission for Gate {
    fn admit<'a>(
        &'a self,
        route_id: &'a str,
        _limit: Option<u32>,
        _rpm: Option<u32>,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Option<Slot>> {
        Box::pin(async move {
            lock(&self.admits).push((route_id.to_string(), Instant::now()));
            Some(Slot::free())
        })
    }
}

impl Gate {
    fn count(&self) -> usize {
        lock(&self.admits).len()
    }

    /// The waits between consecutive admits (a replayed send takes no time).
    fn waits(&self) -> Vec<Duration> {
        let admits = lock(&self.admits);
        admits.windows(2).map(|w| w[1].1 - w[0].1).collect()
    }
}

fn secs(waits: &[f64]) -> Vec<Duration> {
    waits.iter().map(|s| Duration::from_secs_f64(*s)).collect()
}

/// Everything one call's attempts borrow.
struct Rig {
    book: Book,
    jobs: Jobs,
    gate: Gate,
    cancel: Cancel,
    scopes: Scopes,
    namer: Box<dyn Fn() -> String + Sync>,
}

impl Rig {
    fn new() -> Rig {
        let n = Arc::new(AtomicU64::new(0));
        Rig {
            book: Book::default(),
            jobs: Jobs::default(),
            gate: Gate::default(),
            cancel: Cancel::new(),
            scopes: vec![("scene['door']".into(), Usd(10_000_000))],
            namer: Box::new(move || {
                let n = n.fetch_add(1, Ordering::SeqCst) + 1;
                format!("{INSTANCE}/inv.{n}")
            }),
        }
    }

    fn attempts(&self, call: CallRequest, hold: Usd) -> Attempts<'_> {
        let capability = call.route.capability.clone();
        Attempts {
            call,
            hold,
            scopes: &self.scopes,
            hold_name: &*self.namer,
            limit: None,
            rpm: None,
            book: &self.book,
            jobs: &self.jobs,
            pacing: &self.gate,
            cancel: &self.cancel,
            backoff: Backoff::default(),
            job: None,
            check: engine_check(&capability),
        }
    }

    /// No job record was written or removed: a plain call keeps none.
    fn assert_no_job_record(&self) {
        assert_eq!(
            *lock(&self.jobs.touched),
            0,
            "a plain call keeps no job record"
        );
    }
}

fn hold_name(n: u32) -> String {
    format!("{INSTANCE}/inv.{n}")
}

// ---------------------------------------------------------------------------------------------
// The real adapters

fn route(capability: &str, model: &str, adapter: &str) -> RouteRef {
    RouteRef {
        capability: capability.into(),
        model: model.into(),
        provider: "elevenlabs".into(),
        contract: json!({"adapter": adapter, "adapter_behavior": 1}),
    }
}

fn call(route: RouteRef, request: Value) -> CallRequest {
    CallRequest {
        route,
        request,
        files: IndexMap::new(),
        take: vec![1],
        key: "c".repeat(64),
        attempt: 1,
    }
}

fn sound_call() -> CallRequest {
    call(
        route(
            "sound.generate",
            "eleven_text_to_sound_v2",
            "elevenlabs-sound-effect",
        ),
        json!({"prompt": "wooden door opens", "duration": 0.6, "loop": false}),
    )
}

/// The adapter a live run registers for `call`'s route, over `transport` with `keys`.
fn adapter_for(
    call: &CallRequest,
    transport: Arc<dyn Transport>,
    keys: Keys,
) -> Arc<dyn RequestAdapter> {
    let mut adapters = Adapters::new();
    elevenlabs::register(&mut adapters, &setup(transport, keys));
    match adapters.serving(&call.route) {
        Some(Adapter::Request(adapter)) => Arc::clone(adapter),
        other => panic!("no request adapter serves {}: {other:?}", call.route.id()),
    }
}

fn sound_request() -> Expect {
    Expect::new(Method::Post, SOUND_URL, Lane::Provider)
        .credential("xi-api-key", KEY)
        .header("content-type", "application/json")
        .header("accept", "audio/mpeg")
        .body(ExpectBody::JsonText(SOUND_BODY.into()))
}

fn mp3(bytes: &[u8]) -> HttpResponse {
    HttpResponse::new(200, bytes.to_vec()).with_header("content-type", "audio/mpeg")
}

fn sound_answer(bytes: &[u8]) -> Answer {
    Answer::new(Value::Null, None).with_file("audio", "audio/mpeg", bytes.to_vec())
}

/// Plays `exchanges` through the real sound adapter and the retry owner.
async fn play_sound(rig: &Rig, exchanges: Vec<Exchange>) -> (Outcome, Arc<ReplayTransport>) {
    let transport = Arc::new(ReplayTransport::new(exchanges));
    let call = sound_call();
    let adapter = adapter_for(&call, transport.clone(), test_keys());
    let outcome = send_plain(adapter.as_ref(), &rig.attempts(call, SOUND_HOLD)).await;
    transport.assert_done();
    (outcome, transport)
}

/// What the call reported never holds the key or the provider's message.
fn assert_clean(outcome: &Outcome) {
    let shown = format!("{outcome:?}");
    assert!(!shown.contains(KEY), "the key leaked: {shown}");
    assert!(
        !shown.contains("secret"),
        "a provider message leaked: {shown}"
    );
}

// ---------------------------------------------------------------------------------------------
// The ratified rule, on the ElevenLabs sound route

#[tokio::test(start_paused = true)]
async fn a_429_then_a_good_answer_is_one_attempt_and_one_hold_settled_in_full() {
    let rig = Rig::new();
    let (outcome, transport) = play_sound(
        &rig,
        vec![
            sound_request().reply(HttpResponse::json(
                429,
                &json!({"detail": {"status": "system_busy", "message": "secret quota"}}),
            )),
            sound_request().reply(mp3(MP3_FRAME).with_header("character-cost", "11")),
        ],
    )
    .await;
    assert_clean(&outcome);
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: sound_answer(MP3_FRAME),
            charged: SOUND_HOLD,
            attempts: 1,
        }
    );
    assert_eq!(
        rig.book.holds(),
        vec![hold_name(1)],
        "one hold for both sends"
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
    assert_eq!(rig.book.charged(), Usd(100_000), "$0.10, the whole hold");
    assert_eq!(transport.requests().len(), 2, "the same request, resent");
    assert_eq!(rig.gate.count(), 2, "every send is admitted");
    assert_eq!(rig.gate.waits(), secs(&[0.5]));
    rig.assert_no_job_record();
}

#[tokio::test(start_paused = true)]
async fn bad_bytes_then_a_good_answer_are_two_billed_attempts() {
    let rig = Rig::new();
    let (outcome, _) = play_sound(
        &rig,
        vec![
            sound_request().reply(mp3(b"not audio")),
            sound_request().reply(mp3(MP3)),
        ],
    )
    .await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: sound_answer(MP3),
            charged: SOUND_HOLD,
            attempts: 2,
        }
    );
    assert_eq!(rig.book.holds(), vec![hold_name(1), hold_name(2)]);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), None), (hold_name(2), None)],
        "the refused answer was billed in full"
    );
    assert_eq!(rig.book.charged(), Usd(200_000), "$0.10 + $0.10");
    assert_eq!(rig.gate.waits(), secs(&[0.5]));
    rig.assert_no_job_record();
}

#[tokio::test(start_paused = true)]
async fn an_empty_answer_and_a_wrong_kind_are_billed_attempts_too() {
    let rig = Rig::new();
    let (outcome, _) = play_sound(
        &rig,
        vec![
            sound_request().reply(mp3(b"")),
            sound_request().reply(
                HttpResponse::new(200, MP3.to_vec()).with_header("content-type", "audio/wav"),
            ),
            sound_request().reply(HttpResponse::new(200, MP3.to_vec())),
        ],
    )
    .await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: sound_answer(MP3),
            charged: SOUND_HOLD,
            attempts: 3,
        }
    );
    assert_eq!(rig.book.charged(), Usd(300_000));
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn a_401_is_one_billed_attempt_and_the_call_fails() {
    let rig = Rig::new();
    let (outcome, transport) = play_sound(
        &rig,
        vec![sound_request().reply(HttpResponse::json(
            401,
            &json!({"detail": {"status": "invalid_api_key", "message": "secret test-elevenlabs-key"}}),
        ))],
    )
    .await;
    assert_clean(&outcome);
    assert_eq!(
        outcome,
        Outcome::Failed("ElevenLabs sound generation returned HTTP 401: invalid_api_key".into())
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
    assert_eq!(rig.book.charged(), SOUND_HOLD, "not documented as unbilled");
    assert_eq!(
        transport.requests().len(),
        1,
        "a deterministic refusal is not resent"
    );
    assert!(rig.gate.waits().is_empty());
}

#[tokio::test(start_paused = true)]
async fn six_5xx_answers_are_six_settled_holds_and_the_call_fails() {
    let rig = Rig::new();
    let exchanges = (0..MAX_SENDS)
        .map(|_| sound_request().reply(HttpResponse::new(500, b"internal error".to_vec())))
        .collect();
    let (outcome, transport) = play_sound(&rig, exchanges).await;
    assert_eq!(
        outcome,
        Outcome::Failed("ElevenLabs sound generation returned HTTP 500".into())
    );
    assert_eq!(transport.requests().len(), MAX_SENDS as usize);
    assert_eq!(rig.book.holds(), (1..=6).map(hold_name).collect::<Vec<_>>());
    assert_eq!(
        rig.book.settlements(),
        (1..=6).map(|n| (hold_name(n), None)).collect::<Vec<_>>()
    );
    assert_eq!(rig.book.charged(), Usd(600_000));
    rig.book.assert_all_settled();
    assert_eq!(rig.gate.waits(), secs(&[0.5, 1.0, 2.0, 4.0, 8.0]));
}

#[tokio::test(start_paused = true)]
async fn six_429s_end_the_call_under_one_hold_at_no_cost() {
    let rig = Rig::new();
    let exchanges = (0..MAX_SENDS)
        .map(|_| {
            sound_request()
                .reply(HttpResponse::new(429, Vec::new()).with_header("retry-after", "1"))
        })
        .collect();
    let (outcome, transport) = play_sound(&rig, exchanges).await;
    assert_eq!(
        outcome,
        Outcome::Failed("ElevenLabs sound generation returned HTTP 429; retry-after 1".into())
    );
    assert_eq!(transport.requests().len(), MAX_SENDS as usize);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(rig.book.charged(), Usd::ZERO);
}

#[tokio::test(start_paused = true)]
async fn an_unsent_request_is_resent_and_a_lost_answer_is_billed() {
    let rig = Rig::new();
    let (outcome, _) = play_sound(
        &rig,
        vec![
            sound_request().fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "could not connect to https://api.elevenlabs.io",
            )),
            sound_request().fail(TransportError::after_send(
                TransportErrorKind::Timeout,
                "the deadline passed while reading",
            )),
            sound_request().reply(mp3(MP3)),
        ],
    )
    .await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: sound_answer(MP3),
            charged: SOUND_HOLD,
            attempts: 2,
        }
    );
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), None), (hold_name(2), None)],
        "the unsent send rode on the first hold, which the lost answer then billed"
    );
    assert_eq!(rig.gate.waits(), secs(&[0.5, 1.0]));
}

#[tokio::test(start_paused = true)]
async fn a_missing_key_is_capability_refused_at_no_cost_and_sends_nothing() {
    let rig = Rig::new();
    let call = sound_call();
    let adapter = adapter_for(&call, Arc::new(NoNetwork), Keys::none());
    let outcome = send_plain(adapter.as_ref(), &rig.attempts(call, SOUND_HOLD)).await;
    assert_eq!(
        outcome,
        Outcome::Refused("ELEVENLABS_API_KEY is not set".into())
    );
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(rig.book.charged(), Usd::ZERO);
    assert_eq!(
        rig.gate.count(),
        1,
        "refused on the first send, never retried"
    );
}

#[tokio::test(start_paused = true)]
async fn the_network_switched_off_is_capability_refused_at_no_cost() {
    let rig = Rig::new();
    let call = sound_call();
    let adapter = adapter_for(&call, Arc::new(Offline), test_keys());
    let outcome = send_plain(adapter.as_ref(), &rig.attempts(call, SOUND_HOLD)).await;
    assert_eq!(
        outcome,
        Outcome::Refused(
            "ElevenLabs sound generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
                .into()
        )
    );
    assert_eq!(rig.book.charged(), Usd::ZERO);
}

// ---------------------------------------------------------------------------------------------
// The speech route

fn speech_call(request: Value) -> CallRequest {
    call(
        route("speech.generate", "eleven_v3", "elevenlabs-speech"),
        request,
    )
}

#[tokio::test(start_paused = true)]
async fn a_speech_line_is_answered_and_settled_at_its_own_hold() {
    let rig = Rig::new();
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::new(
            Method::Post,
            "https://api.elevenlabs.io/v1/text-to-speech/voice-7?output_format=mp3_44100_192",
            Lane::Provider,
        )
        .credential("xi-api-key", KEY)
        .body(ExpectBody::JsonText(
            r#"{"text":"[calm] the lantern is lit","model_id":"eleven_v3","voice_settings":{"stability":0.5}}"#
                .into(),
        ))
        .reply(mp3(MP3)),
    ]));
    let call = speech_call(json!({
        "text": "[calm] the lantern is lit",
        "voice": "voice-7",
        "stability": 0.5,
        "language_code": null,
        "max_chars": 120,
    }));
    let adapter = adapter_for(&call, transport.clone(), test_keys());
    let outcome = send_plain(adapter.as_ref(), &rig.attempts(call, SPEECH_HOLD)).await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: sound_answer(MP3),
            charged: SPEECH_HOLD,
            attempts: 1,
        }
    );
    transport.assert_done();
}

#[tokio::test(start_paused = true)]
async fn a_speech_value_out_of_range_is_refused_at_no_cost() {
    let rig = Rig::new();
    let call = speech_call(json!({"text": "hello", "voice": "voices/../7"}));
    let adapter = adapter_for(&call, Arc::new(NoNetwork), test_keys());
    let outcome = send_plain(adapter.as_ref(), &rig.attempts(call, SPEECH_HOLD)).await;
    assert_eq!(
        outcome,
        Outcome::Refused("the provider voice is not a voice id".into())
    );
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}
