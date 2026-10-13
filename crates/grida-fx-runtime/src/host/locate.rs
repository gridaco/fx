//! Choosing what runs a node host (protocol.md §1 "Starting a host").
//!
//! **Python.** The interpreter: `GRIDA_FX_PYTHON` when set; otherwise the project's
//! `.venv/bin/python` (`.venv\Scripts\python.exe` on Windows) when it exists; otherwise the
//! planning project's, when that is another project and it exists; otherwise
//! `GRIDA_FX_SDK_PYTHON`, the interpreter running an SDK that started the engine; otherwise
//! `python3` on `PATH` ([`python_interpreter`]).
//!
//! **TypeScript.** The host is `<node> <host-entry.js>`. Node ([`node_runtime`]):
//! `GRIDA_FX_NODE` when set; otherwise `GRIDA_FX_SDK_NODE`, the Node running the SDK or the
//! launcher that started the engine; otherwise `node` on `PATH`. The host program
//! ([`node_host_entry`]): the `@grida/fx` Node would resolve from the host's project root (the
//! first folder at or above it holding `node_modules/@grida/fx/package.json`), its
//! `dist/host-entry.js`; a package without one is `the project's @grida/fx has no TypeScript node
//! host: upgrade @grida/fx in the project`. Otherwise the one Node would resolve from the
//! planning project's root, when that is another project, the same way. Otherwise
//! `GRIDA_FX_SDK_NODE_HOST`, the host program of the `@grida/fx` that started the engine, when it
//! names a file; otherwise none: `no @grida/fx for the TypeScript node host: run npm install
//! @grida/fx in the project`. Messages never name the host program by its absolute path, and the
//! engine keeps the folder of a `@grida/fx` outside the home out of what it records and prints
//! ([`host_package`]).
//!
//! A node host's project is the workflow's home, so a workflow kept in a nested project of a
//! monorepo finds the repository's `.venv` when it is run from the repository's root, and the
//! repository's `node_modules/@grida/fx` by walking up from the home; a workflow whose home lies
//! outside the planning project (a `workflows:` folder elsewhere) falls back to the planning
//! project's. A stand-in host's project is the planning project itself (no second root).
//!
//! An empty variable counts as unset. The interpreter (or Node) is returned as found: the
//! variable's value as given (a bare name is looked up on `PATH` when the host starts), a
//! project's interpreter under its root, or the bare name `python3` (`node`). A virtual
//! environment's `python` is a symbolic link that must not be resolved, so nothing here
//! canonicalizes.
//!
//! Checking the program (protocol.md §1): before a host starts, [`probe`] asks it about itself,
//! standard input closed and standard error discarded, so nothing it prints reaches the user.
//! Python is asked for its version and whether `grida` is importable, with `-c` and without
//! `-P` (which Python before 3.11 refuses); Node, with `-e`, for its version and whether it has
//! `module.registerHooks` and is neither Bun nor Deno (a script any Node can read). Each program
//! is asked once per process; concurrent starts wait for the one answer. A program that cannot
//! be started is [`Probe::Missing`]; one that does not answer within 10 seconds, or answers
//! something else, is [`Probe::Unknown`], and the host is started as before. [`refusal`] refuses
//! a program that answered and cannot host nodes, so the host is never started:
//! - an interpreter before [`MINIMUM_PYTHON`]: `<interpreter> is Python 3.9.6; FX's node host
//!   needs Python 3.11 or later with grida: run uv sync or pip install grida in the project, or
//!   set GRIDA_FX_PYTHON` ([`too_old`]);
//! - a Node before [`MINIMUM_NODE`]: `<node> is Node 22.12.0; FX's TypeScript node host needs
//!   Node 24 or later: install it, or set GRIDA_FX_NODE`;
//! - one without `module.registerHooks` (an older Node, Bun, Deno): `<node> is not Node.js 24 or
//!   later with module.registerHooks; FX's TypeScript node host needs it: install Node 24 or
//!   later, or set GRIDA_FX_NODE`.

use grida_fx_core::host::Language;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The variable that chooses the interpreter.
pub const PYTHON_VAR: &str = "GRIDA_FX_PYTHON";

/// The interpreter of the SDK that started the engine: the last choice before `python3`, so a
/// project's own `.venv` still wins over it (protocol.md §1, §8).
pub const SDK_PYTHON_VAR: &str = "GRIDA_FX_SDK_PYTHON";

