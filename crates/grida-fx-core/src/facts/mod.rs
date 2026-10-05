//! File facts (spec/facts.md defines every rule; identity.md §4 summarises them): values the
//! engine computes from a file's bytes. A node host never computes them; the expression
//! `facts(f)` reads them while planning, and every file ref sent to a host carries them
//! (protocol.md §3.1).
//!
//! `facts(f)` always has `bytes` (the size) and `kind` (spec/facts.md §1). Images (`image/png`,
//! `image/jpeg`, `image/webp`, `image/gif`) add `width`, `height` (first frame), `has_alpha` (an
//! alpha channel, a PNG `tRNS` chunk, a WebP alpha flag, a GIF transparent index) and `opaque`
//! (`has_alpha` false, or every pixel of the first frame fully opaque) (§2). WAV audio adds
//! `duration` ([`wav`], §3); MP4 video ([`mp4`], §4) and Matroska/WebM video ([`matroska`], §5)
//! add `width`, `height`, `fps`, `duration`, `frames` and `has_alpha`. Decoders: `png`, `gif`,
//! `image-webp`; JPEG dimensions from its SOF marker (JPEG has no alpha); audio and video
//! containers are parsed natively, with no tool and no new dependency.
//! Keys are written in this order: `bytes`, `kind`, then images `width`, `height`, `has_alpha`,
//! `opaque`; WAV `duration`; video `width`, `height`, `fps`, `duration`, `frames`, `has_alpha`
//! (each video member only when the container gives it). Numbers go through
//! [`crate::value::number`], so a whole number is written as an integer (`24`, not `24.0`).
//!
//! Refusals: an image whose bytes do not decode, an MP4 or Matroska/WebM file whose structure
//! does not parse. A WAV file is never refused; one this rule cannot read has no `duration`.
//!
//! Per format:
//! - PNG: the header gives the size; `has_alpha` is a color type with alpha or a `tRNS` chunk.
//!   Only then are the pixels of the first frame (the `IDAT` image) decoded, with `tRNS` and low
//!   bit depths expanded, and `opaque` is every alpha at its maximum (255, or 65535 at 16 bits).
//! - GIF: the size is the logical screen's; `has_alpha` is a transparent index on the first frame,
//!   and `opaque` holds when none of that frame's pixels uses it.
//! - WebP: the canvas size and the alpha flag; with alpha, the first frame is decoded and
//!   `opaque` is every alpha at 255.
//! - JPEG: the size from the first start-of-frame marker; never alpha.

mod ffv1;
pub mod matroska;
pub mod mp4;
pub mod wav;

use serde_json::{Map, Value};
use std::io::Cursor;

/// The image facts of one picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageFacts {
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub opaque: bool,
}

/// The file facts of `bytes` of the given kind, as an object. An image whose bytes do not decode
/// is refused with a sentence (`"<reason>"`); the caller prefixes the file name.
pub fn file_facts(bytes: &[u8], kind: &str) -> Result<Value, String> {
    let mut facts = Map::new();
    facts.insert("bytes".into(), Value::from(bytes.len() as u64));
    facts.insert("kind".into(), Value::from(kind));
    if let Some(image) = image_facts(bytes, kind)? {
        facts.insert("width".into(), Value::from(image.width));
        facts.insert("height".into(), Value::from(image.height));
        facts.insert("has_alpha".into(), Value::from(image.has_alpha));
        facts.insert("opaque".into(), Value::from(image.opaque));
    }
    if let Some(audio) = audio_facts(bytes, kind)? {
        audio.insert_into(&mut facts);
    }
    if let Some(video) = video_facts(bytes, kind)? {
        video.insert_into(&mut facts);
    }
    Ok(Value::Object(facts))
}

/// The image facts of an image kind's bytes; `Ok(None)` for kinds that are not images.
pub fn image_facts(bytes: &[u8], kind: &str) -> Result<Option<ImageFacts>, String> {
    match kind {
        "image/png" => png_facts(bytes).map(Some),
        "image/gif" => gif_facts(bytes).map(Some),
        "image/webp" => webp_facts(bytes).map(Some),
        "image/jpeg" => jpeg_facts(bytes).map(Some),
        _ => Ok(None),
    }
}

/// The audio facts of an audio kind's bytes (spec/facts.md); `Ok(None)` for other kinds, and for
/// audio FX computes no facts of (`audio/mpeg`, `audio/ogg`).
pub fn audio_facts(bytes: &[u8], kind: &str) -> Result<Option<wav::WavFacts>, String> {
    match kind {
        "audio/wav" => wav::wav_facts(bytes),
        _ => Ok(None),
    }
}

