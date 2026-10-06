//! MP4 (ISO base media) video facts (spec/facts.md §4): the first track whose handler is
//! `vide`, in file order.
//!
//! - `width`, `height`: the visual sample entry's in `stsd` (never `tkhd`'s, never rotated);
//! - `fps`: from the track's sample durations (`stts`, then fragments), leaving out those of 0,
//!   the last one, and a first one unlike the second: `timescale / d` when every duration is a
//!   multiple of the smallest, `d`; otherwise the durations are a constant rate rounded to the
//!   timescale, and `fps` is the whole number, else the multiple of 1000/1001, else the simplest
//!   fraction, that their count and sum allow (`Rate::fps`); rounded to 6 places;
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
//! used, nothing is allocated from a declared size, and only the boxes these facts name are
//! read: the top-level walk reads each box's header and skips its content, so an `mdat` is
//! never read.

use super::source::{Fail, Source};
use super::{VideoFacts, png_has_alpha, round6, seconds};
use std::io::{Cursor, Read, Seek};

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
const VISUAL_ENTRY: u64 = 78;
/// The most durations a frame rate is read from (spec/facts.md §4.3): from 2^64 on, none.
const MOST_DURATIONS: u128 = 1 << 64;

/// The facts of `bytes` read as MP4 (module doc).
pub fn mp4_facts(bytes: &[u8]) -> Result<Option<VideoFacts>, String> {
    let mut source = Source::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    read(&mut source).map_err(|fail| match fail {
        Fail::Refused(reason) => reason,
        Fail::Io(error) => error.to_string(),
    })
}

/// The facts of an MP4 file read through `source` (module doc); a refusal's reason carries its
/// prefix.
pub(crate) fn read<R: Read + Seek>(source: &mut Source<R>) -> Result<Option<VideoFacts>, Fail> {
    movie(source).map_err(|fail| match fail {
        Fail::Refused(reason) => Fail::Refused(format!("not an MP4 file ({reason})")),
        io => io,
    })
}

/// A box: its type, where its content starts, and where it ends.
#[derive(Debug, Clone, Copy)]
struct Mp4Box {
    kind: [u8; 4],
    content: u64,
    end: u64,
}

impl Mp4Box {
    fn name(&self) -> String {
        String::from_utf8_lossy(&self.kind).into_owned()
    }
}

/// A walk over the boxes laid end to end in `at..end`. The first box that does not fit is an
/// error, and ends the walk.
struct Walk {
    at: u64,
    end: u64,
    top: bool,
    failed: bool,
}

impl Walk {
    fn top(len: u64) -> Self {
        Walk {
            at: 0,
            end: len,
            top: true,
            failed: false,
        }
    }

    fn range(at: u64, end: u64) -> Self {
        Walk {
            at,
            end,
            top: false,
            failed: false,
        }
    }

    fn within(parent: Mp4Box) -> Self {
        Self::range(parent.content, parent.end)
    }

    fn next<R: Read + Seek>(&mut self, source: &mut Source<R>) -> Option<Result<Mp4Box, Fail>> {
        if self.failed || self.at >= self.end {
            return None;
        }
        let result = self.read(source);
        self.failed = result.is_err();
        Some(result)
    }

    fn read<R: Read + Seek>(&mut self, source: &mut Source<R>) -> Result<Mp4Box, Fail> {
        let at = self.at;
        let left = self.end - at;
        let cut = || format!("a box header at byte {at} is cut short");
        if left < 8 {
            return Err(cut().into());
        }
        let head = source.bytes(at, 8)?;
        let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let kind = [head[4], head[5], head[6], head[7]];
        let (size, header) = match size32 {
            1 => {
                if left < 16 {
                    return Err(cut().into());
                }
                let large = source.bytes(at + 8, 8)?;
                (u64::from_be_bytes(large.try_into().unwrap_or_default()), 16)
            }
            0 => (left, 8),
            n => (u64::from(n), 8),
        };
        if size < header {
            return Err(format!("a box at byte {at} is smaller than its header").into());
        }
        if size > left {
            let parent = if self.top {
                "the file"
            } else {
                "its parent box"
            };
            return Err(format!("a box at byte {at} runs past the end of {parent}").into());
        }
        let end = at + size;
        self.at = end;
        Ok(Mp4Box {
            kind,
            content: at + header,
            end,
        })
    }
}

/// Every child of `parent` (all of them parsed, so a broken one refuses the file).
fn children<R: Read + Seek>(source: &mut Source<R>, parent: Mp4Box) -> Result<Vec<Mp4Box>, Fail> {
    let mut walk = Walk::within(parent);
    let mut boxes = Vec::new();
    while let Some(b) = walk.next(source) {
        boxes.push(b?);
    }
    Ok(boxes)
}

