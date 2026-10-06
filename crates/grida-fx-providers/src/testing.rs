//! Helpers for adapter tests (feature `testing`): a [`Setup`] over a test transport, a call
//! builder that puts request files in a temporary store, synthetic media, and `block_on`.
//! Everything here is synthetic: made-up keys, media built in code.

use crate::adapter::{CallRequest, RequestFile, RouteRef};
use crate::clock::{Clock, FakeClock};
use crate::keys::{KeyName, Keys};
use crate::setup::{Endpoints, Setup};
use crate::transport::Transport;
use grida_fx_core::routes::parse_route_id;
use indexmap::IndexMap;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Made-up keys, one per provider: `test-<provider>-key`.
pub fn test_keys() -> Keys {
    Keys::from_pairs(&[
        (KeyName::OpenAi, "test-openai-key"),
        (KeyName::OpenRouter, "test-openrouter-key"),
        (KeyName::Fal, "test-fal-key"),
        (KeyName::Tripo, "test-tripo-key"),
        (KeyName::ElevenLabs, "test-elevenlabs-key"),
    ])
}

/// A setup over `transport` with `keys`, the default endpoints and a fresh [`FakeClock`].
pub fn setup(transport: Arc<dyn Transport>, keys: Keys) -> Setup {
    setup_with_clock(transport, keys, Arc::new(FakeClock::new()))
}

/// As [`setup`], with the given clock.
pub fn setup_with_clock(transport: Arc<dyn Transport>, keys: Keys, clock: Arc<dyn Clock>) -> Setup {
    Setup {
        transport,
        keys,
        endpoints: Endpoints::default(),
        clock,
    }
}

/// Runs a future to completion on a current-thread runtime with time enabled.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

/// A call under test, with the temporary folder its files live in.
#[derive(Debug)]
pub struct TestCall {
    pub call: CallRequest,
    _store: tempfile::TempDir,
}

impl std::ops::Deref for TestCall {
    type Target = CallRequest;
    fn deref(&self) -> &CallRequest {
        &self.call
    }
}

/// Builds a [`CallRequest`] (attempt 1, take `[1]`).
#[derive(Debug)]
pub struct CallBuilder {
    route: RouteRef,
    request: Value,
    files: IndexMap<String, RequestFile>,
    store: tempfile::TempDir,
}

impl CallBuilder {
    /// A call of `capability` on `route` (`model@provider`), with contract `{}` and request `{}`.
    pub fn new(capability: &str, route: &str) -> CallBuilder {
        let (model, provider) = parse_route_id(route).expect("a route id model@provider");
        CallBuilder {
            route: RouteRef {
                capability: capability.into(),
                model,
                provider,
                contract: json!({}),
            },
            request: json!({}),
            files: IndexMap::new(),
            store: tempfile::tempdir().expect("a temporary store"),
        }
    }

    pub fn contract(mut self, contract: Value) -> CallBuilder {
        self.route.contract = contract;
        self
    }

    pub fn request(mut self, request: Value) -> CallBuilder {
        self.request = request;
        self
    }

    /// Stores `bytes` as a file of `kind` and returns its file value `{"file": digest}`.
    pub fn file(&mut self, kind: &str, bytes: &[u8]) -> Value {
        let digest = format!("{:x}", Sha256::digest(bytes));
        let path = self.store.path().join(&digest);
        std::fs::write(&path, bytes).expect("a writable temporary store");
        self.files.insert(
            digest.clone(),
            RequestFile {
                digest: digest.clone(),
                kind: kind.into(),
                size: bytes.len() as u64,
                path,
            },
        );
        json!({"file": digest})
    }

    pub fn build(self) -> TestCall {
        TestCall {
            call: CallRequest {
                route: self.route,
                request: self.request,
                files: self.files,
                take: vec![1],
                key: "0".repeat(64),
                attempt: 1,
            },
            _store: self.store,
        }
    }
}

