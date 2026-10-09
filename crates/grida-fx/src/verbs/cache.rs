//! `grida-fx cache prune [--forget-user ID]… [--yes] [--json]` (spec/store.md §9, "Pruning a
//! store"; docs/guide/10-cleanup.md).
//!
//! Removes from the planning project's store, and from its stand-in store, what no run of the
//! project still names, so the disk the removed runs' files took comes back. A run that needs a
//! removed result makes it again, and a removed paid answer is paid for again.
//!
//! - **Refusals**, all checked before anything is removed, in the preview too: the store overlaps
//!   the runs tree or `.fx/service` (`overlap`); another project, or an unknown earlier one, uses
//!   the store (`shared_cache`; `--forget-user ID` drops a user once you know it no longer does);
//!   a job not settled (`job_outstanding`); a runs tree, run index or catalog that cannot be read
//!   whole (`incomplete_walk`, `foreign_run`, `unreadable_run`, `unreadable_entry`,
//!   `unreachable_run`, `missing_run`); a run whose interrupted invocation reserved money without
//!   naming the call (`legacy_interrupted_run`); and, with `--yes`, a store or run in use
//!   (`cache_in_use`: its lock, a run's `run.lock` or a work claim is held) or a file system that
//!   cannot lock (`lock_unsupported`).
//! - **What stays**: every 64-hex token in every run's `plan.json` and `events.jsonl` names what
//!   stays (file digests, step identities, call keys), and so do the tokens of every record that
//!   stays, and of every job record; a store file linked from anywhere else (a run folder the
//!   project does not know) stays with every record that names it. Everything else under
//!   `results/`, `calls/` and `files/` goes, records first, with what killed invocations left
//!   (`Store::sweep`).
//! - Without `--yes` nothing is removed. With it, the stores are locked exclusively and
//!   everything is checked and counted again under the lock.
//! - Exit 0 when pruned (or for a preview), 1 when refused or when something could not be
//!   removed, 2 for usage.

use super::run_set::{Holds, ProblemKind, RunSet};
use crate::cli::{CacheArgs, CachePruneArgs, CacheVerb};
use crate::print::{labelled, print_json, print_line, shown_path};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::money::Usd;
use grida_fx_core::value::is_digest;
use grida_fx_runtime::store::Store;
use grida_fx_runtime::store::lease::{LEGACY, NotLocked, holds_anything, user_id};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

const KIND: &str = "fx-cache-prune-v1";

/// `grida-fx cache`.
pub fn run(args: &CacheArgs) -> Result<u8, Error> {
    match &args.verb {
        CacheVerb::Prune(args) => prune(args),
    }
}

/// Why the prune cannot go ahead.
struct Refusal {
    code: &'static str,
    message: String,
}

fn refusal(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
    }
}