/// The video facts of a video kind's bytes (spec/facts.md); `Ok(None)` for other kinds.
pub fn video_facts(bytes: &[u8], kind: &str) -> Result<Option<VideoFacts>, String> {
    match kind {
        "video/mp4" => mp4::mp4_facts(bytes),
        "video/webm" | "video/x-matroska" => matroska::matroska_facts(bytes),
        _ => Ok(None),
    }
}

/// The facts of a video's first video track (spec/facts.md). A member the container does not
/// give is `None` and left out of `facts(f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoFacts {
    pub width: u32,
    pub height: u32,
    /// Frames per second, rounded to 6 decimal places.
    pub fps: Option<f64>,
    /// Seconds, rounded to 6 decimal places.
    pub duration: Option<f64>,
    pub frames: Option<u64>,
    pub has_alpha: Option<bool>,
}

impl VideoFacts {
    /// Adds `width`, `height`, `fps`, `duration`, `frames`, `has_alpha`, in that order; a member
    /// that is `None` is left out.
    pub fn insert_into(&self, facts: &mut Map<String, Value>) {
        facts.insert("width".into(), Value::from(self.width));
        facts.insert("height".into(), Value::from(self.height));
        let numbers = [
            ("fps", self.fps),
            ("duration", self.duration),
            ("frames", self.frames.map(|frames| frames as f64)),
        ];
        for (name, number) in numbers {
            if let Some(Ok(value)) = number.map(crate::value::number) {
                facts.insert(name.into(), value);
            }
        }
        if let Some(has_alpha) = self.has_alpha {
            facts.insert("has_alpha".into(), Value::from(has_alpha));
        }
    }
}

/// `x` rounded to 6 decimal places, ties to even on its exact binary value (spec/facts.md §1):
/// the shortest decimal of 6 places, read back.
pub(crate) fn round6(x: f64) -> f64 {
    format!("{x:.6}").parse().unwrap_or(x)
}

/// `units` of `1 / scale` seconds: `units` converted to binary64, divided by `scale`
/// (spec/facts.md §1). `scale` is never 0.
pub(crate) fn seconds(units: u128, scale: u64) -> f64 {
    units as f64 / scale as f64
}

/// The image `has_alpha` (§2) of a video frame coded as a PNG picture: a color type with alpha
/// or a `tRNS` chunk before the image data; `None` when its header does not decode.
pub(crate) fn png_has_alpha(frame: &[u8]) -> Option<bool> {
    let reader = png::Decoder::new(Cursor::new(frame)).read_info().ok()?;
    let info = reader.info();
    Some(
        info.trns.is_some()
            || matches!(
                info.color_type,
                png::ColorType::GrayscaleAlpha | png::ColorType::Rgba
            ),
    )
}

fn png_facts(bytes: &[u8]) -> Result<ImageFacts, String> {
    let refused = |error: png::DecodingError| format!("not a PNG picture ({error})");
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(refused)?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    let has_alpha = info.trns.is_some()
        || matches!(
            info.color_type,
            png::ColorType::GrayscaleAlpha | png::ColorType::Rgba
        );
    if !has_alpha {
        return Ok(ImageFacts {
            width,
            height,
            has_alpha,
            opaque: true,
        });
    }
    let (color, depth) = reader.output_color_type();
    let channels = match color {
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgba => 4,
        // EXPAND turns every picture with transparency into one with an alpha channel.
        _ => return Err("not a PNG picture (its transparency did not expand)".into()),
    };
    let sample = if depth == png::BitDepth::Sixteen {
        2
    } else {
        1
    };
    let pixel = channels * sample;
    let mut opaque = true;
    // Rows one at a time (every pass of an interlaced picture covers each pixel once), so a large
    // picture never needs a whole-frame buffer. Every row is decoded, so a picture whose pixel
    // data is cut short is refused whatever its first pixels hold.
    while let Some(row) = reader.next_row().map_err(refused)? {
        let data = row.data();
        if data.len() % pixel != 0 {
            return Err("not a PNG picture (a row has a partial pixel)".into());
        }
        opaque = opaque
            && data
                .chunks_exact(pixel)
                .all(|p| p[pixel - sample..].iter().all(|&b| b == u8::MAX));
    }
    Ok(ImageFacts {
        width,
        height,
        has_alpha,
        opaque,
    })
}

