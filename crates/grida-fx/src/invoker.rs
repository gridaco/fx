//! Who started the command, and so how a person types it (spec/distribution.md §2).
//!
//! `GRIDA_FX_INVOKER` names the launcher: `grida-fx` (or unset or empty: `@grida/fx`'s
//! `grida-fx`, or the binary itself), `grida` (the npm `grida` command's `grida fx`) or `python`
//! (`python -m grida.fx`). Any other value is refused before the command line is read. The name
//! changes only the command layer's own words: usage and help, the `<command>: ` error prefix
//! and the hints that say what to type next. Documents never change: what the core computes
//! (graphs, plans, problems, prices) and everything written to a project, a run, the cache or an
//! export name the engine `grida-fx`, whichever launcher started it.

use std::ffi::OsString;
use std::sync::OnceLock;

/// The environment variable a launcher sets.
pub const VARIABLE: &str = "GRIDA_FX_INVOKER";

/// A launcher of the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invoker {
    /// `@grida/fx`'s `grida-fx` command, or the binary run directly.
    Standalone,
    /// The npm `grida` command: `grida fx`.
    Grida,
    /// The Python distribution: `python -m grida.fx`.
    Python,
}

impl Invoker {
    /// Every launcher, in the order the refusal lists their names.
    pub const ALL: [Invoker; 3] = [Invoker::Standalone, Invoker::Grida, Invoker::Python];

    /// The value of [`VARIABLE`] that names it.
    pub fn name(self) -> &'static str {
        match self {
            Invoker::Standalone => "grida-fx",
            Invoker::Grida => "grida",
            Invoker::Python => "python",
        }
    }

    /// The command a person types to run a verb, without the verb.
    pub fn command(self) -> &'static str {
        match self {
            Invoker::Standalone => "grida-fx",
            Invoker::Grida => "grida fx",
            Invoker::Python => "python -m grida.fx",
        }
    }

    /// The launcher a value of [`VARIABLE`] names: unset or empty is [`Invoker::Standalone`];
    /// any other unknown value is refused with the sentence to print.
    pub fn from_value(value: Option<OsString>) -> Result<Invoker, String> {
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            return Ok(Invoker::Standalone);
        };
        Invoker::ALL
            .into_iter()
            .find(|invoker| value == invoker.name())
            .ok_or_else(|| {
                let names: Vec<&str> = Invoker::ALL.iter().map(|i| i.name()).collect();
                format!("{VARIABLE} must be one of {}", names.join(", "))
            })
    }
}

static INVOKER: OnceLock<Invoker> = OnceLock::new();

/// Sets the launcher for the rest of the process; the first call wins.
pub fn set(invoker: Invoker) {
    let _ = INVOKER.set(invoker);
}

/// The launcher [`set`] recorded, else [`Invoker::Standalone`].
pub fn current() -> Invoker {
    INVOKER.get().copied().unwrap_or(Invoker::Standalone)
}

/// The command a person types, as [`Invoker::command`] says for [`current`].
pub fn command() -> &'static str {
    current().command()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_and_empty_are_standalone() {
        assert_eq!(Invoker::from_value(None), Ok(Invoker::Standalone));
        assert_eq!(
            Invoker::from_value(Some(OsString::new())),
            Ok(Invoker::Standalone)
        );
    }

    #[test]
    fn each_name_reads_back_as_its_launcher() {
        for invoker in Invoker::ALL {
            assert_eq!(
                Invoker::from_value(Some(invoker.name().into())),
                Ok(invoker)
            );
        }
        assert_eq!(Invoker::Grida.command(), "grida fx");
        assert_eq!(Invoker::Python.command(), "python -m grida.fx");
        assert_eq!(Invoker::Standalone.command(), "grida-fx");
    }

    #[test]
    fn an_unknown_name_is_refused() {
        for value in ["npm", "Grida", "grida fx", " grida"] {
            assert_eq!(
                Invoker::from_value(Some(value.into())),
                Err("GRIDA_FX_INVOKER must be one of grida-fx, grida, python".to_string()),
                "{value}"
            );
        }
    }
}
