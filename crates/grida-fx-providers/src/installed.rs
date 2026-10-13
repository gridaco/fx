//! What the adapters of programs the user installed share: Codex and Grok Build
//! (src/codex/README.md; src/grok/README.md). Finding the program, asking its version, starting it
//! without the user's keys, reading its lines, checking a request's pictures, and the bounded words
//! a reason may carry.

use crate::adapter::{CallRequest, Identity, RouteRef, Sent};
use crate::wire;
use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdout, Command};

/// The longest line such a program may send (an image may arrive base64 on one line).
pub(crate) const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// A lock that a panic elsewhere does not poison.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A program the user installed, and where [`find_program`] found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// Absolute, to an executable file.
    pub path: PathBuf,
    /// Where it was found, in words a reason may carry (never a path): the variable that names
    /// it, `PATH`, or the fallback's own (`the ChatGPT app`).
    pub origin: &'static str,
}

/// The program, as an absolute path to an executable file:
/// - `variable` when set and not blank (relative to the working directory), and nothing else
///   when it names no executable file;
/// - else the first `name` in an absolute `PATH` entry (empty and relative entries are skipped, so
///   a project's own folder never supplies it);
/// - else `fallback`'s path when it is executable, with its origin.
pub(crate) fn find_program(
    env: &dyn Fn(&str) -> Option<String>,
    variable: &'static str,
    name: &str,
    fallback: Option<(PathBuf, &'static str)>,
) -> Option<Program> {
    if let Some(named) = env(variable).filter(|v| !v.trim().is_empty()) {
        let named = PathBuf::from(named);
        let path = if named.is_absolute() {
            named
        } else {
            std::env::current_dir().ok()?.join(named)
        };
        return executable(&path).then_some(Program {
            path,
            origin: variable,
        });
    }
    let on_path = env("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| executable(candidate))
    });
    let found = on_path.map(|path| (path, "PATH"));
    let (path, origin) = found.or_else(|| fallback.filter(|(path, _)| executable(path)))?;
    Some(Program { path, origin })
}

/// A program's version against the oldest its behaviour was verified on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    /// That one or later.
    Supported(String),
    Older {
        version: String,
        minimum: &'static str,
    },
    /// The program named none, or none FX reads.
    Unnamed,
}

impl Version {
    /// `version` against `minimum`, by their leading `major.minor.patch` numbers.
    pub(crate) fn of(version: Option<&str>, minimum: &'static str) -> Version {
        let least = numbers(minimum).expect("a version");
        match version.and_then(|version| numbers(version).map(|n| (version, n))) {
            Some((version, found)) if found >= least => Version::Supported(version.to_string()),
            Some((version, _)) => Version::Older {
                version: version.to_string(),
                minimum,
            },
            None => Version::Unnamed,
        }
    }
}

/// A version's leading `major.minor.patch` numbers (a `-` or `+` suffix passed over).
fn numbers(version: &str) -> Option<[u64; 3]> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(numbers)
}

/// What `program --version` names within `deadline`, started as [`command`] starts it with stdin
/// closed: the first word it prints that is a version (`codex-cli 0.160.1`; a leading `v` passed
/// over), as [`version`] keeps it. `None` when it fails, prints none, or runs out of time; an
/// error when it cannot be started.
pub(crate) async fn ask_version(
    program: &Path,
    folder: &Path,
    also: &'static [&'static str],
    deadline: Duration,
) -> std::io::Result<Option<String>> {
    let mut command = command(program, ["--version"], folder, also);
    command.stdin(Stdio::null());
    let child = command.spawn()?;
    let output = match tokio::time::timeout(deadline, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return Ok(None),
    };
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(|word| word.strip_prefix('v').unwrap_or(word))
        .find(|word| numbers(word).is_some())
        .and_then(version))
}

/// A regular file this user may run (on Unix, any execute bit).
fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// Every variable the program never inherits: the allowlisted keys ([`crate::keys::ALLOWED`]),
/// then `also`.
pub(crate) fn withheld(also: &'static [&'static str]) -> impl Iterator<Item = &'static str> {
    crate::keys::ALLOWED
        .iter()
        .copied()
        .chain(also.iter().copied())
}

/// A command that starts `program` with `args` in `folder`: its own environment, less
/// [`withheld`]`(also)`; no stderr; killed when dropped; and in a process group of its own, which
/// [`crate::process_group::Group::enlist`] takes the child to lead.
pub(crate) fn command(
    program: &Path,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    folder: &Path,
    also: &'static [&'static str],
) -> Command {
    let mut command = Command::new(program);
    command.args(args).current_dir(folder);
    for variable in withheld(also) {
        command.env_remove(variable);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// Whether `route` is an `image.generate` or `image.edit` route on `provider` whose contract is
/// one the adapter of `identity` serves (the engine's check, [`crate::checks::contract`], made
/// again for a direct caller).
pub(crate) fn served(route: &RouteRef, provider: &str, identity: &Identity) -> Result<(), String> {
    let capability = route.capability.as_str();
    if !matches!(capability, "image.generate" | "image.edit") || route.provider != provider {
        return Err(crate::checks::not_served(route));
    }
    crate::checks::contract(route, identity)
}

/// An image request's prompt, verbatim (never trimmed or rewritten); refused when blank.
pub(crate) fn prompt(request: &Value) -> Result<&str, String> {
    let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("an image call needs its prompt as text".into());
    }
    Ok(prompt)
}

/// Refuses an image request's `size` other than `auto` with `refusal`.
pub(crate) fn auto_size(request: &Value, refusal: &str) -> Result<(), String> {
    match request.get("size") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(size)) if size == "auto" => Ok(()),
        Some(_) => Err(refusal.into()),
    }
}

