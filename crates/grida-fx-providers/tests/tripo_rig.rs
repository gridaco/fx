//! Tripo `mesh.rig` over synthetic exchanges (spec/providers.md §7, §9.4;
//! spec/capabilities.md §8). Nothing here reaches the network: every exchange is scripted on a
//! `ReplayTransport` (which panics on any request it was not given) or the setup is built over
//! `NoNetwork`; time is a `FakeClock`. The key is the made-up `test-tripo-key`.

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Collected, LongJob, Submitted};
use grida_fx_providers::clock::{Clock, FakeClock};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::testing::{
    CallBuilder, TestCall, block_on, media, setup_with_clock, test_keys,
};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Part, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::tripo::rig::{CHECK_DEADLINE, TripoRig};
use grida_fx_providers::tripo::{POLL, TripoApi};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "test-tripo-key";
const BEARER: &str = "Bearer test-tripo-key";
const API: &str = "https://openapi.tripo3d.ai/v3";
const STORAGE: &str = "https://tripo-data.rg1.data.tripo3d.com/out/rigged.glb";
const ROUTE: &str = "v1.0-20240301@tripo";
const RIG_BODY: &str = r#"{"input":"tok1","model":"v1.0-20240301","rig_type":"biped","spec":"mixamo","out_format":"glb"}"#;

// ---------------------------------------------------------------------------------------------
// Harness

struct Harness {
    transport: Arc<ReplayTransport>,
    clock: Arc<FakeClock>,
    rig: TripoRig,
}

