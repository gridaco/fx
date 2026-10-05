//! The verbs of step 3: they exist, take any arguments, and exit 2.

use grida_fx_core::Error;

/// `<verb> is not available until the runner lands`.
pub fn run(verb: &str) -> Result<u8, Error> {
    Err(Error::unavailable(format!(
        "{verb} is not available until the runner lands"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_verb_is_unavailable() {
        let error = run("reroll").unwrap_err();
        assert_eq!(error.kind, grida_fx_core::ErrorKind::Unavailable);
        assert_eq!(
            error.to_string(),
            "reroll is not available until the runner lands"
        );
    }
}