fn prune(args: &CachePruneArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let project = Project::find(&cwd)?;
    let main = Store::open(&project.cache_dir());
    let stand_in = Store::open(&project.cache_dir().join("stand-in"));
    let mut refused = Vec::new();
    let exists = main.root().exists();
    // No removal runs while this prune reads the runs (spec/store.md §9).
    let _removals = if args.yes && exists {
        Some(
            grida_fx_runtime::run_index::removals(&project.root)
                .map_err(|error| Error::io(".fx/runs/lock", &error))?,
        )
    } else {
        None
    };
    if args.yes && exists {
        for store in [&main, &stand_in] {
            match store.lock_exclusive() {
                Ok(()) => {}
                Err(NotLocked::Held) => refused.push(refusal(
                    "cache_in_use",
                    format!(
                        "{} is in use by a run, a plan or another command; prune when it ends",
                        shown_path(store.root(), &cwd)
                    ),
                )),
                Err(NotLocked::Unsupported(error)) => refused.push(refusal(
                    "lock_unsupported",
                    format!(
                        "{} cannot be locked ({error}), so a run could write it while it is \
                         pruned",
                        shown_path(store.root(), &cwd)
                    ),
                )),
            }
            if !refused.is_empty() {
                break;
            }
        }
    }
    let set = RunSet::collect(&project);
    let mut forgotten = Vec::new();
    if refused.is_empty() {
        refused.extend(checks(&project, &main, &set, &cwd, args, &mut forgotten));
    }
    if args.yes && refused.is_empty() {
        refused.extend(in_use(&main, &stand_in, &set, &cwd));
    }
    if !refused.is_empty() {
        report_refused(&refused, args.json);
        return Ok(1);
    }
    let mut stores = Vec::new();
    for (name, store, stand) in [("main", &main, false), ("stand_in", &stand_in, true)] {
        let plan = roots(&set, stand, &cwd).and_then(|roots| Pruning::compute(store, &roots, &cwd));
        match plan {
            Ok(plan) => stores.push((name, store, plan)),
            Err(refusal) => refused.push(refusal),
        }
    }
    if !refused.is_empty() {
        report_refused(&refused, args.json);
        return Ok(1);
    }
    let mut errors = Vec::new();
    if args.yes && exists {
        for user in &forgotten {
            let _ = fs::remove_file(main.root().join("projects").join(user));
        }
        // The project that pruned is a user as much as one that ran (spec/store.md §6).
        let users = main.root().join("projects");
        let _ = fs::create_dir_all(&users)
            .and_then(|()| fs::write(users.join(user_id(&project.root)), ""));
        for (_, store, plan) in &stores {
            errors.extend(plan.apply(store));
            store.sweep();
        }
    }
    report(&stores, &set, &cwd, &main, &forgotten, &errors, args);
    Ok(u8::from(!errors.is_empty()))
}

/// The refusals of the module doc that need no lock. `forgotten` gets the users
/// `--forget-user` drops.
fn checks(
    project: &Project,
    main: &Store,
    set: &RunSet,
    cwd: &Path,
    args: &CachePruneArgs,
    forgotten: &mut Vec<String>,
) -> Vec<Refusal> {
    let mut refused = Vec::new();
    // Overlap: pruning must never walk the runs or the service's state.
    let store = main.root().canonicalize().ok();
    let service = project.root.join(".fx/service").canonicalize().ok();
    if let Some(store) = &store {
        let overlapping = [
            set.runs_root.as_ref(),
            service.as_ref(),
            Some(&project.root),
        ]
        .into_iter()
        .flatten()
        .any(|other| {
            other.starts_with(store) || (other != &project.root && store.starts_with(other))
        });
        if overlapping {
            refused.push(refusal(
                "overlap",
                format!(
                    "{} overlaps the project's runs or its .fx/service; give fx.yaml a cache of \
                     its own",
                    shown_path(main.root(), cwd)
                ),
            ));
            return refused;
        }
    }
    // Users.
    let own = user_id(&project.root);
    match main.users() {
        Ok(users) => {
            let mut others: Vec<(String, std::time::SystemTime)> =
                users.into_iter().filter(|(user, _)| user != &own).collect();
            if others.is_empty()
                && !main.root().join("projects").exists()
                && holds_anything(main.root())
            {
                others.push((LEGACY.to_string(), std::time::SystemTime::UNIX_EPOCH));
            }
            for (user, used) in others {
                if args.forget_user.iter().any(|forget| forget == &user) {
                    forgotten.push(user);
                    continue;
                }
                let what = if user == LEGACY {
                    "projects that used it before FX recorded its users, or runs of this project \
                     outside its runs folder from before FX indexed them"
                        .to_string()
                } else {
                    let day = chrono::DateTime::<chrono::Local>::from(used).format("%Y-%m-%d");
                    format!("another project, last on {day}")
                };
                refused.push(refusal(
                    "shared_cache",
                    format!(
                        "{} is also used by {what} (user {user}); prune it from no project while \
                         another still uses it, or pass --forget-user {user} once it no longer does",
                        shown_path(main.root(), cwd)
                    ),
                ));
            }
            for forget in &args.forget_user {
                if !forgotten.contains(forget) {
                    refused.push(refusal(
                        "shared_cache",
                        format!("--forget-user {forget} names no other user of this cache"),
                    ));
                }
            }
        }
        Err(error) => refused.push(refusal(
            "shared_cache",
            format!("the cache's projects/ cannot be read: {error}"),
        )),
    }
    // Jobs.
    match main.jobs() {
        Ok(jobs) => {
            for job in jobs {
                let state = job.to_value()["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if state != "settled" {
                    refused.push(refusal(
                        "job_outstanding",
                        format!(
                            "job {} is {state}: run the workflow again to collect it, or \
                             grida-fx jobs --forget it once you have checked the provider",
                            job.key
                        ),
                    ));
                }
            }
        }
        Err(error) => refused.push(refusal("job_outstanding", error.to_string())),
    }
    // Every run, read whole.
    for problem in &set.problems {
        let code = match problem.kind {
            ProblemKind::Unreadable | ProblemKind::NotAFolder => "incomplete_walk",
            ProblemKind::Foreign => "foreign_run",
            ProblemKind::UnreadableRun => "unreadable_run",
            // Searched for runs; what its log names stays (roots).
            ProblemKind::NoPlan => continue,
            ProblemKind::UnreadableEntry => "unreadable_entry",
            ProblemKind::Unreachable => "unreachable_run",
        };
        refused.push(refusal(
            code,
            format!("{}: {}", shown_path(&problem.path, cwd), problem.message),
        ));
    }
    for found in &set.found {
        match &found.holds {
            Holds::Missing => refused.push(refusal(
                "missing_run",
                format!(
                    "{} is gone; if it is, forget it with grida-fx runs remove {}",
                    shown_path(&found.folder, cwd),
                    shown_path(&found.folder, cwd)
                ),
            )),
            Holds::Recorded(recorded)
                if !recorded.stand_in && unnamed_reservation(&recorded.events) =>
            {
                refused.push(refusal(
                    "legacy_interrupted_run",
                    format!(
                        "{} was interrupted by an engine that did not name its calls before \
                         paying; run it again to finish it, or remove it",
                        shown_path(&found.folder, cwd)
                    ),
                ));
            }
            _ => {}
        }
    }
    refused
}