fn harness_with(exchanges: Vec<Exchange>, clock: FakeClock) -> Harness {
    let transport = Arc::new(ReplayTransport::new(exchanges));
    let clock = Arc::new(clock);
    let setup = setup_with_clock(
        Arc::clone(&transport) as Arc<dyn Transport>,
        test_keys(),
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    Harness {
        transport,
        clock,
        rig: TripoRig::new(TripoApi::new(&setup)),
    }
}

fn harness(exchanges: Vec<Exchange>) -> Harness {
    harness_with(exchanges, FakeClock::new())
}

fn offline_rig(keys: Keys) -> TripoRig {
    let setup = setup_with_clock(Arc::new(NoNetwork), keys, Arc::new(FakeClock::new()));
    TripoRig::new(TripoApi::new(&setup))
}

impl Harness {
    fn submit(&self, call: &TestCall) -> Submitted {
        let outcome = block_on(self.rig.submit(call));
        assert_clean(&format!("{outcome:?}"));
        outcome
    }

    fn collect(&self, call: &TestCall, handle: Value) -> Collected {
        let outcome = block_on(self.rig.collect(call, &handle));
        if let Collected::Answered(answer) = &outcome {
            assert_clean(&answer.data.to_string());
        } else {
            assert_clean(&format!("{outcome:?}"));
        }
        outcome
    }

    /// Every scripted exchange was used; API requests carry the key, downloads carry none.
    fn done(&self) {
        self.transport.assert_done();
        for request in self.transport.requests() {
            match request.lane {
                Lane::Download => assert!(request.credential.is_none(), "{}", request.url),
                Lane::Provider | Lane::Upload => {
                    let credential = request.credential.as_ref().expect("an API credential");
                    assert_eq!(credential.header, "authorization");
                    assert_eq!(credential.header_value(), BEARER);
                    assert!(request.url.starts_with(API), "{}", request.url);
                }
            }
        }
    }

    /// No request reached the paid rig endpoint.
    fn assert_no_rig_post(&self) {
        assert!(
            self.transport
                .requests()
                .iter()
                .all(|r| !r.url.ends_with("/animations/rig")),
            "the paid rig was posted"
        );
    }
}

fn assert_clean(shown: &str) {
    assert!(!shown.contains(KEY), "the key leaked: {shown}");
}

fn ok(data: Value) -> HttpResponse {
    HttpResponse::json(200, &json!({"code": 0, "data": data}))
}

fn upload_expect() -> Expect {
    Expect::new(Method::Post, format!("{API}/files"), Lane::Upload)
        .credential("authorization", BEARER)
        .body(ExpectBody::Multipart(vec![Part::file(
            "file",
            "unrigged.glb",
            "model/gltf-binary",
            media::GLB.to_vec(),
        )]))
}

fn upload() -> Exchange {
    upload_expect().reply(ok(json!({"file_token": "tok1"})))
}

fn check_expect() -> Expect {
    Expect::new(
        Method::Post,
        format!("{API}/animations/rig-check"),
        Lane::Provider,
    )
    .credential("authorization", BEARER)
    .body(ExpectBody::JsonText(r#"{"input":"tok1"}"#.into()))
}

fn check_posted() -> Exchange {
    check_expect().reply(ok(json!({"task_id": "task1"})))
}

fn read_expect(task_id: &str) -> Expect {
    Expect::get(format!("{API}/tasks/{task_id}")).credential("authorization", BEARER)
}

fn read(task_id: &str, state: &str, output: Value, credits: Value) -> Exchange {
    read_expect(task_id).reply(ok(json!({"task_id": task_id, "status": state,
                                         "output": output, "credits_consumed": credits})))
}

/// The check task ends with this output.
fn checked(output: Value) -> Exchange {
    read("task1", "success", output, json!(0))
}

fn rig_expect(body: &str) -> Expect {
    Expect::new(
        Method::Post,
        format!("{API}/animations/rig"),
        Lane::Provider,
    )
    .credential("authorization", BEARER)
    .body(ExpectBody::JsonText(body.into()))
}

fn rig_posted(body: &str) -> Exchange {
    rig_expect(body).reply(ok(json!({"task_id": "task2"})))
}

fn download(url: &str, bytes: &[u8]) -> Exchange {
    Expect::download(url).reply(HttpResponse::new(200, bytes.to_vec()))
}

fn not_sent() -> TransportError {
    TransportError::not_sent(TransportErrorKind::Connect, "connection refused")
}

fn after_send() -> TransportError {
    TransportError::after_send(TransportErrorKind::Other, "connection reset")
}

fn builder() -> CallBuilder {
    CallBuilder::new("mesh.rig", ROUTE)
        .contract(json!({"adapter": "tripo-rig", "adapter_behavior": 1}))
}

fn rig_call(extra: Value) -> TestCall {
    let mut b = builder();
    let model = b.file("model/gltf-binary", media::GLB);
    let mut request = json!({"model": model});
    if let (Value::Object(request), Value::Object(extra)) = (&mut request, extra) {
        request.extend(extra);
    }
    b.request(request).build()
}

fn glb(tag: u8) -> Vec<u8> {
    let mut bytes = media::GLB.to_vec();
    bytes.push(tag);
    bytes
}

fn fbx(tag: u8) -> Vec<u8> {
    let mut bytes = media::FBX.to_vec();
    bytes.push(tag);
    bytes
}

fn plain_handle() -> Value {
    json!({"task_id": "task2",
           "check": {"task_id": "task1", "riggable": true, "rig_type": "biped"},
           "advisory_override": false})
}

// ---------------------------------------------------------------------------------------------
// Submit

#[test]
fn a_rig_is_checked_free_then_posted_once() {
    // spec/providers.md §9.4 "Rig" (mirrors stage-gen's "a rig is checked free then posted once").
    let h = harness(vec![
        upload(),
        check_posted(),
        read("task1", "queued", json!({}), json!(0)),
        checked(json!({"riggable": true, "rig_type": "biped"})),
        rig_posted(RIG_BODY),
    ]);
    let outcome = h.submit(&rig_call(json!({})));
    assert_eq!(
        outcome,
        Submitted::Accepted {
            handle: plain_handle()
        }
    );
    assert_eq!(h.clock.sleeps(), [POLL]);
    h.done();
    let requests = h.transport.requests();
    assert_eq!(requests[1].timeout, Duration::from_secs(180));
    assert_eq!(requests[4].timeout, Duration::from_secs(180));

    // The canonical request of character-3d (spec/capabilities.md §8): the same wire.
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true, "rig_type": "biped"})),
        rig_posted(RIG_BODY),
    ]);
    let call = rig_call(json!({"allow_negative_check": true, "rig_type": "biped",
                               "skeleton": "mixamo"}));
    assert_eq!(
        h.submit(&call),
        Submitted::Accepted {
            handle: plain_handle()
        }
    );
    h.done();
}

