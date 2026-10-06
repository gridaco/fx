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

use std::path::{Path, PathBuf};

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
}
