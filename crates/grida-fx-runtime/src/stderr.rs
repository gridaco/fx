//! The engine's stderr: the lines the engine prints there itself ([`line`]), the writes of the
//! copies of TypeScript hosts' stderr (`host::process`), and a forced exit's last word
//! ([`write_within`]).
//!
//! Every line the engine prints to stderr while a command runs goes through [`line`], never
//! `eprintln!`: whole, under the stderr lock the copies of hosts' stderr write under too, so the
//! two never splice. A stderr that a parent made non-blocking and that cannot take more now is
//! waited for until it can, as a blocking one is; any other failure (the reader has gone) drops
//! the line. Nothing here panics: `eprintln!` does on a stderr that cannot take more now, and a
//! line printed on a host connection's reader task (a body's `progress`) would then end that
//! task, leaving the host's answers unread and its run waiting for good.
//!
//! [`write_within`] is for a command that must end now: it never takes the stderr lock, which a
//! copy blocked on a reader that does not read may hold, and it waits a bounded time for the
//! stream to take its text.

use std::fmt::Display;
use std::time::Duration;

/// Prints `text` and a newline to this process's stderr, whole (module doc).
pub fn line(text: impl Display) {
    let mut whole = text.to_string();
    whole.push('\n');
    #[cfg(unix)]
    write_whole(&mut std::io::stderr().lock(), whole.as_bytes());
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let _ = std::io::stderr().lock().write_all(whole.as_bytes());
    }
}

/// Writes `text` to this process's stderr without its lock, waiting `wait` at most for the stream
/// to take it, in as many writes as it takes; what it has not taken by then is dropped. A device
/// that cannot be polled (macOS's `/dev/null`) is written to. For a command that must end now (a
/// forced interruption).
#[cfg(unix)]
pub fn write_within(text: &str, wait: Duration) {
    use std::os::fd::AsFd;
    write_to_within(std::io::stderr().as_fd(), text.as_bytes(), wait);
}

/// [`write_within`] to `fd`; how much of `text` was written.
#[cfg(unix)]
fn write_to_within(fd: std::os::fd::BorrowedFd<'_>, text: &[u8], wait: Duration) -> usize {
    use rustix::io::Errno;
    use std::time::Instant;
    let by = Instant::now() + wait;
    let mut rest = text;
    while !rest.is_empty() {
        if !until_writable(fd, Some(by.saturating_duration_since(Instant::now()))) {
            break;
        }
        match rustix::io::write(fd, rest) {
            Ok(0) => break,
            Ok(written) => rest = &rest[written..],
            Err(Errno::INTR | Errno::AGAIN) if Instant::now() < by => {}
            Err(_) => break,
        }
    }
    text.len() - rest.len()
}

/// No copy holds the stderr lock where hosts' stderr is not copied.
#[cfg(not(unix))]
pub fn write_within(text: &str, _wait: Duration) {
    use std::io::Write;
    let _ = std::io::stderr().write_all(text.as_bytes());
}

/// The most whole lines written at once: POSIX's smallest `PIPE_BUF`, so a pipe takes the write
/// whole, with no other writer's text inside it.
#[cfg(unix)]
const WHOLE_WRITE: usize = 512;

/// How long to wait before writing again to a stream that cannot be polled.
#[cfg(unix)]
const MOMENT: Duration = Duration::from_millis(10);

/// Writes `text` to `out` in [`whole_writes`]; whether it all was written. A non-blocking `out`
/// that cannot take more now ([`std::io::ErrorKind::WouldBlock`]) is waited for until it can
/// ([`until_writable`]) and given the rest; any other failure (the reader has gone) is final.
#[cfg(unix)]
pub(crate) fn write_whole(
    out: &mut (impl std::io::Write + std::os::fd::AsFd),
    text: &[u8],
) -> bool {
    use std::io::ErrorKind;
    whole_writes(text).all(|mut part| {
        while !part.is_empty() {
            match out.write(part) {
                Ok(0) => return false,
                Ok(written) => part = &part[written..],
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    until_writable(out.as_fd(), None);
                }
                Err(_) => return false,
            }
        }
        true
    })
}

