//! MP4 (ISO base media) video facts (spec/facts.md §4): the first track whose handler is
//! `vide`, in file order.
//!
//! - `width`, `height`: the visual sample entry's in `stsd` (never `tkhd`'s, never rotated);
//! - `fps`: `timescale / delta` from the track's sample-duration runs (`stts`, then fragments):
//!   the first run's delta when there is one run, or two whose second holds one sample; else
//!   the greatest common divisor of every run's delta; rounded to 6 places;
//! - `duration`: `mdhd` duration, or for a fragmented track the sum of its sample durations,
//!   trimmed by an edit list's non-empty edits (rescaled to the media timescale), over the
//!   timescale; rounded to 6 places;
//! - `frames`: the `stsz` sample count plus the fragments' `trun` sample counts; edit lists do
//!   not change it;
//! - `has_alpha`: by sample entry: false for `avc1`–`avc4`, `hvc1`, `hev1`, `av01`, `vp08`,
//!   `vp09`, and for `mp4v` carrying MPEG-1, -2 or -4 Visual or JPEG; a PNG track's first sample
//!   decides as an image's would; left out for anything else.
//!
//! A file with no video track has no video facts. A file whose boxes do not parse, that has no
//! `moov`, or whose video track lacks what these facts need is refused with
//! `not an MP4 file (<reason>)`. Every size is checked against the bytes present before it is
//! used, and nothing is allocated from a declared size.

use super::{VideoFacts, png_has_alpha, round6, seconds};

/// Sample entries whose codecs never decode with alpha.
const NO_ALPHA_ENTRIES: [&[u8; 4]; 9] = [
    b"avc1", b"avc2", b"avc3", b"avc4", b"hvc1", b"hev1", b"av01", b"vp08", b"vp09",
];
/// `objectTypeIndication`s of an `mp4v` entry that never decode with alpha: MPEG-4 Visual,
/// MPEG-2 Visual (six profiles), MPEG-1 Visual and JPEG.
const NO_ALPHA_OBJECTS: [u8; 9] = [0x20, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x6A, 0x6C];
/// The `objectTypeIndication` of PNG.
const PNG_OBJECT: u8 = 0x6D;
/// The fields of a visual sample entry before its child boxes.
const VISUAL_ENTRY: usize = 78;

/// The facts of `bytes` read as MP4 (module doc).
pub fn mp4_facts(bytes: &[u8]) -> Result<Option<VideoFacts>, String> {
    read(bytes).map_err(|reason| format!("not an MP4 file ({reason})"))
}

/// A box: its type, where it starts, where its content starts, and where it ends.
#[derive(Debug, Clone, Copy)]
struct Mp4Box {
    kind: [u8; 4],
    content: usize,
    end: usize,
}

impl Mp4Box {
    fn name(&self) -> String {
        String::from_utf8_lossy(&self.kind).into_owned()
    }
}

/// The boxes laid end to end in `file[at..end]`. The first box that does not fit is an error,
/// and ends the walk.
struct Boxes<'a> {
    file: &'a [u8],
    at: usize,
    end: usize,
    top: bool,
    failed: bool,
}

impl<'a> Boxes<'a> {
    fn top(file: &'a [u8]) -> Self {
        Boxes {
            file,
            at: 0,
            end: file.len(),
            top: true,
            failed: false,
        }
    }

    fn range(file: &'a [u8], at: usize, end: usize) -> Self {
        Boxes {
            file,
            at,
            end,
            top: false,
            failed: false,
        }
    }

    fn within(file: &'a [u8], parent: Mp4Box) -> Self {
        Self::range(file, parent.content, parent.end)
    }

    fn read(&mut self) -> Result<Mp4Box, String> {
        let at = self.at;
        let left = (self.end - at) as u64;
        let cut = || format!("a box header at byte {at} is cut short");
        if left < 8 {
            return Err(cut());
        }
        let size32 = be32(self.file, at).ok_or_else(cut)?;
        let mut kind = [0; 4];
        kind.copy_from_slice(&self.file[at + 4..at + 8]);
        let (size, header) = match size32 {
            1 => {
                if left < 16 {
                    return Err(cut());
                }
                (be64(self.file, at + 8).ok_or_else(cut)?, 16)
            }
            0 => (left, 8),
            n => (u64::from(n), 8),
        };
        if size < header {
            return Err(format!("a box at byte {at} is smaller than its header"));
        }
        if size > left {
            let parent = if self.top {
                "the file"
            } else {
                "its parent box"
            };
            return Err(format!("a box at byte {at} runs past the end of {parent}"));
        }
        // `size <= left`, so it fits in a usize.
        let end = at + size as usize;
        self.at = end;
        Ok(Mp4Box {
            kind,
            content: at + header as usize,
            end,
        })
    }
}

