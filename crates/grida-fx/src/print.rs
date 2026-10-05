//! Output helpers.
//!
//! Results go to stdout, errors to stderr. A closed stdout (`grida-fx nodes | head -1`) is not an
//! error worth reporting: what could not be written is dropped, and the exit status stays the
//! command's own.

use grida_fx_core::Error;
use serde_json::Value;
use std::io::Write;

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
