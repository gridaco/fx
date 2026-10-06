//! The transport's pure parts (spec/providers.md §2, §4.3, §8): multipart encoding, the default
//! transport's configuration, the `retry_after` a rate-limited response asks for, and the
//! redaction every reason passes through.
//!
//! The default reqwest transport is never constructed here (spec/providers.md §2); its header,
//! body, cap and classification logic is unit-tested inside `transport::http`. Nothing here sends.

use grida_fx_providers::adapter::{RETRY_AFTER_LIMIT, retry_after_at};
use grida_fx_providers::keys::{KeyName, Keys};
use grida_fx_providers::testing::{setup, test_keys};
use grida_fx_providers::transport::http::HttpConfig;
use grida_fx_providers::transport::multipart::{boundary, content_type, encode};
use grida_fx_providers::transport::replay::NoNetwork;
use grida_fx_providers::transport::{HttpResponse, Part};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The first bytes of a PNG: enough for a file part (synthesized here, not an image).
const PNG_HEAD: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

fn two_parts() -> Vec<Part> {
    vec![
        Part::text("model", "img-a"),
        Part::file("image", "a.png", "image/png", PNG_HEAD.to_vec()),
    ]
}

#[test]
fn a_text_field_and_a_file_part_encode_exactly() {
    let parts = two_parts();
    let b = boundary(&parts);
    // SHA-256 over the length-prefixed fields, computed independently of this crate.
    assert_eq!(b, "----grida-fx-0c98ece48a9eb5053f8738d8d2fa3f55");
    let mut expected = Vec::new();
    expected.extend_from_slice(
        format!(
            "--{b}\r\n\
             Content-Disposition: form-data; name=\"model\"\r\n\
             \r\n\
             img-a\r\n\
             --{b}\r\n\
             Content-Disposition: form-data; name=\"image\"; filename=\"a.png\"\r\n\
             Content-Type: image/png\r\n\
             \r\n"
        )
        .as_bytes(),
    );
    expected.extend_from_slice(PNG_HEAD);
    expected.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    assert_eq!(encode(&parts, &b), expected);
    assert_eq!(
        content_type(&b),
        "multipart/form-data; boundary=----grida-fx-0c98ece48a9eb5053f8738d8d2fa3f55"
    );
}

#[test]
fn the_same_parts_always_encode_to_the_same_bytes() {
    let first = boundary(&two_parts());
    assert_eq!(boundary(&two_parts()), first);
    assert_eq!(
        encode(&two_parts(), &first),
        encode(&two_parts(), &boundary(&two_parts()))
    );
    let mut changed = two_parts();
    changed[1].data.push(0);
    assert_ne!(boundary(&changed), first, "the boundary follows the bytes");
    let renamed = vec![
        Part::text("model", "img-a"),
        Part::file("image[]", "a.png", "image/png", PNG_HEAD.to_vec()),
    ];
    assert_ne!(boundary(&renamed), first);
}

#[test]
fn no_parts_encode_to_the_closing_delimiter() {
    let b = boundary(&[]);
    assert!(b.starts_with("----grida-fx-"));
    assert_eq!(encode(&[], &b), format!("--{b}--\r\n").into_bytes());
}

#[test]
fn a_boundary_never_occurs_in_a_part() {
    // A part carrying the boundary the plain parts would get: the boundary moves away.
    let plain = boundary(&two_parts());
    let mut carrying = two_parts();
    carrying.push(Part::file(
        "file",
        "b.bin",
        "application/octet-stream",
        format!("\r\n--{plain}\r\n").into_bytes(),
    ));
    let b = boundary(&carrying);
    assert_ne!(b, plain);
    for part in &carrying {
        assert!(
            !part
                .data
                .windows(b.len())
                .any(|window| window == b.as_bytes())
        );
    }
}