#[test]
fn a_doubted_model_is_rigged_only_when_allowed() {
    // spec/providers.md §9.4 "Rig" (mirrors stage-gen's "a doubted model is rigged only when
    // allowed").
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": false, "rig_type": "biped"})),
    ]);
    assert_eq!(
        h.submit(&rig_call(json!({}))),
        Submitted::Refused {
            reason: r#"Tripo's check doubts this model (riggable=false, rig_type="biped")"#.into()
        }
    );
    h.assert_no_rig_post();
    h.done();

    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": false, "rig_type": "biped"})),
        rig_posted(RIG_BODY),
    ]);
    assert_eq!(
        h.submit(&rig_call(json!({"allow_negative_check": true}))),
        Submitted::Accepted {
            handle: json!({"task_id": "task2",
                           "check": {"task_id": "task1", "riggable": false, "rig_type": "biped"},
                           "advisory_override": true})
        }
    );
    h.done();

    // A rig type other than the one asked for is doubted.
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true, "rig_type": "quadruped"})),
    ]);
    assert_eq!(
        h.submit(&rig_call(json!({"allow_negative_check": false}))),
        Submitted::Refused {
            reason: r#"Tripo's check doubts this model (riggable=true, rig_type="quadruped")"#
                .into()
        }
    );
    h.done();

    // So is a check without a rig type; it is kept as null.
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true})),
    ]);
    assert_eq!(
        h.submit(&rig_call(json!({}))),
        Submitted::Refused {
            reason: "Tripo's check doubts this model (riggable=true, rig_type=null)".into()
        }
    );
    h.done();
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true})),
        rig_posted(RIG_BODY),
    ]);
    assert_eq!(
        h.submit(&rig_call(json!({"allow_negative_check": true}))),
        Submitted::Accepted {
            handle: json!({"task_id": "task2",
                           "check": {"task_id": "task1", "riggable": true, "rig_type": null},
                           "advisory_override": true})
        }
    );
    h.done();

    // A rig type Tripo answers in an odd shape is withheld from the reason.
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true, "rig_type": "a long sentence from Tripo"})),
    ]);
    assert_eq!(
        h.submit(&rig_call(json!({}))),
        Submitted::Refused {
            reason: "Tripo's check doubts this model (riggable=true, rig_type=(withheld))".into()
        }
    );
    h.done();

    // The request's rig type and skeleton go to the wire; `skeleton` is sent as `spec`.
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true, "rig_type": "quadruped"})),
        rig_posted(
            r#"{"input":"tok1","model":"v1.0-20240301","rig_type":"quadruped","spec":"tripo","out_format":"glb"}"#,
        ),
    ]);
    assert_eq!(
        h.submit(&rig_call(
            json!({"rig_type": "quadruped", "skeleton": "tripo"})
        )),
        Submitted::Accepted {
            handle: json!({"task_id": "task2",
                           "check": {"task_id": "task1", "riggable": true, "rig_type": "quadruped"},
                           "advisory_override": false})
        }
    );
    h.done();

    // Empty text is the default.
    let h = harness(vec![
        upload(),
        check_posted(),
        checked(json!({"riggable": true, "rig_type": "biped"})),
        rig_posted(RIG_BODY),
    ]);
    assert!(matches!(
        h.submit(&rig_call(json!({"rig_type": "", "skeleton": null}))),
        Submitted::Accepted { .. }
    ));
    h.done();
}