fn first(boxes: &[Mp4Box], kind: &[u8; 4]) -> Option<Mp4Box> {
    boxes.iter().find(|b| &b.kind == kind).copied()
}

/// `count` bytes at `offset` in a box's content, or `its <type> box is too short`.
fn field<R: Read + Seek>(
    source: &mut Source<R>,
    b: Mp4Box,
    offset: u64,
    count: usize,
) -> Result<&[u8], Fail> {
    let content = b.end - b.content;
    if offset
        .checked_add(count as u64)
        .is_none_or(|end| end > content)
    {
        return Err(format!("its {} box is too short", b.name()).into());
    }
    Ok(source.bytes(b.content + offset, count)?)
}

fn u8_field<R: Read + Seek>(source: &mut Source<R>, b: Mp4Box, offset: u64) -> Result<u8, Fail> {
    Ok(field(source, b, offset, 1)?[0])
}

fn u32_field<R: Read + Seek>(source: &mut Source<R>, b: Mp4Box, offset: u64) -> Result<u32, Fail> {
    let bytes = field(source, b, offset, 4)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn u64_field<R: Read + Seek>(source: &mut Source<R>, b: Mp4Box, offset: u64) -> Result<u64, Fail> {
    let bytes = field(source, b, offset, 8)?;
    let mut array = [0; 8];
    array.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(array))
}

/// The timescale of an `mvhd` or `mdhd` box (content offset 20 in version 1, else 12).
fn timescale<R: Read + Seek>(source: &mut Source<R>, b: Mp4Box) -> Result<u32, Fail> {
    let offset = if u8_field(source, b, 0)? == 1 { 20 } else { 12 };
    u32_field(source, b, offset)
}

/// `(timescale, duration, the duration that means unknown)` of an `mdhd` box.
fn scale_and_duration<R: Read + Seek>(
    source: &mut Source<R>,
    b: Mp4Box,
) -> Result<(u32, u64, u64), Fail> {
    if u8_field(source, b, 0)? == 1 {
        Ok((
            u32_field(source, b, 20)?,
            u64_field(source, b, 24)?,
            u64::MAX,
        ))
    } else {
        let scale = u32_field(source, b, 12)?;
        let duration = u32_field(source, b, 16)?;
        Ok((scale, u64::from(duration), u64::from(u32::MAX)))
    }
}

/// A track's sample durations, for its length and its frame rate (spec/facts.md §4.3).
///
/// The rate is read from the durations above 0, in order, leaving out the last one and, when
/// another is left besides it, a first one that differs from the second by more than half the
/// second. The durations are summarised as they arrive: the first and second are kept, the
/// latest run is held back (its last sample may be the track's last), and the rest are counted,
/// summed, and folded into their smallest value and greatest common divisor.
#[derive(Debug, Default, Clone)]
struct Rate {
    /// Every sample's duration, 0 among them, summed: the track's length when it has fragments.
    total: u128,
    /// The durations above 0.
    samples: u128,
    first: u64,
    second: u64,
    /// The latest run of durations after the first: `(count, duration)`.
    held: (u128, u64),
    /// The durations after the first and before the held run.
    count: u128,
    sum: u128,
    smallest: u64,
    divisor: u64,
}

impl Rate {
    /// `count` samples of `duration` units each.
    fn add(&mut self, count: u64, duration: u64) {
        self.total = self
            .total
            .saturating_add(u128::from(count) * u128::from(duration));
        if count == 0 || duration == 0 {
            return;
        }
        let mut count = count;
        if self.samples == 0 {
            self.first = duration;
            self.samples = 1;
            count -= 1;
            if count == 0 {
                return;
            }
        }
        if self.samples == 1 {
            self.second = duration;
        }
        self.samples = self.samples.saturating_add(u128::from(count));
        let (held, held_duration) = self.held;
        if held_duration == duration {
            self.held.0 = held.saturating_add(u128::from(count));
        } else {
            self.keep(held, held_duration);
            self.held = (u128::from(count), duration);
        }
    }

    /// Counts `count` durations of `duration` into the summary.
    fn keep(&mut self, count: u128, duration: u64) {
        if count == 0 {
            return;
        }
        self.count = self.count.saturating_add(count);
        self.sum = self
            .sum
            .saturating_add(count.saturating_mul(u128::from(duration)));
        self.smallest = if self.smallest == 0 {
            duration
        } else {
            self.smallest.min(duration)
        };
        self.divisor = gcd(self.divisor, duration);
    }

