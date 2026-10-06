//! File facts against the media fixtures of spec/facts.md §6.
//!
//! Every folder under `spec/vectors/facts/` holds media files and an `expected.json` that maps
//! each file name to its facts (members in their written order) or to `{"refused": "<message>"}`.
//! `tools/make_fixtures.py` made the files and reads them back the same way in Python; this test
//! holds the engine to the same values, exactly, whether it reads them from bytes in memory or
//! from the file, and checks that reading a file for its facts reads only what the rules name.

use grida_fx_core::facts::{
    FactsError, RefFacts, file_facts, read_file_facts, read_ref_facts, reader_facts,
};
use grida_fx_core::kinds::kind_of;
use grida_fx_core::value::parse_json;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{self, Read, Seek, SeekFrom};
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
            // Read from the file, the facts are the same.
            let from_file = read_file_facts(&folder.join(name), kind_of(name));
            match (&got, from_file) {
                (Ok(facts), Ok(read)) => assert_eq!(facts, &read, "{label}: read from the file"),
                (Err(error), Err(FactsError::Refused(reason))) => {
                    assert_eq!(error, &reason, "{label}: read from the file")
                }
                (got, read) => panic!("{label}: {got:?} in memory, {read:?} from the file"),
            }
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

/// A file of `len` bytes that holds `pieces` at their offsets and zeros everywhere else, and
/// counts the bytes read from it.
struct Sparse {
    len: u64,
    pieces: Vec<(u64, Vec<u8>)>,
    at: u64,
    read: u64,
}

impl Sparse {
    fn new(len: u64, pieces: Vec<(u64, Vec<u8>)>) -> Self {
        Sparse {
            len,
            pieces,
            at: 0,
            read: 0,
        }
    }
}

impl Read for Sparse {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = (self.len.saturating_sub(self.at)).min(buf.len() as u64) as usize;
        for (i, byte) in buf[..count].iter_mut().enumerate() {
            let at = self.at + i as u64;
            *byte = self
                .pieces
                .iter()
                .find(|(start, piece)| at >= *start && at - start < piece.len() as u64)
                .map_or(0, |(start, piece)| piece[(at - start) as usize]);
        }
        self.at += count as u64;
        self.read += count as u64;
        Ok(count)
    }
}

impl Seek for Sparse {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.at = match to {
            SeekFrom::Start(at) => at,
            SeekFrom::End(delta) => self.len.checked_add_signed(delta).unwrap(),
            SeekFrom::Current(delta) => self.at.checked_add_signed(delta).unwrap(),
        };
        Ok(self.at)
    }
}

/// The facts of a sparse file, and the bytes reading them took.
fn sparse_facts(file: Sparse, kind: &str) -> (Value, u64) {
    let mut file = file;
    let facts = reader_facts(&mut file, kind).unwrap();
    (facts, file.read)
}

const TIB: u64 = 1 << 40;

