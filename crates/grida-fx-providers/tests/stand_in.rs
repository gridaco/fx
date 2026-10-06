//! The providers' checks of a stand-in's answer (spec/protocol.md §6.1 "Stand-in answers";
//! spec/capabilities.md §1 "Answers"; spec/providers.md §4.4): the request check, the shape and
//! its sentences, the kinds of files that gave none, the data FX writes, and the route's check,
//! an adapter's or every route's. Nothing is sent: the adapters are built keyless over a
//! transport that refuses every exchange. Media are built in code, except two clips read from
//! the spec's facts vectors (`spec/vectors/facts/mp4/`, made by the commands in its README).

use grida_fx_providers::adapter::{
    Adapter, Answer, AnsweredFile, CallRequest, RequestAdapter, Sent,
};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::live::{adapters, offline_setup};
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::testing::{CallBuilder, TestCall, media};
use grida_fx_providers::{BoxFuture, StandInChecks, StandInFile};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::Arc;

/// A clip of 64 × 48 at 24 fps running 2 s.
const CLIP: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/h264_24fps.mp4");
/// An MP4 with an audio track only.
const AUDIO_ONLY: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/audio_only.mp4");

/// The rig facts a `mesh.rig` answer carries.
fn rig_data() -> Value {
    json!({"facts": {"riggable": true, "checked_rig_type": "biped", "advisory_override": false}})
}

fn file(kind: Option<&str>, bytes: &[u8]) -> StandInFile {
    StandInFile {
        kind: kind.map(str::to_string),
        bytes: bytes.to_vec(),
    }
}

/// Files by name, in order.
fn files(list: &[(&str, Option<&str>, &[u8])]) -> IndexMap<String, StandInFile> {
    list.iter()
        .map(|(name, kind, bytes)| (name.to_string(), file(*kind, bytes)))
        .collect()
}

/// One file without a kind.
fn one(name: &str, bytes: &[u8]) -> IndexMap<String, StandInFile> {
    files(&[(name, None, bytes)])
}

fn call(capability: &str, route: &str, request: Value) -> TestCall {
    CallBuilder::new(capability, route).request(request).build()
}

/// A call whose request names a picture file under `member`, beside `request`'s members.
fn call_with_picture(capability: &str, route: &str, member: &str, request: Value) -> TestCall {
    let mut builder = CallBuilder::new(capability, route);
    let picture = builder.file("image/png", &media::png(2, 2, Some(255)));
    let mut request = request;
    request[member] = picture;
    builder.request(request).build()
}

fn checked(
    call: &CallRequest,
    files: IndexMap<String, StandInFile>,
    data: Value,
) -> Result<Answer, String> {
    StandInChecks::new().answer(call, files, data)
}

fn refusal(call: &CallRequest, files: IndexMap<String, StandInFile>, data: Value) -> String {
    match checked(call, files, data) {
        Err(reason) => reason,
        Ok(answer) => panic!("expected a refusal, got {answer:?}"),
    }
}

// --- the request -------------------------------------------------------------------------------

#[test]
fn requests_are_checked_against_a_shipped_capability_and_nothing_else() {
    let checks = StandInChecks::new();
    let image = json!({"file": "a".repeat(64)});
    assert_eq!(
        checks.request("image.generate", &json!({"prompt": "p"})),
        Ok(())
    );
    for (capability, request, sentence) in [
        ("image.generate", json!({}), "image.generate needs prompt"),
        (
            "image.generate",
            json!({"prompt": "p", "quality": "low"}),
            "image.generate takes no member quality",
        ),
        (
            "image.edit",
            json!({"prompt": 1, "image": image}),
            "prompt is text",
        ),
        (
            "image.edit",
            json!({"prompt": "p", "image": null}),
            "image.edit needs image",
        ),
        (
            "mesh.generate",
            json!({"views": [image]}),
            "views is files by name",
        ),
        (
            "agent.turn",
            json!([]),
            "a request for agent.turn is a JSON object",
        ),
    ] {
        assert_eq!(
            checks.request(capability, &request),
            Err(sentence.to_string()),
            "{capability} {request}"
        );
    }
    // A route's refusals of values are not made: a blank prompt, a size no route serves.
    assert_eq!(
        checks.request(
            "image.generate",
            &json!({"prompt": " ", "size": "big", "background": "plaid"})
        ),
        Ok(())
    );
    // A capability FX does not ship has no request check.
    assert_eq!(checks.request("vision.review", &json!({"x": 1})), Ok(()));
    assert_eq!(checks.request("acme.thing", &json!([1])), Ok(()));
}

