//! What one command shares between its tasks.
//!
//! - [`Engine`]: built once per command (`run`, and the planning verbs when an `at: plan` step must
//!   run): the tokio runtime handle, the home project, the store, the node-host pool, the
//!   provider adapters, whether paid calls are admitted (`--live`), and a stand-in run's stand-in.
//! - [`Services`]: one run invocation's view of the engine: the ledger and the event log (both
//!   absent while planning), per-route pacing, cancellation and the invocation id. Every task of
//!   the invocation holds an `Arc<Services>`.
//! - [`Cancel`]: a cloneable cancellation token (`$/cancel`, a person stopping the run, a
//!   timeout).
//! - [`RunFiles`]: the files the engine handed one `run` (spec/protocol.md §3.2 `unknown_file`).
//!
//! [`Engine::scrub`] keeps private paths out of what a run records and prints (spec/store.md §7,
//! §8 "Nothing private"): a node's error text (an exception message, say) may name a file by its
//! absolute path, so before it reaches an event or the command's output the engine replaces its
//! store root and its project root with relative labels. A path under the project root becomes
//! project-relative (`/…/acme/nodes/a.py` → `nodes/a.py`, the root itself `.`); the store root
//! becomes its path relative to the project root (`.fx/cache`, or `../shared/.fx/cache` when it
//! lies outside). The command may name further private roots ([`Engine::with_private_roots`]:
//! the planning project, the command's working directory, a stand-in file's folder), each
//! labelled relative to the project root the same way (`..`, `../tools`); a filesystem root is
//! never one. Each root is matched as given and, when it differs, with its symbolic links
//! resolved; the longest root is replaced first; a match inside a longer name (`/…/acme2`) is
//! left alone.
//!
//! Threading: the core (planner, expansion, registry, the sync
//! `PythonHost` that describes and builds) is not `Send` and lives on the command's own thread,
//! the scheduler. Everything that waits (node hosts, paid calls, the agent loop) runs as tokio
//! tasks on [`Engine::handle`]. The scheduler never blocks inside the runtime: it spawns tasks
//! with `handle.spawn` and waits on channels with `blocking_recv`, or calls `handle.block_on`
//! from its own (non-runtime) thread.

use crate::calls::pacing::Pacing;
use crate::events::EventLog;
use crate::host::pool::HostPool;
use crate::ledger::Ledger;
use crate::stand_in::StandIn;
use crate::store::Store;
use grida_fx_core::val::FileValue;
use grida_fx_providers::Adapters;
use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// What a command shares (module doc).
pub struct Engine {
    pub handle: tokio::runtime::Handle,
    /// The home project's root (absolute): node hosts run with it as their working directory.
    pub project_root: PathBuf,
    /// The home project's declared source packages (spec/protocol.md §2 `initialize`).
    pub sources: Vec<String>,
    pub store: Arc<Store>,
    pub hosts: Arc<HostPool>,
    pub adapters: Arc<Adapters>,
    /// `--live`: paid calls that miss the call cache may be sent.
    pub live: bool,
    /// A stand-in run's stand-in (spec/protocol.md §5.7): the paid calls of a run that miss the
    /// call cache are asked of it ([`Engine::with_stand_in`]). Its store is the stand-in store,
    /// which the command opens as the engine's store.
    pub stand_in: Option<Arc<StandIn>>,
    /// Further private roots [`Engine::scrub`] replaces (module doc).
    private_roots: Vec<PathBuf>,
}

impl Engine {
    /// An engine over the home project `project_root` with its store at `store_root` and a pool
    /// of at most `hosts` node hosts started from `host`.
    pub fn new(
        handle: tokio::runtime::Handle,
        host: crate::host::process::HostSpec,
        store_root: &std::path::Path,
        hosts: usize,
        adapters: Adapters,
        live: bool,
    ) -> Engine {
        Engine {
            handle,
            project_root: host.project_root.clone(),
            sources: host.sources.clone(),
            store: Arc::new(Store::open(store_root)),
            hosts: Arc::new(HostPool::new(host, hosts)),
            adapters: Arc::new(adapters),
            live,
            stand_in: None,
            private_roots: Vec::new(),
        }
    }