/// The interpreter for a project, falling back to the planning project's `.venv` when
/// `planning_root` names another project (module doc). `env` reads an environment variable
/// (injected for tests).
pub fn python_interpreter(
    project_root: &Path,
    planning_root: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> PathBuf {
    if let Some(python) = env(PYTHON_VAR).filter(|value| !value.is_empty()) {
        return PathBuf::from(python);
    }
    let venv = project_venv_python(project_root);
    if venv.is_file() {
        return venv;
    }
    if let Some(planning) = planning_root.filter(|planning| *planning != project_root) {
        let venv = project_venv_python(planning);
        if venv.is_file() {
            return venv;
        }
    }
    if let Some(python) = env(SDK_PYTHON_VAR).filter(|value| !value.is_empty()) {
        return PathBuf::from(python);
    }
    PathBuf::from("python3")
}

/// The variable that chooses Node for the TypeScript node host.
pub const NODE_VAR: &str = "GRIDA_FX_NODE";

/// The Node running the SDK or the launcher that started the engine: the choice after
/// [`NODE_VAR`], before `node` on `PATH` (protocol.md §1, §8.2).
pub const SDK_NODE_VAR: &str = "GRIDA_FX_SDK_NODE";

/// The absolute path of the host program (`dist/host-entry.js`) of the `@grida/fx` that started
/// the engine: the TypeScript host program when the project has no `@grida/fx` of its own.
pub const SDK_NODE_HOST_VAR: &str = "GRIDA_FX_SDK_NODE_HOST";

/// Why there is no TypeScript host program: no `@grida/fx` anywhere (module doc).
pub const NO_SDK: &str =
    "no @grida/fx for the TypeScript node host: run npm install @grida/fx in the project";

/// Why there is no TypeScript host program: the project's `@grida/fx` predates the host.
pub const NO_HOST_PROGRAM: &str =
    "the project's @grida/fx has no TypeScript node host: upgrade @grida/fx in the project";

/// Node for the TypeScript host (module doc). `env` reads an environment variable (injected for
/// tests).
pub fn node_runtime(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    [NODE_VAR, SDK_NODE_VAR]
        .into_iter()
        .find_map(|name| env(name).filter(|value| !value.is_empty()))
        .map_or_else(|| PathBuf::from("node"), PathBuf::from)
}

/// The TypeScript host program for a host whose project is `project_root` (absolute): the
/// project's `@grida/fx`, else the planning project's when `planning_root` names another project,
/// else the one that started the engine; `Err` is the sentence (module doc). Folders are walked
/// as Node resolves a package, from each root up, passing over folders named `node_modules`; the
/// first package found decides, even one without the host program.
pub fn node_host_entry(
    project_root: &Path,
    planning_root: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<PathBuf, String> {
    // A planning project at or above the home was walked from the home already.
    let planning = planning_root.filter(|planning| !project_root.starts_with(planning));
    for root in std::iter::once(project_root).chain(planning) {
        if let Some(package) = grida_fx_package(root) {
            let entry = package.join("dist").join("host-entry.js");
            return if entry.is_file() {
                Ok(entry)
            } else {
                Err(NO_HOST_PROGRAM.into())
            };
        }
    }
    match env(SDK_NODE_HOST_VAR).filter(|value| !value.is_empty()) {
        Some(entry) if Path::new(&entry).is_file() => {
            Ok(std::path::absolute(&entry).unwrap_or_else(|_| PathBuf::from(entry)))
        }
        _ => Err(NO_SDK.into()),
    }
}

/// The `@grida/fx` package folder Node would resolve from `root`: the first folder at or above
/// it holding `node_modules/@grida/fx/package.json`, passing over folders named `node_modules`.
fn grida_fx_package(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .filter(|folder| folder.file_name().is_none_or(|name| name != "node_modules"))
        .map(|folder| folder.join("node_modules").join("@grida").join("fx"))
        .find(|package| package.join("package.json").is_file())
}

/// The folder of the `@grida/fx` whose host program is `entry` (`<package>/dist/host-entry.js`),
/// when it lies outside the home `project_root`: the SDK that started the engine, or a package in
/// a folder above the home. The engine replaces it with `@grida/fx` in what it records and prints
/// (`Engine::with_labelled_root`), since Node's messages may name its files. `None` inside the
/// home, whose own paths are scrubbed already, or when the folder would be a filesystem root.
pub fn host_package(entry: &Path, project_root: &Path) -> Option<PathBuf> {
    if entry.starts_with(project_root) {
        return None;
    }
    let package = entry.parent()?.parent()?;
    package.parent()?;
    Some(package.to_path_buf())
}

/// The oldest Python the node host runs on: `-P` needs 3.11 (protocol.md §1).
pub const MINIMUM_PYTHON: (u32, u32) = (3, 11);

/// The oldest Node the TypeScript node host runs on: type stripping and `module.registerHooks`
/// (protocol.md §1).
pub const MINIMUM_NODE: u32 = 24;

/// How long [`probe`] waits for a program's answer.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// What [`probe`] asks Python: the version, and whether `grida` is importable without the
/// working directory on `sys.path` (as the host imports it). Python 2 can read it too, so it can
/// be told that it is too old.
const PROBE_SCRIPT: &str = "import sys
if sys.path and sys.path[0] == '':
    del sys.path[0]
try:
    from importlib.util import find_spec
    grida = find_spec('grida') is not None
except Exception:
    grida = False
v = sys.version_info
sys.stdout.write('fx-python %d.%d.%d %d\\n' % (v[0], v[1], v[2], grida))
";

/// What [`probe`] asks Node (`-e`): the version, and whether `module.registerHooks` exists in a
/// runtime that is neither Bun nor Deno. Plain `var`, `typeof` and string concatenation, so any
/// Node can read it and be told that it is too old. `node:module` comes from `require` (CommonJS,
/// with `module` for a Node that predates `node:` names), else from `process.getBuiltinModule`:
/// `NODE_OPTIONS=--input-type=module` makes `-e` code an ES module, which has no `require`.
const NODE_PROBE_SCRIPT: &str = "var m,r=typeof require==='function'?require:null;try{m=r?\
r('node:module'):process.getBuiltinModule('node:module')}catch(e){try{m=r?r('module'):null}\
catch(f){}}var v=process.versions;process.stdout.write('fx-node '+v.node+' '+(m&&typeof \
m.registerHooks==='function'&&!v.bun&&!v.deno?1:0)+'\\n')";

/// What a program said about itself (module doc, "Checking the program").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// No program by that name or path.
    Missing,
    /// It answered: its version, and whether it can host nodes beyond its version: Python can
    /// import `grida`; Node has `module.registerHooks` and is neither Bun nor Deno.
    Answered {
        version: (u32, u32, u32),
        ready: bool,
    },
    /// It could not be asked, did not answer in time, or answered something else.
    Unknown,
}

impl Probe {
    /// The version a program answered, as it writes it (`3.9.6`, `22.12.0`).
    pub fn version_text(&self) -> Option<String> {
        match self {
            Probe::Answered {
                version: (major, minor, micro),
                ..
            } => Some(format!("{major}.{minor}.{micro}")),
            _ => None,
        }
    }

    /// Whether the program answered a version before the language's minimum
    /// ([`MINIMUM_PYTHON`], [`MINIMUM_NODE`]).
    pub fn is_too_old(&self, language: Language) -> bool {
        match (self, language) {
            (
                Probe::Answered {
                    version: (major, minor, _),
                    ..
                },
                Language::Python,
            ) => (*major, *minor) < MINIMUM_PYTHON,
            (
                Probe::Answered {
                    version: (major, ..),
                    ..
                },
                Language::TypeScript,
            ) => *major < MINIMUM_NODE,
            _ => false,
        }
    }
}

/// Asks `program` (spawned as the host would be: see `host::program`) about itself as a host of
/// `language`, once per process (module doc). Different programs are asked side by side.
pub async fn probe(language: Language, program: &Path) -> Probe {
    type Probes = BTreeMap<(Language, PathBuf), Arc<tokio::sync::OnceCell<Probe>>>;
    static PROBES: Mutex<Probes> = Mutex::new(BTreeMap::new());
    let program = super::program(program);
    let answer = PROBES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((language, program.clone()))
        .or_default()
        .clone();
    *answer.get_or_init(|| ask(language, program)).await
}

/// [`probe`]'s question, asked.
async fn ask(language: Language, program: PathBuf) -> Probe {
    let (option, script, word) = match language {
        Language::Python => ("-c", PROBE_SCRIPT, "fx-python"),
        Language::TypeScript => ("-e", NODE_PROBE_SCRIPT, "fx-node"),
    };
    let mut command = tokio::process::Command::new(&program);
    command
        .args([option, script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(PROBE_LIMIT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            read_probe(word, &String::from_utf8_lossy(&output.stdout))
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Probe::Missing,
        _ => Probe::Unknown,
    }
}

/// The answer to a probe script: its last line, `<word> <major>.<minor>.<micro> <0|1>`
/// (`fx-python 3.12.4 1`, `fx-node 24.14.0 1`). A Node version's pre-release part
/// (`25.0.0-pre`) is passed over.
fn read_probe(word: &str, stdout: &str) -> Probe {
    let read = || {
        let line = stdout.lines().rev().find(|line| !line.trim().is_empty())?;
        let mut words = line.split_whitespace();
        if words.next()? != word {
            return None;
        }
        let version = words.next()?;
        let version = version
            .split_once('-')
            .map_or(version, |(release, _)| release);
        let mut parts = version.split('.').map(str::parse::<u32>);
        let version = (
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
        );
        if parts.next().is_some() {
            return None;
        }
        let ready = match words.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        Some(Probe::Answered { version, ready })
    };
    read().unwrap_or(Probe::Unknown)
}

/// The refusal of an interpreter that answered a version before [`MINIMUM_PYTHON`] (module doc);
/// `label` names it as messages do.
pub fn too_old(probe: &Probe, label: &str) -> Option<String> {
    if !probe.is_too_old(Language::Python) {
        return None;
    }
    let (major, minor) = MINIMUM_PYTHON;
    Some(format!(
        "{label} is Python {}; FX's node host needs Python {major}.{minor} or later with grida: \
         run uv sync or pip install grida in the project, or set {PYTHON_VAR}",
        probe.version_text().unwrap_or_default()
    ))
}

/// The refusal of a program that answered and cannot host nodes of `language`, so the host is
/// not started (module doc); `label` names it as messages do. A Python without `grida` is not
/// refused here: its host says what is missing when it fails to start.
pub fn refusal(language: Language, probe: &Probe, label: &str) -> Option<String> {
    match language {
        Language::Python => too_old(probe, label),
        Language::TypeScript => match probe {
            Probe::Answered { .. } if probe.is_too_old(language) => Some(format!(
                "{label} is Node {}; FX's TypeScript node host needs Node {MINIMUM_NODE} or \
                 later: install it, or set {NODE_VAR}",
                probe.version_text().unwrap_or_default()
            )),
            Probe::Answered { ready: false, .. } => Some(format!(
                "{label} is not Node.js {MINIMUM_NODE} or later with module.registerHooks; FX's \
                 TypeScript node host needs it: install Node {MINIMUM_NODE} or later, or set \
                 {NODE_VAR}"
            )),
            _ => None,
        },
    }
}

/// The project's virtual-environment interpreter, whether or not it exists.
fn project_venv_python(project_root: &Path) -> PathBuf {
    if cfg!(windows) {
        project_root
            .join(".venv")
            .join("Scripts")
            .join("python.exe")
    } else {
        project_root.join(".venv").join("bin").join("python")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_variable_wins() {
        let dir = tempfile::tempdir().unwrap();
        let env = |name: &str| (name == PYTHON_VAR).then(|| "/opt/py/bin/python".to_string());
        assert_eq!(
            python_interpreter(dir.path(), None, &env),
            PathBuf::from("/opt/py/bin/python")
        );
        let bare = |name: &str| (name == PYTHON_VAR).then(|| "python3.12".to_string());
        assert_eq!(
            python_interpreter(dir.path(), None, &bare),
            PathBuf::from("python3.12")
        );
    }

    #[test]
    fn then_the_project_venv_then_python3() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            python_interpreter(dir.path(), None, &no_env),
            PathBuf::from("python3")
        );
        let venv = project_venv_python(dir.path());
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::fs::write(&venv, "").unwrap();
        assert_eq!(python_interpreter(dir.path(), None, &no_env), venv);
        // An empty variable counts as unset.
        let empty = |name: &str| (name == PYTHON_VAR).then(String::new);
        assert_eq!(python_interpreter(dir.path(), None, &empty), venv);
    }

    #[cfg(unix)]
    #[test]
    fn a_venv_link_is_not_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-python");
        std::fs::write(&real, "").unwrap();
        let venv = project_venv_python(dir.path());
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &venv).unwrap();
        assert_eq!(python_interpreter(dir.path(), None, &no_env), venv);
        // A dangling link is no interpreter.
        std::fs::remove_file(&real).unwrap();
        assert_eq!(
            python_interpreter(dir.path(), None, &no_env),
            PathBuf::from("python3")
        );
    }

    #[test]
    fn then_the_planning_projects_venv() {
        let dir = tempfile::tempdir().unwrap();
        let planning = dir.path().to_path_buf();
        let home = planning.join("workflows/thing");
        std::fs::create_dir_all(&home).unwrap();
        let venv = project_venv_python(&planning);
        assert_eq!(
            python_interpreter(&home, Some(&planning), &no_env),
            PathBuf::from("python3")
        );
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::fs::write(&venv, "").unwrap();
        assert_eq!(python_interpreter(&home, Some(&planning), &no_env), venv);
        // Without the planning project, or when the home is the planning project.
        assert_eq!(
            python_interpreter(&home, None, &no_env),
            PathBuf::from("python3")
        );
        assert_eq!(
            python_interpreter(&planning, Some(&planning), &no_env),
            venv
        );
        // The home's own .venv comes first, and the variable before both.
        let own = project_venv_python(&home);
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, "").unwrap();
        assert_eq!(python_interpreter(&home, Some(&planning), &no_env), own);
        let env = |name: &str| (name == PYTHON_VAR).then(|| "/opt/py/bin/python".to_string());
        assert_eq!(
            python_interpreter(&home, Some(&planning), &env),
            PathBuf::from("/opt/py/bin/python")
        );
    }

    #[test]
    fn the_sdks_interpreter_comes_after_every_venv() {
        let dir = tempfile::tempdir().unwrap();
        let sdk = |name: &str| (name == SDK_PYTHON_VAR).then(|| "/opt/sdk/bin/python".to_string());
        assert_eq!(
            python_interpreter(dir.path(), None, &sdk),
            PathBuf::from("/opt/sdk/bin/python")
        );
        let empty = |name: &str| (name == SDK_PYTHON_VAR).then(String::new);
        assert_eq!(
            python_interpreter(dir.path(), None, &empty),
            PathBuf::from("python3")
        );
        let venv = project_venv_python(dir.path());
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::fs::write(&venv, "").unwrap();
        assert_eq!(python_interpreter(dir.path(), None, &sdk), venv);
        let both = |name: &str| match name {
            PYTHON_VAR => Some("/opt/py/bin/python".to_string()),
            SDK_PYTHON_VAR => Some("/opt/sdk/bin/python".to_string()),
            _ => None,
        };
        assert_eq!(
            python_interpreter(dir.path(), None, &both),
            PathBuf::from("/opt/py/bin/python")
        );
    }

    #[test]
    fn probe_answers_are_read() {
        assert_eq!(
            read_probe("fx-python", "fx-python 3.9.6 1\n"),
            Probe::Answered {
                version: (3, 9, 6),
                ready: true
            }
        );
        // What a site hook printed first is passed over.
        assert_eq!(
            read_probe(
                "fx-python",
                "hello from sitecustomize\nfx-python 3.12.4 0\n\n"
            ),
            Probe::Answered {
                version: (3, 12, 4),
                ready: false
            }
        );
        for text in [
            "",
            "Python 3.9.6\n",
            "fx-python 3.x.6 1\n",
            "fx-python 3.12 1\n",
        ] {
            assert_eq!(read_probe("fx-python", text), Probe::Unknown, "{text:?}");
        }
    }

    #[test]
    fn only_a_version_before_3_11_is_refused() {
        let answered = |version| Probe::Answered {
            version,
            ready: true,
        };
        assert_eq!(
            too_old(&answered((3, 9, 6)), "python3").as_deref(),
            Some(
                "python3 is Python 3.9.6; FX's node host needs Python 3.11 or later with grida: \
                 run uv sync or pip install grida in the project, or set GRIDA_FX_PYTHON"
            )
        );
        assert!(too_old(&answered((2, 7, 18)), "python").is_some());
        assert_eq!(too_old(&answered((3, 11, 0)), "python3"), None);
        assert_eq!(too_old(&answered((3, 14, 1)), "python3"), None);
        assert_eq!(too_old(&Probe::Unknown, "python3"), None);
        assert_eq!(too_old(&Probe::Missing, "python3"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_interpreter_is_asked_once() {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python-asked");
        grida_fx_providers::testing::write_program(
            &python,
            "#!/bin/sh\necho asked >> \"$(dirname \"$0\")/asked.txt\"\necho 'fx-python 3.9.6 0'\n",
        );
        let answered = Probe::Answered {
            version: (3, 9, 6),
            ready: false,
        };
        assert_eq!(probe(Language::Python, &python).await, answered);
        assert_eq!(probe(Language::Python, &python).await, answered);
        let asked = std::fs::read_to_string(dir.path().join("asked.txt")).unwrap();
        assert_eq!(asked.lines().count(), 1);
        assert_eq!(
            probe(Language::Python, &dir.path().join("no-such-python")).await,
            Probe::Missing
        );
    }

    /// The probe's script runs on a real Python, whichever version `python3` is.
    #[tokio::test]
    async fn a_real_python_answers() {
        let found = std::process::Command::new("python3")
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|output| output.status.success());
        if !found {
            eprintln!("skipped: python3 is not on PATH");
            return;
        }
        match probe(Language::Python, Path::new("python3")).await {
            Probe::Answered {
                version: (major, _, _),
                ..
            } => assert_eq!(major, 3),
            other => panic!("python3 answered {other:?}"),
        }
    }

    #[test]
    fn node_is_the_variable_then_the_sdks_then_node() {
        assert_eq!(node_runtime(&no_env), PathBuf::from("node"));
        let sdk = |name: &str| (name == SDK_NODE_VAR).then(|| "/opt/sdk/bin/node".to_string());
        assert_eq!(node_runtime(&sdk), PathBuf::from("/opt/sdk/bin/node"));
        let both = |name: &str| match name {
            NODE_VAR => Some("node24".to_string()),
            SDK_NODE_VAR => Some("/opt/sdk/bin/node".to_string()),
            _ => None,
        };
        assert_eq!(node_runtime(&both), PathBuf::from("node24"));
        // An empty variable counts as unset.
        let empty = |name: &str| match name {
            NODE_VAR => Some(String::new()),
            SDK_NODE_VAR => Some("/opt/sdk/bin/node".to_string()),
            _ => None,
        };
        assert_eq!(node_runtime(&empty), PathBuf::from("/opt/sdk/bin/node"));
        let blank = |name: &str| (name == SDK_NODE_VAR).then(String::new);
        assert_eq!(node_runtime(&blank), PathBuf::from("node"));
    }

    /// Writes `@grida/fx` into `folder/node_modules`, with its host program when `entry`.
    fn install_sdk(folder: &Path, entry: bool) -> PathBuf {
        let package = folder.join("node_modules/@grida/fx");
        std::fs::create_dir_all(package.join("dist")).unwrap();
        std::fs::write(package.join("package.json"), "{}").unwrap();
        let program = package.join("dist/host-entry.js");
        if entry {
            std::fs::write(&program, "").unwrap();
        }
        program
    }

    #[test]
    fn the_host_program_is_the_one_node_would_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let home = root.join("workflows/thing");
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(
            node_host_entry(&home, None, &no_env),
            Err(NO_SDK.to_string())
        );
        // An ancestor's package, as Node would find it from the home.
        let outer = install_sdk(&root, true);
        assert_eq!(node_host_entry(&home, None, &no_env), Ok(outer.clone()));
        assert_eq!(node_host_entry(&root, None, &no_env), Ok(outer));
        // The nearest package wins, even one without the host program.
        install_sdk(&home, false);
        assert_eq!(
            node_host_entry(&home, None, &no_env),
            Err(NO_HOST_PROGRAM.to_string())
        );
        let inner = install_sdk(&home, true);
        assert_eq!(node_host_entry(&home, None, &no_env), Ok(inner));
    }

    #[test]
    fn then_the_host_program_of_the_sdk_that_started_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("acme");
        std::fs::create_dir_all(&home).unwrap();
        let sdk = dir.path().join("sdk/dist/host-entry.js");
        let named = sdk.to_string_lossy().into_owned();
        let env = move |name: &str| (name == SDK_NODE_HOST_VAR).then(|| named.clone());
        // Not a file yet.
        assert_eq!(node_host_entry(&home, None, &env), Err(NO_SDK.to_string()));
        std::fs::create_dir_all(sdk.parent().unwrap()).unwrap();
        std::fs::write(&sdk, "").unwrap();
        assert_eq!(node_host_entry(&home, None, &env), Ok(sdk));
        let empty = |name: &str| (name == SDK_NODE_HOST_VAR).then(String::new);
        assert_eq!(
            node_host_entry(&home, None, &empty),
            Err(NO_SDK.to_string())
        );
        // The project's own package comes first, even one too old to host.
        install_sdk(&home, false);
        assert_eq!(
            node_host_entry(&home, None, &env),
            Err(NO_HOST_PROGRAM.to_string())
        );
    }

    #[test]
    fn then_the_planning_projects_host_program() {
        // A workflow whose home lies outside the planning project (a `workflows:` folder
        // elsewhere): `app` plans, `shared` is the home.
        let dir = tempfile::tempdir().unwrap();
        let planning = dir.path().join("app");
        let home = dir.path().join("shared");
        std::fs::create_dir_all(&planning).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let sdk = dir.path().join("sdk/dist/host-entry.js");
        std::fs::create_dir_all(sdk.parent().unwrap()).unwrap();
        std::fs::write(&sdk, "").unwrap();
        let named = sdk.to_string_lossy().into_owned();
        let env = move |name: &str| (name == SDK_NODE_HOST_VAR).then(|| named.clone());
        assert_eq!(
            node_host_entry(&home, Some(&planning), &no_env),
            Err(NO_SDK.to_string())
        );
        // The planning project's package comes before the SDK's.
        let planned = install_sdk(&planning, true);
        assert_eq!(
            node_host_entry(&home, Some(&planning), &env),
            Ok(planned.clone())
        );
        assert_eq!(node_host_entry(&home, None, &env), Ok(sdk));
        // The home's own package comes first, even one too old to host.
        install_sdk(&home, false);
        assert_eq!(
            node_host_entry(&home, Some(&planning), &env),
            Err(NO_HOST_PROGRAM.to_string())
        );
        let own = install_sdk(&home, true);
        assert_eq!(node_host_entry(&home, Some(&planning), &env), Ok(own));
        // A planning project above the home is walked from the home, nearest first.
        let nested = planning.join("workflows/thing");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            node_host_entry(&nested, Some(&planning), &no_env),
            Ok(planned)
        );
    }

    #[test]
    fn a_host_program_outside_the_home_names_its_package() {
        let home = Path::new("/w/acme");
        assert_eq!(
            host_package(
                Path::new("/w/acme/node_modules/@grida/fx/dist/host-entry.js"),
                home
            ),
            None
        );
        assert_eq!(
            host_package(
                Path::new("/w/node_modules/@grida/fx/dist/host-entry.js"),
                home
            ),
            Some(PathBuf::from("/w/node_modules/@grida/fx"))
        );
        assert_eq!(
            host_package(Path::new("/opt/sdk/dist/host-entry.js"), home),
            Some(PathBuf::from("/opt/sdk"))
        );
        // Never a filesystem root.
        assert_eq!(host_package(Path::new("/dist/host-entry.js"), home), None);
    }

    #[test]
    fn a_folder_named_node_modules_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acme");
        let nested = root.join("node_modules/dep");
        std::fs::create_dir_all(&nested).unwrap();
        let outer = install_sdk(&root, true);
        // `node_modules/node_modules` is never searched, as in Node.
        install_sdk(&root.join("node_modules"), false);
        assert_eq!(node_host_entry(&nested, None, &no_env), Ok(outer));
    }

    #[test]
    fn node_answers_are_read() {
        assert_eq!(
            read_probe("fx-node", "fx-node 24.14.0 1\n"),
            Probe::Answered {
                version: (24, 14, 0),
                ready: true
            }
        );
        assert_eq!(
            read_probe("fx-node", "(node:1) a warning\nfx-node 25.0.0-pre 0\n"),
            Probe::Answered {
                version: (25, 0, 0),
                ready: false
            }
        );
        for text in [
            "",
            "v24.14.0\n",
            "fx-python 3.12.4 1\n",
            "fx-node 24.14 1\n",
            "fx-node 24.14.0.1 1\n",
            "fx-node 24.14.0 2\n",
            "fx-node 24.14.0\n",
        ] {
            assert_eq!(read_probe("fx-node", text), Probe::Unknown, "{text:?}");
        }
    }

    #[test]
    fn only_node_24_with_its_hooks_hosts() {
        let answered = |version, ready| Probe::Answered { version, ready };
        let refused = |probe: Probe| refusal(Language::TypeScript, &probe, "node");
        assert_eq!(
            refused(answered((22, 12, 0), true)).as_deref(),
            Some(
                "node is Node 22.12.0; FX's TypeScript node host needs Node 24 or later: install \
                 it, or set GRIDA_FX_NODE"
            )
        );
        // Too old wins: installing hooks would not help.
        assert!(
            refused(answered((18, 0, 0), false))
                .is_some_and(|sentence| sentence.starts_with("node is Node 18.0.0;"))
        );
        assert_eq!(
            refused(answered((24, 3, 0), false)).as_deref(),
            Some(
                "node is not Node.js 24 or later with module.registerHooks; FX's TypeScript node \
                 host needs it: install Node 24 or later, or set GRIDA_FX_NODE"
            )
        );
        assert_eq!(refused(answered((24, 0, 0), true)), None);
        assert_eq!(refused(answered((25, 2, 1), true)), None);
        assert_eq!(refused(Probe::Unknown), None);
        assert_eq!(refused(Probe::Missing), None);
        // Python keeps its own rule, and a Python without grida is not refused here.
        assert_eq!(
            refusal(Language::Python, &answered((3, 9, 6), true), "python3"),
            too_old(&answered((3, 9, 6), true), "python3")
        );
        assert_eq!(
            refusal(Language::Python, &answered((3, 12, 0), false), "python3"),
            None
        );
        assert!(!answered((3, 12, 0), true).is_too_old(Language::Python));
        assert!(answered((22, 0, 0), true).is_too_old(Language::TypeScript));
        assert!(!answered((24, 0, 0), true).is_too_old(Language::TypeScript));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn node_is_asked_with_e_once() {
        let dir = tempfile::tempdir().unwrap();
        let node = dir.path().join("node-asked");
        grida_fx_providers::testing::write_program(
            &node,
            "#!/bin/sh\necho \"$1\" >> \"$(dirname \"$0\")/asked.txt\"\nif [ \"$1\" = -e ]; then \
             echo 'fx-node 22.12.0 1'; fi\n",
        );
        let answered = Probe::Answered {
            version: (22, 12, 0),
            ready: true,
        };
        assert_eq!(probe(Language::TypeScript, &node).await, answered);
        assert_eq!(probe(Language::TypeScript, &node).await, answered);
        let asked = std::fs::read_to_string(dir.path().join("asked.txt")).unwrap();
        assert_eq!(asked, "-e\n");
        // The same program asked as a Python is another question.
        assert_eq!(probe(Language::Python, &node).await, Probe::Unknown);
        assert_eq!(
            probe(Language::TypeScript, &dir.path().join("no-such-node")).await,
            Probe::Missing
        );
    }

    /// The probe's script answers alike when `NODE_OPTIONS` makes `-e` code an ES module, which
    /// has no `require` (on a real Node; skipped without one).
    #[test]
    fn a_real_node_answers_without_require() {
        let ask = |options: Option<&str>| {
            let mut command = std::process::Command::new("node");
            command
                .args(["-e", NODE_PROBE_SCRIPT])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .env_remove("NODE_OPTIONS");
            if let Some(options) = options {
                command.env("NODE_OPTIONS", options);
            }
            command
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| read_probe("fx-node", &String::from_utf8_lossy(&output.stdout)))
        };
        let Some(plain) = ask(None) else {
            eprintln!("skipped: node is not on PATH");
            return;
        };
        assert!(matches!(plain, Probe::Answered { .. }), "{plain:?}");
        let module = ask(Some("--input-type=module"));
        let Some(Probe::Answered {
            version: (major, ..),
            ..
        }) = module
        else {
            panic!("node answered {module:?} as an ES module");
        };
        // `process.getBuiltinModule` arrived in Node 22.3; every Node 24 has it.
        if major >= MINIMUM_NODE {
            assert_eq!(module, Some(plain));
        }
    }

    /// The probe's script runs on a real Node, whichever version `node` is.
    #[tokio::test]
    async fn a_real_node_answers() {
        let found = std::process::Command::new("node")
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|output| output.status.success());
        if !found {
            eprintln!("skipped: node is not on PATH");
            return;
        }
        match probe(Language::TypeScript, Path::new("node")).await {
            Probe::Answered {
                version: (major, _, _),
                ready,
            } => {
                assert!(major >= 10, "node {major}");
                // `module.registerHooks` arrived in Node 22.15 and 23.5.
                if major >= MINIMUM_NODE {
                    assert!(ready, "node {major} has no module.registerHooks");
                }
            }
            other => panic!("node answered {other:?}"),
        }
    }
}