impl Iterator for Boxes<'_> {
    type Item = Result<Mp4Box, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.at >= self.end {
            return None;
        }
        let result = self.read();
        self.failed = result.is_err();
        Some(result)
    }
}

/// Every child of `parent` (all of them parsed, so a broken one refuses the file).
fn children(file: &[u8], parent: Mp4Box) -> Result<Vec<Mp4Box>, String> {
    Boxes::within(file, parent).collect()
}

fn first(boxes: &[Mp4Box], kind: &[u8; 4]) -> Option<Mp4Box> {
    boxes.iter().find(|b| &b.kind == kind).copied()
}

/// `len` bytes at `offset` in a box's content, or `its <type> box is too short`.
fn field(file: &[u8], b: Mp4Box, offset: u64, len: u64) -> Result<&[u8], String> {
    let short = || format!("its {} box is too short", b.name());
    let content = (b.end - b.content) as u64;
    if offset.checked_add(len).is_none_or(|end| end > content) {
        return Err(short());
    }
    // In bounds of the box, so in bounds of the file.
    let start = b.content + offset as usize;
    Ok(&file[start..start + len as usize])
}

fn u32_field(file: &[u8], b: Mp4Box, offset: u64) -> Result<u32, String> {
    let bytes = field(file, b, offset, 4)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn u64_field(file: &[u8], b: Mp4Box, offset: u64) -> Result<u64, String> {
    let bytes = field(file, b, offset, 8)?;
    let mut array = [0; 8];
    array.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(array))
}

/// `(timescale, duration, the duration that means unknown)` of an `mvhd` or `mdhd` box.
fn scale_and_duration(file: &[u8], b: Mp4Box) -> Result<(u32, u64, u64), String> {
    let version = field(file, b, 0, 1)?[0];
    if version == 1 {
        Ok((u32_field(file, b, 20)?, u64_field(file, b, 24)?, u64::MAX))
    } else {
        let duration = u32_field(file, b, 16)?;
        Ok((
            u32_field(file, b, 12)?,
            u64::from(duration),
            u64::from(u32::MAX),
        ))
    }
}

/// A track's sample-duration runs (spec/facts.md §4): consecutive equal deltas merge.
#[derive(Debug, Default)]
struct Rate {
    runs: u64,
    first: u64,
    last_delta: u64,
    last_count: u64,
    divisor: u64,
    /// The sum of every sample's duration.
    total: u128,
}

impl Rate {
    fn add(&mut self, count: u64, delta: u64) {
        if count == 0 {
            return;
        }
        self.total += u128::from(count) * u128::from(delta);
        if self.runs > 0 && delta == self.last_delta {
            self.last_count = self.last_count.saturating_add(count);
            return;
        }
        self.runs += 1;
        if self.runs == 1 {
            self.first = delta;
        }
        self.last_delta = delta;
        self.last_count = count;
        self.divisor = gcd(self.divisor, delta);
    }

