//! Exact run selection and the bounded runner-owned local control client.

use crate::cli::{CancelArgs, InspectArgs};
use crate::print::{print_json, print_line};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_runtime::control::{self, ControlResult};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

pub fn parse_timeout(text: &str) -> Result<Duration, String> {
    let invalid = || "timeout uses a positive integer followed by s, m or h".to_string();
    let Some((unit, digits)) = text.as_bytes().split_last() else {
        return Err(invalid());
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(invalid());
    }
    let count = std::str::from_utf8(digits)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|count| *count > 0)
        .ok_or_else(invalid)?;
    let multiplier = match unit {
        b's' => 1,
        b'm' => 60,
        b'h' => 3600,
        _ => return Err(invalid()),
    };
    let seconds = count.checked_mul(multiplier).ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
}

/// Exact selectors reuse the recorded-source catalog, including its bounded traversal. A
/// relative spelling that names both a folder and a distinct named run is refused.
fn exact_folder(
    given: &str,
    cwd: &Path,
    wait: Option<(Instant, Duration)>,
) -> Result<PathBuf, (&'static str, String)> {
    let path = cwd.join(given);
    let parts: Vec<_> = Path::new(given)
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect();
    let named = match parts.as_slice() {
        [Component::Normal(id), Component::Normal(name)] => id
            .to_str()
            .zip(name.to_str())
            .filter(|(_, name)| super::run_catalog::validate_name(name).is_ok()),
        _ => None,
    };
    let found = if let Some((id, name)) = named {
        let project = Project::find(cwd).map_err(|_| {
            (
                "invalid_target",
                "the planning project cannot be read".into(),
            )
        })?;
        super::run_catalog::resolve_control(&project.runs_dir(), id, name, wait).map_err(
            |error| {
                let code = if error.message.contains("deadline expired") {
                    "wait_timeout"
                } else if error.message.contains("ambiguous")
                    || error.message.contains("multiple sources")
                {
                    "ambiguous_target"
                } else {
                    "invalid_target"
                };
                (
                    code,
                    "the run selector cannot be resolved uniquely; use an absolute run folder path"
                        .into(),
                )
            },
        )?
    } else {
        None
    };
    if path.is_dir() {
        if let Some(found) = found
            && path.canonicalize().ok() != found.canonicalize().ok()
        {
            return Err((
                "ambiguous_target",
                "the target names both a folder and a named run; use an absolute run folder path"
                    .into(),
            ));
        }
        return Ok(path);
    }
    if let Some(found) = found {
        return Ok(found);
    }
    if matches!(parts.as_slice(), [Component::Normal(_)]) {
        return Err(("invalid_target", "control requires an explicit run folder or exact WORKFLOW_ID/NAME; bare workflow IDs do not select a run".into()));
    }
    Ok(path)
}

fn selected(
    given: &str,
    operation: &str,
    wait: Option<(Instant, Duration)>,
) -> Result<PathBuf, Box<ControlResult>> {
    let cwd = super::planning::working_directory().map_err(|_| {
        Box::new(ControlResult::error(
            operation,
            "invalid_target",
            "the current directory cannot be read",
        ))
    })?;
    exact_folder(given, &cwd, wait)
        .map_err(|(code, message)| Box::new(ControlResult::error(operation, code, &message)))
}

fn report(result: &ControlResult, json: bool) -> u8 {
    if json {
        print_json(
            &serde_json::to_value(result).expect("control results contain only JSON values"),
        );
    } else {
        let invocation = result.invocation_id.as_deref().unwrap_or("unknown");
        print_line(&format!(
            "{}  invocation {invocation}; state {}; cleanup {}",
            result.outcome, result.recorded_state, result.cleanup
        ));
        if let Some(message) = &result.message {
            print_line(message);
        }
        if let Some(availability) = &result.availability {
            print_line(&format!(
                "control {availability}; can_cancel {}",
                result.can_cancel.unwrap_or(false)
            ));
        }
    }
    result.exit_status()
}

pub fn inspect(args: &InspectArgs) -> Result<u8, Error> {
    let result = match selected(&args.run, "inspect", None) {
        Ok(folder) => control::inspect(&folder),
        Err(result) => *result,
    };
    Ok(report(&result, args.json))
}

pub fn cancel(args: &CancelArgs) -> Result<u8, Error> {
    let started = Instant::now();
    let timeout = args.timeout.unwrap_or(Duration::from_secs(30));
    let result = match selected(&args.run, "cancel", args.wait.then_some((started, timeout))) {
        Ok(folder) => control::cancel(
            &folder,
            args.invocation.as_deref(),
            args.wait,
            timeout,
            &args.source,
            started,
        ),
        Err(result) => *result,
    };
    Ok(report(&result, args.json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timeout_is_positive_bounded_and_explicit() {
        assert_eq!(parse_timeout("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_timeout("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_timeout("1h").unwrap(), Duration::from_secs(3600));
        for text in [
            "",
            "0s",
            "1",
            "-1s",
            "1.5s",
            "1S",
            " 1s",
            "18446744073709551615h",
        ] {
            assert!(parse_timeout(text).is_err(), "{text}");
        }
    }

    fn named(root: &Path) -> PathBuf {
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        let folder = root.join("runs/case/named/run");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("plan.json"),
            json!({"kind":"fx-graph-v1", "workflow":{"id":"case", "file":"case.yaml"}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            folder.join("events.jsonl"),
            format!("{}\n", json!({"event":"run_started", "name":"baseline"})),
        )
        .unwrap();
        folder
    }

    #[test]
    fn control_never_uses_latest_and_refuses_path_name_ambiguity() {
        let root = tempfile::tempdir().unwrap();
        let actual = named(root.path());
        assert_eq!(
            exact_folder("case/baseline", root.path(), None)
                .unwrap()
                .canonicalize()
                .unwrap(),
            actual.canonicalize().unwrap()
        );
        assert_eq!(
            exact_folder("case", root.path(), None).unwrap_err().0,
            "invalid_target"
        );
        std::fs::create_dir_all(root.path().join("case/baseline")).unwrap();
        assert_eq!(
            exact_folder("case/baseline", root.path(), None)
                .unwrap_err()
                .0,
            "ambiguous_target"
        );
        assert_eq!(
            exact_folder(actual.to_str().unwrap(), root.path(), None).unwrap(),
            actual
        );
    }
}