#[test]
fn only_a_glb_is_rigged_and_bad_requests_send_nothing() {
    // spec/providers.md §5 (mirrors stage-gen's "only a glb is rigged").
    let refused = |call: TestCall, keys: Keys| match block_on(offline_rig(keys).submit(&call)) {
        Submitted::Refused { reason } => {
            assert_clean(&reason);
            reason
        }
        other => panic!("not refused: {other:?}"),
    };
    let mut b = builder();
    let model = b.file("model/fbx", media::FBX);
    assert_eq!(
        refused(b.request(json!({"model": model})).build(), test_keys()),
        "Tripo rigs a GLB, not model/fbx"
    );
    let unknown = json!({"file": "f".repeat(64)});
    assert_eq!(
        refused(
            builder().request(json!({"model": unknown})).build(),
            test_keys()
        ),
        "a rig call needs its model"
    );
    assert_eq!(
        refused(builder().request(json!({})).build(), test_keys()),
        "mesh.rig needs model"
    );
    assert_eq!(
        refused(
            builder().request(json!({"model": "m.glb"})).build(),
            test_keys()
        ),
        "model is a file"
    );
    assert_eq!(
        refused(
            rig_call(json!({"allow_negative_check": "yes"})),
            test_keys()
        ),
        "allow_negative_check is true or false"
    );
    assert_eq!(
        refused(rig_call(json!({"out_format": "fbx"})), test_keys()),
        "mesh.rig takes no member out_format"
    );
    assert_eq!(
        refused(rig_call(json!({})), Keys::none()),
        "TRIPO_API_KEY is not set"
    );
    let mut b = CallBuilder::new("mesh.rig", ROUTE).contract(json!({"adapter": "tripo-multiview"}));
    let model = b.file("model/gltf-binary", media::GLB);
    assert_eq!(
        refused(b.request(json!({"model": model})).build(), test_keys()),
        "v1.0-20240301@tripo is not a mesh.rig route this adapter serves"
    );
}

#[test]
fn every_check_failure_is_free() {
    // spec/providers.md §9.4 "Free phase": nothing paid is posted in any of these.
    let deadline_clock = || {
        FakeClock::scripted(&[
            Duration::ZERO,
            Duration::ZERO,
            CHECK_DEADLINE + Duration::from_secs(100),
        ])
    };
    let cases: Vec<(Vec<Exchange>, FakeClock, Submitted)> = vec![
        (
            vec![upload(), check_expect().fail(not_sent())],
            FakeClock::new(),
            Submitted::not_received("the Tripo riggability check failed: connection refused"),
        ),
        (
            vec![upload(), check_expect().fail(after_send())],
            FakeClock::new(),
            Submitted::not_received("the Tripo riggability check failed: connection reset"),
        ),
        (
            vec![
                upload(),
                check_expect().reply(HttpResponse::json(401, &json!({"code": 1}))),
            ],
            FakeClock::new(),
            Submitted::Refused {
                reason: "Tripo refused the riggability check with HTTP 401".into(),
            },
        ),
        (
            vec![
                upload(),
                check_expect().reply(HttpResponse::json(429, &json!({"code": 1}))),
            ],
            FakeClock::new(),
            Submitted::not_received("Tripo HTTP 429; body withheld"),
        ),
        (
            vec![
                upload(),
                check_expect().reply(HttpResponse::new(503, Vec::new())),
            ],
            FakeClock::new(),
            Submitted::not_received("Tripo HTTP 503; body withheld"),
        ),
        (
            vec![
                upload(),
                check_expect().reply(ok(json!({"task_id": "a b"}))),
            ],
            FakeClock::new(),
            Submitted::not_received("Tripo's answer to the riggability check has no task id"),
        ),
        (
            vec![
                upload(),
                check_posted(),
                read("task1", "failed", json!({}), json!(0)),
            ],
            FakeClock::new(),
            Submitted::Refused {
                reason: "Tripo's riggability check ended as failed".into(),
            },
        ),
        (
            vec![
                upload(),
                check_posted(),
                read("task1", "running", json!({}), json!(0)),
                read("task1", "running", json!({}), json!(0)),
            ],
            deadline_clock(),
            Submitted::not_received("Tripo task task1 is still running; collect it later"),
        ),
        (
            vec![
                upload(),
                check_posted(),
                // Polling task1, Tripo answers about task9.
                read_expect("task1").reply(ok(json!({"task_id": "task9", "status": "success",
                                                     "output": {"riggable": true}}))),
            ],
            FakeClock::new(),
            Submitted::not_received("Tripo answered for another task, or with an unknown status"),
        ),
        (
            vec![
                upload(),
                check_posted(),
                checked(json!({"riggable": "yes", "rig_type": "biped"})),
            ],
            FakeClock::new(),
            Submitted::not_received("Tripo's riggability check answered without a verdict"),
        ),
        (
            vec![upload(), check_posted(), checked(json!({}))],
            FakeClock::new(),
            Submitted::not_received("Tripo's riggability check answered without a verdict"),
        ),
        (
            vec![upload_expect().reply(HttpResponse::json(413, &json!({"code": 1})))],
            FakeClock::new(),
            Submitted::Refused {
                reason: "Tripo refused the upload with HTTP 413".into(),
            },
        ),
        (
            vec![upload_expect().fail(not_sent())],
            FakeClock::new(),
            Submitted::not_received("the Tripo upload failed: connection refused"),
        ),
    ];
    for (exchanges, clock, expected) in cases {
        let h = harness_with(exchanges, clock);
        assert_eq!(h.submit(&rig_call(json!({}))), expected);
        h.assert_no_rig_post();
        h.done();
    }
    // A check that keeps failing to be read polls on to its deadline, then is NotReceived.
    let h = harness_with(
        vec![
            upload(),
            check_posted(),
            read_expect("task1").reply(HttpResponse::new(500, Vec::new())),
            read_expect("task1").fail(not_sent()),
        ],
        deadline_clock(),
    );
    assert_eq!(
        h.submit(&rig_call(json!({}))),
        Submitted::not_received("Tripo task task1 could not be read; collect it later")
    );
    h.done();
}