/// spec/facts.md §1: facts read the boxes and elements their rules name, never a whole file. A
/// 1 TiB `mdat`, `data` chunk or block costs no more to read past than a small one.
#[test]
fn facts_read_only_what_their_rules_name() {
    // An MP4 whose moov comes first, then a 64-bit mdat of 1 TiB.
    let fixture = std::fs::read(root().join("mp4/faststart.mp4")).unwrap();
    let mdat = fixture.windows(4).position(|w| w == b"mdat").unwrap() - 4;
    let mut head = fixture[..mdat].to_vec();
    head.extend([0, 0, 0, 1]);
    head.extend(b"mdat");
    head.extend((TIB + 16).to_be_bytes());
    let len = mdat as u64 + TIB + 16;
    let (facts, read) = sparse_facts(Sparse::new(len, vec![(0, head)]), "video/mp4");
    assert_eq!(
        facts,
        json!({"bytes": len, "kind": "video/mp4", "width": 64, "height": 48, "fps": 24,
               "duration": 1, "frames": 24, "has_alpha": false})
    );
    assert!(read <= 2 * 64 * 1024, "{read} bytes read");

    // A WAV of 1 TiB whose data chunk declares the most it can: only the chunk headers are read.
    let wav = std::fs::read(root().join("wav/pcm16_8k.wav")).unwrap();
    let data = wav.windows(4).position(|w| w == b"data").unwrap();
    let mut header = wav[..data + 8].to_vec();
    header[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    header[data + 4..data + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let (facts, read) = sparse_facts(Sparse::new(TIB, vec![(0, header)]), "audio/wav");
    assert_eq!(
        facts,
        json!({"bytes": TIB, "kind": "audio/wav", "duration": 268435.455875})
    );
    assert!(read <= 64 * 1024, "{read} bytes read");

    // A Matroska file whose first block holds 1 TiB of frame data: only its header is read.
    let element = |id: &[u8], content: &[u8]| {
        let mut out = id.to_vec();
        out.push(0x01);
        out.extend(&(content.len() as u64).to_be_bytes()[1..]);
        out.extend(content);
        out
    };
    let uint = |id: &[u8], value: u64| element(id, &value.to_be_bytes());
    let ebml = element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"webm"));
    let info = element(
        &[0x15, 0x49, 0xA9, 0x66],
        &[
            uint(&[0x2A, 0xD7, 0xB1], 1_000_000),
            element(&[0x44, 0x89], &1000f64.to_be_bytes()),
        ]
        .concat(),
    );
    let video = element(&[0xE0], &[uint(&[0xB0], 64), uint(&[0xBA], 48)].concat());
    let entry = [
        uint(&[0xD7], 1),
        uint(&[0x83], 1),
        element(&[0x86], b"V_VP9"),
        uint(&[0x23, 0xE3, 0x83], 41_666_666),
        video,
    ]
    .concat();
    let tracks = element(&[0x16, 0x54, 0xAE, 0x6B], &element(&[0xAE], &entry));
    // An unknown-size Segment and Cluster, then a SimpleBlock of 1 TiB.
    let mut head = ebml;
    head.extend([0x18, 0x53, 0x80, 0x67, 0xFF]);
    head.extend(info);
    head.extend(tracks);
    head.extend([0x1F, 0x43, 0xB6, 0x75, 0xFF]);
    head.extend([0xA3, 0x01]);
    head.extend(&TIB.to_be_bytes()[1..]);
    head.extend([0x81, 0, 0, 0x80]);
    let len = head.len() as u64 - 4 + TIB;
    let (facts, read) = sparse_facts(Sparse::new(len, vec![(0, head)]), "video/webm");
    assert_eq!(
        facts,
        json!({"bytes": len, "kind": "video/webm", "width": 64, "height": 48, "fps": 24,
               "duration": 1, "frames": 1, "has_alpha": false})
    );
    assert!(read <= 64 * 1024, "{read} bytes read");
}

/// protocol.md §3.1: a file ref of a file whose facts are refused carries `bytes` and `kind`
/// only; the reason comes beside them, for the engine to log.
#[test]
fn ref_facts_of_a_refused_file_are_its_size_and_kind() {
    let refused = root().join("mp4/truncated.mp4");
    assert_eq!(
        read_ref_facts(&refused, "video/mp4").unwrap(),
        RefFacts {
            facts: json!({"bytes": 2000, "kind": "video/mp4"})
                .as_object()
                .unwrap()
                .clone(),
            refused: Some(
                "not an MP4 file (a box at byte 40 runs past the end of the file)".into()
            ),
        }
    );
    // An audio-only MP4 has the same members, and no refusal.
    let audio = read_ref_facts(&root().join("mp4/audio_only.mp4"), "video/mp4").unwrap();
    assert_eq!(audio.facts.keys().collect::<Vec<_>>(), ["bytes", "kind"]);
    assert_eq!(audio.refused, None);
    // A file that decodes carries all of its facts.
    let clip = root().join("mp4/h264_2997.mp4");
    let facts = read_ref_facts(&clip, "video/mp4").unwrap();
    assert_eq!(
        Value::Object(facts.facts),
        read_file_facts(&clip, "video/mp4").unwrap()
    );
    // A file that cannot be read is an error, not a refusal.
    assert!(read_ref_facts(&root().join("mp4/missing.mp4"), "video/mp4").is_err());
    assert!(matches!(
        read_file_facts(&root().join("mp4/missing.mp4"), "video/mp4"),
        Err(FactsError::Io(_))
    ));
}