    /// `fps` at `scale` units a second (spec/facts.md §4.3), rounded to 6 places; `None` when the
    /// timescale is 0 or no duration is above 0.
    fn fps(&self, scale: u32) -> Option<f64> {
        if scale == 0 || self.samples == 0 {
            return None;
        }
        if self.samples == 1 {
            return Some(ratio(u128::from(scale), u128::from(self.first)));
        }
        let mut rate = self.clone();
        // The last duration says only when the track ends.
        let (held, held_duration) = rate.held;
        rate.keep(held - 1, held_duration);
        let left = self.samples - 1;
        if !(left >= 2 && 2 * self.first.abs_diff(self.second) > self.second) {
            rate.keep(1, self.first);
        }
        if rate.count >= MOST_DURATIONS {
            return None;
        }
        let (smallest, divisor) = (rate.smallest, rate.divisor);
        if smallest == divisor {
            // Every duration is a multiple of the smallest: a constant rate, or a varying one on
            // a grid.
            return Some(ratio(u128::from(scale), u128::from(smallest)));
        }
        rounded_rate(scale, rate.count, rate.sum, divisor)
    }
}

/// `p / q` in binary64, rounded to 6 places (spec/facts.md §1).
fn ratio(p: u128, q: u128) -> f64 {
    round6(p as f64 / q as f64)
}

/// The frame rate of `count` durations summing to `sum` that are a constant rate rounded to a
/// grid of `grid` units of `scale` a second (spec/facts.md §4.3). The rates they allow are those
/// strictly between `scale × count ÷ (sum + grid)` and `scale × count ÷ (sum − grid)`; of them,
/// the whole number nearest `scale × count ÷ sum`, else the multiple of 1000/1001 nearest it,
/// else the fraction with the smallest denominator. Every duration is at least twice `grid`, so
/// `sum` exceeds it.
fn rounded_rate(scale: u32, count: u128, sum: u128, grid: u64) -> Option<f64> {
    let units = u128::from(scale).checked_mul(count)?;
    let grid = u128::from(grid);
    let (low, high) = (sum.checked_add(grid)?, sum.checked_sub(grid)?);
    if let Some(whole) = nearest_whole(units, sum, low, high) {
        return Some(ratio(whole, 1));
    }
    let scaled = |n: u128| n.checked_mul(1000);
    if let Some(k) = nearest_whole(
        units.checked_mul(1001)?,
        scaled(sum)?,
        scaled(low)?,
        scaled(high)?,
    ) {
        return Some(ratio(k.checked_mul(1000)?, 1001));
    }
    let (p, q) = simplest(units, low, units, high)?;
    Some(ratio(p, q))
}

/// The whole number strictly between `n / low` and `n / high` (`low > mid > high > 0`) that is
/// nearest `n / mid`, the smaller of two equally near; `None` when there is none.
fn nearest_whole(n: u128, mid: u128, low: u128, high: u128) -> Option<u128> {
    let first = n / low + 1;
    let last = n.checked_sub(1)? / high;
    if first > last {
        return None;
    }
    let (below, rest) = (n / mid, n % mid);
    let nearest = if rest <= mid - rest { below } else { below + 1 };
    Some(nearest.clamp(first, last))
}