#[test]
fn the_paid_rig_follows_the_task_outcomes() {
    // spec/providers.md §9.4 "Paid POST", as for the mesh.
    let cases: Vec<(Exchange, Submitted)> = vec![
        (
            rig_expect(RIG_BODY).fail(not_sent()),
            Submitted::not_received("the Tripo task was not sent: connection refused"),
        ),
        (
            rig_expect(RIG_BODY).fail(after_send()),
            Submitted::Uncertain {
                reason: "Tripo may have taken the task; it is not posted again (connection reset)"
                    .into(),
            },
        ),
        (
            rig_expect(RIG_BODY).reply(HttpResponse::json(429, &json!({"code": 1}))),
            Submitted::not_received("Tripo took no task (HTTP 429)"),
        ),
        (
            rig_expect(RIG_BODY).reply(HttpResponse::json(422, &json!({"code": 1}))),
            Submitted::Failed {
                reason: "Tripo refused the task with HTTP 422".into(),
                cost: None,
                retryable: false,
            },
        ),
        (
            rig_expect(RIG_BODY).reply(HttpResponse::new(504, Vec::new())),
            Submitted::Uncertain {
                reason:
                    "Tripo may have taken the task; it is not posted again (Tripo HTTP 504; body withheld)"
                        .into(),
            },
        ),
        (
            rig_expect(RIG_BODY).reply(ok(json!({"task_id": null}))),
            Submitted::Uncertain {
                reason:
                    "Tripo may have taken the task; it is not posted again (Tripo's answer has no task id)"
                        .into(),
            },
        ),
    ];
    for (exchange, expected) in cases {
        let h = harness(vec![
            upload(),
            check_posted(),
            checked(json!({"riggable": true, "rig_type": "biped"})),
            exchange,
        ]);
        assert_eq!(h.submit(&rig_call(json!({}))), expected);
        h.done();
    }
}

// ---------------------------------------------------------------------------------------------
// Collect

