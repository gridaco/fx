//! WAV audio facts (spec/facts.md §3): `duration` in seconds, rounded to 6 decimal places.
//!
//! Only a RIFF/WAVE file whose `fmt ` chunk is PCM (format 1) or WAVE_FORMAT_EXTENSIBLE (0xFFFE)
//! with the PCM sub-format has a duration: `nframes / frame_rate`, `nframes` being the `data`
//! chunk's declared size divided by `channels × ceil(bits / 8)`. Chunks are walked in order, with
//! a pad byte after an odd-sized one, up to the first `data` chunk, and never past the end the
//! RIFF header declares. Anything else (another format, RF64, a missing or truncated chunk, a
//! frame rate or a channel count of 0) has no duration: the facts are `bytes` and `kind` only.
//! WAV facts are never refused. Rounding is to 6 decimal places, ties to even, on the correctly
//! rounded quotient.

use super::{round6, seconds};
use serde_json::{Map, Value};

/// `WAVE_FORMAT_PCM`.
const PCM: u16 = 1;
/// `WAVE_FORMAT_EXTENSIBLE`.
const EXTENSIBLE: u16 = 0xFFFE;
/// `KSDATAFORMAT_SUBTYPE_PCM`, as its 16 bytes are stored.
const PCM_SUBFORMAT: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];
/// Where the RIFF body (`WAVE` and the chunks) starts.
const BODY: u64 = 8;

/// The facts of a WAV file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavFacts {
    pub duration: f64,
}

impl WavFacts {
    /// Adds `duration`.
    pub fn insert_into(&self, facts: &mut Map<String, Value>) {
        if let Ok(duration) = crate::value::number(self.duration) {
            facts.insert("duration".into(), duration);
        }
    }
}

/// The facts of `bytes` read as WAV (module doc); `Ok(None)` when it has no duration. Never an
/// error: a file this rule cannot read has no duration (spec/facts.md §3).
pub fn wav_facts(bytes: &[u8]) -> Result<Option<WavFacts>, String> {
    Ok(duration(bytes).map(|duration| WavFacts { duration }))
}

/// What a `fmt ` chunk says.
#[derive(Debug, Clone, Copy)]
struct Format {
    rate: u32,
    /// `channels × ceil(bits / 8)`, never 0.
    frame_size: u64,
}

fn duration(bytes: &[u8]) -> Option<f64> {
    if bytes.get(0..4)? != b"RIFF" {
        return None;
    }
    // Offsets below are relative to the body, which the RIFF size bounds; reading also stops at
    // the end of the file.
    let declared = u64::from(le32(bytes, 4)?);
    if declared < 4 || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format: Option<Format> = None;
    let mut at: u64 = 4;
    loop {
        // A chunk header lies wholly inside the declared body and the file, or the walk ends.
        if at + 8 > declared {
            return None;
        }
        let header = slice(bytes, BODY + at, BODY + at + 8)?;
        let id = &header[0..4];
        let size = u64::from(le32(header, 4)?);
        let content = at + 8;
        if id == b"data" {
            let format = format?;
            if format.rate == 0 {
                return None;
            }
            let frames = size / format.frame_size;
            return Some(round6(seconds(u128::from(frames), u64::from(format.rate))));
        }
        if id == b"fmt " {
            // The chunk's content as far as its size, the body and the file allow.
            let end = (content + size).min(declared);
            let available = (BODY + end).min(bytes.len() as u64);
            let start = (BODY + content).min(available);
            format = Some(read_format(slice(bytes, start, available)?)?);
        }
        // The next chunk, after one pad byte when the size is odd, must start inside the body.
        let next = content + size + (size & 1);
        if next > declared {
            return None;
        }
        at = next;
    }
}

/// A `fmt ` chunk's format: PCM or extensible PCM, at least one channel, a sample width above 0.
fn read_format(content: &[u8]) -> Option<Format> {
    let tag = le16(content, 0)?;
    let channels = le16(content, 2)?;
    let rate = le32(content, 4)?;
    let bits = le16(content, 14)?;
    match tag {
        PCM => {}
        EXTENSIBLE => {
            // cbSize, valid bits and the channel mask (8 bytes), then the sub-format.
            if content.get(24..40)? != PCM_SUBFORMAT {
                return None;
            }
        }
        _ => return None,
    }
    let width = u64::from(bits).div_ceil(8);
    let frame_size = u64::from(channels) * width;
    (frame_size != 0).then_some(Format { rate, frame_size })
}