    /// The same engine scrubbing `roots` too (module doc); a filesystem root is left out.
    pub fn with_private_roots(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Engine {
        for root in roots {
            if root.parent().is_some() && !self.private_roots.contains(&root) {
                self.private_roots.push(root);
            }
        }
        self
    }

    /// The same engine answering its runs' paid calls with `stand_in` (a stand-in run; the
    /// store given to [`Engine::new`] must be the stand-in store).
    pub fn with_stand_in(mut self, stand_in: Arc<StandIn>) -> Engine {
        self.stand_in = Some(stand_in);
        self
    }

    /// Whether this engine runs stand-in runs.
    pub fn stand_in_run(&self) -> bool {
        self.stand_in.is_some()
    }

    /// `text` with the store root, the project root and the further private roots replaced by
    /// relative labels (module doc).
    pub fn scrub(&self, text: &str) -> String {
        let project = self.project_root.as_path();
        let store = self.store.root();
        let mut labelled: Vec<(&std::path::Path, String)> = vec![
            (store, relative_label(store, project)),
            (project, ".".into()),
        ];
        for root in &self.private_roots {
            labelled.push((root.as_path(), relative_label(root, project)));
        }
        let mut roots: Vec<(String, String)> = Vec::new();
        for (root, label) in labelled {
            roots.push((root.to_string_lossy().into_owned(), label.clone()));
            if let Ok(resolved) = std::fs::canonicalize(root)
                && resolved != root
            {
                roots.push((resolved.to_string_lossy().into_owned(), label));
            }
        }
        scrub_paths(text, &roots)
    }
}

/// `path` relative to `base`, with `/` between segments (`..` to climb); `.` for `base` itself.
/// Paths with no common start (other drives) give `path`'s last segment.
pub(crate) fn relative_label(path: &std::path::Path, base: &std::path::Path) -> String {
    let path: Vec<_> = path.components().collect();
    let base: Vec<_> = base.components().collect();
    let common = path.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let segment = |c: &std::path::Component<'_>| c.as_os_str().to_string_lossy().into_owned();
    if common == 0 {
        return path.last().map_or_else(|| ".".to_string(), segment);
    }
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_string(), base.len() - common).collect();
    parts.extend(path[common..].iter().map(segment));
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Replaces each `(root, label)` in `text`, the longest root first (module doc): `<root>/x`
/// becomes `<label>/x` (`x` when the label is `.`) and the root alone its label; a match inside a
/// longer name is left alone. [`Engine::scrub`] uses it with the engine's roots; the command
/// uses it with its own labels.
pub fn scrub_paths(text: &str, roots: &[(String, String)]) -> String {
    let mut roots: Vec<&(String, String)> = roots.iter().collect();
    roots.sort_by_key(|root| std::cmp::Reverse(root.0.len()));
    let mut text = text.to_string();
    for (root, label) in roots {
        text = replace_root(&text, root, label);
    }
    text
}

/// Whether `c` can continue (or precede) a path segment, so a match next to it is part of a
/// longer name.
fn joins_a_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~' | '/' | '\\')
}

/// Whether the text right after a match continues the name (`/…/acme2`, `/…/acme.d`), so the
/// match is not the root. A `.` that ends a sentence (`… in /…/acme.`) does not continue it.
fn continues_the_name(after: &str) -> bool {
    let mut chars = after.chars();
    match chars.next() {
        Some('.') => chars
            .next()
            .is_some_and(|c| joins_a_name(c) && !matches!(c, '/' | '\\')),
        Some(c) => joins_a_name(c) && !matches!(c, '/' | '\\'),
        None => false,
    }
}

