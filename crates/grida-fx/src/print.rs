//! Output helpers.
//!
//! Results go to stdout, errors to stderr. A closed stdout (`grida-fx nodes | head -1`) is not an
//! error worth reporting: what could not be written is dropped, and the exit status stays the
//! command's own.
//!
//! Paths are shown the way a person would type them from where the command runs ([`shown_path`]):
//! never a private absolute path the user did not give.

use grida_fx_core::Error;
use serde_json::Value;
use std::io::Write;
use std::path::{Component, Path};

/// The width of the label field of the run verbs' summary lines (`run       runs/one`).
pub const LABEL_WIDTH: usize = 10;

/// Prints an FX value to stdout in identity.md §5's JSON layout followed by `\n`.
pub fn print_json(value: &Value) {
    let mut text = grida_fx_core::value::write_json(value);
    text.push('\n');
    print_text(&text);
}

/// Prints one line of text to stdout, adding its `\n`.
pub fn print_line(line: &str) {
    let mut text = String::with_capacity(line.len() + 1);
    text.push_str(line);
    text.push('\n');
    print_text(&text);
}

/// Prints text to stdout as it is.
pub fn print_text(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

/// Prints `grida-fx: <message>` to stderr.
pub fn print_error(error: &Error) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "grida-fx: {error}");
}

/// The node host: the runtime's Python host, started lazily for the project planning opens.
pub fn host() -> grida_fx_runtime::host::PythonHost {
    grida_fx_runtime::host::PythonHost::new()
}

/// A summary line: `label` padded to [`LABEL_WIDTH`] columns, then `text`.
pub fn labelled(label: &str, text: &str) -> String {
    format!("{label:<LABEL_WIDTH$}{text}")
}

/// `path` as a POSIX path relative to `base` (both absolute): `runs/case/2026-10-06-1`, `.` for
/// `base` itself, `../runs/…` when `path` lies beside it. A path that shares no root with `base`
/// (another drive) is shown whole.
pub fn shown_path(path: &Path, base: &Path) -> String {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let path_parts = parts(path);
    let base_parts = parts(base);
    let common = path_parts
        .iter()
        .zip(&base_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let rooted = |p: &Path| p.components().next().map(|c| c.as_os_str().to_owned());
    if common == 0 || rooted(path) != rooted(base) {
        return path.display().to_string();
    }
    let mut shown: Vec<&str> = std::iter::repeat_n("..", base_parts.len() - common).collect();
    shown.extend(path_parts[common..].iter().map(String::as_str));
    if shown.is_empty() {
        ".".to_string()
    } else {
        shown.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_take_ten_columns() {
        assert_eq!(labelled("run", "runs/one"), "run       runs/one");
        assert_eq!(labelled("delivered", "x"), "delivered x");
        assert_eq!(labelled("stopped", "why"), "stopped   why");
    }

    #[cfg(unix)]
    #[test]
    fn paths_are_shown_from_the_working_directory() {
        let base = Path::new("/work/acme");
        assert_eq!(
            shown_path(Path::new("/work/acme/runs/case/2026-10-06-1"), base),
            "runs/case/2026-10-06-1"
        );
        assert_eq!(shown_path(Path::new("/work/acme"), base), ".");
        assert_eq!(shown_path(Path::new("/work/acme/./runs/x"), base), "runs/x");
        assert_eq!(
            shown_path(Path::new("/work/runs/case/1"), Path::new("/work/acme/sub")),
            "../../runs/case/1"
        );
        assert_eq!(shown_path(Path::new("/work"), base), "..");
        assert_eq!(
            shown_path(Path::new("/elsewhere/x"), base),
            "../../elsewhere/x"
        );
    }
}