/// Whether an invocation that never ended reserved money without naming the call, and no later
/// invocation finished the run: its paid answer may be in the store with no event naming it.
/// A later invocation that succeeded ran or reused every step, so the log names what they need.
fn unnamed_reservation(events: &[Value]) -> bool {
    let mut open = false;
    let mut unnamed = false;
    let mut interrupted = false;
    for event in events {
        match event.get("event").and_then(Value::as_str) {
            Some("run_started") => {
                interrupted |= open && unnamed;
                open = true;
                unnamed = false;
            }
            Some("run_finished") => {
                if event.get("ok") == Some(&Value::Bool(true)) {
                    interrupted = false;
                }
                open = false;
                unnamed = false;
            }
            Some("run_cancelled") => {
                open = false;
                unnamed = false;
            }
            Some("budget_reserved") if event.get("call").is_none() => unnamed = true,
            _ => {}
        }
    }
    interrupted || (open && unnamed)
}

/// With the stores locked: a run's `run.lock` or a work claim that is held means an engine
/// that takes no store lock (an older one) is writing.
fn in_use(main: &Store, stand_in: &Store, set: &RunSet, cwd: &Path) -> Vec<Refusal> {
    let mut refused = Vec::new();
    for found in &set.found {
        if matches!(found.holds, Holds::Missing) {
            continue;
        }
        if held(&found.folder.join("run.lock")) {
            refused.push(refusal(
                "cache_in_use",
                format!("{} is running", shown_path(&found.folder, cwd)),
            ));
        }
    }
    for store in [main, stand_in] {
        let Ok(listing) = fs::read_dir(store.work_root()) else {
            continue;
        };
        for item in listing.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.ends_with(".lock") && held(&item.path()) {
                refused.push(refusal(
                    "cache_in_use",
                    format!(
                        "an invocation still holds {}",
                        shown_path(&item.path(), cwd)
                    ),
                ));
            }
        }
    }
    refused
}