#[test]
fn a_rig_answers_its_one_glb_with_the_checks_facts() {
    // spec/providers.md §9.4 "Tasks"; spec/capabilities.md §8.
    let h = harness(vec![
        read("task2", "running", json!({}), json!(0)),
        read("task2", "success", json!({"model": STORAGE}), json!(25)),
        download(STORAGE, &glb(1)),
    ]);
    let outcome = h.collect(&rig_call(json!({})), plain_handle());
    let Collected::Answered(answer) = outcome else {
        panic!("not answered: {outcome:?}");
    };
    assert_eq!(answer.files.len(), 1);
    assert_eq!(answer.files["model"].kind, "model/gltf-binary");
    assert_eq!(answer.files["model"].bytes, glb(1));
    assert_eq!(
        answer.data,
        json!({"facts": {"riggable": true, "checked_rig_type": "biped",
                         "advisory_override": false}})
    );
    assert_eq!(answer.cost, Some(Usd(250_000)));
    assert_eq!(h.clock.sleeps(), [POLL]);
    h.done();

    // GLB and FBX: the GLB, whatever the order; the FBX is downloaded and ignored.
    let fbx_url = "https://api.tripo3d.ai/out/rigged.fbx";
    let h = harness(vec![
        read(
            "task2",
            "success",
            json!({"fbx_model": fbx_url, "model": STORAGE}),
            json!(25),
        ),
        download(fbx_url, &fbx(1)),
        download(STORAGE, &glb(2)),
    ]);
    let Collected::Answered(answer) = h.collect(&rig_call(json!({})), plain_handle()) else {
        panic!("not answered");
    };
    assert_eq!(answer.files["model"].bytes, glb(2));
    h.done();

    // Two GLBs, or none: the task made no single GLB.
    let second = "https://api.tripo3d.ai/out/second.glb";
    for (output, downloads) in [
        (
            json!({"model": STORAGE, "rig_model": second}),
            vec![download(STORAGE, &glb(1)), download(second, &glb(2))],
        ),
        (json!({"model": fbx_url}), vec![download(fbx_url, &fbx(1))]),
    ] {
        let mut exchanges = vec![read("task2", "success", output, json!(25))];
        exchanges.extend(downloads);
        let h = harness(exchanges);
        assert_eq!(
            h.collect(&rig_call(json!({})), plain_handle()),
            Collected::Ended {
                reason: "the Tripo rig task made no single GLB".into()
            }
        );
        h.done();
    }

    // The facts come from the handle, so a later run reports the same ones.
    let h = harness(vec![
        read("task2", "success", json!({"model": STORAGE}), json!("25")),
        download(STORAGE, &glb(1)),
    ]);
    let handle = json!({"task_id": "task2",
                        "check": {"task_id": "task1", "riggable": false, "rig_type": null},
                        "advisory_override": true});
    let Collected::Answered(answer) = h.collect(&rig_call(json!({})), handle) else {
        panic!("not answered");
    };
    assert_eq!(
        answer.data,
        json!({"facts": {"riggable": false, "checked_rig_type": null,
                         "advisory_override": true}})
    );
    assert_eq!(answer.cost, Some(Usd(250_000)));
    h.done();

    // A handle without its check reports nulls.
    let h = harness(vec![
        read("task2", "success", json!({"model": STORAGE}), json!(25)),
        download(STORAGE, &glb(1)),
    ]);
    let Collected::Answered(answer) = h.collect(&rig_call(json!({})), json!({"task_id": "task2"}))
    else {
        panic!("not answered");
    };
    assert_eq!(
        answer.data,
        json!({"facts": {"riggable": null, "checked_rig_type": null,
                         "advisory_override": false}})
    );
    h.done();
}

#[test]
fn a_rig_collect_ends_or_stays_outstanding_like_a_mesh() {
    let h = harness(vec![read("task2", "cancelled", json!({}), json!(0))]);
    assert_eq!(
        h.collect(&rig_call(json!({})), plain_handle()),
        Collected::Ended {
            reason: "Tripo ended task task2 as cancelled".into()
        }
    );
    h.done();

    let clock = FakeClock::scripted(&[Duration::ZERO, Duration::ZERO, Duration::from_secs(601)]);
    let h = harness_with(
        vec![
            read("task2", "queued", json!({}), json!(0)),
            read("task2", "running", json!({}), json!(0)),
        ],
        clock,
    );
    assert_eq!(
        h.collect(&rig_call(json!({})), plain_handle()),
        Collected::Unreachable {
            reason: "Tripo task task2 is still running; collect it later".into()
        }
    );
    h.done();

    let outcome = block_on(offline_rig(test_keys()).collect(&rig_call(json!({})), &json!({})));
    assert_eq!(
        outcome,
        Collected::Unreachable {
            reason: "the Tripo handle has no usable task id".into()
        }
    );
}
