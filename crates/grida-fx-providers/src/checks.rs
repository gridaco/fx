//! The answer checks more than one route makes (spec/providers.md §4.4; spec/capabilities.md).
//!
//! Adapters' `check`s call these, so two routes never disagree on what "an exact size", "an MP3"
//! or "meets the schema" means, and each keeps its own sentences where they differ (a missing
//! file, an unreadable size). [`every_route`] is the check spec/capabilities.md gives a capability
//! for every route (its *Every route's check*, or *Check*; never *A route's check*): what an
//! answer no adapter of FX made is held to, a stand-in's on a route FX has no adapter for
//! (spec/protocol.md §6.1, *Stand-in answers*; [`crate::stand_in`]).
//!
//! A check judges the call (its route, request, files and take) and the answer alone, never
//! anything its adapter's send or collect kept (spec/providers.md §4.4), so a stand-in's answer can
//! be held to it.
//!
//! [`ClipFacts`] are the facts a clip's answer carries as `data` (spec/capabilities.md §6), which
//! fal's video adapter takes from the clip it downloads and the stand-in checks from the clip a
//! stand-in answers.

use crate::adapter::{Answer, AnsweredFile, CallRequest};
use crate::capabilities;
use crate::redact::{Redactor, bounded};
use crate::wire;
use serde_json::{Value, json};

/// The longest schema refusal ([`structured`]), in characters.
pub const SCHEMA_REFUSAL_CHARS: usize = 500;

/// The longest kind a refusal shows.
pub const MAX_KIND_CHARS: usize = 96;

/// The kind of every audio answer FX ships (spec/capabilities.md §9–§11).
const MP3: &str = "audio/mpeg";

/// The check spec/capabilities.md gives the call's capability for every route, by its section:
/// - `image.generate`, `image.edit` (§2, §3): the request's `size` read as `auto` or `<W>x<H>`
///   (`size must be auto or WIDTHxHEIGHT`), then [`image`];
/// - `structured.generate` (§4): [`structured`], with the default redactor;
/// - `music.generate` (§9): [`audio_signature`];
/// - `sound.generate`, `speech.generate` (§10, §11): [`audio`], labelled with the capability;
/// - `background.remove` (§12): [`wire::check_png`].
///
/// A named file that is missing is refused as `the answer holds no <name>`. Any other capability
/// has no such check: `Ok`.
pub fn every_route(call: &CallRequest, answer: &Answer) -> Result<(), String> {
    let capability = call.route.capability.as_str();
    match capability {
        "image.generate" | "image.edit" => {
            let size = wire::parse_size(call.request.get("size"))?;
            image(call, named(answer, "image")?, size)
        }
        "structured.generate" => structured(call, answer, &Redactor::default()),
        "music.generate" => audio_signature(named(answer, "audio")?),
        "sound.generate" | "speech.generate" => {
            audio(answer.files.get("audio"), capability, &Redactor::default())
        }
        "background.remove" => {
            let image = named(answer, "image")?;
            wire::check_png(&image.kind, &image.bytes)
        }
        _ => Ok(()),
    }
}

/// The answer's file `name`: `the answer holds no <name>` when it has none.
fn named<'a>(answer: &'a Answer, name: &str) -> Result<&'a AnsweredFile, String> {
    answer
        .files
        .get(name)
        .ok_or_else(|| format!("the answer holds no {name}"))
}

/// The image checks of spec/capabilities.md §2 on an answer's picture ([`wire::check_image`]):
/// `size` is the exact size the route read from the request (`None` for none), and the
/// background is the request's (`auto` when absent).
pub fn image(
    call: &CallRequest,
    image: &AnsweredFile,
    size: Option<(u32, u32)>,
) -> Result<(), String> {
    let background = call
        .request
        .get("background")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    wire::check_image(&image.kind, &image.bytes, size, background)
}

/// An audio file's bytes carry its kind's signature ([`wire::matches_signature`]): `audio bytes
/// do not match declared media type <kind>`.
pub fn audio_signature(audio: &AnsweredFile) -> Result<(), String> {
    if wire::matches_signature(&audio.kind, &audio.bytes) {
        Ok(())
    } else {
        Err(format!(
            "audio bytes do not match declared media type {}",
            audio.kind
        ))
    }
}