#[test]
fn names_and_file_names_are_quoted_on_one_line() {
    let parts = vec![Part {
        name: "a\"b\\c".into(),
        filename: Some("x\r\ny\".png".into()),
        content_type: Some("image/png\r\nX-Injected: 1".into()),
        data: b"d".to_vec(),
    }];
    let b = boundary(&parts);
    let text = String::from_utf8(encode(&parts, &b)).expect("ascii");
    assert_eq!(
        text,
        format!(
            "--{b}\r\n\
             Content-Disposition: form-data; name=\"a\\\"b\\\\c\"; filename=\"x  y\\\".png\"\r\n\
             Content-Type: image/png  X-Injected: 1\r\n\
             \r\n\
             d\r\n\
             --{b}--\r\n"
        )
    );
}

#[test]
fn the_default_transport_is_configured_not_built() {
    let config = HttpConfig::default();
    assert_eq!(config.connect_timeout, Duration::from_secs(30));
    assert!(config.user_agent.starts_with("grida-fx/"));
}

// ---------------------------------------------------------------------------------------------
// retry_after (spec/providers.md §4.3: a 429 is NotReceived, and the wait it asks for is kept)

fn rate_limited(headers: &[(&str, &str)]) -> HttpResponse {
    headers
        .iter()
        .fold(HttpResponse::new(429, Vec::new()), |response, (n, v)| {
            response.with_header(n, v)
        })
}

#[test]
fn a_rate_limit_carries_the_wait_it_asks_for() {
    let at = |headers: &[(&str, &str)]| retry_after_at(&rate_limited(headers), UNIX_EPOCH);
    assert_eq!(at(&[("Retry-After", "20")]), Some(Duration::from_secs(20)));
    assert_eq!(
        at(&[("retry-after-ms", "1500")]),
        Some(Duration::from_millis(1500))
    );
    assert_eq!(at(&[("x-ratelimit-reset-requests", "1s")]), None);
    assert_eq!(at(&[("retry-after", "later")]), None);
    assert_eq!(at(&[("retry-after", "86401")]), Some(RETRY_AFTER_LIMIT));
    let date = UNIX_EPOCH + Duration::from_secs(784_111_747);
    assert_eq!(
        retry_after_at(
            &rate_limited(&[("retry-after", "Sun, 06 Nov 1994 08:49:37 GMT")]),
            date
        ),
        Some(Duration::from_secs(30))
    );
}

// ---------------------------------------------------------------------------------------------
// Reasons (spec/providers.md §8)

#[test]
fn a_clients_reasons_hold_no_key_and_no_secret_shape() {
    let setup = setup(Arc::new(NoNetwork), test_keys());
    let client = setup.client(KeyName::Fal, &setup.endpoints.fal_run);
    let key = test_keys()
        .get(KeyName::Fal)
        .expect("a made-up fal key")
        .expose()
        .to_string();
    let payload = "QUJD".repeat(30);
    let reason = client.reason(&format!(
        "fal image generation failed: key {key}, authorization: Key {key}, \
         url https://v3.fal.media/files/out.png?token=abc, \
         body {{\"image_data\": \"{payload}\", \"api_key\": \"x-1\"}}, \
         picture data:image/png;base64,{payload}, sk-or-v1-0123456789abcdef"
    ));
    assert!(!reason.contains(&key), "{reason}");
    assert!(!reason.contains(&payload), "{reason}");
    assert!(!reason.contains("token=abc"), "{reason}");
    assert!(!reason.contains("x-1"), "{reason}");
    assert!(!reason.contains("0123456789abcdef"), "{reason}");
    assert_eq!(
        reason,
        "fal image generation failed: key [redacted], authorization: [redacted], \
         url https://v3.fal.media/files/out.png?[redacted] \
         body {\"image_data\": \"[redacted]\", \"api_key\": \"[redacted]\"}, \
         picture data:image/png;base64,[redacted], [redacted]"
    );
    // A signed URL an adapter adds is replaced whole.
    let signed = "https://cdn.example.test/out/a.glb?Expires=1&Signature=s";
    let with = client.redactor.with(signed);
    assert_eq!(
        with.reason(&format!("download of {signed} failed")),
        "download of [redacted] failed"
    );
    assert!(Keys::none().secrets().is_empty());
}