fn slice(bytes: &[u8], start: u64, end: u64) -> Option<&[u8]> {
    bytes.get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
}

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], content: &[u8]) -> Vec<u8> {
        sized_chunk(id, content, content.len() as u32, true)
    }

    fn sized_chunk(id: &[u8; 4], content: &[u8], size: u32, pad: bool) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend(size.to_le_bytes());
        out.extend(content);
        if pad && content.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        riff_sized(&body, body.len() as u32 + 4)
    }

    fn riff_sized(body: &[u8], size: u32) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend(size.to_le_bytes());
        out.extend(b"WAVE");
        out.extend(body);
        out
    }

    fn fmt(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let width = u32::from(bits).div_ceil(8);
        let mut content = Vec::new();
        content.extend(tag.to_le_bytes());
        content.extend(channels.to_le_bytes());
        content.extend(rate.to_le_bytes());
        content.extend((rate * u32::from(channels) * width).to_le_bytes());
        content.extend((u32::from(channels) * width).to_le_bytes()[..2].iter());
        content.extend(bits.to_le_bytes());
        content
    }

    fn pcm(rate: u32, channels: u16, bits: u16, frames: usize) -> Vec<u8> {
        let size = frames * usize::from(channels) * usize::from(bits).div_ceil(8);
        riff(&[
            chunk(b"fmt ", &fmt(PCM, channels, rate, bits)),
            chunk(b"data", &vec![0; size]),
        ])
    }

    fn seconds_of(bytes: &[u8]) -> Option<f64> {
        wav_facts(bytes).unwrap().map(|facts| facts.duration)
    }

    #[test]
    fn pcm_durations() {
        assert_eq!(seconds_of(&pcm(8000, 1, 16, 1000)), Some(0.125));
        assert_eq!(seconds_of(&pcm(48000, 2, 16, 960)), Some(0.02));
        assert_eq!(seconds_of(&pcm(4000, 1, 8, 2000)), Some(0.5));
        assert_eq!(seconds_of(&pcm(96000, 1, 24, 480)), Some(0.005));
        assert_eq!(seconds_of(&pcm(22050, 1, 16, 0)), Some(0.0));
        // Rounded to 6 places, ties to even on the exact binary value.
        assert_eq!(seconds_of(&pcm(44100, 1, 16, 1234)), Some(0.027982));
        assert_eq!(seconds_of(&pcm(44100, 1, 16, 1)), Some(0.000023));
        assert_eq!(seconds_of(&pcm(3, 1, 16, 1)), Some(0.333333));
        // 12 bits take two bytes a sample.
        assert_eq!(seconds_of(&pcm(1000, 1, 12, 10)), Some(0.01));
    }

    #[test]
    fn extensible_needs_the_pcm_subformat() {
        let extensible = |subformat: [u8; 16]| {
            let mut content = fmt(EXTENSIBLE, 2, 44100, 16);
            content.extend(22u16.to_le_bytes());
            content.extend(16u16.to_le_bytes());
            content.extend(3u32.to_le_bytes());
            content.extend(subformat);
            riff(&[chunk(b"fmt ", &content), chunk(b"data", &[0; 441 * 4])])
        };
        assert_eq!(seconds_of(&extensible(PCM_SUBFORMAT)), Some(0.01));
        let mut float = PCM_SUBFORMAT;
        float[0] = 3;
        assert_eq!(seconds_of(&extensible(float)), None);
        // An extensible chunk cut before its sub-format.
        let short = riff(&[
            chunk(b"fmt ", &fmt(EXTENSIBLE, 1, 8000, 16)),
            chunk(b"data", &[0; 16]),
        ]);
        assert_eq!(seconds_of(&short), None);
    }

    #[test]
    fn other_formats_have_no_duration() {
        for tag in [3, 6, 7, 0x11, 0x55] {
            let file = riff(&[
                chunk(b"fmt ", &fmt(tag, 1, 8000, 16)),
                chunk(b"data", &[0; 16]),
            ]);
            assert_eq!(seconds_of(&file), None, "format {tag}");
        }
    }

    #[test]
    fn broken_files_have_no_duration_and_are_never_refused() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"RIFF".to_vec(),
            b"RIFF\x04\0\0\0WAV".to_vec(),
            b"ID3\x03\0\0\0\0\0\0".to_vec(),
            // RF64 and big-endian RIFX.
            [b"RF64".as_slice(), &pcm(8000, 1, 16, 4)[4..]].concat(),
            [b"RIFX".as_slice(), &pcm(8000, 1, 16, 4)[4..]].concat(),
            // A frame rate of 0, no channels, a sample width of 0.
            riff(&[chunk(b"fmt ", &fmt(PCM, 1, 0, 16)), chunk(b"data", &[0; 4])]),
            riff(&[
                chunk(b"fmt ", &fmt(PCM, 0, 8000, 16)),
                chunk(b"data", &[0; 4]),
            ]),
            riff(&[
                chunk(b"fmt ", &fmt(PCM, 1, 8000, 0)),
                chunk(b"data", &[0; 4]),
            ]),
            // A fmt chunk too short for its fields.
            riff(&[chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)[..14])]),
            riff(&[
                chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)[..8]),
                chunk(b"data", &[0; 4]),
            ]),
            // data before fmt; no data; no fmt.
            riff(&[
                chunk(b"data", &[0; 4]),
                chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            ]),
            riff(&[chunk(b"fmt ", &fmt(PCM, 1, 8000, 16))]),
            riff(&[chunk(b"data", &[0; 4])]),
        ];
        for (i, case) in cases.iter().enumerate() {
            assert_eq!(wav_facts(case), Ok(None), "case {i}");
        }
    }

    #[test]
    fn the_declared_data_size_counts() {
        // 96000 stereo frames declared, none present.
        let declared = riff(&[
            chunk(b"fmt ", &fmt(PCM, 2, 48000, 16)),
            sized_chunk(b"data", &[], 96000 * 4, true),
        ]);
        assert_eq!(seconds_of(&declared), Some(2.0));
        // A partial frame does not count.
        let partial = riff(&[
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            chunk(b"data", &[0; 5]),
        ]);
        assert_eq!(seconds_of(&partial), Some(0.00025));
        // A streaming writer's unknown size reads as the largest size.
        let unknown = riff(&[
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            sized_chunk(b"data", &[0; 16], u32::MAX, true),
        ]);
        assert_eq!(seconds_of(&unknown), Some(268435.455875));
    }

    #[test]
    fn odd_chunks_are_padded() {
        let padded = riff(&[
            chunk(b"LIST", b"abc"),
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            chunk(b"data", &[0; 800]),
        ]);
        assert_eq!(seconds_of(&padded), Some(0.05));
        // Without its pad byte the walk reads a header one byte early and goes astray.
        let unpadded = riff(&[
            sized_chunk(b"LIST", b"abc", 3, false),
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            chunk(b"data", &[0; 800]),
        ]);
        assert_eq!(seconds_of(&unpadded), None);
        // An odd fmt chunk (an extra byte after its fields) is padded too.
        let mut long_fmt = fmt(PCM, 1, 8000, 16);
        long_fmt.push(0);
        let odd_fmt = riff(&[chunk(b"fmt ", &long_fmt), chunk(b"data", &[0; 800])]);
        assert_eq!(seconds_of(&odd_fmt), Some(0.05));
    }

    #[test]
    fn the_riff_size_bounds_the_walk() {
        let body = [
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            chunk(b"data", &[0; 800]),
        ]
        .concat();
        // Unknown (streaming) RIFF size: the walk stops at the end of the file instead.
        assert_eq!(seconds_of(&riff_sized(&body, u32::MAX)), Some(0.05));
        // A size that ends inside the data chunk's header.
        assert_eq!(seconds_of(&riff_sized(&body, 4 + 24 + 4)), None);
        // A size that ends right after the data chunk's header is enough.
        assert_eq!(seconds_of(&riff_sized(&body, 4 + 24 + 8)), Some(0.05));
        // A size that ends inside a chunk that is skipped.
        let skipped = [chunk(b"LIST", &[0; 10]), body.clone()].concat();
        assert_eq!(seconds_of(&riff_sized(&skipped, 4 + 10)), None);
        // A size below 4 cannot hold WAVE.
        assert_eq!(seconds_of(&riff_sized(&body, 3)), None);
        // A file cut inside a header ends the walk.
        let mut cut = riff(&[chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)), chunk(b"data", &[])]);
        cut.truncate(cut.len() - 2);
        assert_eq!(seconds_of(&cut), None);
    }

    #[test]
    fn a_later_fmt_chunk_wins() {
        let file = riff(&[
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            chunk(b"fmt ", &fmt(PCM, 1, 16000, 16)),
            chunk(b"data", &[0; 800]),
        ]);
        assert_eq!(seconds_of(&file), Some(0.025));
        let invalid_later = riff(&[
            chunk(b"fmt ", &fmt(PCM, 1, 8000, 16)),
            chunk(b"fmt ", &fmt(3, 1, 16000, 16)),
            chunk(b"data", &[0; 800]),
        ]);
        assert_eq!(seconds_of(&invalid_later), None);
    }

    #[test]
    fn insert_into_writes_numbers_in_fx_form() {
        let mut facts = Map::new();
        WavFacts { duration: 2.0 }.insert_into(&mut facts);
        WavFacts { duration: 0.5 }.insert_into(&mut facts);
        assert_eq!(Value::Object(facts), serde_json::json!({"duration": 0.5}));
        let mut whole = Map::new();
        WavFacts { duration: 2.0 }.insert_into(&mut whole);
        assert_eq!(whole["duration"].as_u64(), Some(2));
    }
}