/// Whether someone holds the lock of the file at `path` (an absent file is held by no one).
fn held(path: &Path) -> bool {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .ok()
        .is_some_and(|file| matches!(file.try_lock(), Err(TryLockError::WouldBlock)))
}

/// The tokens the runs of one store (stand-in or not) name, with what any folder at a run's
/// place without a plan names in its log (for both stores: its mode is unknown). A run file that
/// cannot be read refuses the prune.
fn roots(set: &RunSet, stand_in: bool, cwd: &Path) -> Result<BTreeSet<String>, Refusal> {
    let mut roots = BTreeSet::new();
    let mut read = |folder: &Path, name: &str| match fs::read(folder.join(name)) {
        Ok(bytes) => {
            roots.extend(tokens(&bytes));
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(refusal(
            "unreadable_run",
            format!("{}: {name}: {error}", shown_path(folder, cwd)),
        )),
    };
    for found in &set.found {
        let Some(recorded) = found.recorded() else {
            continue;
        };
        if recorded.stand_in != stand_in {
            continue;
        }
        read(&found.folder, "plan.json")?;
        read(&found.folder, "events.jsonl")?;
    }
    for problem in &set.problems {
        if problem.kind == ProblemKind::NoPlan {
            read(&problem.path, "events.jsonl")?;
        }
    }
    Ok(roots)
}

/// Every run of exactly 64 lowercase hexadecimal characters.
fn tokens(bytes: &[u8]) -> Vec<String> {
    let hex = |b: &u8| b.is_ascii_digit() || (b'a'..=b'f').contains(b);
    let mut found = Vec::new();
    let mut start = None;
    for (index, byte) in bytes.iter().chain(std::iter::once(&b' ')).enumerate() {
        match (hex(byte), start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if index - from == 64 {
                    found.push(String::from_utf8_lossy(&bytes[from..index]).into_owned());
                }
                start = None;
            }
            _ => {}
        }
    }
    found
}

