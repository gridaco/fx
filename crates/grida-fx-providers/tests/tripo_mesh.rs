//! Tripo `mesh.generate` over synthetic exchanges (spec/providers.md §7, §9.4;
//! spec/capabilities.md §7). Nothing here reaches the network: every exchange is scripted on a
//! `ReplayTransport` (which panics on any request it was not given) or the setup is built over
//! `NoNetwork`; time is a `FakeClock`. The key is the made-up `test-tripo-key`.

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Collected, LongJob, Submitted};
use grida_fx_providers::clock::{Clock, FakeClock};
use grida_fx_providers::keys::{KeyName, Keys};
use grida_fx_providers::testing::{
    CallBuilder, TestCall, block_on, media, setup_with_clock, test_keys,
};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport, Reply,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Part, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::tripo::mesh::{COLLECT_DEADLINE, TripoMesh};
use grida_fx_providers::tripo::{MAX_MODEL_BYTES, POLL, TripoApi, Waited};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "test-tripo-key";
const BEARER: &str = "Bearer test-tripo-key";
const API: &str = "https://openapi.tripo3d.ai/v3";
const STORAGE: &str = "https://tripo-data.rg1.data.tripo3d.com/out/model.fbx";
const ROUTE: &str = "P2-20260801@tripo";

// ---------------------------------------------------------------------------------------------
// Harness

struct Harness {
    transport: Arc<ReplayTransport>,
    clock: Arc<FakeClock>,
    mesh: TripoMesh,
    api: TripoApi,
}

