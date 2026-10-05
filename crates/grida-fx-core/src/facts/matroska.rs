//! Matroska and WebM video facts (spec/facts.md §5): the first `TrackEntry` whose `TrackType`
//! is 1 (video).
//!
//! - `width`, `height`: `PixelWidth`, `PixelHeight`;
//! - `fps`: from `DefaultDuration` (nanoseconds per frame): the fraction closest to
//!   10^9 / `DefaultDuration` whose denominator is at most 1001, rounded to 6 places; absent
//!   without it;
//! - `duration`: the segment's `Duration × TimecodeScale`, truncated to whole microseconds, in
//!   seconds, rounded to 6 places;
//! - `frames`: the track's `SimpleBlock` and `BlockGroup` frames (after lacing);
//! - `has_alpha`: by codec as spec/facts.md lists (`V_VP8`, `V_VP9` false even with
//!   `AlphaMode` 1; FFV1 by its transparency flag; PNG by its first frame).
//!
//! An EBML stream that does not parse is refused with `not a Matroska or WebM file (<reason>)`.
//! Every size is checked against the bytes present before it is used; nothing is allocated from
//! a declared size, and elements are only descended into at the levels these facts need.

use super::{VideoFacts, ffv1, png_has_alpha, round6};

const EBML: u32 = 0x1A45_DFA3;
const DOC_TYPE: u32 = 0x4282;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const CLUSTER: u32 = 0x1F43_B675;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const SIMPLE_BLOCK: u32 = 0xA3;
/// The children of a Segment, and the top-level elements: a Cluster of unknown size ends where
/// one of them starts.
const LEVEL_ONE: [u32; 10] = [
    0x114D_9B74, // SeekHead
    INFO,
    TRACKS,
    CLUSTER,
    0x1C53_BB6B, // Cues
    0x1941_A469, // Attachments
    0x1043_A770, // Chapters
    0x1254_C367, // Tags
    EBML,
    SEGMENT,
];
/// Codecs that never decode with alpha.
const NO_ALPHA_CODECS: [&str; 11] = [
    "V_VP8",
    "V_VP9",
    "V_AV1",
    "V_MPEG4/ISO/AVC",
    "V_MPEGH/ISO/HEVC",
    "V_MPEG4/ISO/SP",
    "V_MPEG4/ISO/ASP",
    "V_MPEG4/ISO/AP",
    "V_MPEG1",
    "V_MPEG2",
    "V_THEORA",
];
/// `BITMAPINFOHEADER` compression codes of PNG in `V_MS/VFW/FOURCC`.
const PNG_FOURCCS: [&[u8; 4]; 3] = [b"MPNG", b"PNG1", b"png "];
/// The largest denominator of a frame rate read from `DefaultDuration` (spec/facts.md §5.2).
const RATE_DENOMINATOR: u128 = 1001;

/// The facts of `bytes` read as Matroska or WebM (module doc).
pub fn matroska_facts(bytes: &[u8]) -> Result<Option<VideoFacts>, String> {
    read(bytes).map_err(|reason| format!("not a Matroska or WebM file ({reason})"))
}

/// Why a variable-size integer could not be read.
enum VintError {
    /// It runs past the end of what holds it.
    Short,
    /// Its first byte is 0.
    Invalid,
}

/// `(value without its marker, length, every value bit set)` of the variable-size integer at
/// `at`, which must end by `end`.
fn vint(file: &[u8], at: usize, end: usize) -> Result<(u64, usize, bool), VintError> {
    if at >= end {
        return Err(VintError::Short);
    }
    let first = file[at];
    if first == 0 {
        return Err(VintError::Invalid);
    }
    let length = first.leading_zeros() as usize + 1;
    if at + length > end {
        return Err(VintError::Short);
    }
    let mut value = u64::from(first) & (0xFF >> length);
    for &byte in &file[at + 1..at + length] {
        value = value << 8 | u64::from(byte);
    }
    let all_ones = value == (1u64 << (7 * length)) - 1;
    Ok((value, length, all_ones))
}

/// An element: its ID (marker kept), where it starts, where its content starts, its size
/// (`None` when unknown).
#[derive(Debug, Clone, Copy)]
struct Header {
    id: u32,
    at: usize,
    content: usize,
    size: Option<u64>,
}

fn header(file: &[u8], at: usize, end: usize) -> Result<Header, String> {
    let cut = || format!("an element header at byte {at} is cut short");
    if at >= end {
        return Err(cut());
    }
    let first = file[at];
    if first & 0xF0 == 0 {
        return Err(format!("an element ID at byte {at} is not valid"));
    }
    let length = first.leading_zeros() as usize + 1;
    if at + length > end {
        return Err(cut());
    }
    let id = file[at..at + length]
        .iter()
        .fold(0u32, |id, &byte| id << 8 | u32::from(byte));
    let (size, size_length, unknown) = match vint(file, at + length, end) {
        Ok(read) => read,
        Err(VintError::Short) => return Err(cut()),
        Err(VintError::Invalid) => {
            return Err(format!(
                "an element size at byte {} is not valid",
                at + length
            ));
        }
    };
    Ok(Header {
        id,
        at,
        content: at + length + size_length,
        size: (!unknown).then_some(size),
    })
}