fn gif_facts(bytes: &[u8]) -> Result<ImageFacts, String> {
    let refused = |error: gif::DecodingError| format!("not a GIF picture ({error})");
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::Indexed);
    let mut decoder = options.read_info(Cursor::new(bytes)).map_err(refused)?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    let frame = decoder
        .read_next_frame()
        .map_err(refused)?
        .ok_or("not a GIF picture (it has no frames)")?;
    let (has_alpha, opaque) = match frame.transparent {
        Some(index) => (true, !frame.buffer.contains(&index)),
        None => (false, true),
    };
    Ok(ImageFacts {
        width,
        height,
        has_alpha,
        opaque,
    })
}

fn webp_facts(bytes: &[u8]) -> Result<ImageFacts, String> {
    let refused = |error: image_webp::DecodingError| format!("not a WebP picture ({error})");
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(refused)?;
    let (width, height) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();
    if !has_alpha {
        return Ok(ImageFacts {
            width,
            height,
            has_alpha,
            opaque: true,
        });
    }
    let size = decoder
        .output_buffer_size()
        .ok_or("not a WebP picture (it is too large to decode)")?;
    let mut pixels = vec![0u8; size];
    decoder.read_image(&mut pixels).map_err(refused)?;
    let opaque = pixels.chunks_exact(4).all(|p| p[3] == u8::MAX);
    Ok(ImageFacts {
        width,
        height,
        has_alpha,
        opaque,
    })
}

fn jpeg_facts(bytes: &[u8]) -> Result<ImageFacts, String> {
    let (width, height) =
        jpeg_size(bytes).map_err(|reason| format!("not a JPEG picture ({reason})"))?;
    Ok(ImageFacts {
        width,
        height,
        has_alpha: false,
        opaque: true,
    })
}