// --- the shape ---------------------------------------------------------------------------------

#[test]
fn a_file_without_a_kind_takes_the_one_its_capability_names() {
    let png = media::png(2, 2, Some(255));
    let clip = media::mp4(64, 48, 24, 48);
    let cases = [
        (
            "image.generate",
            "image",
            png.as_slice(),
            Value::Null,
            "image/png",
        ),
        (
            "image.edit",
            "image",
            png.as_slice(),
            Value::Null,
            "image/png",
        ),
        (
            "background.remove",
            "image",
            png.as_slice(),
            Value::Null,
            "image/png",
        ),
        (
            "music.generate",
            "audio",
            media::MP3,
            Value::Null,
            "audio/mpeg",
        ),
        (
            "sound.generate",
            "audio",
            media::MP3_FRAME,
            Value::Null,
            "audio/mpeg",
        ),
        (
            "speech.generate",
            "audio",
            media::MP3,
            Value::Null,
            "audio/mpeg",
        ),
        (
            "video.generate",
            "video",
            clip.as_slice(),
            Value::Null,
            "video/mp4",
        ),
        (
            "mesh.rig",
            "model",
            media::GLB,
            rig_data(),
            "model/gltf-binary",
        ),
        (
            "mesh.generate",
            "model",
            media::FBX,
            Value::Null,
            "model/fbx",
        ),
    ];
    for (capability, name, bytes, data, kind) in cases {
        let call = call(capability, "m-a@acme", json!({}));
        let answer = checked(&call, one(name, bytes), data.clone()).unwrap();
        assert_eq!(
            answer.files[name],
            AnsweredFile {
                kind: kind.into(),
                bytes: bytes.to_vec()
            },
            "{capability}"
        );
        assert_eq!(answer.cost, None, "a stand-in's answer carries no cost");
        // The kind given, when it is the one named, is the same answer.
        let given = checked(&call, files(&[(name, Some(kind), bytes)]), data).unwrap();
        assert_eq!(given, answer, "{capability}");
    }
}

#[test]
fn the_shape_is_refused_in_order_with_its_sentences() {
    let png = media::png(2, 2, Some(255));
    let image = call("image.generate", "img-a@acme", json!({"prompt": "p"}));
    let cases: Vec<(IndexMap<String, StandInFile>, Value, &str)> = vec![
        (IndexMap::new(), Value::Null, "the answer holds no image"),
        // A missing file wins over one not named; that over a kind; a kind over the data.
        (one("mask", &png), Value::Null, "the answer holds no image"),
        (
            files(&[
                ("image", Some("text/plain"), b"hello"),
                ("mask", None, &png),
            ]),
            Value::Null,
            "image.generate returns no file named mask",
        ),
        (
            files(&[("image", Some("text/plain"), b"hello")]),
            json!({"seed": 1}),
            "image is text/plain, not image/png",
        ),
        (
            files(&[("image", Some("Image/PNG "), &png)]),
            Value::Null,
            r#"image is "Image/PNG ", not image/png"#,
        ),
        (
            one("image", &png),
            json!({"seed": 1}),
            "image.generate returns no data",
        ),
        (
            one("image", &png),
            json!(false),
            "image.generate returns no data",
        ),
    ];
    for (files, data, sentence) in cases {
        assert_eq!(refusal(&image, files, data.clone()), sentence, "{data}");
    }

    let others = vec![
        (
            "agent.turn",
            json!({}),
            one("notes", b"n"),
            json!({"text": "", "tool_calls": []}),
            "agent.turn returns no file named notes",
        ),
        (
            "structured.generate",
            json!({}),
            one("image", &png),
            json!({"json": {}}),
            "structured.generate returns no file named image",
        ),
        (
            "music.generate",
            json!({"prompt": "p"}),
            files(&[("audio", Some("audio/wav"), media::WAV)]),
            Value::Null,
            "audio is audio/wav, not audio/mpeg",
        ),
        (
            "music.generate",
            json!({"prompt": "p"}),
            one("audio", media::MP3),
            json!("x"),
            "music.generate returns no data",
        ),
        (
            "background.remove",
            json!({}),
            one("image", &png),
            json!({}),
            "background.remove returns no data",
        ),
        (
            "video.generate",
            json!({"prompt": "p"}),
            files(&[("video", Some("video/webm"), CLIP)]),
            Value::Null,
            "video is video/webm, not video/mp4",
        ),
        (
            "mesh.rig",
            json!({}),
            files(&[("model", Some("model/fbx"), media::FBX)]),
            rig_data(),
            "model is model/fbx, not model/gltf-binary",
        ),
    ];
    for (capability, request, files, data, sentence) in others {
        let call = call(capability, "m-a@acme", request);
        assert_eq!(refusal(&call, files, data), sentence, "{capability}");
    }
}

