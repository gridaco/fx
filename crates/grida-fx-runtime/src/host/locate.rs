//! Choosing the Python interpreter (protocol.md §1 "Starting a host"): `GRIDA_FX_PYTHON` when
//! set; otherwise the project's `.venv/bin/python` (`.venv\Scripts\python.exe` on Windows) when
//! it exists; otherwise the planning project's, when that is another project and it exists;
//! otherwise `GRIDA_FX_SDK_PYTHON`, the interpreter running an SDK that started the engine;
//! otherwise `python3` on `PATH`.
//!
//! A node host's project is the workflow's home, so a workflow kept in a nested project of a
//! monorepo finds the repository's `.venv` when it is run from the repository's root. A stand-in
//! host's project is the planning project itself (no second root).
//!
//! An empty variable counts as unset. The interpreter is returned as found: the
//! variable's value as given (a bare name is looked up on `PATH` when the host starts), a
//! project's interpreter under its root, or the bare name `python3`. A virtual environment's
//! `python` is a symbolic link that must not be resolved, so nothing here canonicalizes.
//!
//! Checking the interpreter (protocol.md §1): before a host starts, [`probe`] asks the
//! interpreter for its version and whether `grida` is importable, with `-c` and without `-P`
//! (which Python before 3.11 refuses), standard input closed and standard error discarded, so
//! nothing it prints reaches the user. Each interpreter is asked once per process; concurrent
//! starts wait for the one answer. An interpreter that cannot be started is [`Probe::Missing`];
//! one that does not answer within 10 seconds, or answers something else, is
//! [`Probe::Unknown`], and the host is started as before. [`too_old`] is the refusal of an
//! interpreter that answered a version before [`MINIMUM_PYTHON`]:
//! `<interpreter> is Python 3.9.6; FX's node host needs Python 3.11 or later with grida: run uv
//! sync or pip install grida in the project, or set GRIDA_FX_PYTHON`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

/// The oldest Python the node host runs on: `-P` needs 3.11 (protocol.md §1).
pub const MINIMUM_PYTHON: (u32, u32) = (3, 11);

/// How long [`probe`] waits for an interpreter's answer.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// What [`probe`] asks: the version, and whether `grida` is importable without the working
/// directory on `sys.path` (as the host imports it). Python 2 can read it too, so it can be told
/// that it is too old.
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

/// What an interpreter said about itself (module doc, "Checking the interpreter").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// No program by that name or path.
    Missing,
    /// It answered: its version, and whether `grida` is importable.
    Answered {
        version: (u32, u32, u32),
        grida: bool,
    },
    /// It could not be asked, did not answer in time, or answered something else.
    Unknown,
}

impl Probe {
    /// The version an interpreter answered, as Python writes it (`3.9.6`).
    pub fn version_text(&self) -> Option<String> {
        match self {
            Probe::Answered {
                version: (major, minor, micro),
                ..
            } => Some(format!("{major}.{minor}.{micro}")),
            _ => None,
        }
    }

    /// Whether the interpreter answered a version before [`MINIMUM_PYTHON`].
    pub fn is_too_old(&self) -> bool {
        matches!(self, Probe::Answered { version: (major, minor, _), .. }
            if (*major, *minor) < MINIMUM_PYTHON)
    }
}

/// Asks `python` (spawned as the host would be: see `host::program`) for its version, once per
/// process (module doc).
pub async fn probe(python: &Path) -> Probe {
    static PROBES: tokio::sync::Mutex<BTreeMap<PathBuf, Probe>> =
        tokio::sync::Mutex::const_new(BTreeMap::new());
    let program = super::program(python);
    let mut probes = PROBES.lock().await;
    if let Some(known) = probes.get(&program) {
        return *known;
    }
    let mut command = tokio::process::Command::new(&program);
    command
        .args(["-c", PROBE_SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let answered = match tokio::time::timeout(PROBE_LIMIT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            read_probe(&String::from_utf8_lossy(&output.stdout))
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Probe::Missing,
        _ => Probe::Unknown,
    };
    probes.insert(program, answered);
    answered
}

/// The answer to [`PROBE_SCRIPT`]: its last line, `fx-python <major>.<minor>.<micro> <0|1>`.
fn read_probe(stdout: &str) -> Probe {
    let read = || {
        let line = stdout.lines().rev().find(|line| !line.trim().is_empty())?;
        let mut words = line.split_whitespace();
        if words.next()? != "fx-python" {
            return None;
        }
        let mut parts = words.next()?.split('.').map(str::parse::<u32>);
        let version = (
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
        );
        let grida = match words.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        Some(Probe::Answered { version, grida })
    };
    read().unwrap_or(Probe::Unknown)
}

/// The refusal of an interpreter that answered a version before [`MINIMUM_PYTHON`] (module doc);
/// `label` names it as messages do.
pub fn too_old(probe: &Probe, label: &str) -> Option<String> {
    if !probe.is_too_old() {
        return None;
    }
    let (major, minor) = MINIMUM_PYTHON;
    Some(format!(
        "{label} is Python {}; FX's node host needs Python {major}.{minor} or later with grida: \
         run uv sync or pip install grida in the project, or set {PYTHON_VAR}",
        probe.version_text().unwrap_or_default()
    ))
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
            read_probe("fx-python 3.9.6 1\n"),
            Probe::Answered {
                version: (3, 9, 6),
                grida: true
            }
        );
        // What a site hook printed first is passed over.
        assert_eq!(
            read_probe("hello from sitecustomize\nfx-python 3.12.4 0\n\n"),
            Probe::Answered {
                version: (3, 12, 4),
                grida: false
            }
        );
        for text in [
            "",
            "Python 3.9.6\n",
            "fx-python 3.x.6 1\n",
            "fx-python 3.12 1\n",
        ] {
            assert_eq!(read_probe(text), Probe::Unknown, "{text:?}");
        }
    }

    #[test]
    fn only_a_version_before_3_11_is_refused() {
        let answered = |version| Probe::Answered {
            version,
            grida: true,
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
            grida: false,
        };
        assert_eq!(probe(&python).await, answered);
        assert_eq!(probe(&python).await, answered);
        let asked = std::fs::read_to_string(dir.path().join("asked.txt")).unwrap();
        assert_eq!(asked.lines().count(), 1);
        assert_eq!(
            probe(&dir.path().join("no-such-python")).await,
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
        match probe(Path::new("python3")).await {
            Probe::Answered {
                version: (major, _, _),
                ..
            } => assert_eq!(major, 3),
            other => panic!("python3 answered {other:?}"),
        }
    }
}