/// The width and height in a JPEG's first start-of-frame segment (`SOF0`–`SOF15` but `DHT`,
/// `JPG` and `DAC`), found by walking the marker segments from the start-of-image marker.
fn jpeg_size(bytes: &[u8]) -> Result<(u32, u32), &'static str> {
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err("no start-of-image marker");
    }
    let mut at = 2;
    loop {
        // Markers may be preceded by any number of 0xFF fill bytes.
        if bytes.get(at) != Some(&0xFF) {
            return Err("a marker is missing");
        }
        while bytes.get(at) == Some(&0xFF) {
            at += 1;
        }
        let marker = *bytes.get(at).ok_or("it ends before its frame header")?;
        at += 1;
        match marker {
            // Markers that stand alone: TEM, RST0–RST7, SOI.
            0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return Err("it has no frame header"),
            _ => {}
        }
        let length = match bytes.get(at..at + 2) {
            Some(&[high, low]) => usize::from(u16::from_be_bytes([high, low])),
            _ => return Err("it ends before its frame header"),
        };
        if length < 2 {
            return Err("a segment has a bad length");
        }
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            // length (2), precision (1), height (2), width (2)
            let segment = bytes
                .get(at..at + 7)
                .ok_or("it ends inside its frame header")?;
            if length < 7 {
                return Err("its frame header is too short");
            }
            let height = u32::from(u16::from_be_bytes([segment[3], segment[4]]));
            let width = u32::from(u16::from_be_bytes([segment[5], segment[6]]));
            if width == 0 || height == 0 {
                return Err("its frame header has no size");
            }
            return Ok((width, height));
        }
        at += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png_bytes(
        width: u32,
        height: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        trns: Option<&[u8]>,
        palette: Option<&[u8]>,
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(color);
            encoder.set_depth(depth);
            if let Some(palette) = palette {
                encoder.set_palette(palette.to_vec());
            }
            if let Some(trns) = trns {
                encoder.set_trns(trns.to_vec());
            }
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        out
    }

    fn facts(bytes: &[u8], kind: &str) -> ImageFacts {
        image_facts(bytes, kind).unwrap().unwrap()
    }

    #[test]
    fn video_facts_are_written_in_order_and_as_fx_numbers() {
        let mut facts = Map::new();
        VideoFacts {
            width: 64,
            height: 48,
            fps: Some(24.0),
            duration: Some(1.001),
            frames: Some(30),
            has_alpha: Some(false),
        }
        .insert_into(&mut facts);
        let keys: Vec<_> = facts.keys().cloned().collect();
        assert_eq!(
            keys,
            ["width", "height", "fps", "duration", "frames", "has_alpha"]
        );
        assert_eq!(facts["fps"].as_u64(), Some(24));
        assert_eq!(facts["duration"].as_f64(), Some(1.001));
        assert_eq!(facts["frames"].as_u64(), Some(30));
        let mut sparse = Map::new();
        VideoFacts {
            width: 1,
            height: 2,
            fps: None,
            duration: None,
            frames: Some(0),
            has_alpha: None,
        }
        .insert_into(&mut sparse);
        assert_eq!(
            Value::Object(sparse),
            json!({"width": 1, "height": 2, "frames": 0})
        );
    }

    #[test]
    fn rounding_to_six_places_ties_to_even() {
        assert_eq!(round6(1.0 / 128.0), 0.007812);
        assert_eq!(round6(3.0 / 128.0), 0.023438);
        assert_eq!(round6(1.0 / 44100.0), 0.000023);
        assert_eq!(round6(1.0 / 3.0), 0.333333);
        assert_eq!(round6(2.0), 2.0);
        assert_eq!(seconds(30030, 30000), 1.001);
    }

    #[test]
    fn audio_and_video_kinds_dispatch() {
        assert_eq!(
            file_facts(b"", "audio/wav").unwrap(),
            json!({"bytes": 0, "kind": "audio/wav"})
        );
        // Other audio kinds have no facts of their own; neither has a video kind FX does not read.
        assert_eq!(
            file_facts(b"ID3", "audio/mpeg").unwrap(),
            json!({"bytes": 3, "kind": "audio/mpeg"})
        );
        assert_eq!(
            file_facts(b"x", "file").unwrap(),
            json!({"bytes": 1, "kind": "file"})
        );
        assert_eq!(
            file_facts(b"not a video", "video/mp4").unwrap_err(),
            "not an MP4 file (a box at byte 0 runs past the end of the file)"
        );
        for kind in ["video/webm", "video/x-matroska"] {
            assert_eq!(
                file_facts(b"not a video", kind).unwrap_err(),
                "not a Matroska or WebM file (it does not start with an EBML header)"
            );
        }
    }

    #[test]
    fn non_images_have_size_and_kind() {
        assert_eq!(
            file_facts(b"hello\n", "text/plain").unwrap(),
            json!({"bytes": 6, "kind": "text/plain"})
        );
        assert_eq!(image_facts(b"x", "image").unwrap(), None);
        assert_eq!(image_facts(b"x", "audio/wav").unwrap(), None);
    }

    #[test]
    fn keys_are_in_order() {
        let png = png_bytes(
            1,
            1,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            None,
            None,
            &[1, 2, 3],
        );
        let value = file_facts(&png, "image/png").unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["bytes", "kind", "width", "height", "has_alpha", "opaque"]
        );
    }

    #[test]
    fn png_alpha_channels() {
        let rgba = |alpha: u8| {
            png_bytes(
                2,
                1,
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                None,
                None,
                &[0, 0, 0, 255, 9, 9, 9, alpha],
            )
        };
        let opaque = ImageFacts {
            width: 2,
            height: 1,
            has_alpha: true,
            opaque: true,
        };
        assert_eq!(facts(&rgba(255), "image/png"), opaque);
        assert_eq!(
            facts(&rgba(254), "image/png"),
            ImageFacts {
                opaque: false,
                ..opaque
            }
        );
        let gray_alpha = png_bytes(
            1,
            2,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            None,
            None,
            &[10, 255, 20, 0],
        );
        assert_eq!(
            facts(&gray_alpha, "image/png"),
            ImageFacts {
                width: 1,
                height: 2,
                has_alpha: true,
                opaque: false
            }
        );
        // 16 bits: 0xFFFE is not fully opaque.
        let deep = |alpha: [u8; 2]| {
            png_bytes(
                1,
                1,
                png::ColorType::GrayscaleAlpha,
                png::BitDepth::Sixteen,
                None,
                None,
                &[0, 0, alpha[0], alpha[1]],
            )
        };
        assert!(facts(&deep([0xFF, 0xFF]), "image/png").opaque);
        assert!(!facts(&deep([0xFF, 0xFE]), "image/png").opaque);
    }

    #[test]
    fn png_without_alpha() {
        let rgb = png_bytes(
            3,
            2,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            None,
            None,
            &[0; 18],
        );
        assert_eq!(
            facts(&rgb, "image/png"),
            ImageFacts {
                width: 3,
                height: 2,
                has_alpha: false,
                opaque: true
            }
        );
    }

    #[test]
    fn png_transparency_chunks() {
        // A palette whose second entry is half transparent.
        let palette = [0, 0, 0, 255, 255, 255];
        let trns = [255, 128];
        let uses_both = png_bytes(
            2,
            1,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            Some(&trns),
            Some(&palette),
            &[0, 1],
        );
        let only_first = png_bytes(
            2,
            1,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            Some(&trns),
            Some(&palette),
            &[0, 0],
        );
        assert_eq!(
            facts(&uses_both, "image/png"),
            ImageFacts {
                width: 2,
                height: 1,
                has_alpha: true,
                opaque: false
            }
        );
        assert_eq!(
            facts(&only_first, "image/png"),
            ImageFacts {
                width: 2,
                height: 1,
                has_alpha: true,
                opaque: true
            }
        );
        // A palette with no tRNS has no alpha.
        let plain = png_bytes(
            2,
            1,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            None,
            Some(&palette),
            &[0, 1],
        );
        assert!(!facts(&plain, "image/png").has_alpha);
        // A grayscale tRNS names one transparent gray (16-bit sample in the chunk).
        let gray = png_bytes(
            2,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            Some(&[0, 7]),
            None,
            &[7, 8],
        );
        assert_eq!(
            facts(&gray, "image/png"),
            ImageFacts {
                width: 2,
                height: 1,
                has_alpha: true,
                opaque: false
            }
        );
        // A 1-bit grayscale with tRNS expands to 8 bits with alpha.
        let bits = png_bytes(
            8,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::One,
            Some(&[0, 1]),
            None,
            &[0b0000_0000],
        );
        assert_eq!(
            facts(&bits, "image/png"),
            ImageFacts {
                width: 8,
                height: 1,
                has_alpha: true,
                opaque: true
            }
        );
    }

    #[test]
    fn broken_pngs_are_refused() {
        let error = image_facts(b"not a png", "image/png").unwrap_err();
        assert!(error.starts_with("not a PNG picture ("), "{error}");
        let mut truncated = png_bytes(
            4,
            4,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            None,
            None,
            &[7; 64],
        );
        truncated.truncate(truncated.len() - 20);
        assert!(image_facts(&truncated, "image/png").is_err());
        let mut opaque = png_bytes(
            4,
            4,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            None,
            None,
            &[255; 64],
        );
        opaque.truncate(opaque.len() - 20);
        assert!(image_facts(&opaque, "image/png").is_err());
    }

    fn gif_bytes(transparent: Option<u8>, pixels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let palette = [0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 255, 0];
            let mut encoder = gif::Encoder::new(&mut out, 4, 3, &palette).unwrap();
            let mut frame = gif::Frame {
                left: 1,
                top: 1,
                width: 2,
                height: 2,
                transparent,
                buffer: std::borrow::Cow::Borrowed(pixels),
                ..gif::Frame::default()
            };
            frame.delay = 0;
            encoder.write_frame(&frame).unwrap();
            // A second frame that would make the picture transparent is never read.
            let second = gif::Frame {
                width: 4,
                height: 3,
                transparent: Some(0),
                buffer: std::borrow::Cow::Owned(vec![0; 12]),
                ..gif::Frame::default()
            };
            encoder.write_frame(&second).unwrap();
        }
        out
    }

    #[test]
    fn gif_first_frame() {
        assert_eq!(
            facts(&gif_bytes(None, &[0, 1, 2, 3]), "image/gif"),
            ImageFacts {
                width: 4,
                height: 3,
                has_alpha: false,
                opaque: true
            }
        );
        assert_eq!(
            facts(&gif_bytes(Some(3), &[0, 1, 2, 3]), "image/gif"),
            ImageFacts {
                width: 4,
                height: 3,
                has_alpha: true,
                opaque: false
            }
        );
        assert_eq!(
            facts(&gif_bytes(Some(3), &[0, 1, 2, 2]), "image/gif"),
            ImageFacts {
                width: 4,
                height: 3,
                has_alpha: true,
                opaque: true
            }
        );
        let error = image_facts(b"GIF89a", "image/gif").unwrap_err();
        assert!(error.starts_with("not a GIF picture ("), "{error}");
    }

    /// A lossless WebP (`VP8L`) of 1×1 whose single pixel is ARGB `0xAARRGGBB` with every
    /// channel coded as a one-symbol prefix code, so the bit stream needs no entropy data.
    fn webp_lossless(alpha_hint: bool, argb: u32) -> Vec<u8> {
        let mut bits = BitWriter::default();
        bits.put(0, 14); // width - 1
        bits.put(0, 14); // height - 1
        bits.put(u32::from(alpha_hint), 1);
        bits.put(0, 3); // version
        bits.put(0, 1); // no transform
        bits.put(0, 1); // no color cache
        bits.put(0, 1); // no meta prefix codes
        // Five prefix codes: green (+ length codes), red, blue, alpha, distance. Each is a simple
        // code with one symbol of 8 bits.
        let [a, r, g, b] = argb.to_be_bytes();
        for symbol in [g, r, b, a, 0] {
            bits.put(1, 1); // simple code
            bits.put(0, 1); // one symbol
            bits.put(1, 1); // the symbol takes 8 bits
            bits.put(u32::from(symbol), 8);
        }
        let mut payload = vec![0x2F];
        payload.extend(bits.finish());
        riff(b"VP8L", &payload)
    }

    fn riff(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut chunk = fourcc.to_vec();
        chunk.extend((payload.len() as u32).to_le_bytes());
        chunk.extend(payload);
        if payload.len() % 2 == 1 {
            chunk.push(0);
        }
        let mut out = b"RIFF".to_vec();
        out.extend(((chunk.len() + 4) as u32).to_le_bytes());
        out.extend(b"WEBP");
        out.extend(chunk);
        out
    }

    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        used: u32,
    }

    impl BitWriter {
        /// Writes `count` bits of `value`, least significant first (the VP8L bit order).
        fn put(&mut self, value: u32, count: u32) {
            for i in 0..count {
                if self.used.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let bit = ((value >> i) & 1) as u8;
                let last = self.bytes.len() - 1;
                self.bytes[last] |= bit << (self.used % 8);
                self.used += 1;
            }
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    #[test]
    fn webp_alpha() {
        assert_eq!(
            facts(&webp_lossless(true, 0xFF10_2030), "image/webp"),
            ImageFacts {
                width: 1,
                height: 1,
                has_alpha: true,
                opaque: true
            }
        );
        assert_eq!(
            facts(&webp_lossless(true, 0x8010_2030), "image/webp"),
            ImageFacts {
                width: 1,
                height: 1,
                has_alpha: true,
                opaque: false
            }
        );
        assert_eq!(
            facts(&webp_lossless(false, 0x8010_2030), "image/webp"),
            ImageFacts {
                width: 1,
                height: 1,
                has_alpha: false,
                opaque: true
            }
        );
        let error = image_facts(b"RIFF\0\0\0\0WEBP", "image/webp").unwrap_err();
        assert!(error.starts_with("not a WebP picture ("), "{error}");
    }

    /// A JPEG's marker skeleton: SOI, an APP0 segment, fill bytes, then a baseline SOF0.
    fn jpeg_bytes(marker: u8, width: u16, height: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        out.extend([0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F']);
        out.extend([0xFF, 0xFF]);
        out.extend([marker, 0x00, 0x0B, 8]);
        out.extend(height.to_be_bytes());
        out.extend(width.to_be_bytes());
        out.extend([1, 1, 0x11, 0]);
        out.extend([0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn jpeg_size_from_the_frame_header() {
        assert_eq!(
            facts(&jpeg_bytes(0xC0, 640, 480), "image/jpeg"),
            ImageFacts {
                width: 640,
                height: 480,
                has_alpha: false,
                opaque: true
            }
        );
        // Progressive frames count; DHT (0xC4) is not a frame header.
        assert_eq!(facts(&jpeg_bytes(0xC2, 3, 5), "image/jpeg").width, 3);
        assert!(image_facts(&jpeg_bytes(0xC4, 3, 5), "image/jpeg").is_err());
        assert_eq!(
            image_facts(&jpeg_bytes(0xC0, 0, 5), "image/jpeg").unwrap_err(),
            "not a JPEG picture (its frame header has no size)"
        );
        assert_eq!(
            image_facts(b"\xFF\xD8\xFF\xDA\x00\x02", "image/jpeg").unwrap_err(),
            "not a JPEG picture (it has no frame header)"
        );
        assert_eq!(
            image_facts(b"GIF89a", "image/jpeg").unwrap_err(),
            "not a JPEG picture (no start-of-image marker)"
        );
        let mut short = jpeg_bytes(0xC0, 9, 9);
        short.truncate(16);
        assert!(image_facts(&short, "image/jpeg").is_err());
    }
}