#[test]
fn structured_data_holds_only_json() {
    let call = call(
        "structured.generate",
        "text-a@acme",
        json!({"prompt": "p", "schema": {"type": "object"}}),
    );
    let sentence = r#"structured.generate returns its data as {"json": <value>}"#;
    for data in [
        Value::Null,
        json!({"value": {}}),
        json!({"json": {}, "usage": 1}),
        json!([{"json": {}}]),
        json!("{}"),
    ] {
        assert_eq!(
            refusal(&call, IndexMap::new(), data.clone()),
            sentence,
            "{data}"
        );
    }
    let answer = checked(&call, IndexMap::new(), json!({"json": {"n": 1}})).unwrap();
    assert_eq!(answer.data, json!({"json": {"n": 1}}));
    assert!(answer.files.is_empty());
}

#[test]
fn rig_data_holds_the_three_facts_with_their_types() {
    let call = call("mesh.rig", "rig-a@acme", json!({}));
    let sentence = concat!(
        r#"mesh.rig returns its data as {"facts": {"riggable", "checked_rig_type", "#,
        r#""advisory_override"}}"#
    );
    for data in [
        Value::Null,
        json!({"facts": {"riggable": true, "checked_rig_type": "biped"}}),
        json!({"facts": {"riggable": "yes", "checked_rig_type": null, "advisory_override": false}}),
        json!({"facts": {"riggable": null, "checked_rig_type": 1, "advisory_override": false}}),
        json!({"facts": {"riggable": null, "checked_rig_type": null, "advisory_override": null}}),
        json!({"facts": {"riggable": true, "checked_rig_type": "biped", "advisory_override": false,
            "task": "t-1"}}),
        json!({"facts": {"riggable": true, "checked_rig_type": "biped",
            "advisory_override": false}, "task": "t-1"}),
    ] {
        assert_eq!(
            refusal(&call, one("model", media::GLB), data.clone()),
            sentence,
            "{data}"
        );
    }
    for data in [
        rig_data(),
        json!({"facts": {"riggable": null, "checked_rig_type": null, "advisory_override": true}}),
    ] {
        let answer = checked(&call, one("model", media::GLB), data.clone()).unwrap();
        assert_eq!(answer.data, data);
    }
}

#[test]
fn agent_turn_data_is_left_to_the_engine() {
    let call = call("agent.turn", "chat-a@acme", json!({}));
    for data in [
        json!({"text": "", "tool_calls": []}),
        json!("not a turn"),
        Value::Null,
    ] {
        assert_eq!(
            checked(&call, IndexMap::new(), data.clone()).unwrap().data,
            data
        );
    }
}

