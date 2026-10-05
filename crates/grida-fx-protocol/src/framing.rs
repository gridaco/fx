//! LSP base-protocol framing (protocol.md §1 "Framing"): `Content-Length: <n>\r\n`, an empty
//! line `\r\n`, then n bytes of UTF-8 JSON. n counts bytes. A reader ignores other header fields
//! (case-insensitive names); a sender writes none.
//!
//! The reader is strict about the frame itself: every header line ends with `\r\n`, a header
//! field is `<name>: <value>`, and exactly one `Content-Length` holds ASCII digits. The body is
//! returned as bytes; the receiver checks that it is UTF-8 and I-JSON.

use std::io::{self, BufRead, Read, Write};

/// The longest header line a reader accepts, in bytes, `\r\n` included.
const MAX_HEADER_LINE: u64 = 8 * 1024;

/// Reads one frame's body. `Ok(None)` at a clean end of stream (no bytes of a next header).
///
/// Errors: `UnexpectedEof` when the stream ends inside a frame; `InvalidData` for a header line
/// without `\r\n`, a field without a colon, a missing, repeated or non-numeric `Content-Length`,
/// or a header line longer than 8 KiB.
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut length: Option<usize> = None;
    let mut first = true;
    loop {
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_HEADER_LINE)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            if first {
                return Ok(None);
            }
            return Err(eof("the stream ended inside a frame header"));
        }
        first = false;
        if line.last() != Some(&b'\n') {
            if read as u64 >= MAX_HEADER_LINE {
                return Err(invalid("a frame header line is longer than 8 KiB"));
            }
            return Err(eof("the stream ended inside a frame header"));
        }
        if line.len() < 2 || line[line.len() - 2] != b'\r' {
            return Err(invalid("a frame header line does not end with \\r\\n"));
        }
        let line = &line[..line.len() - 2];
        if line.is_empty() {
            break;
        }
        let line = std::str::from_utf8(line)
            .ok()
            .filter(|line| line.is_ascii())
            .ok_or_else(|| invalid("a frame header line is not ASCII text"))?;
        let Some((name, value)) = line.split_once(':') else {
            return Err(invalid(format!(
                "a frame header line has no colon: {line:?}"
            )));
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        if length.is_some() {
            return Err(invalid("a frame has more than one Content-Length"));
        }
        let value = value.trim_matches(|c| c == ' ' || c == '\t');
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid(format!(
                "a frame's Content-Length is not a number: {value:?}"
            )));
        }
        let n = value
            .parse::<usize>()
            .map_err(|_| invalid(format!("a frame's Content-Length is too large: {value}")))?;
        length = Some(n);
    }
    let Some(length) = length else {
        return Err(invalid("a frame has no Content-Length"));
    };
    // Read incrementally: a huge length must not allocate before the bytes arrive.
    let mut body = Vec::with_capacity(length.min(64 * 1024));
    reader.by_ref().take(length as u64).read_to_end(&mut body)?;
    if body.len() != length {
        return Err(eof(format!(
            "the stream ended after {} of a frame's {length} bytes",
            body.len()
        )));
    }
    Ok(Some(body))
}

/// Writes one frame and flushes.
pub fn write_message<W: Write>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut frame = Vec::with_capacity(header.len() + body.len());
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(body);
    writer.write_all(&frame)?;
    writer.flush()
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn eof(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_all(bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let mut reader = Cursor::new(bytes.to_vec());
        let mut bodies = Vec::new();
        while let Some(body) = read_message(&mut reader)? {
            bodies.push(body);
        }
        Ok(bodies)
    }

    #[test]
    fn round_trip() {
        let mut out = Vec::new();
        write_message(&mut out, br#"{"a":1}"#).unwrap();
        write_message(&mut out, "{\"t\":\"caf\u{e9}\"}".as_bytes()).unwrap();
        write_message(&mut out, b"").unwrap();
        assert!(out.starts_with(b"Content-Length: 7\r\n\r\n{\"a\":1}"));
        let bodies = read_all(&out).unwrap();
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[0], br#"{"a":1}"#);
        // n counts bytes, not characters: "é" is two bytes.
        assert_eq!(bodies[1].len(), 13);
        assert_eq!(bodies[1], "{\"t\":\"caf\u{e9}\"}".as_bytes());
        assert!(bodies[2].is_empty());
    }

    #[test]
    fn first_message_of_the_example() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"fx-node-protocol-v1","engine":{"name":"grida-fx","version":"0.1.0"},"project_root":"/work/acme","sources":[]}}"#;
        let mut out = Vec::new();
        write_message(&mut out, body).unwrap();
        let mut expected = b"Content-Length: 178\r\n\r\n".to_vec();
        expected.extend_from_slice(body);
        assert_eq!(out, expected);
    }

    #[test]
    fn other_headers_are_ignored() {
        let bytes = b"content-type: application/vscode-jsonrpc; charset=utf-8\r\n\
            CONTENT-LENGTH:  2 \r\nX-Other: 99\r\n\r\n{}";
        assert_eq!(read_all(bytes).unwrap(), vec![b"{}".to_vec()]);
    }

    #[test]
    fn clean_end_of_stream() {
        assert!(read_all(b"").unwrap().is_empty());
        let mut reader = Cursor::new(b"Content-Length: 2\r\n\r\n{}".to_vec());
        assert!(read_message(&mut reader).unwrap().is_some());
        assert!(read_message(&mut reader).unwrap().is_none());
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    fn error_of(bytes: &[u8]) -> io::Error {
        read_all(bytes).unwrap_err()
    }

    #[test]
    fn malformed_frames() {
        assert_eq!(
            error_of(b"\r\n{}").kind(),
            io::ErrorKind::InvalidData,
            "no Content-Length"
        );
        assert_eq!(
            error_of(b"X-Other: 1\r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: two\r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: -2\r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: +2\r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: \r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: 99999999999999999999999999\r\n\r\n").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            error_of(b"Content-Length: 2\n\n{}").kind(),
            io::ErrorKind::InvalidData,
            "bare newlines"
        );
        assert_eq!(
            error_of(b"Content-Length 2\r\n\r\n{}").kind(),
            io::ErrorKind::InvalidData,
            "no colon"
        );
        let long = format!("X: {}\r\n", "a".repeat(9000));
        assert_eq!(error_of(long.as_bytes()).kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn stream_ends_inside_a_frame() {
        assert_eq!(
            error_of(b"Content-Len").kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            error_of(b"Content-Length: 2\r\n").kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            error_of(b"Content-Length: 5\r\n\r\n{}").kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut reader = Cursor::new(b"Content-Length: 2\r\n\r\n{}Content".to_vec());
        assert!(read_message(&mut reader).unwrap().is_some());
        assert_eq!(
            read_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
