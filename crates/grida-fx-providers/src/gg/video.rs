//! GG video: `video.generate`, a long job (src/gg/README.md; spec/adapters.md §6) run by
//! [`super::job`].
//!
//! `submit`: one `POST {base}/videos` with `{model, prompt, duration?, aspectRatio?, resolution?,
//! providerOptions?}` and the attempt's `Idempotency-Key` ([`super::Gateway`]): `duration` a JSON
//! integer when whole; `resolution` sent as is when it is `WIDTHxHEIGHT`, and a tier name (`720p`,
//! `4k`) as `providerOptions: {gg: {resolutionTier}}`; `aspect_ratio` as `aspectRatio`. A request
//! with a `first_frame` goes as `multipart/form-data` instead: that JSON as the text part `input`,
//! then the frame as the file part `image`. GG takes no last frame.
//!
//! `collect`: [`super::job::Jobs::collect`], reading `result.videos[0]`. The bytes decide: the
//! file is the clip when it carries the MP4 signature and an MP4 video track. The answer is file
//! `video` (`video/mp4`) with `data: {"facts": {"width", "height", "duration_seconds", "fps"}}`.
//! `check`: the clip runs within [`DURATION_TOLERANCE`] of a requested `duration`, and has the
//! exact size of a requested `WIDTHxHEIGHT`.
//!
//! Submit refusals, in this order (spec/adapters.md §4): another route's contract; a request that
//! does not fit `video.generate`; no `GG_API_KEY`; a blank prompt; a last frame; a
//! `resolution` that is neither a tier name nor `WIDTHxHEIGHT`; an `aspect_ratio` that is not
//! `W:H`; a first frame whose file the call does not carry, is empty or is not PNG, JPEG or WebP.
//! The model's own limits (durations, tiers, ratios, which models take a frame) are GG's to
//! refuse, at no cost.

use super::Gateway;
use super::job::{Jobs, Kind};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Collected, LongJob, Submitted};
use crate::capabilities;
use crate::checks::ClipFacts;
use crate::clock::Clock;
use crate::transport::{Body, HttpRequest, Lane, Method, Part};
use crate::wire;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::{Arc, LazyLock};

pub use super::job::{COLLECT_DEADLINE, POLL, READ_DEADLINE, SETTLE_WAIT, SUBMIT_DEADLINE};

/// The contract `adapter` this adapter serves.
pub const ADAPTER: &str = "gg-video";

/// The most bytes of one clip.
pub const MAX_VIDEO_BYTES: u64 = 512 * 1024 * 1024;

/// How far a clip's duration may be from the requested one.
pub const DURATION_TOLERANCE: f64 = 0.5;

/// The label of the submit's reasons.
pub const SUBMIT_LABEL: &str = "GG video submission";

/// The label of a job read's reasons.
pub const JOB_LABEL: &str = "GG video job";

/// The label of the clip download's reasons.
pub const DOWNLOAD_LABEL: &str = "GG output video download";

/// The sentence of a job still running at the deadline.
pub const OUTSTANDING: &str =
    "GG has not finished the video job yet; it is collected on the next run";

/// The kinds of a first frame, with their file name extensions.
const FRAME_KINDS: [(&str, &str); 3] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
];

/// A video job (module doc).
pub const KIND: Kind = Kind {
    noun: "video",
    submit_label: SUBMIT_LABEL,
    job_label: JOB_LABEL,
    download_label: DOWNLOAD_LABEL,
    outstanding: OUTSTANDING,
    reference: |result| result.get("videos")?.as_array()?.first(),
    max_bytes: MAX_VIDEO_BYTES,
    deliver,
};

static PIXELS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([1-9][0-9]{0,4})x([1-9][0-9]{0,4})$").expect("a valid pattern")
});

static TIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]{1,16}$").expect("a valid pattern"));

static RATIO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{1,4}(?:\.[0-9]{1,4})?:[0-9]{1,4}(?:\.[0-9]{1,4})?$")
        .expect("a valid pattern")
});

/// GG's video adapter (module doc).
#[derive(Debug, Clone)]
pub struct GgVideo {
    jobs: Jobs,
}

impl GgVideo {
    pub fn new(gateway: Gateway, clock: Arc<dyn Clock>) -> GgVideo {
        GgVideo {
            jobs: Jobs::new(gateway, clock),
        }
    }
}

/// The request's values, once they pass (step 4 of spec/adapters.md §4).
#[derive(Debug, Clone, PartialEq)]
struct Clip<'a> {
    prompt: &'a str,
    duration: Option<f64>,
    resolution: Option<&'a str>,
    aspect_ratio: Option<&'a str>,
}