#[test]
fn a_meshs_kind_and_data_are_taken_from_its_bytes() {
    for route in ["mesh-a@acme", "mesh-a@tripo"] {
        let call = call("mesh.generate", route, json!({}));
        for (kind, bytes, shown) in [
            (None, media::GLB, "model/gltf-binary"),
            (None, media::FBX, "model/fbx"),
            (Some("model/fbx"), media::FBX, "model/fbx"),
            (Some("model/gltf-binary"), media::GLB, "model/gltf-binary"),
        ] {
            let answer = checked(&call, files(&[("model", kind, bytes)]), Value::Null).unwrap();
            assert_eq!(answer.files["model"].kind, shown, "{route}");
            assert_eq!(answer.data, json!({"facts": {"model_kind": shown}}));
        }
        let png = media::png(2, 2, None);
        for (kind, bytes, sentence) in [
            (
                Some("image/png"),
                png.as_slice(),
                "model is image/png, not model/fbx or model/gltf-binary",
            ),
            (
                Some("model"),
                media::GLB,
                "model is model, not model/fbx or model/gltf-binary",
            ),
            (
                None,
                png.as_slice(),
                "model is neither binary glTF nor binary FBX",
            ),
            (
                Some("model/fbx"),
                media::GLB,
                "model is model/fbx, not model/gltf-binary",
            ),
        ] {
            assert_eq!(
                refusal(&call, files(&[("model", kind, bytes)]), Value::Null),
                sentence,
                "{route}"
            );
        }
        assert_eq!(
            refusal(
                &call,
                one("model", media::GLB),
                json!({"facts": {"model_kind": "model/gltf-binary"}})
            ),
            "mesh.generate's data is taken from its file: answer null"
        );
    }
}

#[test]
fn a_clips_data_is_taken_from_its_facts() {
    let call = call("video.generate", "vid-a@acme", json!({"prompt": "p"}));
    let answer = checked(&call, one("video", CLIP), Value::Null).unwrap();
    assert_eq!(
        answer.data,
        json!({"facts": {"width": 64, "height": 48, "duration_seconds": 2, "fps": 24}})
    );
    assert_eq!(answer.files["video"].kind, "video/mp4");
    let synthesized = checked(
        &call,
        one("video", &media::mp4(320, 180, 30, 45)),
        Value::Null,
    );
    assert_eq!(
        synthesized.unwrap().data,
        json!({"facts": {"width": 320, "height": 180, "duration_seconds": 1.5, "fps": 30}})
    );
    // Data the stand-in computed is refused, even when it is right.
    assert_eq!(
        refusal(
            &call,
            one("video", CLIP),
            json!({"facts": {"width": 64, "height": 48, "duration_seconds": 2, "fps": 24}})
        ),
        "video.generate's data is taken from its file: answer null"
    );
    assert_eq!(
        refusal(&call, one("video", AUDIO_ONLY), Value::Null),
        "the answer's video is not a clip FX can read: the file carries no video stream"
    );
    let garbage = refusal(&call, one("video", b"not a clip"), Value::Null);
    assert!(
        garbage
            .starts_with("the answer's video is not a clip FX can read: the clip cannot be read: "),
        "{garbage}"
    );
}

