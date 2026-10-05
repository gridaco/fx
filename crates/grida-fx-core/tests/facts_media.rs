//! File facts against the media fixtures of spec/facts.md §6.
//!
//! Every folder under `spec/vectors/facts/` holds media files and an `expected.json` that maps
//! each file name to its facts (members in their written order) or to `{"refused": "<message>"}`.
//! `tools/make_fixtures.py` made the files and reads them back the same way in Python; this test
//! holds the engine to the same values, exactly.

use grida_fx_core::facts::file_facts;
use grida_fx_core::kinds::kind_of;
use grida_fx_core::value::parse_json;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/facts")
}

fn folders() -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(root())
        .expect("spec/vectors/facts is readable")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    folders
}

fn expected(folder: &Path) -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(folder.join("expected.json")).unwrap();
    match parse_json(&text).unwrap() {
        Value::Object(map) => map,
        other => panic!(
            "{}: expected.json is not an object: {other}",
            folder.display()
        ),
    }
}

#[test]
fn every_folder_is_there() {
    let names: Vec<String> = folders()
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["matroska", "mp4", "wav"]);
}

#[test]
fn every_fixture_has_its_expected_facts() {
    let mut checked = 0;
    for folder in folders() {
        let expected = expected(&folder);
        let on_disk: BTreeSet<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "expected.json")
            .collect();
        let listed: BTreeSet<String> = expected.keys().cloned().collect();
        assert_eq!(
            on_disk,
            listed,
            "{}: files and expected.json",
            folder.display()
        );
        for (name, want) in &expected {
            let bytes = std::fs::read(folder.join(name)).unwrap();
            let got = file_facts(&bytes, kind_of(name));
            let label = format!("{}/{name}", folder.file_name().unwrap().to_string_lossy());
            match (want.get("refused").and_then(Value::as_str), got) {
                (Some(reason), Err(error)) => assert_eq!(error, reason, "{label}"),
                (Some(reason), Ok(facts)) => {
                    panic!("{label}: should be refused ({reason}), read {facts}")
                }
                (None, Err(error)) => panic!("{label}: refused: {error}"),
                (None, Ok(facts)) => {
                    assert_eq!(&facts, want, "{label}");
                    let order = |value: &Value| -> Vec<String> {
                        value.as_object().unwrap().keys().cloned().collect()
                    };
                    assert_eq!(order(&facts), order(want), "{label}: member order");
                }
            }
            checked += 1;
        }
    }
    assert!(checked >= 50, "only {checked} fixtures");
}

/// The members a kind's facts may have, in the order spec/facts.md §1 writes them.
#[test]
fn members_follow_the_written_order() {
    let video = ["width", "height", "fps", "duration", "frames", "has_alpha"];
    for folder in folders() {
        for (name, facts) in expected(&folder) {
            let Some(facts) = facts.as_object() else {
                continue;
            };
            if facts.contains_key("refused") {
                continue;
            }
            let keys: Vec<&str> = facts.keys().map(String::as_str).collect();
            assert_eq!(keys[..2], ["bytes", "kind"], "{name}");
            let rest = &keys[2..];
            match facts["kind"].as_str().unwrap() {
                "audio/wav" => assert!(rest.is_empty() || rest == ["duration"], "{name}"),
                kind if kind.starts_with("video/") => {
                    let mut at = 0;
                    for key in rest {
                        let position = video.iter().position(|v| v == key).unwrap();
                        assert!(position >= at, "{name}: {key} out of order");
                        at = position;
                    }
                }
                _ => assert!(rest.is_empty(), "{name}"),
            }
        }
    }
}

/// Cut short or with bytes overwritten, no fixture makes the readers panic: each read gives
/// facts or a refusal.
#[test]
fn damaged_fixtures_never_panic() {
    for folder in folders() {
        for name in expected(&folder).keys() {
            let bytes = std::fs::read(folder.join(name)).unwrap();
            let kind = kind_of(name);
            let step = (bytes.len() / 150).max(1);
            for cut in (0..bytes.len()).step_by(step) {
                let _ = file_facts(&bytes[..cut], kind);
            }
            for at in (0..bytes.len()).step_by(step) {
                for value in [0x00, 0x01, 0x7F, 0x80, 0xFF] {
                    let mut damaged = bytes.clone();
                    damaged[at] = value;
                    let _ = file_facts(&damaged, kind);
                }
            }
        }
    }
}

/// Bytes from a fixed pseudo-random sequence after each container's signature.
#[test]
fn noise_never_panics() {
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let starts: [(&str, &[u8]); 4] = [
        ("audio/wav", b"RIFF\xff\xff\xff\xffWAVEfmt "),
        ("video/mp4", b"\0\0\0\x18ftypisom"),
        ("video/mp4", b"\0\0\x01\0moov\0\0\0\x6cmvhd"),
        ("video/webm", b"\x1a\x45\xdf\xa3\x84\x42\x82\x81w"),
    ];
    for (kind, start) in starts {
        for round in 0..400 {
            let mut bytes = start.to_vec();
            let len = (next() % 512) as usize;
            bytes.extend((0..len).map(|_| (next() >> 24) as u8));
            if round % 2 == 0 {
                // Small sizes and IDs are likelier to nest than random ones.
                for byte in bytes.iter_mut().skip(start.len()).step_by(7) {
                    *byte &= 0x8F;
                }
            }
            let _ = file_facts(&bytes, kind);
        }
    }
}