/// spec/providers.md §2 item 5: a download carries no credential, through the credential slot or
/// as a header named like one, in any case; the provider lane may not set the credential's header
/// by hand either. Each is refused before anything leaves.
#[test]
fn a_download_refuses_every_header_named_like_a_credential() {
    use grida_fx_providers::transport::{
        Credential, HttpRequest, Lane, Method, Phase, TransportErrorKind, check_request,
    };
    const SIGNED: &str = "https://files.example.test/out/a.png?X-Amz-Signature=deadbeef&e=1";
    for (name, value) in [
        ("authorization", "Key test-key-not-real-fal"),
        ("Authorization", "Bearer test-key-not-real"),
        ("proxy-authorization", "Basic dGVzdA=="),
        ("xi-api-key", "test-key-not-real-el"),
        ("X-API-Key", "test-key-not-real"),
        ("cookie", "session=test-key-not-real"),
        ("x-auth-token", "t"),
        ("x-session-id", "s"),
    ] {
        let request = HttpRequest::new(Method::Get, SIGNED, Lane::Download).header(name, value);
        let error = check_request(&request).expect_err(name);
        assert_eq!(error.phase, Phase::NotSent, "{name}");
        assert_eq!(error.kind, TransportErrorKind::Refused, "{name}");
        assert_eq!(error.reason, "a download never carries a credential");
    }
    let accept = HttpRequest::new(Method::Get, SIGNED, Lane::Download).header("accept", "image/*");
    assert_eq!(check_request(&accept), Ok(()));
    let mut by_hand = HttpRequest::new(Method::Post, "https://api.example.test/v1", Lane::Provider)
        .credential(Credential {
            header: "xi-api-key",
            prefix: "",
            secret: grida_fx_providers::Secret::new("test-key-not-real"),
        });
    by_hand
        .headers
        .push(("XI-API-KEY".into(), "test-key-not-real".into()));
    assert!(check_request(&by_hand).is_err());
}

/// `Debug` of a request or a response never shows a signed URL's path or query, a header's value,
/// a credential or a body: a log line or a panic message may print one.
#[test]
fn requests_and_responses_debug_without_urls_values_or_bodies() {
    use grida_fx_providers::transport::{Body, Credential, HttpRequest, Lane, Method};
    let request = HttpRequest::new(
        Method::Get,
        "https://v3b.fal.media/files/acme/clip.mp4?X-Amz-Signature=deadbeefcafe0123",
        Lane::Download,
    )
    .header("accept", "video/mp4");
    let shown = format!("{request:?}");
    assert!(!shown.contains("deadbeefcafe0123"), "{shown}");
    assert!(!shown.contains("/files/acme"), "{shown}");
    assert!(shown.contains("https://v3b.fal.media"), "{shown}");
    assert!(shown.contains("\"accept\""), "{shown}");
    assert!(!shown.contains("video/mp4"), "{shown}");
    let posted = HttpRequest::new(
        Method::Post,
        "https://api.example.test/v1/x",
        Lane::Provider,
    )
    .credential(Credential {
        header: "authorization",
        prefix: "Bearer ",
        secret: grida_fx_providers::Secret::new("test-key-not-real-debug"),
    })
    .body(Body::Json(serde_json::json!({
        "prompt": "a secret prompt",
        "image": "data:image/png;base64,iVBORw0KGgo="
    })));
    let shown = format!("{posted:?}");
    for hidden in ["test-key-not-real-debug", "a secret prompt", "iVBORw0KGgo"] {
        assert!(!shown.contains(hidden), "{shown}");
    }
    let response = HttpResponse::new(
        200,
        br#"{"video":{"url":"https://v3b.fal.media/x?sig=1"}}"#.to_vec(),
    )
    .with_header("location", "https://v3b.fal.media/x?sig=1");
    let shown = format!("{response:?}");
    assert!(!shown.contains("sig=1"), "{shown}");
    assert!(
        shown.contains("200") && shown.contains("\"location\""),
        "{shown}"
    );
}