/// A store's names in one of its record or file folders: `<top>/<d[:2]>/<d><suffix>`. A folder
/// that cannot be read refuses the prune: what it holds would name nothing.
fn listed(
    store: &Store,
    top: &str,
    suffix: &str,
    cwd: &Path,
) -> Result<BTreeMap<String, PathBuf>, Refusal> {
    let unreadable = |path: &Path, error: &io::Error| {
        refusal(
            "unreadable_store",
            format!("{}: {error}", shown_path(path, cwd)),
        )
    };
    let mut names = BTreeMap::new();
    let fans = match fs::read_dir(store.root().join(top)) {
        Ok(fans) => fans,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(names),
        Err(error) => return Err(unreadable(&store.root().join(top), &error)),
    };
    for fan in fans {
        let fan = fan.map_err(|error| unreadable(&store.root().join(top), &error))?;
        let fan_name = fan.file_name().to_string_lossy().into_owned();
        if fan_name.len() != 2 || !fan.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let entries = fs::read_dir(fan.path()).map_err(|error| unreadable(&fan.path(), &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| unreadable(&fan.path(), &error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(digest) = name.strip_suffix(suffix)
                && is_digest(digest)
                && digest.starts_with(&fan_name)
                && entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                names.insert(digest.to_string(), entry.path());
            }
        }
    }
    Ok(names)
}

/// What pruning one store keeps and removes.
#[derive(Default)]
struct Pruning {
    results: (usize, Vec<PathBuf>),
    calls: (usize, Vec<PathBuf>),
    files: (usize, Vec<PathBuf>),
    freed: u64,
    paid: Usd,
}

impl Pruning {
    fn compute(store: &Store, roots: &BTreeSet<String>, cwd: &Path) -> Result<Pruning, Refusal> {
        let unreadable = |path: &Path, error: &io::Error| {
            refusal(
                "unreadable_store",
                format!("{}: {error}", shown_path(path, cwd)),
            )
        };
        let results = listed(store, "results", ".json", cwd)?;
        let calls = listed(store, "calls", ".json", cwd)?;
        let files = listed(store, "files", "", cwd)?;
        // Every record's tokens, read once, and which records name each token.
        let mut named: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut record_tokens: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (digest, path) in results.iter().chain(calls.iter()) {
            let found = fs::read(path)
                .map(|bytes| tokens(&bytes))
                .map_err(|error| unreadable(path, &error))?;
            for token in &found {
                named.entry(token.clone()).or_default().push(digest.clone());
            }
            record_tokens.insert(digest.clone(), found);
        }
        let mut pending: Vec<String> = roots.iter().cloned().collect();
        if let Ok(jobs) = fs::read_dir(store.root().join("jobs")) {
            for job in jobs.flatten() {
                let bytes =
                    fs::read(job.path()).map_err(|error| unreadable(&job.path(), &error))?;
                pending.extend(tokens(&bytes));
                if let Some(key) = job.file_name().to_string_lossy().strip_suffix(".json") {
                    pending.push(key.to_string());
                }
            }
        }
        let mut kept = BTreeSet::new();
        let close = |pending: &mut Vec<String>, kept: &mut BTreeSet<String>| {
            while let Some(token) = pending.pop() {
                if !kept.insert(token.clone()) {
                    continue;
                }
                if let Some(found) = record_tokens.get(&token) {
                    pending.extend(found.iter().cloned());
                }
            }
        };
        close(&mut pending, &mut kept);
        // A file linked from elsewhere stays, with every record that names it.
        loop {
            let linked: Vec<String> = files
                .iter()
                .filter(|(digest, path)| !kept.contains(*digest) && links(path) > 1)
                .map(|(digest, _)| digest.clone())
                .collect();
            if linked.is_empty() {
                break;
            }
            for digest in linked {
                pending.push(digest.clone());
                if let Some(records) = named.get(&digest) {
                    pending.extend(records.iter().cloned());
                }
            }
            close(&mut pending, &mut kept);
        }
        let mut pruning = Pruning::default();
        for (digest, path) in &results {
            if kept.contains(digest) {
                pruning.results.0 += 1;
            } else {
                pruning.freed += size(path);
                pruning.results.1.push(path.clone());
            }
        }
        for (digest, path) in &calls {
            if kept.contains(digest) {
                pruning.calls.0 += 1;
            } else {
                pruning.freed += size(path);
                pruning.paid = pruning.paid + cost(path);
                pruning.calls.1.push(path.clone());
            }
        }
        for (digest, path) in &files {
            if kept.contains(digest) {
                pruning.files.0 += 1;
            } else {
                pruning.freed += size(path);
                pruning.files.1.push(path.clone());
            }
        }
        Ok(pruning)
    }

    /// Removes records, then files; what could not be removed.
    fn apply(&self, store: &Store) -> Vec<String> {
        let mut errors = Vec::new();
        for path in self
            .results
            .1
            .iter()
            .chain(&self.calls.1)
            .chain(&self.files.1)
        {
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                let relative = path.strip_prefix(store.root()).unwrap_or(path);
                errors.push(format!("{}: {error}", relative.display()));
            }
        }
        errors
    }
}

/// A file's link count (1 where the platform cannot tell).
fn links(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(path).map_or(1, |metadata| metadata.nlink())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        1
    }
}

fn size(path: &Path) -> u64 {
    fs::symlink_metadata(path).map_or(0, |metadata| metadata.len())
}

/// A call record's `cost_usd`, zero when it has none.
fn cost(path: &Path) -> Usd {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|record| Usd::from_value(&record["cost_usd"]).ok())
        .unwrap_or(Usd::ZERO)
}

fn report_refused(refused: &[Refusal], json: bool) {
    if json {
        print_json(&json!({
            "kind": KIND,
            "applied": false,
            "refused": refused
                .iter()
                .map(|r| json!({"code": r.code, "message": r.message}))
                .collect::<Vec<_>>(),
        }));
        return;
    }
    for refusal in refused {
        print_line(&format!("refused: {}: {}", refusal.code, refusal.message));
    }
}

