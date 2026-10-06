//! Reading one output picture out of a fal answer (spec/providers.md §8, §9.3): the object
//! `root.images[0]` of an image call or `root.image` of a background removal.
//!
//! In order:
//! 1. `url` is a non-blank string;
//! 2. `content_type`, when present and not `null`, normalizes to one of the route's media types;
//! 3. a data URL is decoded in place: its media type normalized the same way and equal to the
//!    declared one when both exist, strict base64, at most the cap;
//! 4. any other URL must pass [`super::is_fal_media_url`] before anything is requested; it is
//!    added to the redactor, then fetched **once** on the download lane with only
//!    `accept: image/*`, no credential, no redirect, the cap and the time left. A non-2xx answer
//!    is a failure; a `content-length`, when present, is a whole number within the cap; a
//!    `content-type`, when present, normalizes to one of the route's types and agrees with the
//!    declared one; the body is not empty;
//! 5. the bytes' sniffed kind (PNG, JPEG, WebP or GIF) equals the declared type, else the header's
//!    type, when there is one;
//! 6. for an image call, `width` and `height`: both or neither, positive integers, equal to the
//!    decoded size.
//!
//! Every failure here is structural: fal took (and billed) the request, so the caller reports
//! `Failed { cost, retryable: true }` with the returned sentence, already redacted and bounded.
//! The URL never reaches a sentence or an answer.

use crate::setup::Client;
use crate::transport::{HttpRequest, Lane, Method};
use serde_json::{Map, Value};
use std::time::Duration;

/// The label of a download's reasons.
pub(crate) const DOWNLOAD_LABEL: &str = "fal output image download";

/// What a route accepts of an output picture.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rules {
    /// The declared media types allowed, normalized.
    pub allowed: &'static [&'static str],
    /// Whether `width`/`height` are checked against the decoded picture (images only; a
    /// background removal's are informational).
    pub check_dimensions: bool,
    /// The most bytes of the picture, decoded or downloaded.
    pub max_bytes: u64,
    /// How `allowed` reads in a sentence (`PNG, JPEG, or WebP`).
    pub allowed_words: &'static str,
}

/// A picture read from an answer: its sniffed kind and bytes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Picture {
    pub kind: &'static str,
    pub bytes: Vec<u8>,
}

/// Reads the picture `image` names (module doc). `remaining` bounds the download. The error is a
/// sentence, redacted (with the URL added) and bounded.
pub(crate) async fn read_picture(
    client: &Client,
    image: &Map<String, Value>,
    rules: Rules,
    remaining: Duration,
) -> Result<Picture, String> {
    let url = match image.get("url") {
        Some(Value::String(url)) if !url.trim().is_empty() => url.as_str(),
        _ => return Err(client.reason("fal output image url must be non-empty")),
    };
    let redactor = client.redactor.with(url);
    let fail = |text: &str| redactor.reason(text);
    let declared = match image.get("content_type") {
        None | Some(Value::Null) => None,
        Some(value) => Some(media_type(value, rules).map_err(|text| fail(&text))?),
    };
    let (bytes, expected) = match crate::wire::parse_data_url(url) {
        Some((media, payload)) => {
            let media = media_type(&Value::String(media), rules).map_err(|text| fail(&text))?;
            if declared.is_some_and(|declared| declared != media) {
                return Err(fail(
                    "fal data URI media type does not match response metadata",
                ));
            }
            if (payload.len() as u64 / 4).saturating_mul(3) > rules.max_bytes.saturating_add(2) {
                return Err(fail(&too_large(rules)));
            }
            let bytes = crate::wire::strict_base64(&payload)
                .ok_or_else(|| fail("fal output image data is not strict base64"))?;
            if bytes.len() as u64 > rules.max_bytes {
                return Err(fail(&too_large(rules)));
            }
            (bytes, Some(media))
        }
        None => download(client, &redactor, url, declared, rules, remaining).await?,
    };
    let kind = crate::wire::sniff_image(&bytes)
        .ok_or_else(|| fail("fal output image is not a PNG, JPEG, WebP or GIF picture"))?;
    if let Some(expected) = expected
        && expected != kind
    {
        return Err(fail(&format!(
            "fal output image is {kind}, not the {expected} it was declared as"
        )));
    }
    if rules.check_dimensions {
        check_dimensions(image, kind, &bytes).map_err(|text| fail(&text))?;
    }
    Ok(Picture { kind, bytes })
}