/// Synthetic media, built in code.
pub mod media {
    /// A PNG of `width` × `height`: RGBA with every pixel's alpha `alpha`, or RGB when `alpha` is
    /// `None`.
    pub fn png(width: u32, height: u32, alpha: Option<u8>) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(if alpha.is_some() {
                png::ColorType::Rgba
            } else {
                png::ColorType::Rgb
            });
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("a PNG header");
            let pixel: Vec<u8> = match alpha {
                Some(a) => vec![10, 20, 30, a],
                None => vec![10, 20, 30],
            };
            let data: Vec<u8> = pixel
                .iter()
                .copied()
                .cycle()
                .take(pixel.len() * (width * height) as usize)
                .collect();
            writer.write_image_data(&data).expect("PNG data");
        }
        bytes
    }

    /// An RGBA PNG whose first pixel has alpha `first` and every other pixel 255.
    pub fn png_one_pixel_alpha(width: u32, height: u32, first: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("a PNG header");
            let mut data: Vec<u8> = [10u8, 20, 30, 255]
                .iter()
                .copied()
                .cycle()
                .take(4 * (width * height) as usize)
                .collect();
            data[3] = first;
            writer.write_image_data(&data).expect("PNG data");
        }
        bytes
    }

    /// Bytes that start like a JPEG (not decodable).
    pub const JPEG_HEAD: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00";
    /// Bytes that start like an MP3 (an ID3 tag).
    pub const MP3: &[u8] = b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    /// Bytes that start like an MP3 frame.
    pub const MP3_FRAME: &[u8] = b"\xff\xfb\x90\x00\x00\x00\x00\x00";
    /// A WAV header.
    pub const WAV: &[u8] = b"RIFF\x24\x00\x00\x00WAVEfmt ";
    /// Bytes that start like an MP4.
    pub const MP4_HEAD: &[u8] = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00";
    /// Bytes that start like a GLB.
    pub const GLB: &[u8] = b"glTF\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    /// Bytes that start like a binary FBX.
    pub const FBX: &[u8] = b"Kaydara FBX Binary  \x00\x00\x00\x00";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_carry_their_files() {
        let mut builder = CallBuilder::new("image.edit", "img-a@acme");
        let image = builder.file("image/png", &media::png(2, 2, Some(255)));
        let call = builder
            .request(json!({"prompt": "p", "image": image}))
            .build();
        assert_eq!(call.route.provider, "acme");
        let file = crate::wire::request_file(&call, &call.request["image"], "image").unwrap();
        let bytes = crate::wire::read_file(file, "image").unwrap();
        assert!(crate::wire::matches_signature("image/png", &bytes));
        crate::wire::check_image("image/png", &bytes, Some((2, 2)), "opaque").unwrap();
        assert_eq!(
            crate::wire::check_image("image/png", &bytes, Some((4, 4)), "auto").unwrap_err(),
            "the image is 2x2, not 4x4"
        );
        let rgb = media::png(2, 2, None);
        assert_eq!(
            crate::wire::check_image("image/png", &rgb, None, "transparent").unwrap_err(),
            "the picture asked for as transparent has no alpha channel"
        );
        assert!(crate::wire::check_image("image/png", &rgb, None, "opaque").is_ok());
        let holed = media::png_one_pixel_alpha(2, 2, 254);
        assert_eq!(
            crate::wire::check_image("image/png", &holed, None, "opaque").unwrap_err(),
            "the picture asked for as opaque has transparent pixels"
        );
        assert_eq!(
            crate::wire::check_image("image/jpeg", media::JPEG_HEAD, None, "auto").unwrap_err(),
            "the answer is image/jpeg, not image/png"
        );
        assert_eq!(
            crate::wire::check_image("image/png", b"\x89PNG\r\n\x1a\ntruncated", None, "auto")
                .unwrap_err(),
            "the image data is not decodable"
        );
    }
}
