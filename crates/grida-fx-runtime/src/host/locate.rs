//! Choosing the Python interpreter (protocol.md §1 "Starting a host"): `GRIDA_FX_PYTHON` when
//! set; otherwise the project's `.venv/bin/python` (`.venv\Scripts\python.exe` on Windows) when
//! it exists; otherwise `python3` on `PATH`.
//!
//! An empty `GRIDA_FX_PYTHON` counts as unset. The interpreter is returned as found: the
//! variable's value as given (a bare name is looked up on `PATH` when the host starts), the
//! project's interpreter under `project_root`, or the bare name `python3`. A virtual
//! environment's `python` is a symbolic link that must not be resolved, so nothing here
//! canonicalizes.

use std::path::{Path, PathBuf};

/// The variable that chooses the interpreter.
pub const PYTHON_VAR: &str = "GRIDA_FX_PYTHON";

/// The interpreter for a project. `env` reads an environment variable (injected for tests).
pub fn python_interpreter(project_root: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(python) = env(PYTHON_VAR).filter(|value| !value.is_empty()) {
        return PathBuf::from(python);
    }
    let venv = project_venv_python(project_root);
    if venv.is_file() {
        return venv;
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
            python_interpreter(dir.path(), &env),
            PathBuf::from("/opt/py/bin/python")
        );
        let bare = |name: &str| (name == PYTHON_VAR).then(|| "python3.12".to_string());
        assert_eq!(
            python_interpreter(dir.path(), &bare),
            PathBuf::from("python3.12")
        );
    }

    #[test]
    fn then_the_project_venv_then_python3() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            python_interpreter(dir.path(), &no_env),
            PathBuf::from("python3")
        );
        let venv = project_venv_python(dir.path());
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::fs::write(&venv, "").unwrap();
        assert_eq!(python_interpreter(dir.path(), &no_env), venv);
        // An empty variable counts as unset.
        let empty = |name: &str| (name == PYTHON_VAR).then(String::new);
        assert_eq!(python_interpreter(dir.path(), &empty), venv);
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
        assert_eq!(python_interpreter(dir.path(), &no_env), venv);
        // A dangling link is no interpreter.
        std::fs::remove_file(&real).unwrap();
        assert_eq!(
            python_interpreter(dir.path(), &no_env),
            PathBuf::from("python3")
        );
    }
}
