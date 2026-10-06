//! Reading a file for its facts without holding the whole file (spec/facts.md §1).
//!
//! The readers ask a [`Source`] for the byte ranges their rules name, a few bytes at a time:
//! box and element headers, the fields they read, the samples they look at. A source keeps one
//! window of the file (64 KiB, or the range asked for when that is longer), so walking a table
//! or a run of headers reads the file in large pieces, and the bytes a rule never names (an
//! `mdat`, the frames of a `Cluster`) are never read. Every range a reader asks for lies within
//! a size it has already checked against the file's length.

use std::io::{self, BufRead, Read, Seek, SeekFrom};

/// The size of a source's window.
const WINDOW: usize = 64 * 1024;

/// Why a reader stopped: the file does not decode under its kind's rule (the reason, without
/// the kind's prefix), or reading the file failed.
#[derive(Debug)]
pub(crate) enum Fail {
    Refused(String),
    Io(io::Error),
}

impl From<String> for Fail {
    fn from(reason: String) -> Self {
        Fail::Refused(reason)
    }
}

impl From<&str> for Fail {
    fn from(reason: &str) -> Self {
        Fail::Refused(reason.to_string())
    }
}

impl From<io::Error> for Fail {
    fn from(error: io::Error) -> Self {
        Fail::Io(error)
    }
}

/// A file read through one window.
pub(crate) struct Source<R> {
    inner: R,
    len: u64,
    window: Vec<u8>,
    window_at: u64,
    /// The bytes read from the file so far.
    pub(crate) read: u64,
    /// The first failure to read the file, kept for a decoder that reports it as its own error.
    failure: Option<io::Error>,
}

impl<R: Read + Seek> Source<R> {
    pub(crate) fn new(mut inner: R) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        Ok(Source {
            inner,
            len,
            window: Vec::new(),
            window_at: 0,
            read: 0,
            failure: None,
        })
    }

    /// The file's size in bytes.
    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    /// The `count` bytes at `at`. The range must lie within the file: a reader checks every size
    /// before it reads, so a range past the end means the file changed while it was read.
    pub(crate) fn bytes(&mut self, at: u64, count: usize) -> io::Result<&[u8]> {
        let Some(end) = at.checked_add(count as u64).filter(|&end| end <= self.len) else {
            return Err(self.failed(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the file got shorter while its facts were read",
            )));
        };
        let held = self.window_at + self.window.len() as u64;
        if at < self.window_at || end > held {
            // `self.len - at` is at least `count`, so the window holds the range.
            let size = count.max(WINDOW).min((self.len - at) as usize);
            self.window.clear();
            self.window.resize(size, 0);
            self.window_at = at;
            let read = self
                .inner
                .seek(SeekFrom::Start(at))
                .and_then(|_| self.inner.read_exact(&mut self.window));
            if let Err(error) = read {
                self.window.clear();
                return Err(self.failed(error));
            }
            self.read += size as u64;
        }
        let start = (at - self.window_at) as usize;
        Ok(&self.window[start..start + count])
    }

    /// Keeps the first failure to read the file, and returns it.
    fn failed(&mut self, error: io::Error) -> io::Error {
        let copy = io::Error::new(error.kind(), error.to_string());
        self.failure.get_or_insert(error);
        copy
    }

    /// The first failure to read the file, if any, once.
    pub(crate) fn take_failure(&mut self) -> Option<io::Error> {
        self.failure.take()
    }

    /// The bytes from `at` to the end of the window that holds it, at most `most` of them; a new
    /// window when none holds it. `at` must lie within the file and `most` be above 0.
    fn chunk(&mut self, at: u64, most: usize) -> io::Result<&[u8]> {
        let held = self.window_at + self.window.len() as u64;
        if at >= self.window_at && at < held {
            let start = (at - self.window_at) as usize;
            let count = most.min(self.window.len() - start);
            return Ok(&self.window[start..start + count]);
        }
        let count = most.min(WINDOW).min((self.len - at) as usize);
        self.bytes(at, count)
    }

    /// The byte at `at`, which must lie within the file.
    pub(crate) fn byte(&mut self, at: u64) -> io::Result<u8> {
        Ok(self.bytes(at, 1)?[0])
    }

    /// The bytes `start..end` of the file as a reader of their own, for a decoder that reads a
    /// picture embedded in the file (a PNG frame of a video).
    pub(crate) fn range(&mut self, start: u64, end: u64) -> Range<'_, R> {
        Range {
            source: self,
            start,
            end,
            at: start,
        }
    }
}