/// The audio check of spec/capabilities.md §10–§11, in order: not empty (`<label> returned no
/// audio data`, also when there is no file); an MP3 (`requested mp3 but received <kind>`, the
/// kind redacted and cut to [`MAX_KIND_CHARS`], since a provider's header may echo anything);
/// the MP3 signature (`audio bytes do not match declared media type audio/mpeg`).
pub fn audio(audio: Option<&AnsweredFile>, label: &str, redactor: &Redactor) -> Result<(), String> {
    let Some(audio) = audio.filter(|file| !file.bytes.is_empty()) else {
        return Err(format!("{label} returned no audio data"));
    };
    if audio.kind != MP3 {
        let kind = bounded(&redactor.redact(&audio.kind), MAX_KIND_CHARS);
        return Err(format!("requested mp3 but received {kind}"));
    }
    if !wire::matches_signature(MP3, &audio.bytes) {
        return Err(format!(
            "audio bytes do not match declared media type {MP3}"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Structured output

/// The check of spec/capabilities.md §4: `data.json` (`the answer holds no json value` when
/// there is none) meets the request's **original** schema ([`request_schema`]), as given and not
/// as a route reshapes it, under draft 2020-12 with `format` not asserted. The refusal is
/// `<where>: <message>` for the error at the smallest instance location (segment by segment,
/// indexes by number before names by text, a location before the longer ones it starts), where is
/// the location's segments joined with `/`, or `the answer` at the root. It is redacted with
/// `redactor` and cut to [`SCHEMA_REFUSAL_CHARS`].
pub fn structured(call: &CallRequest, answer: &Answer, redactor: &Redactor) -> Result<(), String> {
    let Some(value) = answer.data.get("json") else {
        return Err("the answer holds no json value".into());
    };
    let schema = request_schema(call)?;
    let validator = schema_validator(&schema)?;
    let mut errors: Vec<(Vec<Segment>, String)> = validator
        .iter_errors(value)
        .map(|error| {
            let path = error
                .instance_path()
                .segments()
                .map(|segment| match segment {
                    jsonschema::paths::LocationSegment::Index(i) => Segment::Index(i),
                    jsonschema::paths::LocationSegment::Property(name) => {
                        Segment::Name(name.into_owned())
                    }
                })
                .collect();
            (path, error.to_string())
        })
        .collect();
    errors.sort_by(|a, b| a.0.cmp(&b.0));
    let Some((path, message)) = errors.first() else {
        return Ok(());
    };
    let place = if path.is_empty() {
        "the answer".to_string()
    } else {
        path.iter()
            .map(Segment::to_string)
            .collect::<Vec<_>>()
            .join("/")
    };
    let refusal = redactor.redact(&format!("{place}: {message}"));
    Err(refusal.chars().take(SCHEMA_REFUSAL_CHARS).collect())
}

/// The validator of a request's original schema: draft 2020-12, `format` not asserted, no remote
/// references (nothing is fetched). `a structured call's schema is not a JSON Schema (draft
/// 2020-12)` when it does not compile.
pub fn schema_validator(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::draft202012::options()
        .should_validate_formats(false)
        .build(schema)
        .map_err(|_| "a structured call's schema is not a JSON Schema (draft 2020-12)".to_string())
}

/// The schema a request file holds: `schema has no bytes to send` (no such file, or no bytes), `a
/// structured call's schema is not a JSON file`, `a structured call's schema is a JSON object`.
pub fn schema_file(call: &CallRequest, value: &Value) -> Result<Value, String> {
    let file = wire::request_file(call, value, "schema")?;
    let bytes = wire::read_file(file, "schema")?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(schema @ Value::Object(_)) => Ok(schema),
        Ok(_) => Err("a structured call's schema is a JSON object".into()),
        Err(_) => Err("a structured call's schema is not a JSON file".into()),
    }
}

/// The request's original schema (`file | object`): inline, or read from its file
/// ([`schema_file`]); `structured.generate needs schema` when it is neither.
pub fn request_schema(call: &CallRequest) -> Result<Value, String> {
    match call.request.get("schema") {
        Some(value) if capabilities::is_file_value(value) => schema_file(call, value),
        Some(schema @ Value::Object(_)) => Ok(schema.clone()),
        _ => Err("structured.generate needs schema".into()),
    }
}

/// One validation error's instance location, ordered as spec/protocol.md §6.2 orders them:
/// segment by segment, indexes by number before names by text, a location before the longer
/// locations it starts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Segment {
    Index(usize),
    Name(String),
}

impl std::fmt::Display for Segment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Segment::Index(i) => write!(f, "{i}"),
            Segment::Name(name) => f.write_str(name),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Clips

/// A clip's file facts, as the answer's `data.facts` carries them (spec/capabilities.md §6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFacts {
    pub width: u32,
    pub height: u32,
    /// Seconds, rounded to 6 places.
    pub duration_seconds: f64,
    /// Frames per second, rounded to 6 places.
    pub fps: f64,
}

impl ClipFacts {
    /// The facts of an MP4's first video track, when it has one with a positive frame rate and a
    /// duration. Otherwise the sentence: `the file carries no video stream`, or the reader's own.
    pub fn of_mp4(bytes: &[u8]) -> Result<ClipFacts, String> {
        let no_stream = || "the file carries no video stream".to_string();
        match grida_fx_core::facts::video_facts(bytes, "video/mp4") {
            Ok(Some(facts)) => match (facts.duration, facts.fps) {
                (Some(duration), Some(fps))
                    if duration.is_finite() && fps.is_finite() && fps > 0.0 =>
                {
                    Ok(ClipFacts {
                        width: facts.width,
                        height: facts.height,
                        duration_seconds: duration,
                        fps,
                    })
                }
                _ => Err(no_stream()),
            },
            Ok(None) => Err(no_stream()),
            Err(reason) => Err(format!("the clip cannot be read: {reason}")),
        }
    }

    /// `{"width", "height", "duration_seconds", "fps"}`.
    pub fn to_json(self) -> Value {
        let number = |x: f64| grida_fx_core::value::number(x).unwrap_or(Value::Null);
        json!({
            "width": self.width,
            "height": self.height,
            "duration_seconds": number(self.duration_seconds),
            "fps": number(self.fps),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Secret;
    use crate::testing::{CallBuilder, media};

    fn answer(name: &str, kind: &str, bytes: &[u8]) -> Answer {
        Answer::new(Value::Null, None).with_file(name, kind, bytes.to_vec())
    }

    #[test]
    fn every_route_checks_pictures_by_the_requests_size_and_background() {
        let call = CallBuilder::new("image.generate", "img-a@acme")
            .request(json!({"prompt": "p", "size": "4x4", "background": "opaque"}))
            .build();
        let opaque = media::png(4, 4, Some(255));
        assert_eq!(
            every_route(&call, &answer("image", "image/png", &opaque)),
            Ok(())
        );
        assert_eq!(
            every_route(
                &call,
                &answer("image", "image/png", &media::png(2, 4, None))
            ),
            Err("the image is 2x4, not 4x4".into())
        );
        assert_eq!(
            every_route(
                &call,
                &answer("image", "image/png", &media::png_one_pixel_alpha(4, 4, 0))
            ),
            Err("the picture asked for as opaque has transparent pixels".into())
        );
        assert_eq!(
            every_route(&call, &Answer::new(Value::Null, None)),
            Err("the answer holds no image".into())
        );
        let shapeless = CallBuilder::new("image.edit", "img-a@acme")
            .request(json!({"prompt": "p", "size": "big"}))
            .build();
        assert_eq!(
            every_route(&shapeless, &answer("image", "image/png", &opaque)),
            Err("size must be auto or WIDTHxHEIGHT".into())
        );
        let transparent = CallBuilder::new("image.edit", "img-a@acme")
            .request(json!({"prompt": "p", "size": "auto", "background": "transparent"}))
            .build();
        assert_eq!(
            every_route(&transparent, &answer("image", "image/png", &opaque)),
            Ok(())
        );
        assert_eq!(
            every_route(
                &transparent,
                &answer("image", "image/png", &media::png(4, 4, None))
            ),
            Err("the picture asked for as transparent has no alpha channel".into())
        );
    }

    #[test]
    fn every_route_checks_audio_and_cutouts() {
        let music = CallBuilder::new("music.generate", "song-a@acme").build();
        assert_eq!(
            every_route(&music, &answer("audio", "audio/mpeg", media::MP3)),
            Ok(())
        );
        assert_eq!(
            every_route(&music, &answer("audio", "audio/mpeg", b"not audio")),
            Err("audio bytes do not match declared media type audio/mpeg".into())
        );
        for capability in ["sound.generate", "speech.generate"] {
            let call = CallBuilder::new(capability, "sfx-a@acme").build();
            assert_eq!(
                every_route(&call, &answer("audio", "audio/mpeg", media::MP3_FRAME)),
                Ok(())
            );
            assert_eq!(
                every_route(&call, &answer("audio", "audio/mpeg", b"")),
                Err(format!("{capability} returned no audio data"))
            );
            assert_eq!(
                every_route(&call, &answer("audio", "audio/wav", media::WAV)),
                Err("requested mp3 but received audio/wav".into())
            );
            assert_eq!(
                every_route(&call, &answer("audio", "audio/mpeg", media::WAV)),
                Err("audio bytes do not match declared media type audio/mpeg".into())
            );
        }
        let cutout = CallBuilder::new("background.remove", "cut-a@acme").build();
        assert_eq!(
            every_route(
                &cutout,
                &answer("image", "image/png", &media::png(2, 2, Some(0)))
            ),
            Ok(())
        );
        assert_eq!(
            every_route(&cutout, &answer("image", "image/png", media::JPEG_HEAD)),
            Err("the answer is image/jpeg, not image/png".into())
        );
        assert_eq!(
            every_route(&cutout, &answer("image", "image/webp", b"RIFF")),
            Err("the answer is image/webp, not image/png".into())
        );
    }

    #[test]
    fn every_route_checks_structured_answers_against_the_original_schema() {
        let schema = json!({"type": "object", "properties": {"n": {"type": "integer"},
            "tags": {"type": "array", "items": {"type": "string"}}}, "required": ["n"],
            "format_note": "kept"});
        let inline = CallBuilder::new("structured.generate", "text-a@acme")
            .request(json!({"prompt": "p", "schema": schema}))
            .build();
        let json_answer = |value: Value| Answer::new(json!({"json": value}), None);
        assert_eq!(every_route(&inline, &json_answer(json!({"n": 1}))), Ok(()));
        assert_eq!(
            every_route(&inline, &json_answer(json!({"n": "1", "tags": [1]}))),
            Err(r#"n: "1" is not of type "integer""#.into()),
            "the smallest location wins"
        );
        assert_eq!(
            every_route(&inline, &json_answer(json!({"tags": ["a", 2]}))),
            Err(r#"the answer: "n" is a required property"#.into())
        );
        assert_eq!(
            every_route(&inline, &Answer::new(json!({}), None)),
            Err("the answer holds no json value".into())
        );
        let mut builder = CallBuilder::new("structured.generate", "text-a@acme");
        let file = builder.file("json", schema.to_string().as_bytes());
        let from_file = builder
            .request(json!({"prompt": "p", "schema": file}))
            .build();
        assert_eq!(
            every_route(&from_file, &json_answer(json!({"n": 2, "tags": ["x"]}))),
            Ok(())
        );
        assert_eq!(
            every_route(&from_file, &json_answer(json!({"n": 2, "tags": ["x", 3]}))),
            Err(r#"tags/1: 3 is not of type "string""#.into())
        );
        let unknown = CallBuilder::new("structured.generate", "text-a@acme")
            .request(json!({"prompt": "p", "schema": {"file": "f".repeat(64)}}))
            .build();
        assert_eq!(
            every_route(&unknown, &json_answer(json!({}))),
            Err("schema has no bytes to send".into())
        );
    }

    #[test]
    fn a_schema_refusal_is_redacted_and_cut() {
        let call = CallBuilder::new("structured.generate", "text-a@acme")
            .request(json!({"prompt": "p", "schema": {"type": "object",
                "properties": {"k": {"const": "x"}}}}))
            .build();
        let secret = "acme-secret-0123456789";
        let answer = Answer::new(json!({"json": {"k": secret.repeat(40)}}), None);
        let refusal =
            structured(&call, &answer, &Redactor::new(vec![Secret::new(secret)])).unwrap_err();
        assert!(refusal.starts_with("k: "), "{refusal}");
        assert!(!refusal.contains(secret), "{refusal}");
        assert!(refusal.chars().count() <= SCHEMA_REFUSAL_CHARS);
    }

    #[test]
    fn capabilities_without_an_every_route_check_pass() {
        for capability in [
            "agent.turn",
            "video.generate",
            "mesh.generate",
            "mesh.rig",
            "x.y",
        ] {
            let call = CallBuilder::new(capability, "m@acme").build();
            assert_eq!(
                every_route(&call, &Answer::new(json!({"anything": 1}), None)),
                Ok(()),
                "{capability}"
            );
        }
    }

    #[test]
    fn a_kind_in_an_audio_refusal_is_redacted_and_cut() {
        let redactor = Redactor::new(vec![Secret::new("acme-key-0123")]);
        let file = AnsweredFile {
            kind: format!("text/acme-key-0123 {}", "long ".repeat(40)),
            bytes: vec![1],
        };
        let refusal = audio(Some(&file), "Acme", &redactor).unwrap_err();
        assert!(refusal.starts_with("requested mp3 but received text/[redacted] long"));
        assert!(refusal.ends_with('…'));
        assert_eq!(
            audio(None, "Acme", &redactor).unwrap_err(),
            "Acme returned no audio data"
        );
    }

    #[test]
    fn clip_facts_are_read_from_the_clip() {
        let clip = media::mp4(64, 48, 24, 72);
        assert_eq!(
            ClipFacts::of_mp4(&clip),
            Ok(ClipFacts {
                width: 64,
                height: 48,
                duration_seconds: 3.0,
                fps: 24.0
            })
        );
        assert_eq!(
            ClipFacts::of_mp4(&clip).unwrap().to_json(),
            json!({"width": 64, "height": 48, "duration_seconds": 3, "fps": 24})
        );
        assert!(
            ClipFacts::of_mp4(b"not a clip")
                .unwrap_err()
                .starts_with("the clip cannot be read: ")
        );
    }
}