/// Step 4 of the module doc: the bytes and the type they are expected to be.
async fn download(
    client: &Client,
    redactor: &crate::redact::Redactor,
    url: &str,
    declared: Option<&'static str>,
    rules: Rules,
    remaining: Duration,
) -> Result<(Vec<u8>, Option<&'static str>), String> {
    let fail = |text: &str| redactor.reason(text);
    let Some(target) = super::fal_media_url(url) else {
        return Err(fail(
            "fal hosted output must use HTTPS on fal.media without userinfo or a custom port",
        ));
    };
    let redactor = redactor.with(target.as_str());
    let fail = |text: &str| redactor.reason(text);
    if remaining.is_zero() {
        return Err(fail(&format!(
            "{DOWNLOAD_LABEL} was not sent: the deadline has passed"
        )));
    }
    let request = HttpRequest::new(Method::Get, target.as_str(), Lane::Download)
        .header("accept", "image/*")
        .timeout(remaining)
        .max_response_bytes(rules.max_bytes);
    let response = match client.send(request).await {
        Ok(response) => response,
        Err(error) => {
            return Err(fail(&format!("{DOWNLOAD_LABEL} failed: {}", error.reason)));
        }
    };
    if !response.is_success() {
        return Err(fail(&super::status_text(DOWNLOAD_LABEL, &response)));
    }
    if let Some(length) = response.header("content-length") {
        let length: u64 = length
            .trim()
            .parse()
            .map_err(|_| fail("fal output content-length must be a non-negative integer"))?;
        if length > rules.max_bytes {
            return Err(fail(&too_large(rules)));
        }
    }
    let header = match response.header("content-type") {
        None => None,
        Some(value) => {
            Some(media_type(&Value::String(value.to_string()), rules).map_err(|text| fail(&text))?)
        }
    };
    if let (Some(declared), Some(header)) = (declared, header)
        && declared != header
    {
        return Err(fail(
            "fal output download media type does not match response metadata",
        ));
    }
    if response.body.is_empty() {
        return Err(fail("fal output image download was empty"));
    }
    if response.body.len() as u64 > rules.max_bytes {
        return Err(fail(&too_large(rules)));
    }
    Ok((response.body, declared.or(header)))
}

/// A declared media type, normalized within `image` and one of the route's (`allowed`).
fn media_type(value: &Value, rules: Rules) -> Result<&'static str, String> {
    let refused = || format!("fal image media type must be {}", rules.allowed_words);
    let Value::String(text) = value else {
        return Err(refused());
    };
    let normalized = crate::wire::normalize_media_type(text, "image").map_err(|_| refused())?;
    rules
        .allowed
        .iter()
        .copied()
        .find(|allowed| *allowed == normalized)
        .ok_or_else(refused)
}

fn too_large(rules: Rules) -> String {
    const MIB: u64 = 1024 * 1024;
    if rules.max_bytes >= MIB && rules.max_bytes.is_multiple_of(MIB) {
        format!(
            "fal output image exceeds the {} MiB safety limit",
            rules.max_bytes / MIB
        )
    } else {
        format!(
            "fal output image exceeds the {} byte safety limit",
            rules.max_bytes
        )
    }
}

/// Step 6 of the module doc.
fn check_dimensions(image: &Map<String, Value>, kind: &str, bytes: &[u8]) -> Result<(), String> {
    let width = dimension(image, "width")?;
    let height = dimension(image, "height")?;
    let (width, height) = match (width, height) {
        (None, None) => return Ok(()),
        (Some(width), Some(height)) => (width, height),
        _ => {
            return Err(
                "fal output image dimensions must include both width and height".to_string(),
            );
        }
    };
    let facts = match grida_fx_core::facts::image_facts(bytes, kind) {
        Ok(Some(facts)) => facts,
        _ => return Err("fal output image is not decodable".to_string()),
    };
    if (facts.width, facts.height) != (width, height) {
        return Err("fal output image dimensions do not match decoded bytes".to_string());
    }
    Ok(())
}