/// A range of a [`Source`] read as a file of its own: offsets are relative to its start, and its
/// end is the end of the range.
pub(crate) struct Range<'s, R> {
    source: &'s mut Source<R>,
    start: u64,
    end: u64,
    at: u64,
}

impl<R: Read + Seek> Read for Range<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl<R: Read + Seek> BufRead for Range<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let left = self.end.saturating_sub(self.at);
        if left == 0 {
            return Ok(&[]);
        }
        // What the window holds from here on, so that small reads do not move it.
        self.source.chunk(self.at, left.min(WINDOW as u64) as usize)
    }

    fn consume(&mut self, amount: usize) {
        self.at = (self.at + amount as u64).min(self.end);
    }
}

impl<R: Read + Seek> Seek for Range<'_, R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let len = self.end - self.start;
        let relative = self.at - self.start;
        let target = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => len.checked_add_signed(delta),
            SeekFrom::Current(delta) => relative.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"))?;
        self.at = self.start.saturating_add(target);
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn ranges_are_served_from_one_window() {
        let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let mut source = Source::new(Cursor::new(&data)).unwrap();
        assert_eq!(source.len(), 200_000);
        assert_eq!(source.bytes(10, 3).unwrap(), &[10, 11, 12]);
        assert_eq!(source.read, WINDOW as u64);
        // Within the window: nothing more is read.
        assert_eq!(source.bytes(60_000, 2).unwrap(), &[96, 97]);
        assert_eq!(source.read, WINDOW as u64);
        // Past it: a new window, cut at the end of the file.
        assert_eq!(source.byte(199_999).unwrap(), 199_999u32 as u8);
        assert_eq!(source.read, WINDOW as u64 + 1);
        // A range longer than a window is read whole.
        assert_eq!(source.bytes(0, 100_000).unwrap().len(), 100_000);
        // Past the end is an error, never a panic.
        assert!(source.bytes(199_999, 2).is_err());
        assert!(source.bytes(u64::MAX, 2).is_err());
    }

    #[test]
    fn small_reads_through_a_range_keep_the_window() {
        let data = vec![7u8; 300_000];
        let mut source = Source::new(Cursor::new(&data)).unwrap();
        let mut range = source.range(0, 300_000);
        let mut byte = [0u8; 1];
        for _ in 0..300_000 {
            range.read_exact(&mut byte).unwrap();
        }
        assert_eq!(range.read(&mut byte).unwrap(), 0);
        assert_eq!(source.read, 300_000);
    }

    #[test]
    fn a_range_reads_as_a_file() {
        let data: Vec<u8> = (0..100u8).collect();
        let mut source = Source::new(Cursor::new(&data)).unwrap();
        let mut range = source.range(10, 20);
        let mut all = Vec::new();
        range.read_to_end(&mut all).unwrap();
        assert_eq!(all, (10..20).collect::<Vec<u8>>());
        assert_eq!(range.seek(SeekFrom::Start(2)).unwrap(), 2);
        let mut two = [0; 2];
        range.read_exact(&mut two).unwrap();
        assert_eq!(two, [12, 13]);
        assert_eq!(range.seek(SeekFrom::End(-1)).unwrap(), 9);
        assert_eq!(range.seek(SeekFrom::Current(-3)).unwrap(), 6);
        assert!(range.seek(SeekFrom::Current(-7)).is_err());
    }
}