/// An image request's input pictures in order, each with its member (`image`, then
/// `references[<i>]`); more than `max` is refused, naming `who` takes at most `max`.
pub(crate) fn input_pictures<'a>(
    request: &'a Value,
    max: usize,
    who: &str,
) -> Result<Vec<(String, &'a Value)>, String> {
    let mut pictures: Vec<(String, &Value)> = request
        .get("image")
        .filter(|image| !image.is_null())
        .map(|image| ("image".to_string(), image))
        .into_iter()
        .collect();
    let references = request.get("references").and_then(Value::as_array);
    pictures.extend(
        references
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, value)| (format!("references[{i}]"), value)),
    );
    if pictures.len() > max {
        return Err(format!("{who} takes at most {max} input pictures"));
    }
    Ok(pictures)
}

/// An input picture: its store copy, and the extension its copy for the program takes.
pub(crate) struct Picture<'a> {
    pub(crate) source: &'a Path,
    pub(crate) extension: &'static str,
}

/// One input picture: a file the call carries, not empty, of one of `kinds` (a kind and the
/// extension its copy takes); another kind is refused with `takes` (such as `Codex takes PNG`).
pub(crate) fn picture<'a>(
    call: &'a CallRequest,
    value: &Value,
    member: &str,
    kinds: &[(&str, &'static str)],
    takes: &str,
) -> Result<Picture<'a>, String> {
    let file = wire::request_file(call, value, member)?;
    let readable = std::fs::metadata(&file.path).is_ok_and(|m| m.is_file() && m.len() > 0);
    if !readable {
        return Err(format!("{member} has no bytes to send"));
    }
    let Some((_, extension)) = kinds.iter().find(|(kind, _)| *kind == file.kind) else {
        return Err(format!("{member} is {}; {takes}", file.kind));
    };
    Ok(Picture {
        source: &file.path,
        extension,
    })
}

/// Copies of the pictures in `folder`, in order, as `picture-<nn>.<extension>`.
pub(crate) fn place(pictures: &[Picture<'_>], folder: &Path) -> Result<Vec<PathBuf>, String> {
    pictures
        .iter()
        .enumerate()
        .map(|(i, picture)| {
            let copy = folder.join(format!("picture-{:02}.{}", i + 1, picture.extension));
            std::fs::copy(picture.source, &copy)
                .map(|_| copy)
                .map_err(|error| grida_fx_core::error::io_reason(&error))
        })
        .collect()
}

/// A failure: never retried, and charged the route's hold ($0) since the program reports no
/// cost.
pub(crate) fn failed(reason: String) -> Sent {
    Sent::Failed {
        reason,
        cost: None,
        retryable: false,
    }
}

/// One line, without its newline, refused as soon as it passes [`MAX_LINE_BYTES`]; `None` at the
/// end of the output. `label` names the program in reasons.
pub(crate) async fn read_line(
    stdout: &mut BufReader<ChildStdout>,
    label: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    loop {
        let available = stdout
            .fill_buf()
            .await
            .map_err(|_| format!("{label}'s output is unreadable"))?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let (taken, ended) = match available.iter().position(|&byte| byte == b'\n') {
            Some(at) => {
                line.extend_from_slice(&available[..at]);
                (at + 1, true)
            }
            None => {
                line.extend_from_slice(available);
                (available.len(), false)
            }
        };
        stdout.consume(taken);
        if line.len() > MAX_LINE_BYTES {
            return Err(format!("{label} sent a message over 64 MiB"));
        }
        if ended {
            return Ok(Some(line));
        }
    }
}

/// `text` when it is safe to show: not empty, at most `max` bytes, and only letters, digits and
/// `extra`.
pub(crate) fn bounded<'a>(text: &'a str, max: usize, extra: &[u8]) -> Option<&'a str> {
    let safe = !text.is_empty()
        && text.len() <= max
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b));
    safe.then_some(text)
}

/// A word from one of the program's closed vocabularies, safe to show: letters, digits and `_`,
/// at most 40 of them; anything else is `unknown`.
pub(crate) fn token(text: &str) -> String {
    bounded(text, 40, b"_").unwrap_or("unknown").to_string()
}

/// A name the program gives (a model's), safe to keep: letters, digits, `.`, `-` and `_`, at most
/// 60 of them.
pub(crate) fn name(text: &str) -> Option<String> {
    bounded(text, 60, b".-_").map(str::to_string)
}

/// A version the program gives, safe to keep: letters, digits, `.`, `-` and `+`, at most 40 of
/// them.
pub(crate) fn version(text: &str) -> Option<String> {
    bounded(text, 40, b".-+").map(str::to_string)
}