fn harness_with(exchanges: Vec<Exchange>, clock: FakeClock, keys: Keys) -> Harness {
    let transport = Arc::new(ReplayTransport::new(exchanges));
    let clock = Arc::new(clock);
    let setup = setup_with_clock(
        Arc::clone(&transport) as Arc<dyn Transport>,
        keys,
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    let api = TripoApi::new(&setup);
    Harness {
        transport,
        clock,
        mesh: TripoMesh::new(api.clone()),
        api,
    }
}

fn harness(exchanges: Vec<Exchange>) -> Harness {
    harness_with(exchanges, FakeClock::new(), test_keys())
}

/// An adapter whose transport panics on any request.
fn offline_mesh(keys: Keys) -> TripoMesh {
    let setup = setup_with_clock(Arc::new(NoNetwork), keys, Arc::new(FakeClock::new()));
    TripoMesh::new(TripoApi::new(&setup))
}

impl Harness {
    fn submit(&self, call: &TestCall) -> Submitted {
        let outcome = block_on(self.mesh.submit(call));
        assert_clean(&format!("{outcome:?}"));
        outcome
    }

    fn collect(&self, call: &TestCall, handle: Value) -> Collected {
        let outcome = block_on(self.mesh.collect(call, &handle));
        assert_clean(&format!("{:?}", without_bytes(&outcome)));
        outcome
    }

    /// Every scripted exchange was used; API requests carry the key, downloads carry none.
    fn done(&self) {
        self.transport.assert_done();
        assert_credentials(&self.transport.requests());
    }

    fn methods_and_urls(&self) -> Vec<(Method, String)> {
        self.transport
            .requests()
            .into_iter()
            .map(|r| (r.method, r.url))
            .collect()
    }
}

fn assert_credentials(requests: &[grida_fx_providers::HttpRequest]) {
    for request in requests {
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

/// The key appears in no reason, handle or data.
fn assert_clean(shown: &str) {
    assert!(!shown.contains(KEY), "the key leaked: {shown}");
}

/// The outcome without file bytes, for printing.
fn without_bytes(outcome: &Collected) -> Collected {
    match outcome {
        Collected::Answered(answer) => {
            let mut answer = answer.clone();
            for file in answer.files.values_mut() {
                file.bytes.clear();
            }
            Collected::Answered(answer)
        }
        other => other.clone(),
    }
}

fn ok(data: Value) -> HttpResponse {
    HttpResponse::json(200, &json!({"code": 0, "data": data}))
}

fn upload_expect(filename: &str, kind: &str, bytes: &[u8]) -> Expect {
    Expect::new(Method::Post, format!("{API}/files"), Lane::Upload)
        .credential("authorization", BEARER)
        .body(ExpectBody::Multipart(vec![Part::file(
            "file",
            filename,
            kind,
            bytes.to_vec(),
        )]))
}

fn upload(filename: &str, kind: &str, bytes: &[u8], token: &str) -> Exchange {
    upload_expect(filename, kind, bytes).reply(ok(json!({"file_token": token})))
}

fn post_expect(body: ExpectBody) -> Expect {
    Expect::new(
        Method::Post,
        format!("{API}/generation/multiview-to-model"),
        Lane::Provider,
    )
    .credential("authorization", BEARER)
    .body(body)
}

fn read_expect(task_id: &str) -> Expect {
    Expect::get(format!("{API}/tasks/{task_id}")).credential("authorization", BEARER)
}

fn status(task_id: &str, status: &str, output: Value, credits: Value) -> HttpResponse {
    ok(
        json!({"task_id": task_id, "status": status, "output": output,
              "credits_consumed": credits}),
    )
}

fn read(task_id: &str, state: &str) -> Exchange {
    read_expect(task_id).reply(status(task_id, state, json!({}), json!(0)))
}

fn success(task_id: &str, output: Value, credits: Value) -> Exchange {
    read_expect(task_id).reply(status(task_id, "success", output, credits))
}

fn download(url: &str, bytes: &[u8]) -> Exchange {
    Expect::download(url).reply(HttpResponse::new(200, bytes.to_vec()))
}

fn not_sent() -> TransportError {
    TransportError::not_sent(TransportErrorKind::Connect, "connection refused")
}

fn after_send() -> TransportError {
    TransportError::after_send(TransportErrorKind::Timeout, "the request timed out")
}

fn network_off() -> TransportError {
    TransportError::not_sent(
        TransportErrorKind::Refused,
        "the network is off (GRIDA_FX_NETWORK=off)",
    )
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

/// Two distinct PNGs, so each upload's bytes show which view it is.
fn front_png() -> Vec<u8> {
    media::png(1, 1, None)
}

fn back_png() -> Vec<u8> {
    media::png(2, 2, None)
}

fn builder() -> CallBuilder {
    CallBuilder::new("mesh.generate", ROUTE)
        .contract(json!({"adapter": "tripo-multiview", "adapter_behavior": 1}))
}

/// A call with front and back PNG views and the character-3d settings.
fn character_call() -> TestCall {
    let mut builder = builder();
    let back = builder.file("image/png", &back_png());
    let front = builder.file("image/png", &front_png());
    builder
        .request(
            json!({"face_limit": 10000, "pbr": true, "quad": true, "texture": true,
                        "views": {"back": back, "front": front}}),
        )
        .build()
}

/// A call with one front PNG view.
fn front_call() -> TestCall {
    let mut builder = builder();
    let front = builder.file("image/png", &front_png());
    builder.request(json!({"views": {"front": front}})).build()
}

const FRONT_BODY: &str = r#"{"model":"P2-20260801","quad":false,"texture":true,"pbr":false,"inputs":[{"front":"tok1"}]}"#;

fn handle(task_id: &str) -> Value {
    json!({"task_id": task_id})
}

fn answered(outcome: Collected) -> grida_fx_providers::Answer {
    match outcome {
        Collected::Answered(answer) => answer,
        other => panic!("not answered: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Submit

#[test]
fn a_mesh_is_uploaded_in_tripos_order_and_posted_once() {
    // spec/providers.md §9.4 "Mesh": views named back then front upload as front, back.
    let h = harness(vec![
        upload("front.png", "image/png", &front_png(), "tok1"),
        upload("back.png", "image/png", &back_png(), "tok2"),
        post_expect(ExpectBody::JsonText(
            r#"{"model":"P2-20260801","quad":true,"texture":true,"pbr":true,"face_limit":10000,"inputs":[{"front":"tok1"},{"back":"tok2"}]}"#
                .into(),
        ))
        .reply(ok(json!({"task_id": "task1"}))),
    ]);
    let outcome = h.submit(&character_call());
    assert_eq!(
        outcome,
        Submitted::Accepted {
            handle: json!({"task_id": "task1"})
        }
    );
    h.done();
    let requests = h.transport.requests();
    assert_eq!(requests[0].lane, Lane::Upload);
    assert_eq!(requests[0].timeout, Duration::from_secs(300));
    assert_eq!(requests[2].lane, Lane::Provider);
    assert_eq!(requests[2].timeout, Duration::from_secs(180));
    assert!(h.clock.sleeps().is_empty(), "submit never polls");
}

#[test]
fn defaults_jpeg_names_and_four_views_in_order() {
    // spec/providers.md §9.4 "Mesh": defaults, JPEG names, four views in order.
    let mut b = builder();
    let jpeg = media::JPEG_HEAD;
    let left_png = media::png(3, 3, None);
    let right_png = media::png(4, 4, None);
    let right = b.file("image/png", &right_png);
    let left = b.file("image/png", &left_png);
    let back = b.file("image/png", &back_png());
    let front = b.file("image/jpeg", jpeg);
    let call = b
        .request(
            json!({"views": {"right": right, "left": left, "back": back, "front": front},
                        "face_limit": null, "quad": null}),
        )
        .build();
    let h = harness(vec![
        upload("front.jpg", "image/jpeg", jpeg, "tok1"),
        upload("back.png", "image/png", &back_png(), "tok2"),
        upload("left.png", "image/png", &left_png, "tok3"),
        upload("right.png", "image/png", &right_png, "tok4"),
        post_expect(ExpectBody::JsonText(
            r#"{"model":"P2-20260801","quad":false,"texture":true,"pbr":false,"inputs":[{"front":"tok1"},{"back":"tok2"},{"left":"tok3"},{"right":"tok4"}]}"#
                .into(),
        ))
        .reply(ok(json!({"task_id": "task1"}))),
    ]);
    assert!(matches!(h.submit(&call), Submitted::Accepted { .. }));
    h.done();
}

#[test]
fn the_face_limit_bounds_are_inclusive() {
    for limit in [48, 25_000] {
        let mut b = builder();
        let front = b.file("image/png", &front_png());
        let call = b
            .request(json!({"views": {"front": front}, "face_limit": limit}))
            .build();
        let h = harness(vec![
            upload("front.png", "image/png", &front_png(), "tok1"),
            post_expect(ExpectBody::JsonText(format!(
                r#"{{"model":"P2-20260801","quad":false,"texture":true,"pbr":false,"face_limit":{limit},"inputs":[{{"front":"tok1"}}]}}"#
            )))
            .reply(ok(json!({"task_id": "task1"}))),
        ]);
        assert!(
            matches!(h.submit(&call), Submitted::Accepted { .. }),
            "{limit}"
        );
        h.done();
    }
}

#[test]
fn bad_requests_are_refused_before_anything_is_sent() {
    // spec/providers.md §5, §9.4 (mirrors the stage-gen plugin test of a view Tripo does not take).
    let refused = |call: TestCall, keys: Keys| -> String {
        match block_on(offline_mesh(keys).submit(&call)) {
            Submitted::Refused { reason } => {
                assert_clean(&reason);
                reason
            }
            other => panic!("not refused: {other:?}"),
        }
    };
    let png = front_png();
    let with_views = |names: &[&str], extra: Value| {
        let mut b = builder();
        let mut views = Map::new();
        for name in names {
            views.insert(name.to_string(), b.file("image/png", &png));
        }
        let mut request = json!({"views": views});
        if let (Value::Object(request), Value::Object(extra)) = (&mut request, extra) {
            request.extend(extra);
        }
        b.request(request).build()
    };
    let takes = "a multiview task takes front and any of back, left, right";
    assert_eq!(
        refused(
            with_views(&["front", "three_quarter"], json!({})),
            test_keys()
        ),
        format!("{takes}; not three_quarter")
    );
    assert_eq!(
        refused(
            with_views(&["top", "front", "side"], json!({})),
            test_keys()
        ),
        format!("{takes}; not side, top")
    );
    assert_eq!(
        refused(with_views(&["back"], json!({})), test_keys()),
        takes
    );
    assert_eq!(
        refused(with_views(&[], json!({})), test_keys()),
        "a mesh call needs its views, by name"
    );
    for limit in [json!(47), json!(25_001), json!(-1)] {
        assert_eq!(
            refused(
                with_views(&["front"], json!({"face_limit": limit})),
                test_keys()
            ),
            "face_limit is 48 to 25000"
        );
    }
    // The shape of the request (spec/capabilities.md §1, §7).
    assert_eq!(
        refused(
            with_views(&["front"], json!({"face_limit": 10.5})),
            test_keys()
        ),
        "face_limit is a whole number"
    );
    assert_eq!(
        refused(with_views(&["front"], json!({"seed": 7})), test_keys()),
        "mesh.generate takes no member seed"
    );
    assert_eq!(
        refused(
            builder().request(json!({"quad": true})).build(),
            test_keys()
        ),
        "mesh.generate needs views"
    );
    assert_eq!(
        refused(
            builder()
                .request(json!({"views": {"front": "x.png"}}))
                .build(),
            test_keys()
        ),
        "views is files by name"
    );
    // View files: kinds and bytes.
    for kind in ["image/gif", "image/webp"] {
        let mut b = builder();
        let front = b.file(kind, b"GIF89a....");
        let call = b.request(json!({"views": {"front": front}})).build();
        assert_eq!(
            refused(call, test_keys()),
            format!("the front view is {kind}; Tripo takes PNG or JPEG")
        );
    }
    let unknown = json!({"file": "f".repeat(64)});
    assert_eq!(
        refused(
            builder()
                .request(json!({"views": {"front": unknown}}))
                .build(),
            test_keys()
        ),
        "the front view has no bytes to send"
    );
    // The key: missing or blank.
    assert_eq!(
        refused(with_views(&["front"], json!({})), Keys::none()),
        "TRIPO_API_KEY is not set"
    );
    assert_eq!(
        refused(
            with_views(&["front"], json!({})),
            Keys::from_pairs(&[(KeyName::Tripo, "   ")])
        ),
        "TRIPO_API_KEY is not set"
    );
    // The key is checked before the values (spec/providers.md §5).
    assert_eq!(
        refused(with_views(&["three_quarter"], json!({})), Keys::none()),
        "TRIPO_API_KEY is not set"
    );
    // The route's contract.
    let mut b = CallBuilder::new("mesh.generate", ROUTE).contract(json!({"adapter": "tripo-rig"}));
    let front = b.file("image/png", &png);
    assert_eq!(
        refused(
            b.request(json!({"views": {"front": front}})).build(),
            test_keys()
        ),
        "P2-20260801@tripo is not a mesh.generate route this adapter serves"
    );
    let mut b = CallBuilder::new("mesh.rig", ROUTE);
    let model = b.file("model/gltf-binary", media::GLB);
    assert_eq!(
        refused(b.request(json!({"model": model})).build(), test_keys()),
        "P2-20260801@tripo is not a mesh.generate route this adapter serves"
    );
    // A route without a contract is served.
    let h = harness(vec![
        upload("front.png", "image/png", &png, "tok1"),
        post_expect(ExpectBody::JsonText(FRONT_BODY.into())).reply(ok(json!({"task_id": "t"}))),
    ]);
    let mut b = CallBuilder::new("mesh.generate", ROUTE);
    let front = b.file("image/png", &png);
    assert!(matches!(
        h.submit(&b.request(json!({"views": {"front": front}})).build()),
        Submitted::Accepted { .. }
    ));
    h.done();
}

#[test]
fn a_failed_upload_posts_no_task() {
    // spec/providers.md §9.4 "Free phase": every failure is NotReceived, except deterministic
    // statuses.
    let png = front_png();
    let not_received: Vec<(&str, Exchange)> = vec![
        (
            "not sent",
            upload_expect("front.png", "image/png", &png).fail(not_sent()),
        ),
        (
            "maybe sent",
            upload_expect("front.png", "image/png", &png).fail(after_send()),
        ),
        (
            "429",
            upload_expect("front.png", "image/png", &png).reply(
                HttpResponse::json(429, &json!({"code": 2000, "message": "slow down"}))
                    .with_header("retry-after", "12"),
            ),
        ),
        (
            "500",
            upload_expect("front.png", "image/png", &png)
                .reply(HttpResponse::new(500, b"oops".to_vec())),
        ),
        (
            "302",
            upload_expect("front.png", "image/png", &png).reply(
                HttpResponse::new(302, Vec::new()).with_header("location", "https://x.test/"),
            ),
        ),
        (
            "non-JSON",
            upload_expect("front.png", "image/png", &png)
                .reply(HttpResponse::new(200, b"<html>".to_vec())),
        ),
        (
            "code 1",
            upload_expect("front.png", "image/png", &png)
                .reply(HttpResponse::json(200, &json!({"code": 1}))),
        ),
        (
            "no token",
            upload_expect("front.png", "image/png", &png).reply(ok(json!({}))),
        ),
        (
            "token with a slash",
            upload_expect("front.png", "image/png", &png).reply(ok(json!({"file_token": "a/b"}))),
        ),
        (
            "token of 257",
            upload_expect("front.png", "image/png", &png)
                .reply(ok(json!({"file_token": "t".repeat(257)}))),
        ),
        (
            "numeric token",
            upload_expect("front.png", "image/png", &png).reply(ok(json!({"file_token": 7}))),
        ),
    ];
    for (case, exchange) in not_received {
        let h = harness(vec![exchange]);
        let outcome = h.submit(&front_call());
        assert!(
            matches!(&outcome, Submitted::NotReceived { .. }),
            "{case}: {outcome:?}"
        );
        assert_eq!(h.transport.requests().len(), 1, "{case}");
        h.done();
    }
    let reason = |exchange: Exchange| match harness(vec![exchange]).submit(&front_call()) {
        Submitted::NotReceived { reason, .. } | Submitted::Refused { reason } => reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        reason(upload_expect("front.png", "image/png", &png).reply(
            HttpResponse::json(429, &json!({"code": 2000})).with_header("retry-after", "12")
        )),
        "Tripo HTTP 429; body withheld; retry-after 12 s"
    );
    assert_eq!(
        reason(
            upload_expect("front.png", "image/png", &png)
                .reply(HttpResponse::new(500, b"internal secret".to_vec()))
        ),
        "Tripo HTTP 500; body withheld"
    );
    assert_eq!(
        reason(upload_expect("front.png", "image/png", &png).reply(ok(json!({"file_token": ""})))),
        "Tripo's upload answer has no usable file handle"
    );
    assert_eq!(
        reason(upload_expect("front.png", "image/png", &png).fail(not_sent())),
        "the Tripo upload failed: connection refused"
    );
    for status in [400, 401, 403, 404, 413, 415, 422] {
        let h = harness(vec![upload_expect("front.png", "image/png", &png).reply(
            HttpResponse::json(status, &json!({"code": 1, "message": "no"})),
        )]);
        assert_eq!(
            h.submit(&front_call()),
            Submitted::Refused {
                reason: format!("Tripo refused the upload with HTTP {status}")
            }
        );
        h.done();
    }
    // The network turned off: the transport refuses before anything leaves.
    let h = harness(vec![
        upload_expect("front.png", "image/png", &png).fail(network_off()),
    ]);
    assert_eq!(
        h.submit(&front_call()),
        Submitted::Refused {
            reason: "the Tripo upload was not sent: the network is off (GRIDA_FX_NETWORK=off)"
                .into()
        }
    );
    // The second upload fails after the first succeeded: still no task.
    let h = harness(vec![
        upload("front.png", "image/png", &front_png(), "tok1"),
        upload_expect("back.png", "image/png", &back_png())
            .reply(HttpResponse::new(503, Vec::new())),
    ]);
    assert!(matches!(
        h.submit(&character_call()),
        Submitted::NotReceived { .. }
    ));
    h.done();
}

#[test]
fn the_paid_post_is_sent_once_whatever_happens() {
    // spec/providers.md §9.4 "Paid POST" (mirrors stage-gen's "a post whose outcome is unknown is
    // never repeated").
    let uncertain = |detail: &str| Submitted::Uncertain {
        reason: format!("Tripo may have taken the task; it is not posted again ({detail})"),
    };
    let post = || post_expect(ExpectBody::JsonText(FRONT_BODY.into()));
    let cases: Vec<(Exchange, Submitted)> = vec![
        (
            post().fail(not_sent()),
            Submitted::not_received("the Tripo task was not sent: connection refused"),
        ),
        (
            post().fail(after_send()),
            uncertain("the request timed out"),
        ),
        (
            post().reply(HttpResponse::json(429, &json!({"code": 2000}))),
            Submitted::not_received("Tripo took no task (HTTP 429)"),
        ),
        (
            post().reply(
                HttpResponse::json(429, &json!({"code": 2000})).with_header("retry-after", "7"),
            ),
            Submitted::NotReceived {
                reason: "Tripo took no task (HTTP 429); retry-after 7 s".into(),
                retry_after: Some(Duration::from_secs(7)),
            },
        ),
        (
            post().reply(HttpResponse::new(500, Vec::new())),
            uncertain("Tripo HTTP 500; body withheld"),
        ),
        (
            post().reply(HttpResponse::new(502, Vec::new())),
            uncertain("Tripo HTTP 502; body withheld"),
        ),
        (
            post().reply(HttpResponse::new(302, Vec::new())),
            uncertain("Tripo HTTP 302; body withheld"),
        ),
        (
            post().reply(HttpResponse::new(408, Vec::new())),
            uncertain("Tripo HTTP 408; body withheld"),
        ),
        (
            post().reply(HttpResponse::json(
                201,
                &json!({"code": 0, "data": {"task_id": "t"}}),
            )),
            uncertain("Tripo HTTP 201; body withheld"),
        ),
        (
            post().reply(HttpResponse::new(200, b"not json".to_vec())),
            uncertain("Tripo answered with something other than JSON"),
        ),
        (
            post().reply(HttpResponse::json(200, &json!({"code": 2001, "data": {}}))),
            uncertain("Tripo answered with an unsuccessful envelope"),
        ),
        (
            post().reply(HttpResponse::json(200, &json!({"code": 0}))),
            uncertain("Tripo answered without data"),
        ),
        (
            post().reply(ok(json!({"task_id": ""}))),
            uncertain("Tripo's answer has no task id"),
        ),
        (
            post().reply(ok(json!({"task_id": "t".repeat(129)}))),
            uncertain("Tripo's answer has no task id"),
        ),
        (
            post().reply(ok(json!({}))),
            uncertain("Tripo's answer has no task id"),
        ),
    ];
    for (exchange, expected) in cases {
        let h = harness(vec![
            upload("front.png", "image/png", &front_png(), "tok1"),
            exchange,
        ]);
        assert_eq!(h.submit(&front_call()), expected);
        let posts = h
            .methods_and_urls()
            .into_iter()
            .filter(|(m, url)| *m == Method::Post && url.ends_with("/multiview-to-model"))
            .count();
        assert_eq!(posts, 1);
        h.done();
    }
    for status in [400, 401, 403, 404, 422] {
        let h = harness(vec![
            upload("front.png", "image/png", &front_png(), "tok1"),
            post().reply(HttpResponse::json(
                status,
                &json!({"code": 1, "message": "secret"}),
            )),
        ]);
        assert_eq!(
            h.submit(&front_call()),
            Submitted::Failed {
                reason: format!("Tripo refused the task with HTTP {status}"),
                cost: None,
                retryable: false,
            }
        );
        h.done();
    }
    let h = harness(vec![
        upload("front.png", "image/png", &front_png(), "tok1"),
        post().fail(network_off()),
    ]);
    assert!(matches!(h.submit(&front_call()), Submitted::Refused { .. }));
    h.done();
}

// ---------------------------------------------------------------------------------------------
// Collect

#[test]
fn a_finished_mesh_is_downloaded_with_its_cost() {
    // spec/providers.md §7, §9.4 (mirrors stage-gen's "a mesh task is posted once and collected
    // with its cost").
    let h = harness(vec![
        read("task1", "queued"),
        read("task1", "running"),
        success("task1", json!({"pbr_model": STORAGE}), json!(125)),
        download(STORAGE, &fbx(1)),
    ]);
    let answer = answered(h.collect(&front_call(), handle("task1")));
    assert_eq!(answer.files.len(), 1);
    assert_eq!(answer.files["model"].kind, "model/fbx");
    assert_eq!(answer.files["model"].bytes, fbx(1));
    assert_eq!(answer.data, json!({"facts": {"model_kind": "model/fbx"}}));
    assert_eq!(answer.cost, Some(Usd(1_250_000)));
    assert_eq!(h.clock.sleeps(), [POLL, POLL]);
    h.done();
    let requests = h.transport.requests();
    let fetch = requests.last().unwrap();
    assert_eq!(fetch.lane, Lane::Download);
    assert!(fetch.credential.is_none());
    assert_eq!(fetch.max_response_bytes, MAX_MODEL_BYTES);
    assert_eq!(fetch.timeout, Duration::from_secs(300));
    assert!(
        requests[..3]
            .iter()
            .all(|r| r.timeout == Duration::from_secs(300))
    );
}

#[test]
fn an_fbx_is_preferred_and_every_model_is_downloaded() {
    // spec/providers.md §9.4 "Tasks" (mirrors stage-gen's "a mesh is posted once and its fbx
    // kept").
    let glb_url = "https://api.tripo3d.ai/out/model.glb";
    let cases: Vec<(Value, Vec<Exchange>, &str, Vec<u8>)> = vec![
        (
            json!({"model": glb_url, "pbr_model": STORAGE}),
            vec![download(glb_url, &glb(1)), download(STORAGE, &fbx(2))],
            "model/fbx",
            fbx(2),
        ),
        (
            json!({"model": glb_url, "pbr_model": "https://api.tripo3d.ai/out/b.glb"}),
            vec![
                download(glb_url, &glb(1)),
                download("https://api.tripo3d.ai/out/b.glb", &glb(2)),
            ],
            "model/gltf-binary",
            glb(1),
        ),
        (
            json!({"model": glb_url}),
            vec![download(glb_url, &glb(3))],
            "model/gltf-binary",
            glb(3),
        ),
        (
            json!({"model": STORAGE, "base_model": "https://x.tripo3d.ai/b.fbx"}),
            vec![
                download(STORAGE, &fbx(4)),
                download("https://x.tripo3d.ai/b.fbx", &fbx(5)),
            ],
            "model/fbx",
            fbx(4),
        ),
    ];
    for (output, downloads, kind, bytes) in cases {
        let mut exchanges = vec![success("task1", output.clone(), json!(125))];
        exchanges.extend(downloads);
        let h = harness(exchanges);
        let answer = answered(h.collect(&front_call(), handle("task1")));
        assert_eq!(answer.files["model"].kind, kind, "{output}");
        assert_eq!(answer.files["model"].bytes, bytes, "{output}");
        assert_eq!(answer.data, json!({"facts": {"model_kind": kind}}));
        h.done();
    }
}

#[test]
fn a_task_tripo_ended_is_over() {
    // spec/providers.md §9.4 "Collect" (mirrors stage-gen's "a failed task is over").
    for state in ["failed", "cancelled", "banned", "expired"] {
        let h = harness(vec![read("task1", "running"), read("task1", state)]);
        assert_eq!(
            h.collect(&front_call(), handle("task1")),
            Collected::Ended {
                reason: format!("Tripo ended task task1 as {state}")
            }
        );
        h.done();
    }
}

#[test]
fn a_task_still_running_at_the_deadline_stays_tripos() {
    // spec/providers.md §7 (mirrors stage-gen's "one still running stays Tripo's"): clock 0, 0, 100
    // and a deadline of 10 s make exactly two reads.
    let clock = FakeClock::scripted(&[Duration::ZERO, Duration::ZERO, Duration::from_secs(100)]);
    let h = harness_with(
        vec![read("task1", "running"), read("task1", "running")],
        clock,
        test_keys(),
    );
    assert_eq!(
        block_on(h.api.wait("task1", Duration::from_secs(10))),
        Waited::Unreachable("Tripo task task1 is still running; collect it later".into())
    );
    assert_eq!(h.clock.sleeps(), [POLL]);
    h.done();

    // Through collect, with its own deadline (1200 s).
    let clock = FakeClock::scripted(&[
        Duration::ZERO,
        Duration::ZERO,
        COLLECT_DEADLINE + Duration::from_secs(100),
    ]);
    let h = harness_with(
        vec![read("task1", "queued"), read("task1", "running")],
        clock,
        test_keys(),
    );
    assert_eq!(
        h.collect(&front_call(), handle("task1")),
        Collected::Unreachable {
            reason: "Tripo task task1 is still running; collect it later".into()
        }
    );
    h.done();

    // Unscripted time: reads at 0, 5 and 10 s for a 10 s deadline.
    let h = harness(vec![
        read("task1", "queued"),
        read("task1", "running"),
        read("task1", "running"),
    ]);
    assert!(matches!(
        block_on(h.api.wait("task1", Duration::from_secs(10))),
        Waited::Unreachable(_)
    ));
    assert_eq!(h.clock.sleeps(), [POLL, POLL]);
    h.done();

    // A zero deadline still reads once.
    let h = harness(vec![read("task1", "queued")]);
    assert_eq!(
        block_on(h.api.wait("task1", Duration::ZERO)),
        Waited::Unreachable("Tripo task task1 is still queued; collect it later".into())
    );
    assert!(h.clock.sleeps().is_empty());
    h.done();
}

#[test]
fn polls_that_fail_are_polls_that_saw_nothing() {
    // spec/providers.md §7: status reads are polling.
    let h = harness(vec![
        read_expect("task1").reply(HttpResponse::new(502, Vec::new())),
        read_expect("task1").fail(not_sent()),
        read_expect("task1").fail(after_send()),
        read_expect("task1").reply(HttpResponse::json(429, &json!({"code": 1}))),
        read_expect("task1").reply(HttpResponse::new(200, b"<html>".to_vec())),
        read_expect("task1").reply(HttpResponse::json(200, &json!({"code": 1, "data": {}}))),
        read_expect("task1").reply(HttpResponse::json(404, &json!({"code": 1}))),
        success("task1", json!({"model": STORAGE}), json!(125)),
        download(STORAGE, &fbx(1)),
    ]);
    let answer = answered(h.collect(&front_call(), handle("task1")));
    assert_eq!(answer.cost, Some(Usd(1_250_000)));
    assert_eq!(h.clock.sleeps().len(), 7);
    h.done();

    // Failures until the deadline.
    let clock = FakeClock::scripted(&[Duration::ZERO, Duration::ZERO, COLLECT_DEADLINE]);
    let h = harness_with(
        vec![
            read_expect("task1").reply(HttpResponse::new(500, Vec::new())),
            read_expect("task1").fail(after_send()),
        ],
        clock,
        test_keys(),
    );
    assert_eq!(
        h.collect(&front_call(), handle("task1")),
        Collected::Unreachable {
            reason: "Tripo task task1 could not be read; collect it later".into()
        }
    );
    h.done();

    // A status seen once is the one reported, though the last read failed.
    let clock = FakeClock::scripted(&[Duration::ZERO, Duration::ZERO, COLLECT_DEADLINE]);
    let h = harness_with(
        vec![
            read("task1", "running"),
            read_expect("task1").reply(HttpResponse::new(500, Vec::new())),
        ],
        clock,
        test_keys(),
    );
    assert_eq!(
        h.collect(&front_call(), handle("task1")),
        Collected::Unreachable {
            reason: "Tripo task task1 is still running; collect it later".into()
        }
    );
    h.done();

    // A credential or proxy problem, or a refusal by the transport, ends polling at once.
    for (exchange, reason) in [
        (
            read_expect("task1").reply(HttpResponse::json(401, &json!({"code": 1}))),
            "Tripo task task1 could not be read: HTTP 401",
        ),
        (
            read_expect("task1").reply(HttpResponse::new(403, Vec::new())),
            "Tripo task task1 could not be read: HTTP 403",
        ),
        (
            read_expect("task1").reply(HttpResponse::new(307, Vec::new())),
            "Tripo task task1 could not be read: HTTP 307",
        ),
        (
            read_expect("task1").fail(network_off()),
            "Tripo task task1 could not be read: the network is off (GRIDA_FX_NETWORK=off)",
        ),
    ] {
        let h = harness(vec![read("task1", "running"), exchange]);
        assert_eq!(
            h.collect(&front_call(), handle("task1")),
            Collected::Unreachable {
                reason: reason.into()
            }
        );
        h.done();
    }
}

#[test]
fn an_answer_about_another_task_or_an_unknown_status_is_unreachable() {
    // spec/providers.md §7: not Ended, since the job may still run.
    let unknown = Collected::Unreachable {
        reason: "Tripo answered for another task, or with an unknown status".into(),
    };
    let cases = [
        status("task9", "success", json!({"model": STORAGE}), json!(1)),
        status("task1", "paused", json!({}), json!(1)),
        ok(json!({"status": "running"})),
        ok(json!({"task_id": "task1"})),
        ok(json!({"task_id": "task1", "status": 3})),
    ];
    for response in cases {
        let h = harness(vec![read_expect("task1").reply(response)]);
        assert_eq!(h.collect(&front_call(), handle("task1")), unknown);
        h.done();
    }
}

#[test]
fn the_output_scan_decides_what_is_downloaded() {
    // spec/providers.md §9.4 "Tasks" (mirrors stage-gen's "a model from another host is refused").
    let ended = |reason: &str| Collected::Ended {
        reason: reason.into(),
    };
    let outside = "a Tripo model address is outside Tripo's own hosts";
    let malformed = "a Tripo model address is malformed; details withheld";
    let no_model = "a finished Tripo task has no model to download";
    let nine: Map<String, Value> = (0..9)
        .map(|i| {
            (
                format!("model_{i}"),
                json!(format!("https://a.tripo3d.ai/{i}.glb")),
            )
        })
        .collect();
    let cases: Vec<(Value, Collected)> = vec![
        (
            json!({"model": "https://example.com/model.glb"}),
            ended(outside),
        ),
        (
            json!({"model": "http://api.tripo3d.ai/m.glb"}),
            ended(no_model),
        ),
        (
            json!({"model": "https://user@x.tripo3d.ai/m.glb"}),
            ended(outside),
        ),
        (
            json!({"model": "https://x.tripo3d.ai:8443/m.glb"}),
            ended(outside),
        ),
        (
            json!({"models": ["https://api.tripo3d.ai/m.glb"]}),
            ended(no_model),
        ),
        (
            json!({"pbr-model": "https://api.tripo3d.ai/m.glb"}),
            ended(no_model),
        ),
        (
            json!({"rendered_image": "https://api.tripo3d.ai/i.png"}),
            ended(no_model),
        ),
        (Value::Object(nine), ended(no_model)),
        (
            json!({"model": "https://api.tripo3d.ai/m\n.glb"}),
            ended(malformed),
        ),
        (
            json!({"model": format!("https://api.tripo3d.ai/{}", "m".repeat(20_481))}),
            ended(malformed),
        ),
    ];
    for (output, expected) in cases {
        let h = harness(vec![success("task1", output.clone(), json!(125))]);
        assert_eq!(
            h.collect(&front_call(), handle("task1")),
            expected,
            "{output}"
        );
        h.done();
    }
    // A non-object output is no output.
    let h = harness(vec![
        read_expect("task1").reply(ok(json!({"task_id": "task1", "status": "success",
                                              "output": [STORAGE], "credits_consumed": 1}))),
    ]);
    assert_eq!(h.collect(&front_call(), handle("task1")), ended(no_model));
    h.done();

    // Accepted hosts and ports; one download per distinct URL; only model fields.
    let accepted: Vec<(Value, Vec<&str>)> = vec![
        (
            json!({"model": "https://x.tripo3d.ai:443/m.glb"}),
            vec!["https://x.tripo3d.ai:443/m.glb"],
        ),
        (
            json!({"model": "https://api.tripo3d.ai/m.glb"}),
            vec!["https://api.tripo3d.ai/m.glb"],
        ),
        (
            json!({"model": "https://api.tripo3d.ai/m.glb", "pbr_model": "https://api.tripo3d.ai/m.glb"}),
            vec!["https://api.tripo3d.ai/m.glb"],
        ),
        (
            json!({"rendered_image": "https://api.tripo3d.ai/i.png", "model": "https://api.tripo3d.ai/2.glb"}),
            vec!["https://api.tripo3d.ai/2.glb"],
        ),
        (
            json!({"result": {"mesh": {"url": "https://api.tripo3d.ai/deep.glb"}}}),
            vec!["https://api.tripo3d.ai/deep.glb"],
        ),
    ];
    for (output, urls) in accepted {
        let mut exchanges = vec![success("task1", output.clone(), json!(125))];
        exchanges.extend(urls.iter().map(|url| download(url, &glb(1))));
        let h = harness(exchanges);
        let answer = answered(h.collect(&front_call(), handle("task1")));
        assert_eq!(answer.files["model"].kind, "model/gltf-binary", "{output}");
        h.done();
    }
    // A foreign second URL ends the job after the first was downloaded.
    let h = harness(vec![
        success(
            "task1",
            json!({"model": "https://api.tripo3d.ai/m.glb", "pbr_model": "https://example.com/m.fbx"}),
            json!(125),
        ),
        download("https://api.tripo3d.ai/m.glb", &glb(1)),
    ]);
    assert_eq!(h.collect(&front_call(), handle("task1")), ended(outside));
    h.done();
}

#[test]
fn downloads_are_fetched_once_and_capped() {
    // spec/providers.md §8, §9.4: downloads.
    let signed = "https://tripo-data.rg1.data.tripo3d.com/out/model.glb?sig=signed-secret";
    let unreachable = |reason: &str| Collected::Unreachable {
        reason: reason.into(),
    };
    let cases: Vec<(Exchange, Collected)> = vec![
        (
            Expect::download(signed).reply(HttpResponse::new(404, Vec::new())),
            unreachable("Tripo download HTTP 404"),
        ),
        (
            Expect::download(signed).reply(
                HttpResponse::new(302, Vec::new())
                    .with_header("location", "https://api.tripo3d.ai/elsewhere.glb"),
            ),
            unreachable("Tripo download HTTP 302"),
        ),
        (
            Expect::download(signed).reply(HttpResponse::new(500, b"secret".to_vec())),
            unreachable("Tripo download HTTP 500"),
        ),
        (
            Expect::download(signed).fail(after_send()),
            unreachable("Tripo download failed; URL withheld"),
        ),
        (
            Expect::download(signed).fail(network_off()),
            unreachable("Tripo download failed; URL withheld"),
        ),
        (
            Exchange {
                expect: Expect::download(signed),
                reply: Reply::Zeros {
                    status: 200,
                    headers: Vec::new(),
                    len: MAX_MODEL_BYTES + 1,
                },
            },
            unreachable("a Tripo model is larger than 150 MB"),
        ),
        (
            // Exactly the cap is read (and then sniffed): the cap was not crossed.
            Exchange {
                expect: Expect::download(signed),
                reply: Reply::Zeros {
                    status: 200,
                    headers: Vec::new(),
                    len: MAX_MODEL_BYTES,
                },
            },
            Collected::Ended {
                reason: "Tripo returned a model that is neither GLB nor binary FBX".into(),
            },
        ),
        (
            Expect::download(signed).reply(HttpResponse::new(200, b"PK\x03\x04rest".to_vec())),
            Collected::Ended {
                reason: "Tripo returned a model that is neither GLB nor binary FBX".into(),
            },
        ),
    ];
    for (exchange, expected) in cases {
        let h = harness(vec![
            success("task1", json!({"model": signed}), json!(125)),
            exchange,
        ]);
        let outcome = h.collect(&front_call(), handle("task1"));
        assert_eq!(outcome, expected);
        assert!(!format!("{outcome:?}").contains("signed-secret"));
        assert_eq!(h.transport.requests().len(), 2, "one GET per URL");
        h.done();
    }
}

#[test]
fn the_cost_is_the_finished_tasks_credits_rounded_up() {
    // The round-up rule of spec/providers.md §6.
    let cases: Vec<(Option<Value>, Option<Usd>)> = vec![
        (Some(json!(125)), Some(Usd(1_250_000))),
        (Some(json!(12.5)), Some(Usd(125_000))),
        (Some(json!(0)), Some(Usd(0))),
        (Some(json!("125")), Some(Usd(1_250_000))),
        (Some(json!(0.00001)), Some(Usd(1))),
        (Some(json!(0.00005)), Some(Usd(1))),
        (Some(json!(0.00015)), Some(Usd(2))),
        (Some(json!(true)), None),
        (Some(json!(-1)), None),
        (Some(Value::Null), None),
        (None, None),
        (Some(json!("abc")), None),
        (Some(json!({})), None),
    ];
    for (credits, cost) in cases {
        let mut data =
            json!({"task_id": "task1", "status": "success", "output": {"model": STORAGE}});
        if let Some(credits) = &credits {
            data["credits_consumed"] = credits.clone();
        }
        let h = harness(vec![
            read_expect("task1").reply(ok(data)),
            download(STORAGE, &fbx(1)),
        ]);
        let answer = answered(h.collect(&front_call(), handle("task1")));
        assert_eq!(answer.cost, cost, "{credits:?}");
        h.done();
    }
}

#[test]
fn a_later_run_only_collects() {
    // spec/providers.md §7: a fresh adapter collects by the handle alone.
    let h = harness(vec![
        read("task7", "running"),
        success("task7", json!({"model": STORAGE}), json!("2.5")),
        download(STORAGE, &fbx(7)),
    ]);
    let answer = answered(h.collect(&front_call(), handle("task7")));
    assert_eq!(answer.cost, Some(Usd(25_000)));
    assert!(
        h.methods_and_urls()
            .iter()
            .all(|(method, _)| *method == Method::Get)
    );
    h.done();

    // A handle this adapter did not write: nothing is requested.
    for bad in [
        json!({}),
        json!({"task_id": 7}),
        json!({"task_id": "task/7"}),
        json!({"task_id": ""}),
        json!(null),
    ] {
        let outcome = block_on(offline_mesh(test_keys()).collect(&front_call(), &bad));
        assert_eq!(
            outcome,
            Collected::Unreachable {
                reason: "the Tripo handle has no usable task id".into()
            },
            "{bad}"
        );
    }
    // Without the key nothing is requested, and the job stays outstanding.
    let outcome = block_on(offline_mesh(Keys::none()).collect(&front_call(), &handle("task7")));
    assert_eq!(
        outcome,
        Collected::Unreachable {
            reason: "TRIPO_API_KEY is not set".into()
        }
    );
    // wait() refuses an id that is not Tripo's before any request.
    let setup = setup_with_clock(Arc::new(NoNetwork), test_keys(), Arc::new(FakeClock::new()));
    assert_eq!(
        block_on(TripoApi::new(&setup).wait("../files", Duration::from_secs(10))),
        Waited::Unreachable("not a Tripo task id".into())
    );
}

#[test]
fn the_key_never_reaches_a_reason_a_handle_or_data() {
    // Credential hygiene across a whole submit and collect.
    let h = harness(vec![
        upload("front.png", "image/png", &front_png(), "tok1"),
        post_expect(ExpectBody::JsonText(FRONT_BODY.into())).reply(ok(json!({"task_id": "task1"}))),
        success("task1", json!({"model": STORAGE}), json!(125)),
        download(STORAGE, &fbx(1)),
    ]);
    let call = front_call();
    let Submitted::Accepted { handle } = h.submit(&call) else {
        panic!("not accepted");
    };
    assert_eq!(handle, json!({"task_id": "task1"}));
    let answer = answered(h.collect(&call, handle));
    assert_clean(&answer.data.to_string());
    h.done();
}
