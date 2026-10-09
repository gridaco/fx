//! `grida-fx jobs [--forget <key>]` (spec/store.md §5).
//!
//! The store is the cache of the project found from the working directory. Lists its job records
//! (`Store::jobs`, sorted by key), one line each: `<key>  <state>  <capability> on <route id>,
//! take <take joined by .>`, then, for a `submitting` record with a note, `  <note>`: why its
//! submit's outcome is unknown, naming the provider's job id when one was returned (spec/store.md
//! §5), its control characters and runs of whitespace shown as one space. `settled` records are
//! listed too (a settled job is submitted anew by the next run, and its record may be removed at
//! any time). An empty or missing `jobs/` prints nothing. An unreadable record is an error (exit
//! 2) naming `jobs/<key>.json`.
//!
//! `--forget <key>`: the key must be 64 lowercase hex (`not a digest: <key>`, exit 2, checked
//! before the store is touched); no such record: `the cache holds no job <key>` (exit 2); else the
//! record is removed and `forgot job <key>; its call is submitted again on the next run` printed.
//! A person forgets a `submitting` job once they have checked the provider's dashboard.

use crate::cli::JobsArgs;
use crate::print::print_line;
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_runtime::store::Store;
use grida_fx_runtime::store::records::JobRecord;

/// Runs `grida-fx jobs`.
pub fn run(args: &JobsArgs) -> Result<u8, Error> {
    if let Some(key) = &args.forget {
        check_key(key)?;
    }
    let cwd = super::planning::working_directory()?;
    let project = Project::find(&cwd)?;
    let store = Store::open(&project.cache_dir());
    grida_fx_runtime::store::lease::set_user(&project.root, &project.cache_dir());
    let Some(key) = &args.forget else {
        for job in store.jobs()? {
            print_line(&job_line(&job));
        }
        return Ok(0);
    };
    if store.load_job(key)?.is_none() {
        return Err(Error::usage(format!("the cache holds no job {key}")));
    }
    store.remove_job(key)?;
    print_line(&format!(
        "forgot job {key}; its call is submitted again on the next run"
    ));
    Ok(0)
}

/// A job key is a call key: 64 lowercase hex characters (spec/store.md §1).
fn check_key(key: &str) -> Result<(), Error> {
    if grida_fx_core::value::is_digest(key) {
        Ok(())
    } else {
        Err(Error::usage(format!("not a digest: {key}")))
    }
}

/// One job's listing line.
fn job_line(job: &JobRecord) -> String {
    let take = job
        .take
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".");
    let mut line = format!(
        "{}  {}  {} on {}, take {take}",
        job.key,
        job.state.as_str(),
        job.capability,
        job.route.id
    );
    if let Some(note) = &job.note {
        // One line, whatever the record holds.
        let words: Vec<&str> = note
            .split(|c: char| c.is_whitespace() || c.is_control())
            .filter(|word| !word.is_empty())
            .collect();
        if !words.is_empty() {
            line.push_str("  ");
            line.push_str(&words.join(" "));
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_runtime::store::records::{JobState, RouteEntry};
    use serde_json::json;

    #[test]
    fn a_job_line_names_its_call() {
        let job = JobRecord {
            key: "1".repeat(64),
            capability: "video.generate".into(),
            route: RouteEntry {
                id: "vid@acme".into(),
                fingerprint: "f".repeat(64),
            },
            request: json!({"prompt": "a kite"}),
            take: vec![2, 1],
            state: JobState::Submitting,
            handle: None,
            note: None,
        };
        assert_eq!(
            job_line(&job),
            format!(
                "{}  submitting  video.generate on vid@acme, take 2.1",
                "1".repeat(64)
            )
        );
        let settled = JobRecord {
            take: vec![1],
            state: JobState::Settled,
            handle: Some(json!({"job": "j-1"})),
            ..job
        };
        assert!(job_line(&settled).ends_with("  settled  video.generate on vid@acme, take 1"));
    }

    #[test]
    fn a_job_line_shows_why_a_submit_is_uncertain() {
        let job = JobRecord {
            key: "1".repeat(64),
            capability: "video.generate".into(),
            route: RouteEntry {
                id: "vid@acme".into(),
                fingerprint: "f".repeat(64),
            },
            request: json!({"prompt": "a kite"}),
            take: vec![1],
            state: JobState::Submitting,
            handle: None,
            note: Some("fal took the job but returned no handle (request req-7)".into()),
        };
        assert_eq!(
            job_line(&job),
            format!(
                "{}  submitting  video.generate on vid@acme, take 1  fal took the job but \
                 returned no handle (request req-7)",
                "1".repeat(64)
            )
        );
        let spread = JobRecord {
            note: Some(" two\nlines\u{7}and\t a tab ".into()),
            ..job.clone()
        };
        assert!(job_line(&spread).ends_with("take 1  two lines and a tab"));
        let blank = JobRecord {
            note: Some(" \n ".into()),
            ..job
        };
        assert!(job_line(&blank).ends_with("take 1"));
    }

    #[test]
    fn only_a_digest_can_be_forgotten() {
        assert!(check_key(&"0a".repeat(32)).is_ok());
        for key in ["k0", "../x", &"A".repeat(64), &"0".repeat(63), ""] {
            let error = check_key(key).unwrap_err();
            assert_eq!(error.kind, grida_fx_core::ErrorKind::Usage);
            assert_eq!(error.message, format!("not a digest: {key}"));
        }
    }
}