/// `width` or `height`: absent or `null`, else a positive integer (not a boolean, not `32.0`).
fn dimension(image: &Map<String, Value>, key: &str) -> Result<Option<u32>, String> {
    match image.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("fal output image {key} must be a positive integer")),
        Some(_) => Err(format!("fal output image {key} must be a positive integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::keys::{KeyName, Keys};
    use crate::setup::{Endpoints, Setup};
    use crate::testing::{block_on, media};
    use crate::transport::HttpResponse;
    use crate::transport::replay::{Exchange, Expect, NoNetwork, ReplayTransport, Reply};
    use serde_json::json;
    use std::sync::Arc;

    const RULES: Rules = Rules {
        allowed: &["image/png", "image/jpeg", "image/webp"],
        check_dimensions: true,
        max_bytes: 8,
        allowed_words: "PNG, JPEG, or WebP",
    };

    fn client(transport: Arc<dyn crate::transport::Transport>) -> Client {
        let setup = Setup {
            transport,
            keys: Keys::from_pairs(&[(KeyName::Fal, "test-fal-key")]),
            endpoints: Endpoints::default(),
            clock: Arc::new(FakeClock::new()),
        };
        super::super::FalClients::new(&setup).run
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_body_crossing_a_small_cap_is_refused() {
        let url = "https://v3b.fal.media/files/a.png";
        let transport = Arc::new(ReplayTransport::new(vec![
            Expect::download(url)
                .header("accept", "image/*")
                .reply(HttpResponse::new(200, b"123456789".to_vec())),
        ]));
        let error = block_on(read_picture(
            &client(transport.clone()),
            &object(json!({"url": url})),
            RULES,
            Duration::from_secs(5),
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "fal output image download failed: the response is larger than 8 bytes"
        );
        assert_eq!(transport.requests()[0].max_response_bytes, 8);
        assert_eq!(transport.requests()[0].timeout, Duration::from_secs(5));
        transport.assert_done();
    }

    #[test]
    fn an_oversized_data_url_is_refused_before_decoding() {
        let url = crate::wire::data_url("image/png", &[0u8; 64]);
        let error = block_on(read_picture(
            &client(Arc::new(NoNetwork)),
            &object(json!({"url": url})),
            RULES,
            Duration::from_secs(5),
        ))
        .unwrap_err();
        assert_eq!(error, "fal output image exceeds the 8 byte safety limit");
    }

    #[test]
    fn no_time_left_requests_nothing() {
        let error = block_on(read_picture(
            &client(Arc::new(NoNetwork)),
            &object(json!({"url": "https://v3b.fal.media/a.png"})),
            RULES,
            Duration::ZERO,
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "fal output image download was not sent: the deadline has passed"
        );
    }

    #[test]
    fn a_zero_length_reply_over_the_cap_is_never_allocated() {
        let url = "https://v3b.fal.media/files/a.png";
        let transport = Arc::new(ReplayTransport::new(vec![Exchange {
            expect: Expect::download(url).header("accept", "image/*"),
            reply: Reply::Zeros {
                status: 200,
                headers: Vec::new(),
                len: 9,
            },
        }]));
        let error = block_on(read_picture(
            &client(transport.clone()),
            &object(json!({"url": url})),
            RULES,
            Duration::from_secs(5),
        ))
        .unwrap_err();
        assert!(error.contains("larger than 8 bytes"), "{error}");
        transport.assert_done();
    }

    #[test]
    fn dimensions_are_both_or_neither_and_match() {
        let png = media::png(2, 3, None);
        assert_eq!(
            check_dimensions(&object(json!({})), "image/png", &png),
            Ok(())
        );
        assert_eq!(
            check_dimensions(&object(json!({"width": 2, "height": 3})), "image/png", &png),
            Ok(())
        );
        assert_eq!(
            check_dimensions(
                &object(json!({"width": 2, "height": null})),
                "image/png",
                &png
            )
            .unwrap_err(),
            "fal output image dimensions must include both width and height"
        );
        assert_eq!(
            check_dimensions(&object(json!({"width": 3, "height": 2})), "image/png", &png)
                .unwrap_err(),
            "fal output image dimensions do not match decoded bytes"
        );
        for bad in [json!(true), json!(2.0), json!(0), json!(-2), json!("2")] {
            assert_eq!(
                check_dimensions(
                    &object(json!({"width": bad, "height": 3})),
                    "image/png",
                    &png
                )
                .unwrap_err(),
                "fal output image width must be a positive integer"
            );
        }
        assert_eq!(
            check_dimensions(
                &object(json!({"width": 2, "height": 3})),
                "image/jpeg",
                media::JPEG_HEAD
            )
            .unwrap_err(),
            "fal output image is not decodable"
        );
    }

    #[test]
    fn media_types_are_normalized_within_the_route() {
        assert_eq!(media_type(&json!("Image/JPG"), RULES), Ok("image/jpeg"));
        assert_eq!(media_type(&json!("image/png; q=1"), RULES), Ok("image/png"));
        for bad in [
            json!("image/gif"),
            json!("application/octet-stream"),
            json!(""),
            json!(1),
        ] {
            assert_eq!(
                media_type(&bad, RULES).unwrap_err(),
                "fal image media type must be PNG, JPEG, or WebP"
            );
        }
    }
}