/// The fraction `p / q` with the smallest denominator strictly between `xn / xd` and `yn / yd`
/// (`0 <= xn / xd < yn / yd`; there is exactly one), by the continued fractions of the bounds.
fn simplest(mut xn: u128, mut xd: u128, mut yn: u128, mut yd: u128) -> Option<(u128, u128)> {
    // The convergents so far: (p0, q0) the one before (p1, q1).
    let (mut p0, mut q0, mut p1, mut q1) = (0u128, 1u128, 1u128, 0u128);
    let step = |term: u128, p0: u128, q0: u128, p1: u128, q1: u128| {
        Some((
            term.checked_mul(p1)?.checked_add(p0)?,
            term.checked_mul(q1)?.checked_add(q0)?,
        ))
    };
    loop {
        let whole = xn / xd;
        // The smallest whole number above x lies below y: it ends the fraction.
        if whole < (yn - 1) / yd {
            return step(whole + 1, p0, q0, p1, q1);
        }
        // Both bounds lie in [whole, whole + 1]: take `whole` and go on with the reciprocals of
        // what is left over.
        let x_rest = xn % xd;
        let y_rest = yn.checked_sub(whole.checked_mul(yd)?)?;
        let (p, q) = step(whole, p0, q0, p1, q1)?;
        (p0, q0, p1, q1) = (p1, q1, p, q);
        if x_rest == 0 {
            // x is whole: the simplest fraction above it is x + 1/m for the smallest m with
            // 1/m below y - x.
            return step(yd / y_rest + 1, p0, q0, p1, q1);
        }
        (xn, xd, yn, yd) = (yd, y_rest, xd, x_rest);
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

fn movie<R: Read + Seek>(source: &mut Source<R>) -> Result<Option<VideoFacts>, Fail> {
    let mut moov = None;
    let mut moofs = Vec::new();
    let mut walk = Walk::top(source.len());
    while let Some(b) = walk.next(source) {
        let b = b?;
        match &b.kind {
            b"moov" if moov.is_none() => moov = Some(b),
            b"moof" => moofs.push(b),
            _ => {}
        }
    }
    let moov = moov.ok_or("it has no moov box")?;
    let mut mvhd = None;
    let mut mvex = None;
    let mut video = None;
    let mut walk = Walk::within(moov);
    while let Some(b) = walk.next(source) {
        let b = b?;
        match &b.kind {
            b"mvhd" if mvhd.is_none() => mvhd = Some(b),
            b"mvex" if mvex.is_none() => mvex = Some(b),
            b"trak" if video.is_none() => video = video_trak(source, b)?,
            _ => {}
        }
    }
    let Some(video) = video else {
        return Ok(None);
    };
    let mdhd = first(&video.mdia, b"mdhd").ok_or("its video track has no mdhd box")?;
    let (scale, media_duration, unknown) = scale_and_duration(source, mdhd)?;
    let mut stbl = Vec::new();
    if let Some(minf) = first(&video.mdia, b"minf") {
        let minf = children(source, minf)?;
        if let Some(found) = first(&minf, b"stbl") {
            stbl = children(source, found)?;
        }
    }
    let stsd = first(&stbl, b"stsd").ok_or("its video track has no stsd box")?;
    let entries = u32_field(source, stsd, 4)?;
    let entry = if entries > 0 {
        Walk::range(stsd.content + 8, stsd.end)
            .next(source)
            .transpose()?
    } else {
        None
    };
    let entry = entry.ok_or("its video track has no sample entry")?;
    if entry.end - entry.content < VISUAL_ENTRY {
        return Err("its video sample entry is too short".into());
    }
    let size = source.bytes(entry.content + 24, 4)?;
    let width = u32::from(u16::from_be_bytes([size[0], size[1]]));
    let height = u32::from(u16::from_be_bytes([size[2], size[3]]));

    let mut has_alpha = None;
    let mut png = &entry.kind == b"png ";
    if NO_ALPHA_ENTRIES.contains(&&entry.kind) {
        has_alpha = Some(false);
    } else if &entry.kind == b"mp4v" {
        let object = object_type(source, entry.content + VISUAL_ENTRY, entry.end)?;
        if object.is_some_and(|object| NO_ALPHA_OBJECTS.contains(&object)) {
            has_alpha = Some(false);
        }
        png = object == Some(PNG_OBJECT);
    }

    let mut rate = Rate::default();
    if let Some(stts) = first(&stbl, b"stts") {
        let count = u32_field(source, stts, 4)?;
        for i in 0..u64::from(count) {
            let entry = field(source, stts, 8 + 8 * i, 8)?;
            let count = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let delta = u32::from_be_bytes([entry[4], entry[5], entry[6], entry[7]]);
            rate.add(u64::from(count), u64::from(delta));
        }
    }
    let mut frames: u64 = 0;
    let mut first_size = None;
    if let Some(stsz) = first(&stbl, b"stsz") {
        let size = u32_field(source, stsz, 4)?;
        frames = u64::from(u32_field(source, stsz, 8)?);
        // The first entry is read only for a PNG track's first sample (spec/facts.md §4.4).
        if png && frames > 0 {
            first_size = Some(if size != 0 {
                size
            } else {
                u32_field(source, stsz, 12)?
            });
        }
    } else if let Some(stz2) = first(&stbl, b"stz2") {
        frames = u64::from(u32_field(source, stz2, 8)?);
    }

    let mut track_id = None;
    if let Some(tkhd) = first(&video.children, b"tkhd") {
        let offset = if u8_field(source, tkhd, 0)? == 1 {
            20
        } else {
            12
        };
        track_id = Some(u32_field(source, tkhd, offset)?);
    }
    let mut fragmented = false;
    if let Some(track_id) = track_id {
        let mut trex_duration = 0;
        if let Some(mvex) = mvex {
            let mut walk = Walk::within(mvex);
            while let Some(b) = walk.next(source) {
                let b = b?;
                if &b.kind == b"trex" && u32_field(source, b, 4)? == track_id {
                    // track_ID, default_sample_description_index, default_sample_duration
                    trex_duration = u32_field(source, b, 12)?;
                    break;
                }
            }
        }
        for moof in &moofs {
            let mut walk = Walk::within(*moof);
            while let Some(b) = walk.next(source) {
                let b = b?;
                if &b.kind == b"traf" {
                    let (matched, count) = traf(source, b, track_id, trex_duration, &mut rate)?;
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
    if let Some(edits) = edits(source, &video.children, scale, mvhd)? {
        units = units.min(edits);
    }
    let fps = rate.fps(scale);
    let duration = (scale != 0).then(|| round6(seconds(units, u64::from(scale))));

    if let Some(size) = first_size {
        let mut offset = None;
        if let Some(stco) = first(&stbl, b"stco") {
            if u32_field(source, stco, 4)? > 0 {
                offset = Some(u64::from(u32_field(source, stco, 8)?));
            }
        } else if let Some(co64) = first(&stbl, b"co64")
            && u32_field(source, co64, 4)? > 0
        {
            offset = Some(u64_field(source, co64, 8)?);
        }
        if let Some(offset) = offset
            && let Some(end) = offset.checked_add(u64::from(size))
            && end <= source.len()
            && let Some(alpha) = png_has_alpha(source.range(offset, end))
        {
            has_alpha = Some(alpha);
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
fn video_trak<R: Read + Seek>(
    source: &mut Source<R>,
    trak: Mp4Box,
) -> Result<Option<VideoTrak>, Fail> {
    let trak_children = children(source, trak)?;
    let Some(mdia) = first(&trak_children, b"mdia") else {
        return Ok(None);
    };
    let mdia = children(source, mdia)?;
    let Some(hdlr) = first(&mdia, b"hdlr") else {
        return Ok(None);
    };
    // version and flags (4), pre_defined (4), handler_type (4)
    if field(source, hdlr, 8, 4)? != b"vide" {
        return Ok(None);
    }
    Ok(Some(VideoTrak {
        children: trak_children,
        mdia,
    }))
}

/// The non-empty edits' durations (a media time other than -1 and a duration above 0), each
/// rescaled from the movie to the media timescale and rounded to nearest, summed; `None` when
/// the track has no such edit, or the movie has no timescale. `mvhd`'s timescale is read only
/// here, for a track with an `edts`; its duration never.
fn edits<R: Read + Seek>(
    source: &mut Source<R>,
    trak: &[Mp4Box],
    scale: u32,
    mvhd: Option<Mp4Box>,
) -> Result<Option<u128>, Fail> {
    let (Some(edts), Some(mvhd)) = (first(trak, b"edts"), mvhd) else {
        return Ok(None);
    };
    let movie_scale = timescale(source, mvhd)?;
    if movie_scale == 0 {
        return Ok(None);
    }
    let Some(elst) = first(&children(source, edts)?, b"elst") else {
        return Ok(None);
    };
    let version = u8_field(source, elst, 0)?;
    let count = u32_field(source, elst, 4)?;
    let (movie, media) = (u128::from(movie_scale), u128::from(scale));
    let mut total: u128 = 0;
    let mut found = false;
    for i in 0..u64::from(count) {
        let (duration, empty) = if version == 1 {
            let at = 8 + 20 * i;
            (
                u64_field(source, elst, at)?,
                u64_field(source, elst, at + 8)? == u64::MAX,
            )
        } else {
            let at = 8 + 12 * i;
            let duration = u64::from(u32_field(source, elst, at)?);
            (duration, u32_field(source, elst, at + 4)? == u32::MAX)
        };
        if !empty && duration > 0 {
            found = true;
            total = total.saturating_add((u128::from(duration) * media + movie / 2) / movie);
        }
    }
    Ok(found.then_some(total))
}

/// One `traf`: whether it belongs to the track, and its samples (durations into `rate`).
fn traf<R: Read + Seek>(
    source: &mut Source<R>,
    traf: Mp4Box,
    track_id: u32,
    trex_duration: u32,
    rate: &mut Rate,
) -> Result<(bool, u64), Fail> {
    let boxes = children(source, traf)?;
    let Some(tfhd) = first(&boxes, b"tfhd") else {
        return Ok((false, 0));
    };
    let flags = u32_field(source, tfhd, 0)? & 0xFF_FFFF;
    if u32_field(source, tfhd, 4)? != track_id {
        return Ok((false, 0));
    }
    let mut default = trex_duration;
    if flags & 0x08 != 0 {
        // base_data_offset (8) and sample_description_index (4) come first when present.
        let offset =
            8 + if flags & 0x01 != 0 { 8 } else { 0 } + if flags & 0x02 != 0 { 4 } else { 0 };
        default = u32_field(source, tfhd, offset)?;
    }
    let mut frames: u64 = 0;
    for trun in boxes.iter().filter(|b| &b.kind == b"trun") {
        let flags = u32_field(source, *trun, 0)? & 0xFF_FFFF;
        let count = u64::from(u32_field(source, *trun, 4)?);
        let offset =
            8 + if flags & 0x001 != 0 { 4 } else { 0 } + if flags & 0x004 != 0 { 4 } else { 0 };
        let per_sample = 4 * u64::from((flags & 0xF00).count_ones());
        let content = trun.end - trun.content;
        if offset + count * per_sample > content {
            return Err("its trun box is too short".into());
        }
        frames = frames.saturating_add(count);
        if flags & 0x100 != 0 {
            for i in 0..count {
                let duration = u32_field(source, *trun, offset + i * per_sample)?;
                rate.add(1, u64::from(duration));
            }
        } else {
            rate.add(count, u64::from(default));
        }
    }
    Ok((true, frames))
}

/// The `objectTypeIndication` of the first `esds` box among a sample entry's child boxes
/// (`start..end`); `None` when there is none, or the boxes or the descriptors do not parse
/// (spec/facts.md §4.4).
fn object_type<R: Read + Seek>(
    source: &mut Source<R>,
    start: u64,
    end: u64,
) -> Result<Option<u8>, Fail> {
    let mut walk = Walk::range(start, end);
    let esds = loop {
        match walk.next(source) {
            None | Some(Err(Fail::Refused(_))) => return Ok(None),
            Some(Err(io)) => return Err(io),
            Some(Ok(b)) if &b.kind == b"esds" => break b,
            Some(Ok(_)) => {}
        }
    };
    // version and flags (4), then the ES_Descriptor (tag 3).
    let Some((0x03, at)) = descriptor(source, esds, 4)? else {
        return Ok(None);
    };
    // ES_ID (2) and the flags byte, then what the flags add: dependsOn_ES_ID (2) with 0x80, a
    // URL (its length byte and that many bytes) with 0x40, OCR_ES_Id (2) with 0x20.
    let Some(flags) = esds_byte(source, esds, at + 2)? else {
        return Ok(None);
    };
    let mut at = at + 3;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        let Some(length) = esds_byte(source, esds, at)? else {
            return Ok(None);
        };
        at += 1 + u64::from(length);
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    // The DecoderConfigDescriptor (tag 4) starts with the objectTypeIndication.
    let Some((0x04, at)) = descriptor(source, esds, at)? else {
        return Ok(None);
    };
    esds_byte(source, esds, at)
}

/// The byte at `at` in an `esds` box's content; `None` past its end.
fn esds_byte<R: Read + Seek>(
    source: &mut Source<R>,
    esds: Mp4Box,
    at: u64,
) -> Result<Option<u8>, Fail> {
    if at >= esds.end - esds.content {
        return Ok(None);
    }
    Ok(Some(source.byte(esds.content + at)?))
}

/// The descriptor at `at` in an `esds` box's content: its tag and where its content starts.
/// Its size takes one to four bytes, each but the last with its continuation bit (0x80) set; a
/// fourth byte ends it whatever its bit. The size itself is not used.
fn descriptor<R: Read + Seek>(
    source: &mut Source<R>,
    esds: Mp4Box,
    at: u64,
) -> Result<Option<(u8, u64)>, Fail> {
    let Some(tag) = esds_byte(source, esds, at)? else {
        return Ok(None);
    };
    let mut at = at + 1;
    for _ in 0..4 {
        let Some(size) = esds_byte(source, esds, at)? else {
            return Ok(None);
        };
        at += 1;
        if size & 0x80 == 0 {
            break;
        }
    }
    Ok(Some((tag, at)))
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

    /// The durations of `frames` frames at `num / den` per second whose timestamps are rounded
    /// to the nearest unit of `scale` (halves up), as a muxer writes them.
    fn rounded(scale: u64, num: u64, den: u64, frames: u64) -> Vec<(u32, u32)> {
        let at = |k: u64| (2 * k * scale * den + num) / (2 * num);
        (0..frames)
            .map(|k| (1, (at(k + 1) - at(k)) as u32))
            .collect()
    }

    #[test]
    fn the_rate_rule() {
        let fps_at = |scale: u32, runs: &[(u32, u32)]| {
            facts(&movie(&[&Track::video(scale, 1, runs)], &[], &[])).fps
        };
        let fps = |runs: &[(u32, u32)]| fps_at(12288, runs);
        assert_eq!(fps(&[(24, 512)]), Some(24.0));
        // One sample: its own duration.
        assert_eq!(fps(&[(1, 512)]), Some(24.0));
        // The last duration is left out, whatever it is.
        assert_eq!(fps(&[(23, 512), (1, 256)]), Some(24.0));
        assert_eq!(fps(&[(47, 512), (1, 1061)]), Some(24.0));
        assert_eq!(fps(&[(1, 512), (1, 256)]), Some(24.0));
        // Runs of 0 samples and durations of 0 are skipped; equal runs count as one.
        assert_eq!(fps(&[(10, 512), (14, 512)]), Some(24.0));
        assert_eq!(
            fps(&[(10, 512), (0, 7), (14, 512), (3, 0), (1, 999)]),
            Some(24.0)
        );
        // A first duration that differs from the second by more than half of it is left out
        // (a first frame that starts early, as with audio in front of it); one nearer is not.
        assert_eq!(fps(&[(1, 1061), (47, 512)]), Some(24.0));
        assert_eq!(fps(&[(1, 700), (47, 512)]), Some(23.8125));
        // Durations that are all multiples of the smallest: its rate (a varying rate on a grid).
        assert_eq!(
            fps(&[(14, 512), (7, 1024), (1, 512), (1, 1536), (1, 512)]),
            Some(24.0)
        );
        assert_eq!(fps(&[(9, 1001), (1, 2002), (20, 1001)]), Some(12.275724));
        // Otherwise a constant rate rounded to the timescale: a whole number, else a multiple of
        // 1000/1001, else the simplest fraction, that the count and the sum allow.
        assert_eq!(fps_at(1000, &rounded(1000, 24, 1, 48)), Some(24.0));
        assert_eq!(
            fps_at(1000, &rounded(1000, 30000, 1001, 120)),
            Some(29.97003)
        );
        assert_eq!(fps_at(1000, &rounded(1000, 15, 2, 31)), Some(7.5));
        assert_eq!(
            fps_at(10_000_000, &rounded(10_000_000, 24, 1, 31)),
            Some(24.0)
        );
        assert_eq!(
            fps_at(90_000, &rounded(90_000, 60000, 1001, 240)),
            Some(59.94006)
        );
        // Milliseconds rescaled to 16000 units: 528 and 544, on a grid of 16.
        let remuxed: Vec<(u32, u32)> = rounded(1000, 30, 1, 60)
            .into_iter()
            .map(|(count, delta)| (count, delta * 16))
            .collect();
        assert_eq!(fps_at(16_000, &remuxed), Some(30.0));
        // The rates allowed lie strictly between the bounds: 12 is a bound here.
        assert_eq!(fps(&[(9, 1024), (1, 1536), (2, 1024)]), Some(11.988012));
        assert_eq!(fps(&[(10, 512), (1, 513), (13, 511)]), Some(24.02439));
        // No samples, or only durations of 0: no rate.
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
    fn rate_summarises_as_durations_arrive() {
        let mut rate = Rate::default();
        rate.add(3, 10);
        rate.add(2, 10);
        rate.add(0, 99);
        rate.add(4, 0);
        rate.add(1, 20);
        assert_eq!((rate.samples, rate.first, rate.second), (6, 10, 10));
        assert_eq!(rate.held, (1, 20));
        assert_eq!((rate.count, rate.sum), (4, 40));
        assert_eq!(rate.total, 70);
        // The last duration (20) is left out: 10 units a frame.
        assert_eq!(rate.fps(100), Some(10.0));
        // 10, 20 and 15 are no multiples of 10: 7 durations of 85 units in all, give or take a
        // unit of 5, allow the rates strictly between 7.78 and 8.75.
        rate.add(1, 15);
        rate.add(1, 99);
        assert_eq!(rate.fps(100), Some(8.0));
        assert_eq!(Rate::default().fps(100), None);
        assert_eq!(gcd(0, 7), 7);
    }

    #[test]
    fn whole_numbers_and_simplest_fractions_in_an_interval() {
        // Rates strictly between 60000/2001 and 60000/1999, nearest 60000/2000.
        assert_eq!(nearest_whole(60000, 2000, 2001, 1999), Some(30));
        // Between 1.5 and 3.5, 2 and 3 are equally near 2.5: the smaller.
        assert_eq!(nearest_whole(105, 42, 70, 30), Some(2));
        // The nearest one inside the interval when the nearest overall is outside it.
        assert_eq!(nearest_whole(100, 30, 40, 29), Some(3));
        // Between 1.4 and 1.75 there is none, and the bounds themselves do not count.
        assert_eq!(nearest_whole(7, 9, 5, 4), None);
        assert_eq!(nearest_whole(6, 4, 3, 2), None);
        assert_eq!(simplest(1, 3, 1, 2), Some((2, 5)));
        assert_eq!(simplest(0, 1, 1, 1), Some((1, 2)));
        assert_eq!(simplest(7, 2, 4, 1), Some((11, 3)));
        assert_eq!(simplest(3, 1, 5, 1), Some((4, 1)));
        assert_eq!(simplest(3, 1, 4, 1), Some((7, 2)));
        assert_eq!(simplest(29_955, 1000, 29_985, 1000), Some((689, 23)));
    }

    #[test]
    fn the_mvhd_duration_is_never_read() {
        // A version 0 mvhd whose content ends after its timescale (spec/facts.md §4.3): its
        // duration is never read, and its timescale only for a track with an edit list.
        let short_mvhd = full(
            b"mvhd",
            0,
            0,
            &[[0; 8].as_slice(), &1000u32.to_be_bytes()].concat(),
        );
        let with = |track: &Track, mvhd: &[u8]| {
            let mut moov = mvhd.to_vec();
            moov.extend(track.bytes());
            let mut file = mp4box(b"ftyp", b"isom\0\0\x02\0isom");
            file.extend(mp4box(b"moov", &moov));
            mp4_facts(&file)
        };
        let track = Track::video(12288, 24576, &[(48, 512)]);
        assert_eq!(
            with(&track, &short_mvhd).unwrap().unwrap().duration,
            Some(2.0)
        );
        let mut audio = Track::video(48000, 48000, &[(47, 1024)]);
        audio.handler = *b"soun";
        assert_eq!(with(&audio, &short_mvhd), Ok(None));
        let mut edited = Track::video(12288, 24576, &[(48, 512)]);
        edited.edits = Some(vec![(1500, 0)]);
        assert_eq!(
            with(&edited, &short_mvhd).unwrap().unwrap().duration,
            Some(1.5)
        );
        // Its timescale is read for a track with an edit list: too short for it is refused.
        let shorter = full(b"mvhd", 0, 0, &[0; 8]);
        assert_eq!(
            with(&edited, &shorter).unwrap_err(),
            "not an MP4 file (its mvhd box is too short)"
        );
        assert_eq!(with(&track, &shorter).unwrap().unwrap().duration, Some(2.0));
    }

    #[test]
    fn the_stsz_table_is_read_only_for_a_png_track() {
        // An H.264 track whose stsz has a sample size of 0, 24 samples and no table.
        let cut_stsz = full(
            b"stsz",
            0,
            0,
            &[0u32.to_be_bytes(), 24u32.to_be_bytes()].concat(),
        );
        let with_stsz = |entry: Vec<u8>| {
            let stbl = [stsd(&[entry]), stts(&[(24, 512)]), cut_stsz.clone()].concat();
            let minf = mp4box(b"minf", &mp4box(b"stbl", &stbl));
            let mdia = mp4box(b"mdia", &[mdhd(12288, 12288), hdlr(b"vide"), minf].concat());
            let trak = mp4box(b"trak", &[tkhd(1), mdia].concat());
            let mut file = mp4box(b"ftyp", b"isom\0\0\x02\0isom");
            file.extend(mp4box(b"moov", &[mvhd(1000, 0), trak].concat()));
            mp4_facts(&file)
        };
        let f = with_stsz(entry(b"avc1", 64, 48, &[])).unwrap().unwrap();
        assert_eq!((f.frames, f.has_alpha), (Some(24), Some(false)));
        // A PNG track reads the table's first entry for its first sample: cut short, refused.
        assert_eq!(
            with_stsz(entry(b"png ", 64, 48, &[])).unwrap_err(),
            "not an MP4 file (its stsz box is too short)"
        );
    }

    #[test]
    fn esds_descriptors() {
        let alpha = |esds_rest: &[u8]| {
            let mut track = Track::video(12288, 12288, &[(24, 512)]);
            track.entry = entry(b"mp4v", 64, 48, &full(b"esds", 0, 0, esds_rest));
            facts(&movie(&[&track], &[], &[])).has_alpha
        };
        // ES_Descriptor with ES_ID 1 and flags 0xE0: dependsOn_ES_ID (2), a URL of 3 bytes
        // (its length byte first), OCR_ES_Id (2); then the DecoderConfigDescriptor.
        let mut flagged = vec![0x03, 30, 0, 1, 0xE0, 0, 7, 3, b'a', b'b', b'c', 0, 9];
        flagged.extend([0x04, 13, 0x20, 0x11]);
        assert_eq!(alpha(&flagged), Some(false));
        // A size whose fourth byte keeps its continuation bit still ends there.
        assert_eq!(
            alpha(&[0x03, 0x80, 0x80, 0x80, 0x80, 0, 1, 0, 0x04, 1, 0x6C]),
            Some(false)
        );
        // An ES_Descriptor that is not first, or a cut descriptor: no has_alpha.
        assert_eq!(alpha(&[0x04, 13, 0x20]), None);
        assert_eq!(alpha(&[0x03, 5, 0, 1, 0x40]), None);
    }

    #[test]
    fn an_stsd_with_no_entries_has_no_sample_entry() {
        // spec/facts.md §4.2: an entry count of 0 means no sample entry, whatever follows.
        let mut track = Track::video(12288, 12288, &[(24, 512)]);
        track.entry = entry(b"avc1", 64, 48, &[]);
        let mut file = movie(&[&track], &[], &[]);
        let at = file.windows(4).position(|w| w == b"stsd").unwrap() + 8;
        file[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            mp4_facts(&file).unwrap_err(),
            "not an MP4 file (its video track has no sample entry)"
        );
    }
}