fn report(
    stores: &[(&str, &Store, Pruning)],
    set: &RunSet,
    cwd: &Path,
    main: &Store,
    forgotten: &[String],
    errors: &[String],
    args: &CachePruneArgs,
) {
    let runs = set
        .found
        .iter()
        .filter(|found| found.recorded().is_some())
        .count();
    if args.json {
        let stores: Vec<Value> = stores
            .iter()
            .map(|(name, _, plan)| {
                json!({
                    "store": name,
                    "results": {"kept": plan.results.0, "removed": plan.results.1.len()},
                    "calls": {"kept": plan.calls.0, "removed": plan.calls.1.len()},
                    "files": {"kept": plan.files.0, "removed": plan.files.1.len()},
                    "freed_bytes": plan.freed,
                    "paid_usd": plan.paid.to_value(),
                })
            })
            .collect();
        print_json(&json!({
            "kind": KIND,
            "applied": args.yes,
            "refused": [],
            "runs_seen": runs,
            "forgotten_users": forgotten,
            "stores": stores,
            "errors": errors,
        }));
        return;
    }
    print_line(&labelled("cache", &shown_path(main.root(), cwd)));
    print_line(&labelled("runs", &format!("{runs} runs name what stays")));
    for (name, _, plan) in stores {
        let label = if *name == "main" {
            "removes"
        } else {
            "stand-in"
        };
        let label = if args.yes && *name == "main" {
            "removed"
        } else {
            label
        };
        print_line(&labelled(
            label,
            &format!(
                "{} results, {} calls (paid {}), {} files: {}; keeps {} results, {} calls, {} files",
                plan.results.1.len(),
                plan.calls.1.len(),
                plan.paid.dollars_2(),
                plan.files.1.len(),
                super::runs::human(plan.freed),
                plan.results.0,
                plan.calls.0,
                plan.files.0,
            ),
        ));
    }
    for user in forgotten {
        print_line(&labelled("forgot", &format!("user {user} of this cache")));
    }
    for error in errors {
        print_line(&labelled("failed", error));
    }
    if !args.yes {
        print_line(&labelled(
            "next",
            "add --yes to prune; a run that needs a removed result makes it again, and pays again \
             for a removed call",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_exactly_64_lowercase_hex() {
        let digest = "a".repeat(64);
        let text = format!(
            r#"{{"file":"{digest}","long":"{}","short":"{}","upper":"{}","plan":"x{digest}y"}}"#,
            "b".repeat(65),
            "c".repeat(63),
            "D".repeat(64)
        );
        assert_eq!(
            tokens(text.as_bytes()),
            vec![digest.clone(), digest.clone()]
        );
        assert_eq!(tokens(digest.as_bytes()), vec!["a".repeat(64)]);
    }

    #[test]
    fn an_interrupted_invocation_that_named_no_call_is_told_apart() {
        let start = json!({"event": "run_started"});
        let named = json!({"event": "budget_reserved", "call": "k"});
        let unnamed = json!({"event": "budget_reserved"});
        let finished = json!({"event": "run_finished"});
        assert!(!unnamed_reservation(&[start.clone(), named.clone()]));
        assert!(unnamed_reservation(&[start.clone(), unnamed.clone()]));
        assert!(!unnamed_reservation(&[
            start.clone(),
            unnamed.clone(),
            finished.clone()
        ]));
        assert!(unnamed_reservation(&[
            start.clone(),
            unnamed.clone(),
            start.clone(),
            named.clone(),
            finished
        ]));
        // A later invocation that succeeded ran or reused every step: the log names what stays.
        let succeeded = json!({"event": "run_finished", "ok": true});
        assert!(!unnamed_reservation(&[
            start.clone(),
            unnamed,
            start,
            named,
            succeeded
        ]));
    }
}