/// An element with its extent: `file[content..end]`.
#[derive(Debug, Clone, Copy)]
struct Element {
    id: u32,
    content: usize,
    end: usize,
}

/// The elements of known size laid end to end in `file[at..end]`; the first that does not fit
/// is an error and ends the walk.
struct Children<'a> {
    file: &'a [u8],
    at: usize,
    end: usize,
    failed: bool,
}

impl<'a> Children<'a> {
    fn of(file: &'a [u8], parent: Element) -> Self {
        Children {
            file,
            at: parent.content,
            end: parent.end,
            failed: false,
        }
    }

    fn read(&mut self) -> Result<Element, String> {
        let header = header(self.file, self.at, self.end)?;
        let Some(size) = header.size else {
            return Err(format!(
                "an element at byte {} has an unknown size",
                header.at
            ));
        };
        let end = fits(header, size, self.end, "its parent")?;
        self.at = end;
        Ok(Element {
            id: header.id,
            content: header.content,
            end,
        })
    }
}

impl Iterator for Children<'_> {
    type Item = Result<Element, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.at >= self.end {
            return None;
        }
        let result = self.read();
        self.failed = result.is_err();
        Some(result)
    }
}

/// The end of an element of `size` that must end by `limit`, or `runs past the end of <what>`.
fn fits(header: Header, size: u64, limit: usize, what: &str) -> Result<usize, String> {
    let room = (limit - header.content) as u64;
    if size > room {
        return Err(format!(
            "an element at byte {} runs past the end of {what}",
            header.at
        ));
    }
    // `size <= room`, so it fits in a usize.
    Ok(header.content + size as usize)
}

/// The children of a Segment, like [`Children`] except that a Cluster may have an unknown size:
/// it then ends where the next level-1 element starts, or at the end of the Segment.
struct SegmentChildren<'a> {
    file: &'a [u8],
    at: usize,
    end: usize,
    failed: bool,
}

impl SegmentChildren<'_> {
    fn read(&mut self) -> Result<Element, String> {
        let header = header(self.file, self.at, self.end)?;
        let end = match header.size {
            Some(size) => fits(header, size, self.end, "its parent")?,
            None if header.id == CLUSTER => {
                let mut stop = header.content;
                while stop < self.end {
                    let child = self::header(self.file, stop, self.end)?;
                    if LEVEL_ONE.contains(&child.id) {
                        break;
                    }
                    let Some(size) = child.size else {
                        return Err(format!("an element at byte {stop} has an unknown size"));
                    };
                    stop = fits(child, size, self.end, "its parent")?;
                }
                stop
            }
            None => {
                return Err(format!(
                    "an element at byte {} has an unknown size",
                    header.at
                ));
            }
        };
        self.at = end;
        Ok(Element {
            id: header.id,
            content: header.content,
            end,
        })
    }
}

impl Iterator for SegmentChildren<'_> {
    type Item = Result<Element, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.at >= self.end {
            return None;
        }
        let result = self.read();
        self.failed = result.is_err();
        Some(result)
    }
}

fn uint(file: &[u8], element: Element) -> Result<u64, String> {
    let bytes = &file[element.content..element.end];
    if bytes.len() > 8 {
        return Err(format!(
            "an integer at byte {} is longer than 8 bytes",
            element.content
        ));
    }
    Ok(bytes
        .iter()
        .fold(0u64, |value, &byte| value << 8 | u64::from(byte)))
}

fn float(file: &[u8], element: Element) -> Result<f64, String> {
    let bytes = &file[element.content..element.end];
    match bytes.len() {
        0 => Ok(0.0),
        4 => Ok(f64::from(f32::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
        ]))),
        8 => {
            let mut array = [0; 8];
            array.copy_from_slice(bytes);
            Ok(f64::from_be_bytes(array))
        }
        n => Err(format!(
            "a float at byte {} is {n} bytes long",
            element.content
        )),
    }
}

/// A string element's bytes without trailing NUL padding.
fn text(file: &[u8], element: Element) -> &[u8] {
    let bytes = &file[element.content..element.end];
    let len = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    &bytes[..len]
}

/// What the facts need from a `TrackEntry`; the first of each element counts.
#[derive(Debug, Default)]
struct Track<'a> {
    number: Option<u64>,
    kind: Option<u64>,
    codec: Option<&'a [u8]>,
    private: Option<&'a [u8]>,
    default_duration: Option<u64>,
    video: bool,
    width: Option<u64>,
    height: Option<u64>,
}