    /// The delta the frame rate is read from; 0 when there is none.
    fn delta(&self) -> u64 {
        match self.runs {
            0 => 0,
            1 => self.first,
            2 if self.last_count == 1 => self.first,
            _ => self.divisor,
        }
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The first video track's own boxes.
struct VideoTrak {
    children: Vec<Mp4Box>,
    mdia: Vec<Mp4Box>,
}

fn read(file: &[u8]) -> Result<Option<VideoFacts>, String> {
    let mut moov = None;
    let mut moofs = Vec::new();
    for b in Boxes::top(file) {
        let b = b?;
        match &b.kind {
            b"moov" if moov.is_none() => moov = Some(b),
            b"moof" => moofs.push(b),
            _ => {}
        }
    }
    let moov = moov.ok_or("it has no moov box")?;
    let mut movie_scale = None;
    let mut mvex = None;
    let mut video = None;
    for b in Boxes::within(file, moov) {
        let b = b?;
        match &b.kind {
            b"mvhd" if movie_scale.is_none() => {
                movie_scale = Some(scale_and_duration(file, b)?.0);
            }
            b"mvex" if mvex.is_none() => mvex = Some(b),
            b"trak" if video.is_none() => video = video_trak(file, b)?,
            _ => {}
        }
    }
    let Some(video) = video else {
        return Ok(None);
    };
    let mdhd = first(&video.mdia, b"mdhd").ok_or("its video track has no mdhd box")?;
    let (scale, media_duration, unknown) = scale_and_duration(file, mdhd)?;
    let mut stbl = Vec::new();
    if let Some(minf) = first(&video.mdia, b"minf") {
        let minf = children(file, minf)?;
        if let Some(found) = first(&minf, b"stbl") {
            stbl = children(file, found)?;
        }
    }
    let stsd = first(&stbl, b"stsd").ok_or("its video track has no stsd box")?;
    let entries = u32_field(file, stsd, 4)?;
    let entry = if entries > 0 {
        Boxes::range(file, stsd.content + 8, stsd.end)
            .next()
            .transpose()?
    } else {
        None
    };
    let entry = entry.ok_or("its video track has no sample entry")?;
    if entry.end - entry.content < VISUAL_ENTRY {
        return Err("its video sample entry is too short".into());
    }
    let width = u32::from(be16(file, entry.content + 24).unwrap_or(0));
    let height = u32::from(be16(file, entry.content + 26).unwrap_or(0));

    let mut rate = Rate::default();
    if let Some(stts) = first(&stbl, b"stts") {
        let count = u32_field(file, stts, 4)?;
        for i in 0..u64::from(count) {
            let count = u32_field(file, stts, 8 + 8 * i)?;
            let delta = u32_field(file, stts, 12 + 8 * i)?;
            rate.add(u64::from(count), u64::from(delta));
        }
    }
    let mut frames: u64 = 0;
    let mut first_size = None;
    if let Some(stsz) = first(&stbl, b"stsz") {
        let size = u32_field(file, stsz, 4)?;
        frames = u64::from(u32_field(file, stsz, 8)?);
        if frames > 0 {
            first_size = Some(if size != 0 {
                size
            } else {
                u32_field(file, stsz, 12)?
            });
        }
    } else if let Some(stz2) = first(&stbl, b"stz2") {
        frames = u64::from(u32_field(file, stz2, 8)?);
    }

    let mut track_id = None;
    if let Some(tkhd) = first(&video.children, b"tkhd") {
        let version = field(file, tkhd, 0, 1)?[0];
        let offset = if version == 1 { 20 } else { 12 };
        track_id = Some(u32_field(file, tkhd, offset)?);
    }
    let mut fragmented = false;
    if let Some(track_id) = track_id {
        let mut trex_duration = 0;
        if let Some(mvex) = mvex {
            for b in Boxes::within(file, mvex) {
                let b = b?;
                if &b.kind == b"trex" && u32_field(file, b, 4)? == track_id {
                    // track_ID, default_sample_description_index, default_sample_duration
                    trex_duration = u32_field(file, b, 12)?;
                    break;
                }
            }
        }
        for moof in &moofs {
            for b in Boxes::within(file, *moof) {
                let b = b?;
                if &b.kind == b"traf" {
                    let (matched, count) = traf(file, b, track_id, trex_duration, &mut rate)?;
                    fragmented |= matched;
                    frames = frames.saturating_add(count);
                }
            }
        }
    }

    let mut units = if !fragmented && media_duration > 0 && media_duration < unknown {
        u128::from(media_duration)
    } else {
        rate.total
    };
    if let Some(edits) = edits(file, &video.children, scale, movie_scale)? {
        units = units.min(edits);
    }
    let delta = rate.delta();
    let fps = (scale != 0 && delta != 0).then(|| round6(f64::from(scale) / delta as f64));
    let duration = (scale != 0).then(|| round6(seconds(units, u64::from(scale))));

    let mut has_alpha = None;
    let mut png = &entry.kind == b"png ";
    if NO_ALPHA_ENTRIES.contains(&&entry.kind) {
        has_alpha = Some(false);
    } else if &entry.kind == b"mp4v" {
        let object = object_type(file, entry.content + VISUAL_ENTRY, entry.end);
        if object.is_some_and(|object| NO_ALPHA_OBJECTS.contains(&object)) {
            has_alpha = Some(false);
        }
        png = object == Some(PNG_OBJECT);
    }
    if png && let Some(size) = first_size {
        let mut offset = None;
        if let Some(stco) = first(&stbl, b"stco") {
            if u32_field(file, stco, 4)? > 0 {
                offset = Some(u64::from(u32_field(file, stco, 8)?));
            }
        } else if let Some(co64) = first(&stbl, b"co64")
            && u32_field(file, co64, 4)? > 0
        {
            offset = Some(u64_field(file, co64, 8)?);
        }
        if let Some(offset) = offset
            && let Some(end) = offset.checked_add(u64::from(size))
            && end <= file.len() as u64
        {
            // Both ends are within the file, so they fit in a usize.
            let sample = &file[offset as usize..end as usize];
            if let Some(alpha) = png_has_alpha(sample) {
                has_alpha = Some(alpha);
            }
        }
    }

    Ok(Some(VideoFacts {
        width,
        height,
        fps,
        duration,
        frames: Some(frames),
        has_alpha,
    }))
}

/// The track's boxes when its `mdia`'s `hdlr` names the `vide` handler; `None` for any other
/// track.
fn video_trak(file: &[u8], trak: Mp4Box) -> Result<Option<VideoTrak>, String> {
    let trak_children = children(file, trak)?;
    let Some(mdia) = first(&trak_children, b"mdia") else {
        return Ok(None);
    };
    let mdia = children(file, mdia)?;
    let Some(hdlr) = first(&mdia, b"hdlr") else {
        return Ok(None);
    };
    // version and flags (4), pre_defined (4), handler_type (4)
    if field(file, hdlr, 8, 4)? != b"vide" {
        return Ok(None);
    }
    Ok(Some(VideoTrak {
        children: trak_children,
        mdia,
    }))
}

/// The non-empty edits' durations (a media time other than -1 and a duration above 0), each
/// rescaled from the movie to the media timescale and rounded to nearest, summed; `None` when
/// the track has no such edit or the movie has no timescale.
fn edits(
    file: &[u8],
    trak: &[Mp4Box],
    scale: u32,
    movie_scale: Option<u32>,
) -> Result<Option<u128>, String> {
    let (Some(edts), Some(movie_scale)) = (first(trak, b"edts"), movie_scale) else {
        return Ok(None);
    };
    if movie_scale == 0 {
        return Ok(None);
    }
    let Some(elst) = first(&children(file, edts)?, b"elst") else {
        return Ok(None);
    };
    let version = field(file, elst, 0, 1)?[0];
    let count = u32_field(file, elst, 4)?;
    let (movie, media) = (u128::from(movie_scale), u128::from(scale));
    let mut total: u128 = 0;
    let mut found = false;
    for i in 0..u64::from(count) {
        let (duration, empty) = if version == 1 {
            let at = 8 + 20 * i;
            (
                u64_field(file, elst, at)?,
                u64_field(file, elst, at + 8)? == u64::MAX,
            )
        } else {
            let at = 8 + 12 * i;
            let duration = u64::from(u32_field(file, elst, at)?);
            (duration, u32_field(file, elst, at + 4)? == u32::MAX)
        };
        if !empty && duration > 0 {
            found = true;
            total = total.saturating_add((u128::from(duration) * media + movie / 2) / movie);
        }
    }
    Ok(found.then_some(total))
}

/// One `traf`: whether it belongs to the track, and its samples (durations into `rate`).
fn traf(
    file: &[u8],
    traf: Mp4Box,
    track_id: u32,
    trex_duration: u32,
    rate: &mut Rate,
) -> Result<(bool, u64), String> {
    let boxes = children(file, traf)?;
    let Some(tfhd) = first(&boxes, b"tfhd") else {
        return Ok((false, 0));
    };
    let flags = u32_field(file, tfhd, 0)? & 0xFF_FFFF;
    if u32_field(file, tfhd, 4)? != track_id {
        return Ok((false, 0));
    }
    let mut default = trex_duration;
    if flags & 0x08 != 0 {
        // base_data_offset (8) and sample_description_index (4) come first when present.
        let offset =
            8 + if flags & 0x01 != 0 { 8 } else { 0 } + if flags & 0x02 != 0 { 4 } else { 0 };
        default = u32_field(file, tfhd, offset)?;
    }
    let mut frames: u64 = 0;
    for trun in boxes.iter().filter(|b| &b.kind == b"trun") {
        let flags = u32_field(file, *trun, 0)? & 0xFF_FFFF;
        let count = u64::from(u32_field(file, *trun, 4)?);
        let offset =
            8 + if flags & 0x001 != 0 { 4 } else { 0 } + if flags & 0x004 != 0 { 4 } else { 0 };
        let per_sample = 4 * u64::from((flags & 0xF00).count_ones());
        let content = (trun.end - trun.content) as u64;
        if offset + count * per_sample > content {
            return Err("its trun box is too short".into());
        }
        frames = frames.saturating_add(count);
        if flags & 0x100 != 0 {
            for i in 0..count {
                let duration = u32_field(file, *trun, offset + i * per_sample)?;
                rate.add(1, u64::from(duration));
            }
        } else {
            rate.add(count, u64::from(default));
        }
    }
    Ok((true, frames))
}

/// The `objectTypeIndication` of the first `esds` box among a sample entry's child boxes
/// (`file[start..end]`); `None` when there is none, or the boxes or the descriptors do not
/// parse.
fn object_type(file: &[u8], start: usize, end: usize) -> Option<u8> {
    let esds = Boxes::range(file, start, end)
        .map_while(Result::ok)
        .find(|b| &b.kind == b"esds")?;
    let body = &file[esds.content..esds.end];
    // A descriptor: its tag, then its size in up to four bytes of 7 bits.
    let descriptor = |mut at: usize| -> Option<(u8, usize)> {
        let tag = *body.get(at)?;
        at += 1;
        for _ in 0..4 {
            let byte = *body.get(at)?;
            at += 1;
            if byte & 0x80 == 0 {
                break;
            }
        }
        Some((tag, at))
    };
    // version and flags (4), then the ES_Descriptor (tag 3).
    let (tag, at) = descriptor(4)?;
    if tag != 0x03 {
        return None;
    }
    // ES_ID (2) and the flags byte, then what the flags add.
    let mut at = at + 3;
    let flags = *body.get(at - 1)?;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        at += 1 + usize::from(*body.get(at)?);
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    // The DecoderConfigDescriptor (tag 4) starts with the objectTypeIndication.
    let (tag, at) = descriptor(at)?;
    if tag != 0x04 {
        return None;
    }
    body.get(at).copied()
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of `kind` around `content`.
    fn mp4box(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut out = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(content);
        out
    }

    fn full(kind: &[u8; 4], version: u8, flags: u32, rest: &[u8]) -> Vec<u8> {
        let mut content = vec![version];
        content.extend(&flags.to_be_bytes()[1..]);
        content.extend(rest);
        mp4box(kind, &content)
    }

    fn mvhd(scale: u32, duration: u32) -> Vec<u8> {
        let mut rest = vec![0; 8];
        rest.extend(scale.to_be_bytes());
        rest.extend(duration.to_be_bytes());
        rest.extend([0; 80]);
        full(b"mvhd", 0, 0, &rest)
    }

    fn mdhd(scale: u32, duration: u32) -> Vec<u8> {
        let mut rest = vec![0; 8];
        rest.extend(scale.to_be_bytes());
        rest.extend(duration.to_be_bytes());
        rest.extend([0; 4]);
        full(b"mdhd", 0, 0, &rest)
    }

    fn mdhd_v1(scale: u32, duration: u64) -> Vec<u8> {
        let mut rest = vec![0; 16];
        rest.extend(scale.to_be_bytes());
        rest.extend(duration.to_be_bytes());
        rest.extend([0; 4]);
        full(b"mdhd", 1, 0, &rest)
    }

    fn tkhd(id: u32) -> Vec<u8> {
        let mut rest = vec![0; 8];
        rest.extend(id.to_be_bytes());
        rest.extend([0; 68]);
        full(b"tkhd", 0, 3, &rest)
    }

    fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
        let mut rest = vec![0; 4];
        rest.extend(handler);
        rest.extend([0; 13]);
        full(b"hdlr", 0, 0, &rest)
    }

    fn entry(kind: &[u8; 4], width: u16, height: u16, extra: &[u8]) -> Vec<u8> {
        let mut content = vec![0; 24];
        content.extend(width.to_be_bytes());
        content.extend(height.to_be_bytes());
        content.extend([0; 50]);
        // depth 24, pre_defined -1
        content[74..78].copy_from_slice(&[0, 24, 0xFF, 0xFF]);
        assert_eq!(content.len(), 78);
        content.extend(extra);
        mp4box(kind, &content)
    }

    fn stsd(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut rest = (entries.len() as u32).to_be_bytes().to_vec();
        for e in entries {
            rest.extend(e);
        }
        full(b"stsd", 0, 0, &rest)
    }

    fn stts(runs: &[(u32, u32)]) -> Vec<u8> {
        let mut rest = (runs.len() as u32).to_be_bytes().to_vec();
        for (count, delta) in runs {
            rest.extend(count.to_be_bytes());
            rest.extend(delta.to_be_bytes());
        }
        full(b"stts", 0, 0, &rest)
    }

    fn stsz(count: u32) -> Vec<u8> {
        let mut rest = 100u32.to_be_bytes().to_vec();
        rest.extend(count.to_be_bytes());
        full(b"stsz", 0, 0, &rest)
    }

    fn elst(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut rest = (entries.len() as u32).to_be_bytes().to_vec();
        for (duration, time) in entries {
            rest.extend(duration.to_be_bytes());
            rest.extend(time.to_be_bytes());
            rest.extend([0, 1, 0, 0]);
        }
        mp4box(b"edts", &full(b"elst", 0, 0, &rest))
    }

    struct Track {
        id: u32,
        handler: [u8; 4],
        mdhd: Vec<u8>,
        entry: Vec<u8>,
        stts: Vec<(u32, u32)>,
        stsz: u32,
        edits: Option<Vec<(u32, i32)>>,
    }

    impl Track {
        fn video(scale: u32, duration: u32, runs: &[(u32, u32)]) -> Self {
            Track {
                id: 1,
                handler: *b"vide",
                mdhd: mdhd(scale, duration),
                entry: entry(b"avc1", 64, 48, &[]),
                stts: runs.to_vec(),
                stsz: runs.iter().map(|r| r.0).sum(),
                edits: None,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let stbl = [
                stsd(std::slice::from_ref(&self.entry)),
                stts(&self.stts),
                stsz(self.stsz),
            ]
            .concat();
            let minf = mp4box(b"minf", &mp4box(b"stbl", &stbl));
            let mdia = mp4box(
                b"mdia",
                &[self.mdhd.clone(), hdlr(&self.handler), minf].concat(),
            );
            let mut trak = tkhd(self.id);
            if let Some(edits) = &self.edits {
                trak.extend(elst(edits));
            }
            trak.extend(mdia);
            mp4box(b"trak", &trak)
        }
    }

    fn movie(tracks: &[&Track], extra_moov: &[u8], after: &[u8]) -> Vec<u8> {
        let mut moov = mvhd(1000, 0);
        for track in tracks {
            moov.extend(track.bytes());
        }
        moov.extend(extra_moov);
        let mut file = mp4box(b"ftyp", b"isom\0\0\x02\0isom");
        file.extend(mp4box(b"moov", &moov));
        file.extend(after);
        file
    }

    fn facts(file: &[u8]) -> VideoFacts {
        mp4_facts(file).unwrap().unwrap()
    }

    #[test]
    fn a_plain_track() {
        let track = Track::video(12288, 24576, &[(48, 512)]);
        assert_eq!(
            facts(&movie(&[&track], &[], &[])),
            VideoFacts {
                width: 64,
                height: 48,
                fps: Some(24.0),
                duration: Some(2.0),
                frames: Some(48),
                has_alpha: Some(false),
            }
        );
    }

    #[test]
    fn the_first_video_track_counts() {
        let mut audio = Track::video(48000, 48000, &[(47, 1024)]);
        audio.handler = *b"soun";
        let video = Track::video(30000, 30030, &[(30, 1001)]);
        let mut second = Track::video(600, 600, &[(24, 25)]);
        second.entry = entry(b"avc1", 8, 8, &[]);
        let facts = facts(&movie(&[&audio, &video, &second], &[], &[]));
        assert_eq!((facts.width, facts.height), (64, 48));
        assert_eq!(facts.fps, Some(29.97003));
        assert_eq!(facts.duration, Some(1.001));
        // No video track: no video facts.
        assert_eq!(mp4_facts(&movie(&[&audio], &[], &[])), Ok(None));
    }

    #[test]
    fn the_rate_rule() {
        let fps =
            |runs: &[(u32, u32)]| facts(&movie(&[&Track::video(12288, 1, runs)], &[], &[])).fps;
        assert_eq!(fps(&[(24, 512)]), Some(24.0));
        // A last sample of another length does not count.
        assert_eq!(fps(&[(23, 512), (1, 256)]), Some(24.0));
        // Equal runs merge.
        assert_eq!(fps(&[(10, 512), (14, 512)]), Some(24.0));
        assert_eq!(fps(&[(10, 512), (0, 7), (14, 512), (1, 999)]), Some(24.0));
        // Otherwise the greatest common divisor of the deltas.
        assert_eq!(
            fps(&[(14, 512), (7, 1024), (1, 512), (1, 1536), (1, 512)]),
            Some(24.0)
        );
        assert_eq!(fps(&[(9, 1024), (1, 1536), (2, 1024)]), Some(24.0));
        assert_eq!(fps(&[(9, 1001), (1, 2002), (20, 1001)]), Some(12.275724));
        // No samples, or a delta of 0: no rate.
        assert_eq!(fps(&[]), None);
        assert_eq!(fps(&[(5, 0)]), None);
        let none_scale = facts(&movie(&[&Track::video(0, 100, &[(5, 1)])], &[], &[]));
        assert_eq!((none_scale.fps, none_scale.duration), (None, None));
    }

    #[test]
    fn edit_lists_trim_the_duration_not_the_frames() {
        // mvhd's timescale is 1000; the media's 12288.
        let edited = |edits: &[(u32, i32)]| {
            let mut track = Track::video(12288, 24576, &[(48, 512)]);
            track.edits = Some(edits.to_vec());
            facts(&movie(&[&track], &[], &[]))
        };
        let cut = edited(&[(1500, 6144)]);
        assert_eq!((cut.duration, cut.frames), (Some(1.5), Some(48)));
        // An empty edit (media time -1) and a zero-length edit are left out.
        assert_eq!(edited(&[(500, -1), (1000, 1024)]).duration, Some(1.0));
        assert_eq!(edited(&[(0, 1024)]).duration, Some(2.0));
        // Longer edits never lengthen it; each edit rounds to nearest.
        assert_eq!(edited(&[(3000, 0)]).duration, Some(2.0));
        assert_eq!(edited(&[(42, 0)]).duration, Some(0.041992));
        assert_eq!(edited(&[(500, 0), (500, 0)]).duration, Some(1.0));
    }

    #[test]
    fn a_v1_mdhd_and_an_unknown_duration() {
        let mut track = Track::video(12288, 0, &[(24, 512)]);
        track.mdhd = mdhd_v1(12288, 36864);
        assert_eq!(facts(&movie(&[&track], &[], &[])).duration, Some(3.0));
        // All ones means unknown: the samples' sum stands in.
        track.mdhd = mdhd(12288, u32::MAX);
        assert_eq!(facts(&movie(&[&track], &[], &[])).duration, Some(1.0));
        track.mdhd = mdhd(12288, 0);
        assert_eq!(facts(&movie(&[&track], &[], &[])).duration, Some(1.0));
    }

    fn trex(id: u32, duration: u32) -> Vec<u8> {
        let mut rest = id.to_be_bytes().to_vec();
        rest.extend(1u32.to_be_bytes());
        rest.extend(duration.to_be_bytes());
        rest.extend([0; 8]);
        full(b"trex", 0, 0, &rest)
    }

    fn moof(id: u32, default: Option<u32>, durations: Result<&[u32], u32>) -> Vec<u8> {
        let mut tfhd_rest = id.to_be_bytes().to_vec();
        let mut flags = 0x02_0000;
        if let Some(default) = default {
            flags |= 0x08;
            tfhd_rest.extend(default.to_be_bytes());
        }
        let tfhd = full(b"tfhd", 0, flags, &tfhd_rest);
        let trun = match durations {
            Ok(durations) => {
                let mut rest = (durations.len() as u32).to_be_bytes().to_vec();
                rest.extend(0u32.to_be_bytes()); // data offset
                for d in durations {
                    rest.extend(d.to_be_bytes());
                    rest.extend(100u32.to_be_bytes());
                }
                full(b"trun", 0, 0x301, &rest)
            }
            Err(count) => full(b"trun", 0, 0, &count.to_be_bytes()),
        };
        mp4box(b"moof", &mp4box(b"traf", &[tfhd, trun].concat()))
    }

    #[test]
    fn fragments() {
        let empty = Track::video(12288, 0, &[]);
        let mvex = mp4box(b"mvex", &trex(1, 512));
        let after = [
            moof(1, None, Err(12)),
            moof(1, Some(512), Ok(&[512; 12])),
            // Another track's fragment.
            moof(2, None, Ok(&[7; 5])),
        ]
        .concat();
        let f = facts(&movie(&[&empty], &mvex, &after));
        assert_eq!(
            (f.fps, f.duration, f.frames),
            (Some(24.0), Some(1.0), Some(24))
        );
        // Samples in the moov and in fragments: every sample counts, mdhd does not.
        let first_half = Track::video(12288, 6144, &[(12, 512)]);
        let f = facts(&movie(&[&first_half], &mvex, &moof(1, None, Err(12))));
        assert_eq!((f.duration, f.frames), (Some(1.0), Some(24)));
        // A variable rate inside a fragment.
        let f = facts(&movie(
            &[&empty],
            &mvex,
            &moof(1, None, Ok(&[512, 1024, 512, 1536])),
        ));
        assert_eq!(
            (f.fps, f.duration, f.frames),
            (Some(24.0), Some(0.291667), Some(4))
        );
        // A trun that claims more samples than it holds is refused.
        let lying = full(b"trun", 0, 0x100, &1000u32.to_be_bytes());
        let tfhd = full(b"tfhd", 0, 0, &1u32.to_be_bytes());
        let bad = mp4box(b"moof", &mp4box(b"traf", &[tfhd, lying].concat()));
        assert_eq!(
            mp4_facts(&movie(&[&empty], &mvex, &bad)).unwrap_err(),
            "not an MP4 file (its trun box is too short)"
        );
    }

    #[test]
    fn sample_entries_decide_alpha() {
        let alpha = |entry: Vec<u8>| {
            let mut track = Track::video(12288, 12288, &[(24, 512)]);
            track.entry = entry;
            facts(&movie(&[&track], &[], &[])).has_alpha
        };
        for kind in NO_ALPHA_ENTRIES {
            assert_eq!(alpha(entry(kind, 64, 48, &[])), Some(false));
        }
        assert_eq!(alpha(entry(b"ap4h", 64, 48, &[])), None);
        assert_eq!(alpha(entry(b"mp4v", 64, 48, &[])), None);
        let esds = |object: u8| {
            // ES_Descriptor: tag 3, size, ES_ID 1, flags 0; DecoderConfigDescriptor: tag 4.
            let mut rest = vec![0x03, 0x80, 0x80, 0x80, 20, 0, 1, 0];
            rest.extend([0x04, 13, object, 0x11]);
            rest.extend([0; 11]);
            full(b"esds", 0, 0, &rest)
        };
        assert_eq!(alpha(entry(b"mp4v", 64, 48, &esds(0x20))), Some(false));
        assert_eq!(alpha(entry(b"mp4v", 64, 48, &esds(0x6C))), Some(false));
        assert_eq!(alpha(entry(b"mp4v", 64, 48, &esds(0x40))), None);
        // A PNG track with no sample to read leaves it out.
        assert_eq!(alpha(entry(b"mp4v", 64, 48, &esds(PNG_OBJECT))), None);
        // Child boxes that do not parse leave it out; they never refuse the file.
        assert_eq!(alpha(entry(b"mp4v", 64, 48, &[0, 0, 0, 99, b'e'])), None);
    }

    #[test]
    fn sizes_come_from_the_sample_entry() {
        let mut track = Track::video(12288, 12288, &[(24, 512)]);
        track.entry = entry(b"avc1", 66, 50, &[]);
        let f = facts(&movie(&[&track], &[], &[]));
        assert_eq!((f.width, f.height), (66, 50));
    }

    #[test]
    fn broken_boxes_are_refused() {
        let refused = |file: &[u8]| mp4_facts(file).unwrap_err();
        let track = Track::video(12288, 24576, &[(48, 512)]);
        let good = movie(&[&track], &[], &[]);
        assert_eq!(
            refused(b"not a video"),
            "not an MP4 file (a box at byte 0 runs past the end of the file)"
        );
        assert_eq!(refused(b""), "not an MP4 file (it has no moov box)");
        assert_eq!(
            refused(&mp4box(b"ftyp", b"isom")),
            "not an MP4 file (it has no moov box)"
        );
        let mut cut = good.clone();
        cut.truncate(good.len() - 10);
        assert_eq!(
            refused(&cut),
            "not an MP4 file (a box at byte 20 runs past the end of the file)"
        );
        let mut trailing = good.clone();
        trailing.extend([0, 0, 0]);
        assert_eq!(
            refused(&trailing),
            format!(
                "not an MP4 file (a box header at byte {} is cut short)",
                good.len()
            )
        );
        assert_eq!(
            refused(&[0, 0, 0, 4, b'f', b'r', b'e', b'e']),
            "not an MP4 file (a box at byte 0 is smaller than its header)"
        );
        // A 64-bit size cut short, and one too large.
        assert!(refused(&[0, 0, 0, 1, b'm', b'd', b'a', b't', 0]).contains("cut short"));
        let mut large = vec![0, 0, 0, 1, b'm', b'd', b'a', b't'];
        large.extend(u64::MAX.to_be_bytes());
        assert!(refused(&large).contains("runs past the end of the file"));
        // A child that runs past its parent.
        let mut inner = mp4box(b"moov", &mp4box(b"trak", &[0; 4]));
        inner[11] = 200;
        assert_eq!(
            refused(&inner),
            "not an MP4 file (a box at byte 8 runs past the end of its parent box)"
        );
        // A table whose declared count runs past its box.
        let mut lying = Track::video(12288, 24576, &[(48, 512)]);
        lying.stts = vec![(48, 512); 3];
        let mut file = movie(&[&lying], &[], &[]);
        let at = file.windows(4).position(|w| w == b"stts").unwrap() + 8;
        file[at..at + 4].copy_from_slice(&1_000_000u32.to_be_bytes());
        assert_eq!(
            refused(&file),
            "not an MP4 file (its stts box is too short)"
        );
    }

    #[test]
    fn a_video_track_needs_its_boxes() {
        let mut track = Track::video(12288, 24576, &[(48, 512)]);
        track.mdhd = Vec::new();
        assert_eq!(
            mp4_facts(&movie(&[&track], &[], &[])).unwrap_err(),
            "not an MP4 file (its video track has no mdhd box)"
        );
        let mut short = Track::video(12288, 24576, &[(48, 512)]);
        short.entry = mp4box(b"avc1", &[0; 40]);
        assert_eq!(
            mp4_facts(&movie(&[&short], &[], &[])).unwrap_err(),
            "not an MP4 file (its video sample entry is too short)"
        );
        let mut truncated_mdhd = Track::video(12288, 24576, &[(48, 512)]);
        truncated_mdhd.mdhd = full(b"mdhd", 0, 0, &[0; 6]);
        assert_eq!(
            mp4_facts(&movie(&[&truncated_mdhd], &[], &[])).unwrap_err(),
            "not an MP4 file (its mdhd box is too short)"
        );
    }

    #[test]
    fn size_zero_runs_to_the_end_and_size_one_is_64_bit() {
        let track = Track::video(12288, 24576, &[(48, 512)]);
        let mut file = movie(&[&track], &[], &[]);
        // An mdat of size 0 runs to the end of the file.
        file.extend([0, 0, 0, 0, b'm', b'd', b'a', b't', 1, 2, 3]);
        assert_eq!(facts(&file).frames, Some(48));
        // A 64-bit moov size.
        let moov = mp4box(b"moov", &[mvhd(1000, 0), track.bytes()].concat());
        let mut large = vec![0, 0, 0, 1, b'm', b'o', b'o', b'v'];
        large.extend(((moov.len() + 8) as u64).to_be_bytes());
        large.extend(&moov[8..]);
        assert_eq!(facts(&large).frames, Some(48));
    }

    #[test]
    fn rate_merges_and_counts() {
        let mut rate = Rate::default();
        rate.add(3, 10);
        rate.add(2, 10);
        rate.add(1, 20);
        assert_eq!((rate.runs, rate.last_count, rate.delta()), (2, 1, 10));
        rate.add(1, 15);
        assert_eq!(rate.delta(), 5);
        assert_eq!(rate.total, 85);
        assert_eq!(gcd(0, 7), 7);
    }
}