#[test]
fn a_capability_fx_does_not_ship_needs_kinds_and_keeps_its_data() {
    for capability in ["acme.thing", "vision.review"] {
        let call = call(capability, "m-a@acme", json!({"anything": 1}));
        let answer = checked(
            &call,
            files(&[
                ("report", Some("text/plain"), b"fine"),
                ("score", Some("json"), b"{}"),
            ]),
            json!({"verdict": "pass"}),
        )
        .unwrap();
        assert_eq!(
            answer,
            Answer::new(json!({"verdict": "pass"}), None)
                .with_file("report", "text/plain", b"fine".to_vec())
                .with_file("score", "json", b"{}".to_vec())
        );
        assert_eq!(
            refusal(&call, one("report", b"fine"), Value::Null),
            format!("report has no kind, and {capability} names none for it")
        );
        for (kind, shown) in [
            ("Text/Plain", r#""Text/Plain""#),
            (
                "text/plain; charset=utf-8",
                r#""text/plain; charset=utf-8""#,
            ),
            ("", r#""""#),
        ] {
            assert_eq!(
                refusal(
                    &call,
                    files(&[("report", Some(kind), b"fine")]),
                    Value::Null
                ),
                format!("report is {shown}, not a kind")
            );
        }
        assert!(checked(&call, IndexMap::new(), Value::Null).is_ok());
    }
}

// --- the route's check -------------------------------------------------------------------------

#[test]
fn a_route_no_adapter_serves_gets_every_routes_check() {
    let sized = call(
        "image.generate",
        "img-a@acme",
        json!({"prompt": "p", "size": "4x4", "background": "opaque"}),
    );
    assert!(checked(&sized, one("image", &media::png(4, 4, None)), Value::Null).is_ok());
    assert_eq!(
        refusal(&sized, one("image", &media::png(2, 2, None)), Value::Null),
        "the image is 2x2, not 4x4"
    );
    assert_eq!(
        refusal(
            &sized,
            one("image", &media::png_one_pixel_alpha(4, 4, 0)),
            Value::Null
        ),
        "the picture asked for as opaque has transparent pixels"
    );
    assert_eq!(
        refusal(&sized, one("image", media::JPEG_HEAD), Value::Null),
        "the answer is image/jpeg, not image/png"
    );
    let transparent = call_with_picture(
        "image.edit",
        "img-a@acme",
        "image",
        json!({"prompt": "p", "background": "transparent"}),
    );
    assert_eq!(
        refusal(
            &transparent,
            one("image", &media::png(4, 4, None)),
            Value::Null
        ),
        "the picture asked for as transparent has no alpha channel"
    );
    let shapeless = call(
        "image.generate",
        "img-a@acme",
        json!({"prompt": "p", "size": "big"}),
    );
    assert_eq!(
        refusal(
            &shapeless,
            one("image", &media::png(4, 4, None)),
            Value::Null
        ),
        "size must be auto or WIDTHxHEIGHT"
    );

    let structured = call(
        "structured.generate",
        "text-a@acme",
        json!({"prompt": "p", "schema": {"type": "object", "properties":
            {"n": {"type": "integer"}}, "required": ["n"]}}),
    );
    assert!(checked(&structured, IndexMap::new(), json!({"json": {"n": 1}})).is_ok());
    assert_eq!(
        refusal(&structured, IndexMap::new(), json!({"json": {"n": "one"}})),
        r#"n: "one" is not of type "integer""#
    );

    let music = call("music.generate", "song-a@acme", json!({"prompt": "p"}));
    assert_eq!(
        refusal(&music, one("audio", b"not audio"), Value::Null),
        "audio bytes do not match declared media type audio/mpeg"
    );
    let sound = call("sound.generate", "sfx-a@acme", json!({"prompt": "p"}));
    assert_eq!(
        refusal(&sound, one("audio", b""), Value::Null),
        "sound.generate returned no audio data"
    );
    assert_eq!(
        refusal(&sound, one("audio", media::WAV), Value::Null),
        "audio bytes do not match declared media type audio/mpeg"
    );
    let cutout = call_with_picture("background.remove", "cut-a@acme", "image", json!({}));
    assert_eq!(
        refusal(&cutout, one("image", b"GIF89a"), Value::Null),
        "the answer is image/gif, not image/png"
    );
    // video.generate, mesh.rig and agent.turn have no every-route check.
    let video = call(
        "video.generate",
        "vid-a@acme",
        json!({"prompt": "p", "duration": 9}),
    );
    assert!(checked(&video, one("video", CLIP), Value::Null).is_ok());
}

#[test]
fn a_route_an_adapter_serves_gets_its_check() {
    let request = json!({"prompt": "p", "size": "big"});
    let png = media::png(4, 4, None);
    assert_eq!(
        refusal(
            &call("image.generate", "img-a@openai", request.clone()),
            one("image", &png),
            Value::Null
        ),
        "OpenAI image size must be auto or WIDTHxHEIGHT"
    );
    assert_eq!(
        refusal(
            &call("image.generate", "img-a@acme", request.clone()),
            one("image", &png),
            Value::Null
        ),
        "size must be auto or WIDTHxHEIGHT"
    );
    // fal's check reads a size it cannot read as none.
    assert!(
        checked(
            &call("image.generate", "img-a@fal", request),
            one("image", &png),
            Value::Null
        )
        .is_ok()
    );

    let sound = json!({"prompt": "p"});
    assert_eq!(
        refusal(
            &call("sound.generate", "sfx-a@elevenlabs", sound.clone()),
            one("audio", b""),
            Value::Null
        ),
        "ElevenLabs sound generation returned no audio data"
    );
    assert_eq!(
        refusal(
            &call("sound.generate", "sfx-a@acme", sound),
            one("audio", b""),
            Value::Null
        ),
        "sound.generate returned no audio data"
    );

    // fal's video check holds the clip to the request, which every route's check does not.
    let fal = call_with_picture(
        "video.generate",
        "vid-a@fal",
        "first_frame",
        json!({"prompt": "p", "duration": 3}),
    );
    assert_eq!(
        refusal(&fal, one("video", CLIP), Value::Null),
        "the clip is 64x48, not 720x1280"
    );
    assert_eq!(
        refusal(
            &fal,
            one("video", &media::mp4(720, 1280, 24, 24)),
            Value::Null
        ),
        "the clip runs 1.000 s, not 3 s"
    );
    assert!(
        checked(
            &fal,
            one("video", &media::mp4(720, 1280, 24, 72)),
            Value::Null
        )
        .is_ok()
    );
    let no_frame = call(
        "video.generate",
        "vid-a@fal",
        json!({"prompt": "p", "duration": 3}),
    );
    assert_eq!(
        refusal(
            &no_frame,
            one("video", &media::mp4(720, 1280, 24, 72)),
            Value::Null
        ),
        "this route draws from a first frame"
    );
}

/// An adapter that judges every answer and must never send.
struct Judge;

impl RequestAdapter for Judge {
    fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        panic!("a stand-in run never sends")
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        Err(format!(
            "{} judged {} file(s) for {}",
            call.route.id(),
            answer.files.len(),
            call.key
        ))
    }
}

#[test]
fn given_adapters_replace_fxs_own() {
    let mut judges = Adapters::new();
    judges.register("image.generate", "acme", Adapter::Request(Arc::new(Judge)));
    let checks = StandInChecks::with_adapters(judges);
    let judged = call("image.generate", "img-a@acme", json!({"prompt": "p"}));
    let png = media::png(4, 4, None);
    assert_eq!(
        checks.answer(&judged, one("image", &png), Value::Null),
        Err(format!(
            "img-a@acme judged 1 file(s) for {}",
            "0".repeat(64)
        ))
    );
    // The shape comes first, and a route the given adapters do not serve gets every route's
    // check, even where FX has an adapter of its own.
    assert_eq!(
        checks.answer(&judged, IndexMap::new(), Value::Null),
        Err("the answer holds no image".into())
    );
    let openai = call(
        "image.generate",
        "img-a@openai",
        json!({"prompt": "p", "size": "big"}),
    );
    assert_eq!(
        checks.answer(&openai, one("image", &png), Value::Null),
        Err("size must be auto or WIDTHxHEIGHT".into())
    );
    assert!(format!("{:?}", StandInChecks::default()).starts_with("StandInChecks"));
}

/// For each `(capability, provider)` FX registers an adapter for: a call, the files and data a
/// stand-in answers, and the answer the adapter itself would give live.
fn accepted_live(
    capability: &str,
    provider: &str,
) -> (TestCall, IndexMap<String, StandInFile>, Value, Answer) {
    let route = format!("m-a@{provider}");
    let png = |alpha| media::png(64, 64, alpha);
    let file_of = |name: &str, kind: &str, bytes: Vec<u8>| {
        Answer::new(Value::Null, None).with_file(name, kind, bytes)
    };
    match capability {
        "image.generate" => {
            let call = call(
                capability,
                &route,
                json!({"prompt": "p", "size": "64x64", "background": "opaque"}),
            );
            (
                call,
                one("image", &png(None)),
                Value::Null,
                file_of("image", "image/png", png(None)),
            )
        }
        "image.edit" => {
            let call = call_with_picture(
                capability,
                &route,
                "image",
                json!({"prompt": "p", "size": "64x64", "background": "transparent"}),
            );
            (
                call,
                one("image", &png(Some(0))),
                Value::Null,
                file_of("image", "image/png", png(Some(0))),
            )
        }
        "background.remove" => {
            let call = call_with_picture(capability, &route, "image", json!({}));
            (
                call,
                one("image", &png(Some(0))),
                Value::Null,
                file_of("image", "image/png", png(Some(0))),
            )
        }
        "structured.generate" => {
            let mut builder = CallBuilder::new(capability, &route);
            let schema = json!({"title": "Scene", "type": "object",
                "properties": {"mood": {"type": "string", "enum": ["calm", "tense"]}},
                "required": ["mood"]});
            let schema = builder.file("json", schema.to_string().as_bytes());
            let call = builder
                .request(json!({"prompt": "p", "schema": schema}))
                .build();
            let data = json!({"json": {"mood": "calm"}});
            (call, IndexMap::new(), data.clone(), Answer::new(data, None))
        }
        "agent.turn" => {
            let call = call(
                capability,
                &route,
                json!({"system": "s", "messages": [],
                "tools": [], "tool_choice": "auto"}),
            );
            let data = json!({"text": "done", "tool_calls": []});
            (call, IndexMap::new(), data.clone(), Answer::new(data, None))
        }
        "music.generate" | "sound.generate" => {
            let call = call(capability, &route, json!({"prompt": "p"}));
            (
                call,
                one("audio", media::MP3),
                Value::Null,
                file_of("audio", "audio/mpeg", media::MP3.to_vec()),
            )
        }
        "speech.generate" => {
            let call = call(capability, &route, json!({"text": "t", "voice": "v-1"}));
            (
                call,
                one("audio", media::MP3_FRAME),
                Value::Null,
                file_of("audio", "audio/mpeg", media::MP3_FRAME.to_vec()),
            )
        }
        "video.generate" => {
            let call = call_with_picture(
                capability,
                &route,
                "first_frame",
                json!({"prompt": "p", "duration": 3, "resolution": "720p",
                       "aspect_ratio": "9:16"}),
            );
            let clip = media::mp4(720, 1280, 24, 72);
            let data = json!({"facts": {"width": 720, "height": 1280, "duration_seconds": 3,
                "fps": 24}});
            (
                call,
                one("video", &clip),
                Value::Null,
                Answer::new(data, None).with_file("video", "video/mp4", clip),
            )
        }
        "mesh.generate" => {
            let mut builder = CallBuilder::new(capability, &route);
            let front = builder.file("image/png", &media::png(2, 2, Some(255)));
            let call = builder.request(json!({"views": {"front": front}})).build();
            let data = json!({"facts": {"model_kind": "model/fbx"}});
            (
                call,
                one("model", media::FBX),
                Value::Null,
                Answer::new(data, None).with_file("model", "model/fbx", media::FBX.to_vec()),
            )
        }
        "mesh.rig" => {
            let mut builder = CallBuilder::new(capability, &route);
            let model = builder.file("model/gltf-binary", media::GLB);
            let call = builder.request(json!({"model": model})).build();
            (
                call,
                one("model", media::GLB),
                rig_data(),
                Answer::new(rig_data(), None).with_file(
                    "model",
                    "model/gltf-binary",
                    media::GLB.to_vec(),
                ),
            )
        }
        other => panic!("no case for {other} on {provider}: add one"),
    }
}

/// spec/providers.md §4.4: an adapter's check judges only the call and the answer, so an answer
/// it accepts live passes it the same as a stand-in's answer.
#[test]
fn every_adapters_check_takes_a_stand_ins_answer_as_it_takes_a_live_one() {
    let registered = adapters(&offline_setup(Keys::none()));
    let checks = StandInChecks::new();
    let served = registered.served();
    assert!(served.len() >= 14, "{served:?}");
    for (capability, provider) in served {
        let (call, files, data, live) = accepted_live(&capability, &provider);
        let accepted = match registered.serving(&call.route) {
            Some(Adapter::Request(adapter)) => adapter.check(&call, &live),
            Some(Adapter::Job(adapter)) => adapter.check(&call, &live),
            None => panic!("{capability} on {provider} is served"),
        };
        assert_eq!(accepted, Ok(()), "{capability} on {provider}, live");
        // And the stand-in's answer, once shaped, is the adapter's: the kinds and the data FX
        // writes are the ones the adapter answers with.
        assert_eq!(
            checks.answer(&call, files, data),
            Ok(live),
            "{capability} on {provider}, stand-in"
        );
    }
}

#[test]
fn the_checks_can_be_shared_across_tasks() {
    fn shared<T: Send + Sync + Clone>() {}
    shared::<StandInChecks>();
    shared::<StandInFile>();
}