/// Every whole occurrence of `root` in `text` replaced by `label`; `root/` by `label/`, or by
/// nothing when `label` is `.`.
fn replace_root(text: &str, root: &str, label: &str) -> String {
    let root = root.trim_end_matches(['/', '\\']);
    if root.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(root) {
        let before = rest[..at].chars().next_back();
        let after = &rest[at + root.len()..];
        let next = after.chars().next();
        if before.is_some_and(joins_a_name) || continues_the_name(after) {
            // Part of another, longer path.
            out.push_str(&rest[..at + root.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..at]);
        match next {
            Some(separator @ ('/' | '\\')) => {
                if label != "." {
                    out.push_str(label);
                    out.push('/');
                }
                rest = &after[separator.len_utf8()..];
            }
            _ => {
                out.push_str(label);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// One invocation's services (module doc).
pub struct Services {
    pub engine: Arc<Engine>,
    /// The run's ledger; `None` while planning (an `at: plan` step makes no paid call).
    pub ledger: Option<Arc<Ledger>>,
    /// The run's event log; `None` while planning.
    pub events: Option<Arc<EventLog>>,
    pub pacing: Arc<Pacing>,
    pub cancel: Cancel,
    /// `fx-run-events-v1` `invocation_id`; also names holds and run ids.
    pub invocation_id: String,
    /// Counts `run` requests, for unique `run_id`s.
    pub runs: std::sync::atomic::AtomicU64,
    /// Counts holds, for unique hold ids (`ledger::hold_id`).
    pub holds: std::sync::atomic::AtomicU64,
}

impl Services {
    /// The services of planning: not live whatever the engine says, no ledger, no events. The
    /// invocation id is `plan-<16 hex>`, new each time, so the work dirs of two commands planning
    /// at once never meet.
    pub fn planning(engine: Arc<Engine>) -> Services {
        Services {
            engine,
            ledger: None,
            events: None,
            pacing: Arc::new(Pacing::new()),
            cancel: Cancel::new(),
            invocation_id: format!("plan-{}", crate::events::new_invocation_id()),
            runs: std::sync::atomic::AtomicU64::new(0),
            holds: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The services of a run invocation.
    pub fn run(
        engine: Arc<Engine>,
        ledger: Arc<Ledger>,
        events: Arc<EventLog>,
        invocation_id: String,
        cancel: Cancel,
    ) -> Services {
        Services {
            engine,
            ledger: Some(ledger),
            events: Some(events),
            pacing: Arc::new(Pacing::new()),
            cancel,
            invocation_id,
            runs: std::sync::atomic::AtomicU64::new(0),
            holds: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Claims this invocation's work dirs before the first one is made (`Store::claim_work`), so
    /// another invocation's sweep leaves them alone; best effort (without a claim, a sweep takes
    /// them only once they are an hour old). The claim lasts until the services drop.
    pub fn claim_work(&self) {
        let _ = self.engine.store.claim_work(&self.invocation_id);
    }

    /// Whether uncached paid calls may be sent: the engine is live and this is a run.
    pub fn live(&self) -> bool {
        self.engine.live && self.ledger.is_some()
    }

    /// A new hold id for an attempt of `instance_id` (`ledger::hold_id`).
    pub fn next_hold(&self, instance_id: &str) -> String {
        let n = self
            .holds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        crate::ledger::hold_id(instance_id, &self.invocation_id, n)
    }

    /// A new `run_id`, unique in the invocation: `<invocation id>-<n>`.
    pub fn next_run_id(&self) -> String {
        let n = self.runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        format!("{}-{n}", self.invocation_id)
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        self.engine.store.release_work(&self.invocation_id);
    }
}

/// A cancellation token: clones share one flag.
#[derive(Debug, Clone)]
pub struct Cancel {
    sender: Arc<watch::Sender<bool>>,
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel {
            sender: Arc::new(watch::channel(false).0),
        }
    }

    /// A token cancelled when this one is, which can also be cancelled on its own (a run's token
    /// under the invocation's).
    pub fn child(&self) -> Cancel {
        let child = Cancel::new();
        let parent = self.sender.subscribe();
        let sender = Arc::clone(&child.sender);
        if *parent.borrow() {
            sender.send_replace(true);
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let mut parent = parent;
            handle.spawn(async move {
                if parent.wait_for(|cancelled| *cancelled).await.is_ok() {
                    sender.send_replace(true);
                }
            });
        }
        child
    }

    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    /// Resolves once cancelled.
    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

impl Default for Cancel {
    fn default() -> Self {
        Cancel::new()
    }
}

/// The files the engine handed one run, by digest: its inputs, param files, capability results
/// and `file.put` results (spec/protocol.md §3.2). A file value naming any other digest is
/// refused with `unknown_file`.
#[derive(Debug, Default)]
pub struct RunFiles {
    files: Mutex<IndexMap<String, FileValue>>,
}

impl RunFiles {
    pub fn new() -> RunFiles {
        RunFiles::default()
    }

    /// Hands a file to the run.
    pub fn insert(&self, file: &FileValue) {
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files
            .entry(file.digest.clone())
            .or_insert_with(|| file.clone());
    }

    /// The file handed under `digest`.
    pub fn get(&self, digest: &str) -> Option<FileValue> {
        let files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files.get(digest).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn roots(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(root, label)| (root.to_string(), label.to_string()))
            .collect()
    }

    #[test]
    fn relative_labels() {
        let label = |path: &str, base: &str| relative_label(Path::new(path), Path::new(base));
        assert_eq!(label("/w/acme/.fx/cache", "/w/acme"), ".fx/cache");
        assert_eq!(
            label("/w/shared/.fx/cache", "/w/acme"),
            "../shared/.fx/cache"
        );
        assert_eq!(label("/w/acme", "/w/acme"), ".");
        assert_eq!(label("/w", "/w/acme/sub"), "../..");
    }

    #[test]
    fn roots_become_relative_labels() {
        let roots = roots(&[("/w/acme", "."), ("/w/acme/.fx/cache", ".fx/cache")]);
        assert_eq!(
            scrub_paths(
                "cannot identify image file '/w/acme/.fx/cache/files/ab/abcd'",
                &roots
            ),
            "cannot identify image file '.fx/cache/files/ab/abcd'"
        );
        assert_eq!(
            scrub_paths("No such file: /w/acme/nodes/a.txt (in /w/acme)", &roots),
            "No such file: nodes/a.txt (in .)"
        );
        // Every occurrence, and nothing inside longer names.
        assert_eq!(
            scrub_paths(
                "/w/acme/a and /w/acme2/b and /x/w/acme/c and /w/acme/b",
                &roots
            ),
            "a and /w/acme2/b and /x/w/acme/c and b"
        );
        assert_eq!(scrub_paths("/w/acme", &roots), ".");
        assert_eq!(scrub_paths("nothing private", &roots), "nothing private");
        // A sentence may end right after a root; a name with a suffix is another name.
        assert_eq!(scrub_paths("cwd is /w/acme.", &roots), "cwd is ..");
        assert_eq!(scrub_paths("'/w/acme'", &roots), "'.'");
        assert_eq!(
            scrub_paths("/w/acme.d/x and /w/acme~/y", &roots),
            "/w/acme.d/x and /w/acme~/y"
        );
        // A store outside the project keeps its folder.
        let outside = super::tests::roots(&[
            ("/w/acme", "."),
            ("/w/shared/.fx/cache", "../shared/.fx/cache"),
        ]);
        assert_eq!(
            scrub_paths("work dir /w/shared/.fx/cache/work/x-1/out.png", &outside),
            "work dir ../shared/.fx/cache/work/x-1/out.png"
        );
    }

    #[test]
    fn the_engine_scrubs_its_own_roots() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acme");
        std::fs::create_dir_all(root.join(".fx/cache")).unwrap();
        let engine = Engine::new(
            runtime.handle().clone(),
            crate::host::process::HostSpec {
                python: PathBuf::from("python3"),
                label: "python3".into(),
                project_root: root.clone(),
                sources: Vec::new(),
            },
            &root.join(".fx/cache"),
            1,
            Adapters::new(),
            false,
        );
        let store_file = root.join(".fx/cache/files/ab/abcd");
        let text = format!("cannot open {}", store_file.display());
        assert_eq!(engine.scrub(&text), "cannot open .fx/cache/files/ab/abcd");
        // The same folder through its resolved path (temporary folders are often behind a link).
        let resolved = std::fs::canonicalize(&root).unwrap().join("nodes/x.py");
        let text = format!("{}: line 3", resolved.display());
        assert_eq!(engine.scrub(&text), "nodes/x.py: line 3");

        // Further private roots: a folder above the project and one beside it; never `/`.
        let engine = engine.with_private_roots([
            dir.path().to_path_buf(),
            dir.path().join("tools"),
            PathBuf::from("/"),
        ]);
        let text = format!(
            "{} failed: {}",
            dir.path().join("tools/stand_in.py").display(),
            dir.path().join(".venv/bin/python").display()
        );
        assert_eq!(
            engine.scrub(&text),
            "../tools/stand_in.py failed: ../.venv/bin/python"
        );
        assert_eq!(engine.scrub("a/b /c"), "a/b /c");
    }
}