fn track_entry(file: &[u8], entry: Element) -> Result<Track<'_>, String> {
    let mut track = Track::default();
    for child in Children::of(file, entry) {
        let child = child?;
        match child.id {
            TRACK_NUMBER if track.number.is_none() => track.number = Some(uint(file, child)?),
            TRACK_TYPE if track.kind.is_none() => track.kind = Some(uint(file, child)?),
            CODEC_ID if track.codec.is_none() => track.codec = Some(text(file, child)),
            CODEC_PRIVATE if track.private.is_none() => {
                track.private = Some(&file[child.content..child.end]);
            }
            DEFAULT_DURATION if track.default_duration.is_none() => {
                track.default_duration = Some(uint(file, child)?);
            }
            VIDEO if !track.video => {
                track.video = true;
                for item in Children::of(file, child) {
                    let item = item?;
                    match item.id {
                        PIXEL_WIDTH if track.width.is_none() => {
                            track.width = Some(uint(file, item)?);
                        }
                        PIXEL_HEIGHT if track.height.is_none() => {
                            track.height = Some(uint(file, item)?);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(track)
}

fn read(file: &[u8]) -> Result<Option<VideoFacts>, String> {
    let end = file.len();
    if file.get(0..4) != Some(EBML.to_be_bytes().as_slice()) {
        return Err("it does not start with an EBML header".into());
    }
    let ebml = header(file, 0, end)?;
    let Some(size) = ebml.size else {
        return Err("an element at byte 0 has an unknown size".into());
    };
    let ebml = Element {
        id: EBML,
        content: ebml.content,
        end: fits(ebml, size, end, "the file")?,
    };
    let mut doc_type: &[u8] = b"matroska";
    for child in Children::of(file, ebml) {
        let child = child?;
        if child.id == DOC_TYPE {
            doc_type = text(file, child);
            break;
        }
    }
    if doc_type != b"matroska" && doc_type != b"webm" {
        return Err("its DocType is not matroska or webm".into());
    }
    let mut at = ebml.end;
    let mut segment = None;
    while at < end {
        let header = header(file, at, end)?;
        let stop = match header.size {
            Some(size) => fits(header, size, end, "the file")?,
            None if header.id == SEGMENT => end,
            None => return Err(format!("an element at byte {at} has an unknown size")),
        };
        if header.id == SEGMENT {
            segment = Some((header.content, stop));
            break;
        }
        at = stop;
    }
    let (start, stop) = segment.ok_or("it has no Segment")?;
    let segment_children = || SegmentChildren {
        file,
        at: start,
        end: stop,
        failed: false,
    };

    let mut scale = None;
    let mut duration = None;
    let mut track = None;
    let (mut seen_info, mut seen_tracks) = (false, false);
    for element in segment_children() {
        let element = element?;
        if element.id == INFO && !seen_info {
            seen_info = true;
            for child in Children::of(file, element) {
                let child = child?;
                match child.id {
                    TIMECODE_SCALE if scale.is_none() => scale = Some(uint(file, child)?),
                    DURATION if duration.is_none() => duration = Some(float(file, child)?),
                    _ => {}
                }
            }
        } else if element.id == TRACKS && !seen_tracks {
            seen_tracks = true;
            for child in Children::of(file, element) {
                let child = child?;
                if child.id == TRACK_ENTRY {
                    let entry = track_entry(file, child)?;
                    if entry.kind == Some(1) {
                        track = Some(entry);
                        break;
                    }
                }
            }
        }
    }
    let Some(track) = track else {
        return Ok(None);
    };
    let (Some(width), Some(height)) = (track.width, track.height) else {
        return Err("its video track has no pixel size".into());
    };
    let pixels = |n: u64| {
        u32::try_from(n).map_err(|_| "its video track's pixel size is too large".to_string())
    };
    let (width, height) = (pixels(width)?, pixels(height)?);

    let mut frames: u64 = 0;
    let mut first_frame: Option<&[u8]> = None;
    for element in segment_children() {
        let element = element?;
        if element.id != CLUSTER {
            continue;
        }
        for child in Children::of(file, element) {
            let child = child?;
            let mut count_block = |block: Element| -> Result<(), String> {
                let (number, count, frame) = block_frames(file, block)?;
                if Some(number) == track.number {
                    frames = frames.saturating_add(count);
                    first_frame.get_or_insert(frame);
                }
                Ok(())
            };
            match child.id {
                SIMPLE_BLOCK => count_block(child)?,
                BLOCK_GROUP => {
                    for item in Children::of(file, child) {
                        let item = item?;
                        if item.id == BLOCK {
                            count_block(item)?;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let fps = track
        .default_duration
        .filter(|&nanoseconds| nanoseconds > 0)
        .and_then(frame_rate)
        .map(round6);
    let duration = duration.and_then(|value| seconds(value, scale.unwrap_or(1_000_000)));
    Ok(Some(VideoFacts {
        width,
        height,
        fps,
        duration,
        frames: Some(frames),
        has_alpha: codec_alpha(&track, first_frame),
    }))
}

/// The segment's duration in seconds: `Duration × TimecodeScale` nanoseconds, computed in
/// binary64 as `Duration × TimecodeScale × 1000 ÷ 1000000` microseconds and truncated toward
/// zero, then in seconds rounded to 6 places. A duration that is not a finite number above 0,
/// or a scale of 0, gives none.
fn seconds(duration: f64, scale: u64) -> Option<f64> {
    if !(duration.is_finite() && duration > 0.0) || scale == 0 {
        return None;
    }
    let micros = duration * scale as f64 * 1000.0 / 1_000_000.0;
    micros
        .is_finite()
        .then(|| round6(micros.trunc() / 1_000_000.0))
}

/// The frame rate of a `DefaultDuration` (spec/facts.md §5.2): the fraction closest to
/// 10^9 / `nanoseconds` whose denominator is at most 1001 (of two equally close, the one with
/// the smaller denominator); `None` when that fraction is 0.
fn frame_rate(nanoseconds: u64) -> Option<f64> {
    let (p, q) = nearest_fraction(1_000_000_000, u128::from(nanoseconds), RATE_DENOMINATOR);
    (p != 0).then(|| p as f64 / q as f64)
}

/// The fraction `p / q` closest to `n / d` (`d > 0`) with `1 <= q <= limit`, by its continued
/// fraction: the last convergent within the limit, or the semiconvergent after it.
fn nearest_fraction(n: u128, d: u128, limit: u128) -> (u128, u128) {
    let divisor = gcd(n, d);
    let (n, d) = (n / divisor, d / divisor);
    if d <= limit {
        return (n, d);
    }
    let (mut p0, mut q0, mut p1, mut q1) = (0u128, 1u128, 1u128, 0u128);
    let (mut num, mut den) = (n, d);
    loop {
        let a = num / den;
        let q2 = q0 + a * q1;
        if q2 > limit {
            break;
        }
        (p0, q0, p1, q1) = (p1, q1, p0 + a * p1, q2);
        (num, den) = (den, num - a * den);
    }
    let k = (limit - q0) / q1;
    let semi = (p0 + k * p1, q0 + k * q1);
    let convergent = (p1, q1);
    // |p/q - n/d| compared as |p·d - n·q| / q, with both sides over d.
    let error = |(p, q): (u128, u128)| (p * d).abs_diff(n * q);
    let (semi_error, convergent_error) = (error(semi), error(convergent));
    // semi_error / semi.q against convergent_error / convergent.q
    let left = semi_error * convergent.1;
    let right = convergent_error * semi.1;
    if left < right || (left == right && semi.1 < convergent.1) {
        semi
    } else {
        convergent
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// `(track number, frames, the first frame's bytes)` of a SimpleBlock or Block.
fn block_frames(file: &[u8], block: Element) -> Result<(u64, u64, &[u8]), String> {
    laced(file, block).map_err(|error| match error {
        VintError::Short => format!("a block at byte {} is too short", block.content),
        VintError::Invalid => format!("a block at byte {} is not valid", block.content),
    })
}

fn laced(file: &[u8], block: Element) -> Result<(u64, u64, &[u8]), VintError> {
    let end = block.end;
    let (number, length, _) = vint(file, block.content, end)?;
    // The track number, a 16-bit timecode and the flags.
    let mut at = block.content + length + 3;
    if at > end {
        return Err(VintError::Short);
    }
    let lacing = (file[at - 1] >> 1) & 3;
    if lacing == 0 {
        return Ok((number, 1, &file[at..end]));
    }
    if at >= end {
        return Err(VintError::Short);
    }
    let count = u64::from(file[at]) + 1;
    at += 1;
    if count == 1 {
        return Ok((number, 1, &file[at..end]));
    }
    let first = match lacing {
        // Fixed-size lacing: every frame the same size.
        2 => (end - at) / count as usize,
        // Xiph lacing: each size but the last is a run of 255s and a byte below 255.
        1 => {
            let mut first = None;
            for _ in 1..count {
                let mut size = 0usize;
                loop {
                    let byte = *file.get(at).filter(|_| at < end).ok_or(VintError::Short)?;
                    at += 1;
                    size += usize::from(byte);
                    if byte != 255 {
                        break;
                    }
                }
                first.get_or_insert(size);
            }
            first.unwrap_or(0)
        }
        // EBML lacing: the first size, then the others as signed differences.
        _ => {
            let (first, length, _) = vint(file, at, end)?;
            at += length;
            for _ in 2..count {
                let (_, length, _) = vint(file, at, end)?;
                at += length;
            }
            usize::try_from(first).unwrap_or(usize::MAX)
        }
    };
    let stop = at.saturating_add(first).min(end);
    Ok((number, count, &file[at..stop]))
}

/// `has_alpha` by codec (spec/facts.md §5.4); `None` for codecs the table does not name.
fn codec_alpha(track: &Track<'_>, first_frame: Option<&[u8]>) -> Option<bool> {
    let codec = track.codec.unwrap_or_default();
    let private = track.private.unwrap_or_default();
    if NO_ALPHA_CODECS.iter().any(|name| name.as_bytes() == codec) {
        return Some(false);
    }
    match codec {
        b"V_FFV1" => ffv1::has_alpha(private, first_frame),
        b"V_MS/VFW/FOURCC" if private.len() >= 40 => {
            // BITMAPINFOHEADER: biSize (4) … biCompression at 16.
            let fourcc = &private[16..20];
            if PNG_FOURCCS.iter().any(|png| png.as_slice() == fourcc) {
                first_frame.and_then(png_has_alpha)
            } else if fourcc == b"FFV1" {
                let size = u32::from_le_bytes([private[0], private[1], private[2], private[3]]);
                let extra = usize::try_from(size)
                    .ok()
                    .filter(|&size| (40..=private.len()).contains(&size))
                    .map_or(&[][..], |size| &private[size..]);
                ffv1::has_alpha(extra, first_frame)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An element: its ID bytes, a size of eight bytes (or unknown), its content.
    fn element(id: u32, content: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.push(0x01);
        out.extend(&(content.len() as u64).to_be_bytes()[1..]);
        out.extend(content);
        out
    }

    fn unknown(id: u32, content: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.push(0xFF);
        out.extend(content);
        out
    }

    fn id_bytes(id: u32) -> Vec<u8> {
        let bytes = id.to_be_bytes();
        let skip = bytes.iter().position(|&b| b != 0).unwrap();
        bytes[skip..].to_vec()
    }

    fn uint_element(id: u32, value: u64) -> Vec<u8> {
        element(id, &value.to_be_bytes())
    }

    fn float_element(id: u32, value: f64) -> Vec<u8> {
        element(id, &value.to_be_bytes())
    }

    fn ebml_header(doc_type: &str) -> Vec<u8> {
        element(EBML, &element(DOC_TYPE, doc_type.as_bytes()))
    }

    struct Clip {
        doc_type: &'static str,
        codec: &'static str,
        private: Option<Vec<u8>>,
        default_duration: Option<u64>,
        info: Vec<u8>,
        clusters: Vec<u8>,
        segment_unknown: bool,
        video_type: u64,
    }

    impl Clip {
        fn new(codec: &'static str) -> Self {
            Clip {
                doc_type: "webm",
                codec,
                private: None,
                default_duration: Some(41_666_666),
                info: [
                    uint_element(TIMECODE_SCALE, 1_000_000),
                    float_element(DURATION, 1000.0),
                ]
                .concat(),
                clusters: element(CLUSTER, &simple_blocks(1, 24)),
                segment_unknown: false,
                video_type: 1,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut entry = [
                uint_element(TRACK_NUMBER, 2),
                uint_element(TRACK_TYPE, 2),
                element(CODEC_ID, b"A_OPUS"),
            ]
            .concat();
            let audio = element(TRACK_ENTRY, &entry);
            entry = [
                uint_element(TRACK_NUMBER, 1),
                uint_element(TRACK_TYPE, self.video_type),
                element(CODEC_ID, self.codec.as_bytes()),
            ]
            .concat();
            if let Some(private) = &self.private {
                entry.extend(element(CODEC_PRIVATE, private));
            }
            if let Some(default) = self.default_duration {
                entry.extend(uint_element(DEFAULT_DURATION, default));
            }
            entry.extend(element(
                VIDEO,
                &[
                    uint_element(PIXEL_WIDTH, 64),
                    uint_element(PIXEL_HEIGHT, 48),
                ]
                .concat(),
            ));
            let tracks = element(TRACKS, &[audio, element(TRACK_ENTRY, &entry)].concat());
            let body = [
                element(0xEC, &[0; 3]),
                element(INFO, &self.info),
                tracks,
                self.clusters.clone(),
            ]
            .concat();
            let segment = if self.segment_unknown {
                unknown(SEGMENT, &body)
            } else {
                element(SEGMENT, &body)
            };
            [ebml_header(self.doc_type), segment].concat()
        }
    }

    /// `count` unlaced SimpleBlocks of `track`, and one of track 2 between each.
    fn simple_blocks(track: u8, count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..count {
            out.extend(element(SIMPLE_BLOCK, &[0x80 | track, 0, 0, 0x80, 1, 2, 3]));
            out.extend(element(SIMPLE_BLOCK, &[0x82, 0, 0, 0x80, 9]));
        }
        out
    }

    fn facts(file: &[u8]) -> VideoFacts {
        matroska_facts(file).unwrap().unwrap()
    }

    #[test]
    fn a_plain_clip() {
        assert_eq!(
            facts(&Clip::new("V_VP9").bytes()),
            VideoFacts {
                width: 64,
                height: 48,
                fps: Some(24.0),
                duration: Some(1.0),
                frames: Some(24),
                has_alpha: Some(false),
            }
        );
        let mut matroska = Clip::new("V_MPEG4/ISO/AVC");
        matroska.doc_type = "matroska";
        assert_eq!(facts(&matroska.bytes()).has_alpha, Some(false));
        // An unknown codec leaves has_alpha out.
        assert_eq!(facts(&Clip::new("V_PRORES").bytes()).has_alpha, None);
    }

    #[test]
    fn the_rate_rule() {
        let fps = |nanoseconds: Option<u64>| {
            let mut clip = Clip::new("V_VP9");
            clip.default_duration = nanoseconds;
            facts(&clip.bytes()).fps
        };
        assert_eq!(fps(Some(41_666_666)), Some(24.0));
        assert_eq!(fps(Some(41_666_667)), Some(24.0));
        assert_eq!(fps(Some(40_000_000)), Some(25.0));
        assert_eq!(fps(Some(33_366_667)), Some(29.97003));
        assert_eq!(fps(Some(41_708_333)), Some(23.976024));
        assert_eq!(fps(Some(16_683_333)), Some(59.94006));
        assert_eq!(fps(Some(80_000_000)), Some(12.5));
        assert_eq!(fps(Some(2_000_000_000)), Some(0.5));
        assert_eq!(fps(Some(1)), Some(1_000_000_000.0));
        // A rate that rounds to 0 within the limit, no DefaultDuration, or 0: no fps.
        assert_eq!(fps(Some(u64::MAX)), None);
        assert_eq!(fps(None), None);
        assert_eq!(fps(Some(0)), None);
    }

    #[test]
    fn nearest_fractions() {
        assert_eq!(nearest_fraction(1_000_000_000, 41_666_666, 1001), (24, 1));
        assert_eq!(
            nearest_fraction(1_000_000_000, 33_366_667, 1001),
            (30000, 1001)
        );
        assert_eq!(
            nearest_fraction(1_000_000_000, 33_366_700, 1001),
            (2997, 100)
        );
        assert_eq!(nearest_fraction(6, 4, 1001), (3, 2));
        // 1/3 lies between 0/1 and 1/2 with denominators up to 2, nearer 1/2.
        assert_eq!(nearest_fraction(1, 3, 2), (1, 2));
        // 1/4 is as near 0/1 as 1/2: the smaller denominator wins.
        assert_eq!(nearest_fraction(1, 4, 2), (0, 1));
        // 3/4 between 1/1 and 1/2 with denominators up to 2: equally near; 1/1 wins.
        assert_eq!(nearest_fraction(3, 4, 2), (1, 1));
    }

    #[test]
    fn the_duration_rule() {
        let duration = |scale: Option<u64>, value: Option<f64>, float32: bool| {
            let mut clip = Clip::new("V_VP9");
            let mut info = Vec::new();
            if let Some(scale) = scale {
                info.extend(uint_element(TIMECODE_SCALE, scale));
            }
            if let Some(value) = value {
                if float32 {
                    info.extend(element(DURATION, &(value as f32).to_be_bytes()));
                } else {
                    info.extend(float_element(DURATION, value));
                }
            }
            clip.info = info;
            facts(&clip.bytes()).duration
        };
        assert_eq!(duration(Some(1_000_000), Some(1000.0), false), Some(1.0));
        assert_eq!(duration(None, Some(1500.0), true), Some(1.5));
        // Truncated to whole microseconds.
        assert_eq!(duration(Some(1), Some(1_999_999.9), false), Some(0.001999));
        assert_eq!(
            duration(Some(1_000_000), Some(0.0016667), false),
            Some(0.000001)
        );
        // No Duration, a Duration of 0 or below, a scale of 0: no duration.
        assert_eq!(duration(Some(1_000_000), None, false), None);
        assert_eq!(duration(Some(1_000_000), Some(0.0), false), None);
        assert_eq!(duration(Some(1_000_000), Some(-5.0), false), None);
        assert_eq!(duration(Some(0), Some(1000.0), false), None);
        assert_eq!(duration(Some(1_000_000), Some(f64::NAN), false), None);
    }

    #[test]
    fn lacing_and_block_groups_count_frames() {
        let mut clip = Clip::new("V_VP9");
        let blocks = [
            // Xiph lacing, 3 frames: sizes 256 (255 + 1) and 2, then the last.
            element(SIMPLE_BLOCK, &[0x81, 0, 0, 0x82, 2, 255, 1, 2, 0]),
            // EBML lacing, 4 frames.
            element(SIMPLE_BLOCK, &[0x81, 0, 0, 0x86, 3, 0x81, 0xBF, 0xBF, 1, 2]),
            // Fixed lacing, 2 frames.
            element(SIMPLE_BLOCK, &[0x81, 0, 0, 0x84, 1, 7, 7]),
            // A lace count of 1.
            element(SIMPLE_BLOCK, &[0x81, 0, 0, 0x82, 0, 7]),
            // A BlockGroup with a Block and an unrelated child.
            element(
                BLOCK_GROUP,
                &[element(BLOCK, &[0x81, 0, 0, 0, 5]), element(0x9B, &[1])].concat(),
            ),
            // Another track's laced block.
            element(SIMPLE_BLOCK, &[0x82, 0, 0, 0x84, 9, 7, 7]),
        ]
        .concat();
        clip.clusters = [
            element(CLUSTER, &blocks),
            element(CLUSTER, &simple_blocks(1, 2)),
        ]
        .concat();
        assert_eq!(facts(&clip.bytes()).frames, Some(3 + 4 + 2 + 1 + 1 + 2));
        // A laced block cut before its lace count is refused.
        clip.clusters = element(CLUSTER, &element(SIMPLE_BLOCK, &[0x81, 0, 0, 0x82]));
        let error = matroska_facts(&clip.bytes()).unwrap_err();
        assert!(error.ends_with("is too short)"), "{error}");
        clip.clusters = element(CLUSTER, &element(SIMPLE_BLOCK, &[0x81, 0]));
        assert!(matroska_facts(&clip.bytes()).is_err());
    }

    #[test]
    fn first_frames_of_laced_blocks() {
        let block = |content: &[u8]| {
            let bytes = element(SIMPLE_BLOCK, content);
            let header = header(&bytes, 0, bytes.len()).unwrap();
            let element = Element {
                id: SIMPLE_BLOCK,
                content: header.content,
                end: bytes.len(),
            };
            let (_, count, frame) = block_frames(&bytes, element).unwrap();
            (count, frame.to_vec())
        };
        assert_eq!(block(&[0x81, 0, 0, 0x80, 1, 2, 3]), (1, vec![1, 2, 3]));
        assert_eq!(block(&[0x81, 0, 0, 0x82, 1, 2, 7, 8, 9]), (2, vec![7, 8]));
        assert_eq!(
            block(&[0x81, 0, 0, 0x84, 2, 1, 2, 3, 4, 5, 6]),
            (3, vec![1, 2])
        );
        assert_eq!(
            block(&[0x81, 0, 0, 0x86, 1, 0x82, 4, 5, 6]),
            (2, vec![4, 5])
        );
    }

    #[test]
    fn unknown_sizes() {
        // A Segment of unknown size runs to the end of the file; Clusters of unknown size end
        // where the next level-1 element starts.
        let mut clip = Clip::new("V_VP9");
        clip.segment_unknown = true;
        clip.clusters = [
            unknown(CLUSTER, &simple_blocks(1, 10)),
            unknown(CLUSTER, &simple_blocks(1, 14)),
            element(0x1C53_BB6B, &[0; 4]),
        ]
        .concat();
        assert_eq!(facts(&clip.bytes()).frames, Some(24));
        // Any other element of unknown size is refused.
        clip.clusters = unknown(0x1254_C367, &[0; 4]);
        let error = matroska_facts(&clip.bytes()).unwrap_err();
        assert!(error.ends_with("has an unknown size)"), "{error}");
    }

    #[test]
    fn has_alpha_by_codec() {
        let alpha = |codec: &'static str, private: Option<Vec<u8>>| {
            let mut clip = Clip::new(codec);
            clip.private = private;
            facts(&clip.bytes()).has_alpha
        };
        for codec in NO_ALPHA_CODECS {
            assert_eq!(alpha(codec, None), Some(false), "{codec}");
        }
        // FFV1 without a record reads the first frame, which is no key frame here.
        assert_eq!(alpha("V_FFV1", None), None);
        let bitmap = |fourcc: &[u8; 4], extra: &[u8]| {
            let mut header = vec![0; 40];
            header[..4].copy_from_slice(&40u32.to_le_bytes());
            header[16..20].copy_from_slice(fourcc);
            header.extend(extra);
            Some(header)
        };
        // PNG through VFW: the first frame decides; these frames are no PNG.
        assert_eq!(alpha("V_MS/VFW/FOURCC", bitmap(b"MPNG", &[])), None);
        assert_eq!(alpha("V_MS/VFW/FOURCC", bitmap(b"H264", &[])), None);
        assert_eq!(alpha("V_MS/VFW/FOURCC", Some(vec![0; 12])), None);
    }

    #[test]
    fn a_png_first_frame_decides() {
        let png = |color: u8, trns: bool| {
            let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
            let mut chunk = |kind: &[u8; 4], data: &[u8]| {
                out.extend((data.len() as u32).to_be_bytes());
                out.extend(kind);
                out.extend(data);
                let crc = crc32(&[kind.as_slice(), data].concat());
                out.extend(crc.to_be_bytes());
            };
            let mut ihdr = Vec::new();
            ihdr.extend(1u32.to_be_bytes());
            ihdr.extend(1u32.to_be_bytes());
            ihdr.extend([8, color, 0, 0, 0]);
            chunk(b"IHDR", &ihdr);
            if trns {
                chunk(b"tRNS", &[0, 0, 0, 0, 0, 0]);
            }
            chunk(b"IDAT", &[0x78, 0x01]);
            out
        };
        let clip = |frame: Vec<u8>| {
            let mut clip = Clip::new("V_MS/VFW/FOURCC");
            let mut header = vec![0; 40];
            header[..4].copy_from_slice(&40u32.to_le_bytes());
            header[16..20].copy_from_slice(b"MPNG");
            clip.private = Some(header);
            let mut block = vec![0x81, 0, 0, 0x80];
            block.extend(frame);
            clip.clusters = element(CLUSTER, &element(SIMPLE_BLOCK, &block));
            facts(&clip.bytes()).has_alpha
        };
        assert_eq!(clip(png(6, false)), Some(true));
        assert_eq!(clip(png(2, false)), Some(false));
        assert_eq!(clip(png(2, true)), Some(true));
    }

    /// The CRC-32 of PNG chunks (reflected, polynomial 0xEDB88320).
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn no_video_track() {
        let mut clip = Clip::new("V_VP9");
        clip.video_type = 2;
        assert_eq!(matroska_facts(&clip.bytes()), Ok(None));
        // No Tracks at all.
        let mut no_tracks = ebml_header("matroska");
        no_tracks.extend(element(SEGMENT, &element(INFO, &[])));
        assert_eq!(matroska_facts(&no_tracks), Ok(None));
    }

    #[test]
    fn broken_streams_are_refused() {
        let refused = |bytes: &[u8]| matroska_facts(bytes).unwrap_err();
        assert_eq!(
            refused(b"not a video"),
            "not a Matroska or WebM file (it does not start with an EBML header)"
        );
        assert_eq!(
            refused(b""),
            "not a Matroska or WebM file (it does not start with an EBML header)"
        );
        let mut other = Clip::new("V_VP9");
        other.doc_type = "notwebm";
        assert_eq!(
            refused(&other.bytes()),
            "not a Matroska or WebM file (its DocType is not matroska or webm)"
        );
        let good = Clip::new("V_VP9").bytes();
        let mut cut = good.clone();
        cut.truncate(good.len() - 3);
        let header_len = ebml_header("webm").len();
        assert_eq!(
            refused(&cut),
            format!(
                "not a Matroska or WebM file (an element at byte {header_len} runs past the end of the file)"
            )
        );
        assert_eq!(
            refused(&ebml_header("webm")),
            "not a Matroska or WebM file (it has no Segment)"
        );
        // An ID whose first byte has no marker in its first four bits, and a size of 0 bytes.
        let mut bad_id = ebml_header("webm");
        bad_id.extend([0x08, 0x80]);
        assert_eq!(
            refused(&bad_id),
            format!(
                "not a Matroska or WebM file (an element ID at byte {header_len} is not valid)"
            )
        );
        let mut bad_size = ebml_header("webm");
        bad_size.extend([0xEC, 0x00]);
        assert_eq!(
            refused(&bad_size),
            format!(
                "not a Matroska or WebM file (an element size at byte {} is not valid)",
                header_len + 1
            )
        );
        // A video track without a pixel size.
        let mut no_size = ebml_header("webm");
        let entry = [uint_element(TRACK_NUMBER, 1), uint_element(TRACK_TYPE, 1)].concat();
        no_size.extend(element(
            SEGMENT,
            &element(TRACKS, &element(TRACK_ENTRY, &entry)),
        ));
        assert_eq!(
            refused(&no_size),
            "not a Matroska or WebM file (its video track has no pixel size)"
        );
        // An integer longer than 8 bytes, a float of 3 bytes.
        let mut clip = Clip::new("V_VP9");
        clip.info = element(TIMECODE_SCALE, &[0; 9]);
        assert!(refused(&clip.bytes()).contains("is longer than 8 bytes"));
        clip.info = element(DURATION, &[0; 3]);
        assert!(refused(&clip.bytes()).contains("is 3 bytes long"));
    }

    #[test]
    fn vints() {
        let read = |bytes: &[u8]| vint(bytes, 0, bytes.len()).ok();
        assert_eq!(read(&[0x81]), Some((1, 1, false)));
        assert_eq!(read(&[0x40, 0x02]), Some((2, 2, false)));
        assert_eq!(read(&[0xFF]), Some((127, 1, true)));
        assert_eq!(
            read(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).map(|v| v.2),
            Some(true)
        );
        assert!(read(&[0x00]).is_none());
        assert!(read(&[0x40]).is_none());
        assert!(read(&[]).is_none());
    }
}