/// The value refusals of the module doc, in order. The request already fits `video.generate`.
fn clip(request: &Value) -> Result<Clip<'_>, String> {
    let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("a clip needs its prompt as text".into());
    }
    if !wire::is_absent(request.get("last_frame")) {
        return Err("GG video takes no last frame".into());
    }
    let duration = request.get("duration").and_then(Value::as_f64);
    let text = |name: &str| request.get(name).and_then(Value::as_str);
    let resolution = text("resolution");
    if let Some(resolution) = resolution
        && !PIXELS.is_match(resolution)
        && !TIER.is_match(resolution)
    {
        return Err("resolution is a tier name, such as 720p, or WIDTHxHEIGHT".into());
    }
    let aspect_ratio = text("aspect_ratio");
    if let Some(ratio) = aspect_ratio
        && !RATIO.is_match(ratio)
    {
        return Err("aspect_ratio is W:H, such as 16:9".into());
    }
    Ok(Clip {
        prompt,
        duration,
        resolution,
        aspect_ratio,
    })
}

/// The submit body (module doc), without the frame.
fn body(model: &str, clip: &Clip<'_>) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(model.trim()));
    body.insert("prompt".into(), json!(clip.prompt));
    if let Some(seconds) = clip.duration {
        let whole = seconds.fract() == 0.0 && seconds.abs() < 9_007_199_254_740_992.0;
        body.insert(
            "duration".into(),
            if whole {
                json!(seconds as i64)
            } else {
                grida_fx_core::value::number(seconds).unwrap_or(Value::Null)
            },
        );
    }
    if let Some(ratio) = clip.aspect_ratio {
        body.insert("aspectRatio".into(), json!(ratio));
    }
    match clip.resolution {
        Some(pixels) if PIXELS.is_match(pixels) => {
            body.insert("resolution".into(), json!(pixels));
        }
        Some(tier) => {
            body.insert(
                "providerOptions".into(),
                json!({"gg": {"resolutionTier": tier}}),
            );
        }
        None => {}
    }
    Value::Object(body)
}

/// The first frame's part, when the request has one: `image`, named `first-frame.<ext>`.
fn frame(call: &CallRequest) -> Result<Option<Part>, String> {
    let member = "first_frame";
    let value = match call.request.get(member) {
        None | Some(Value::Null) => return Ok(None),
        Some(value) => value,
    };
    let file = wire::request_file(call, value, member)?;
    let Some((kind, extension)) = FRAME_KINDS
        .iter()
        .copied()
        .find(|(kind, _)| *kind == file.kind)
    else {
        return Err(format!(
            "{member} is {}; GG takes PNG, JPEG or WebP",
            file.kind
        ));
    };
    let bytes = wire::read_file(file, member)?;
    Ok(Some(Part::file(
        "image",
        &format!("first-frame.{extension}"),
        kind,
        bytes,
    )))
}

/// The size a `WIDTHxHEIGHT` names.
fn pixels(text: &str) -> Option<(u32, u32)> {
    let captures = PIXELS.captures(text)?;
    Some((captures[1].parse().ok()?, captures[2].parse().ok()?))
}

/// Reads this adapter's route contract (spec/adapters.md §4 step 1).
fn contract(call: &CallRequest) -> Result<(), String> {
    super::read_contract(call, &["video.generate"], ADAPTER, |v| v == &json!(1), &[]).map(|_| ())
}

/// The clip the download holds (module doc).
fn deliver(bytes: Vec<u8>) -> Result<Answer, String> {
    if !wire::matches_signature("video/mp4", &bytes) {
        return Err("GG output video is not an MP4 file".into());
    }
    let facts = ClipFacts::of_mp4(&bytes)?;
    Ok(Answer::new(json!({"facts": facts.to_json()}), None).with_file("video", "video/mp4", bytes))
}

impl GgVideo {
    /// Steps 1 to 5 of spec/adapters.md §4, then the submit request (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        contract(call)?;
        capabilities::check_request(&call.route.capability, &call.request)?;
        let gateway = &self.jobs.gateway;
        let credential = gateway.credential()?;
        let clip = clip(&call.request)?;
        let input = body(&call.route.model, &clip);
        let body = match frame(call)? {
            None => Body::Json(input),
            Some(image) => Body::Multipart(vec![Part::text("input", &input.to_string()), image]),
        };
        Ok(gateway.paid(
            HttpRequest::new(Method::Post, gateway.client.url("videos"), Lane::Provider)
                .body(body)
                .timeout(SUBMIT_DEADLINE)
                .max_response_bytes(super::job::JSON_RESPONSE_BYTES),
            credential,
            call,
        ))
    }
}

