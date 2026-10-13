//! What the adapters of programs the user installed share: Codex and Grok Build
//! (spec/providers.md §9.6, §9.8). Finding the program, starting it without the user's keys,
//! reading its lines, checking a request's pictures, and the bounded words a reason may carry.

use crate::adapter::{CallRequest, RouteRef, Sent};
use crate::wire;
use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard};
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

/// The program, as an absolute path to an executable file:
/// - `variable` when set and not blank (relative to the working directory), and nothing else
///   when it names no executable file;
/// - else the first `name` in an absolute `PATH` entry (empty and relative entries are skipped, so
///   a project's own folder never supplies it);
/// - else `fallback` when it is executable.
pub(crate) fn find_program(
    env: &dyn Fn(&str) -> Option<String>,
    variable: &str,
    name: &str,
    fallback: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(named) = env(variable).filter(|v| !v.trim().is_empty()) {
        let named = PathBuf::from(named);
        let named = if named.is_absolute() {
            named
        } else {
            std::env::current_dir().ok()?.join(named)
        };
        return executable(&named).then_some(named);
    }
    let on_path = env("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| executable(candidate))
    });
    on_path.or_else(|| fallback.filter(|path| executable(path)))
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

/// Whether `route` is an `image.generate` or `image.edit` route on `provider` whose contract's
/// `adapter` and `adapter_behavior`, when present, are `adapter` and `behavior`.
pub(crate) fn served(
    route: &RouteRef,
    provider: &str,
    adapter: &str,
    behavior: &str,
) -> Result<(), String> {
    let refused = || {
        format!(
            "{} is not a {} route this adapter serves",
            route.id(),
            route.capability
        )
    };
    let capability = route.capability.as_str();
    if !matches!(capability, "image.generate" | "image.edit") || route.provider != provider {
        return Err(refused());
    }
    let contract = match &route.contract {
        Value::Null => return Ok(()),
        Value::Object(contract) => contract,
        _ => return Err(refused()),
    };
    let text_or_absent = |key: &str, want: &str| match contract.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::String(have)) => have == want,
        Some(_) => false,
    };
    if text_or_absent("adapter", adapter) && text_or_absent("adapter_behavior", behavior) {
        Ok(())
    } else {
        Err(refused())
    }
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
