//! `multipart/form-data` encoding (RFC 7578) for [`super::Body::Multipart`].
//!
//! - [`boundary`]: `----grida-fx-` followed by 32 lowercase hex characters derived from the
//!   parts' bytes: the first 16 bytes of SHA-256 over each part's name, file name, content type
//!   and data in order, each prefixed by its length as a little-endian `u64` (an absent file name
//!   or content type counts as empty). The same parts always encode to the same bytes. A boundary
//!   never occurs inside a part: if it would, the digest is taken again over the previous digest
//!   until it does not.
//! - [`encode`]: for each part, `--<boundary>\r\n`, then
//!   `Content-Disposition: form-data; name="<name>"` plus `; filename="<filename>"` when the part
//!   has a file name, `\r\n`, then `Content-Type: <type>\r\n` when it has a content type (a file
//!   part has both, a text field neither), `\r\n`, the data, `\r\n`; finally `--<boundary>--\r\n`.
//!   Names and file names are quoted with `"` and `\` escaped by a `\`, and CR or LF in a name,
//!   a file name or a content type becomes a space, so no part header can start a new line.
//! - [`content_type`]: `multipart/form-data; boundary=<boundary>`.

use super::Part;
use sha2::{Digest, Sha256};

/// What every boundary starts with.
const PREFIX: &str = "----grida-fx-";

/// The boundary for `parts` (module doc).
pub fn boundary(parts: &[Part]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        for field in [
            part.name.as_bytes(),
            part.filename.as_deref().unwrap_or("").as_bytes(),
            part.content_type.as_deref().unwrap_or("").as_bytes(),
            part.data.as_slice(),
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
    }
    first_free(hasher.finalize().into(), parts)
}

/// The boundary of `digest`, rehashing the digest until its boundary occurs in no part.
fn first_free(mut digest: [u8; 32], parts: &[Part]) -> String {
    loop {
        let candidate = boundary_of(&digest);
        if !parts.iter().any(|part| holds(part, candidate.as_bytes())) {
            return candidate;
        }
        digest = Sha256::digest(digest).into();
    }
}

/// `----grida-fx-` and the first 32 hex characters of `digest`.
fn boundary_of(digest: &[u8; 32]) -> String {
    let mut out = String::with_capacity(PREFIX.len() + 32);
    out.push_str(PREFIX);
    for byte in &digest[..16] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Whether `needle` occurs anywhere in the part's bytes (its data or its header fields).
fn holds(part: &Part, needle: &[u8]) -> bool {
    let fields = [
        part.name.as_bytes(),
        part.filename.as_deref().unwrap_or("").as_bytes(),
        part.content_type.as_deref().unwrap_or("").as_bytes(),
        part.data.as_slice(),
    ];
    fields
        .iter()
        .any(|field| field.windows(needle.len()).any(|window| window == needle))
}

/// The body bytes (module doc).
pub fn encode(parts: &[Part], boundary: &str) -> Vec<u8> {
    let size: usize = parts.iter().map(|part| part.data.len() + 160).sum();
    let mut out = Vec::with_capacity(size + boundary.len() * (parts.len() + 1) + 8);
    for part in parts {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"");
        out.extend_from_slice(quoted(&part.name).as_bytes());
        out.push(b'"');
        if let Some(filename) = &part.filename {
            out.extend_from_slice(b"; filename=\"");
            out.extend_from_slice(quoted(filename).as_bytes());
            out.push(b'"');
        }
        out.extend_from_slice(b"\r\n");
        if let Some(content_type) = &part.content_type {
            out.extend_from_slice(b"Content-Type: ");
            out.extend_from_slice(one_line(content_type).as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&part.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--\r\n");
    out
}

/// The `content-type` header value.
pub fn content_type(boundary: &str) -> String {
    format!("multipart/form-data; boundary={boundary}")
}

/// A quoted-string's inside: `"` and `\` escaped, CR and LF made spaces.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' | '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// A header value on one line: CR and LF made spaces.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> Vec<Part> {
        vec![
            Part::text("model", "img-a"),
            Part::file(
                "image[]",
                "a.png",
                "image/png",
                b"\x89PNG\r\n\x1a\nDATA".to_vec(),
            ),
        ]
    }

    #[test]
    fn a_boundary_is_the_prefix_and_32_hex_characters() {
        let b = boundary(&parts());
        assert!(b.starts_with("----grida-fx-"), "{b}");
        let hex = &b["----grida-fx-".len()..];
        assert_eq!(hex.len(), 32);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }

    #[test]
    fn the_boundary_digest_is_length_prefixed_and_ordered() {
        // The digest by hand: each field's length as a little-endian u64, then its bytes.
        let mut hasher = Sha256::new();
        for field in [
            &b"model"[..],
            b"",
            b"",
            b"img-a",
            b"image[]",
            b"a.png",
            b"image/png",
            b"\x89PNG\r\n\x1a\nDATA",
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
        let digest: [u8; 32] = hasher.finalize().into();
        assert_eq!(boundary(&parts()), boundary_of(&digest));
        // Moving bytes between fields changes the boundary.
        let shifted = vec![
            Part::text("modeli", "mg-a"),
            parts().pop().expect("two parts"),
        ];
        assert_ne!(boundary(&shifted), boundary(&parts()));
        let swapped: Vec<Part> = parts().into_iter().rev().collect();
        assert_ne!(boundary(&swapped), boundary(&parts()));
    }

    #[test]
    fn a_colliding_boundary_is_rehashed() {
        let digest: [u8; 32] = Sha256::digest(b"seed").into();
        let first = boundary_of(&digest);
        let collide = vec![Part::file(
            "file",
            "a.bin",
            "application/octet-stream",
            format!("xx\r\n--{first}\r\nyy").into_bytes(),
        )];
        let picked = first_free(digest, &collide);
        let rehashed: [u8; 32] = Sha256::digest(digest).into();
        assert_eq!(picked, boundary_of(&rehashed));
        assert_ne!(picked, first);
        assert!(!holds(&collide[0], picked.as_bytes()));
        // Twice in a row: both candidates occur, the third is taken.
        let second = boundary_of(&rehashed);
        let both = vec![Part::text("a", &first), Part::text("b", &second)];
        let third: [u8; 32] = Sha256::digest(rehashed).into();
        assert_eq!(first_free(digest, &both), boundary_of(&third));
        // No collision: the first candidate.
        assert_eq!(first_free(digest, &parts()), first);
    }
}