impl LongJob for GgVideo {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(async move { self.jobs.submit(call, &KIND, self.prepare(call)).await })
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(async move {
            let id = super::job::job_id(handle);
            self.jobs.collect(id, &KIND, contract(call)).await
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(file) = answer.files.get("video") else {
            return Err("the answer carries no video".into());
        };
        let facts = ClipFacts::of_mp4(&file.bytes)?;
        check_clip(&facts, &call.request)
    }
}

/// The check of a collected clip (module doc): `the clip runs <x> s, not <n> s`; `the clip is
/// <w>x<h>, not <W>x<H>`.
pub fn check_clip(facts: &ClipFacts, request: &Value) -> Result<(), String> {
    if let Some(seconds) = request.get("duration").and_then(Value::as_f64)
        && (facts.duration_seconds - seconds).abs() > DURATION_TOLERANCE
    {
        return Err(format!(
            "the clip runs {:.3} s, not {} s",
            facts.duration_seconds,
            grida_fx_core::value::format_number(seconds)
        ));
    }
    if let Some((width, height)) = request
        .get("resolution")
        .and_then(Value::as_str)
        .and_then(pixels)
        && (facts.width, facts.height) != (width, height)
    {
        return Err(format!(
            "the clip is {}x{}, not {width}x{height}",
            facts.width, facts.height
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gg::job::result_file;

    #[test]
    fn values_are_refused_in_order() {
        let ok =
            json!({"prompt": "p", "duration": 5, "resolution": "720p", "aspect_ratio": "16:9"});
        assert_eq!(
            clip(&ok),
            Ok(Clip {
                prompt: "p",
                duration: Some(5.0),
                resolution: Some("720p"),
                aspect_ratio: Some("16:9"),
            })
        );
        let with = |member: &str, value: Value| {
            let mut request = ok.clone();
            request[member] = value;
            clip(&request).unwrap_err()
        };
        assert_eq!(
            with("prompt", json!(" ")),
            "a clip needs its prompt as text"
        );
        assert_eq!(
            with("last_frame", json!({"file": "a"})),
            "GG video takes no last frame"
        );
        assert!(clip(&json!({"prompt": "p", "first_frame": {"file": "a"}})).is_ok());
        assert!(clip(&json!({"prompt": "p", "last_frame": null})).is_ok());
        for bad in ["", "720 p", "720_p", "a/b", "4k!"] {
            assert_eq!(
                with("resolution", json!(bad)),
                "resolution is a tier name, such as 720p, or WIDTHxHEIGHT",
                "{bad}"
            );
        }
        for bad in ["16x9", "wide", "16:", ":9"] {
            assert_eq!(
                with("aspect_ratio", json!(bad)),
                "aspect_ratio is W:H, such as 16:9",
                "{bad}"
            );
        }
        assert!(clip(&json!({"prompt": "p", "aspect_ratio": "19.5:9"})).is_ok());
    }

    #[test]
    fn bodies_name_a_tier_or_pixels() {
        let tiered =
            json!({"prompt": "p", "duration": 2.0, "resolution": "480p", "aspect_ratio": "9:16"});
        let tiered = clip(&tiered).unwrap();
        assert_eq!(
            serde_json::to_string(&body(" acme/vid-a ", &tiered)).unwrap(),
            r#"{"model":"acme/vid-a","prompt":"p","duration":2,"aspectRatio":"9:16","providerOptions":{"gg":{"resolutionTier":"480p"}}}"#
        );
        let pixels = json!({"prompt": "p", "duration": 4.5, "resolution": "1280x720"});
        let pixels = clip(&pixels).unwrap();
        assert_eq!(
            serde_json::to_string(&body("acme/vid-a", &pixels)).unwrap(),
            r#"{"model":"acme/vid-a","prompt":"p","duration":4.5,"resolution":"1280x720"}"#
        );
        let bare = json!({"prompt": "p"});
        let bare = clip(&bare).unwrap();
        assert_eq!(
            body("acme/vid-a", &bare),
            json!({"model": "acme/vid-a", "prompt": "p"})
        );
    }

    #[test]
    fn the_clip_is_the_first_video() {
        let job = |value: Value| value.as_object().unwrap().clone();
        let url = "https://media.example.test/v.mp4?sig=1";
        let available = job(json!({"status": "succeeded", "result_status": "available",
            "result": {"videos": [{"url": url, "media_type": null, "expires_at": null}]}}));
        assert_eq!(result_file(&available, &KIND), Ok(url));
        for bad in [
            "http://media.example.test/v.mp4",
            "https://u:p@media.example.test/v.mp4",
            "https://media.example.test/v.mp4#t",
        ] {
            let job = job(json!({"status": "succeeded", "result_status": "available",
                "result": {"videos": [{"url": bad}]}}));
            assert_eq!(
                result_file(&job, &KIND),
                Err("GG output video url must be https".into()),
                "{bad}"
            );
        }
        assert_eq!(
            result_file(&job(json!({"result": {"videos": []}})), &KIND),
            Err("GG video job result carries no video".into())
        );
        assert_eq!(
            deliver(b"not a clip".to_vec()).unwrap_err(),
            "GG output video is not an MP4 file"
        );
    }

    #[test]
    fn clips_are_checked_against_what_was_asked() {
        let facts = ClipFacts {
            width: 854,
            height: 480,
            duration_seconds: 2.0,
            fps: 30.0,
        };
        assert_eq!(check_clip(&facts, &json!({"prompt": "p"})), Ok(()));
        assert_eq!(
            check_clip(&facts, &json!({"duration": 2.4, "resolution": "480p"})),
            Ok(())
        );
        assert_eq!(
            check_clip(&facts, &json!({"duration": 5})).unwrap_err(),
            "the clip runs 2.000 s, not 5 s"
        );
        assert_eq!(
            check_clip(&facts, &json!({"resolution": "1280x720"})).unwrap_err(),
            "the clip is 854x480, not 1280x720"
        );
        assert_eq!(
            check_clip(&facts, &json!({"resolution": "854x480"})),
            Ok(())
        );
    }
}