/// Waits until `fd` can take more, or has failed (the next write says how), `within` at most when
/// given; whether to write again (`false`: `within` passed first). It polls `fd`, or sleeps a
/// moment when `fd` cannot be polled (macOS polls no device but a terminal).
#[cfg(unix)]
fn until_writable(fd: std::os::fd::BorrowedFd<'_>, within: Option<Duration>) -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let timeout = within.map(|within| Timespec {
        tv_sec: i64::try_from(within.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(within.subsec_nanos()),
    });
    let mut polled = [PollFd::new(&fd, PollFlags::OUT)];
    match poll(&mut polled, timeout.as_ref()) {
        Ok(0) => false,
        Ok(_)
            if polled[0]
                .revents()
                .intersects(PollFlags::OUT | PollFlags::ERR | PollFlags::HUP) =>
        {
            true
        }
        _ => {
            std::thread::sleep(within.map_or(MOMENT, |within| within.min(MOMENT)));
            true
        }
    }
}

/// `text` in writes: as many whole lines as fit in [`WHOLE_WRITE`] bytes, else one line, else
/// what is left.
#[cfg(unix)]
fn whole_writes(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let fits = &rest[..rest.len().min(WHOLE_WRITE)];
        let end = fits
            .iter()
            .rposition(|&byte| byte == b'\n')
            .or_else(|| rest.iter().position(|&byte| byte == b'\n'))
            .map_or(rest.len(), |at| at + 1);
        let (part, left) = rest.split_at(end);
        rest = left;
        Some(part)
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    /// `count` numbered lines.
    fn numbered(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|number| format!("line {number:04}\n").into_bytes())
            .collect()
    }

    #[test]
    fn whole_lines_go_out_in_writes_a_pipe_takes_whole() {
        let short = format!("{}\n", "s".repeat(99));
        let long = format!("{}\n", "l".repeat(700));
        let text = format!("{}{long}{short}{short}tail", short.repeat(6));
        let writes: Vec<&[u8]> = whole_writes(text.as_bytes()).collect();
        // Five short lines fit in 512 bytes; the sixth goes alone, since the long line does not
        // fit beside it; the long line alone; the last two short lines; the unfinished tail.
        let sizes: Vec<usize> = writes.iter().map(|write| write.len()).collect();
        assert_eq!(sizes, [500, 100, 701, 200, 4]);
        assert_eq!(writes.concat(), text.as_bytes());
    }

    /// An engine stderr a parent made non-blocking, whose reader falls behind: the writer waits
    /// until it takes more, and nothing is lost.
    #[test]
    fn a_non_blocking_stream_that_is_full_is_waited_for() {
        use std::io::Read;
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        // More than a pipe holds.
        let text = numbered(30_000);
        let read = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let mut taken = Vec::new();
            reader.read_to_end(&mut taken).unwrap();
            taken
        });
        assert!(write_whole(&mut writer, &text));
        drop(writer);
        assert!(read.join().unwrap() == text);
    }

    /// A stream whose reader has gone fails for good: nothing more is written.
    #[test]
    fn a_non_blocking_stream_whose_reader_has_gone_is_not_waited_for() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        drop(reader);
        assert!(!write_whole(&mut writer, b"line\n"));
    }

    /// A pipe nobody reads, filled.
    fn full_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
        let (reader, writer) = std::io::pipe().unwrap();
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        let block = vec![b'x'; 4096];
        while rustix::io::write(&writer, &block).is_ok() {}
        while rustix::io::write(&writer, b"x").is_ok() {}
        // Blocking again, as a terminal or a pager's pipe is.
        rustix::io::ioctl_fionbio(&writer, false).unwrap();
        (reader, writer)
    }

    /// A full stream nobody reads is given up when the wait runs out.
    #[test]
    fn a_full_stream_nobody_reads_is_given_up_in_time() {
        use std::os::fd::AsFd;
        let (_reader, writer) = full_pipe();
        let start = Instant::now();
        let wait = Duration::from_millis(300);
        assert_eq!(write_to_within(writer.as_fd(), b"last word\n", wait), 0);
        let took = start.elapsed();
        assert!(took >= wait * 9 / 10 && took < wait * 5, "{took:?}");
    }

    /// A full stream whose reader takes some of it within the wait gets the whole text: the forced
    /// exit's message is not dropped because the stream was full the moment it was written.
    #[test]
    fn a_full_stream_read_within_the_wait_gets_the_whole_text() {
        use std::io::Read;
        use std::os::fd::AsFd;
        let (mut reader, writer) = full_pipe();
        let read = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let mut taken = Vec::new();
            reader.read_to_end(&mut taken).unwrap();
            taken
        });
        let text = b"interrupted again: forcing exit\n";
        let written = write_to_within(writer.as_fd(), text, Duration::from_secs(5));
        drop(writer);
        assert_eq!(written, text.len());
        assert!(read.join().unwrap().ends_with(text));
    }
}
