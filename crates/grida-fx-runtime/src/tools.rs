//! External programs a node type declares in `tools` (protocol.md §4, §5.3 `tools`). `run`
//! hands a body each resolved tool, and `grida-fx doctor` reports every declared one.
//!
//! A tool entry is a name matching `^[a-z0-9][a-z0-9_-]*$`, optionally followed by a version
//! bound starting with one of `<>=!~` (`blender>=4.2`). It resolves to `GRIDA_FX_TOOL_<NAME>`
//! (the name upper-cased, `-` written as `_`) when that is set and names a file (else not found),
//! otherwise to the first executable `<dir>/<name>` on `PATH`.
//!
//! An empty `GRIDA_FX_TOOL_<NAME>` counts as unset, and empty `PATH` entries are skipped. On
//! Windows a name without an extension is also tried with each extension of `PATHEXT`.

use std::path::{Path, PathBuf};

/// The characters that start a version bound.
const BOUND: [char; 5] = ['<', '>', '=', '!', '~'];

/// The name part of a tool entry (`blender>=4.2` → `blender`), trimmed.
pub fn tool_name(entry: &str) -> &str {
    let end = entry.find(BOUND).unwrap_or(entry.len());
    entry[..end].trim()
}

/// `GRIDA_FX_TOOL_<NAME>` for a tool name.
pub fn tool_env_var(name: &str) -> String {
    format!("GRIDA_FX_TOOL_{}", name.to_uppercase().replace('-', "_"))
}

/// Resolves a tool name. `env` reads an environment variable (injected for tests).
pub fn resolve_tool(name: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(configured) = env(&tool_env_var(name)).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(configured);
        return path.is_file().then_some(path);
    }
    let path = env("PATH")?;
    let extensions = executable_extensions(env);
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for extension in &extensions {
            let candidate = dir.join(format!("{name}{extension}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The suffixes to try after a name: none, and on Windows each of `PATHEXT`.
fn executable_extensions(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut extensions = vec![String::new()];
    if cfg!(windows) {
        let pathext = env("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        extensions.extend(
            pathext
                .split(';')
                .filter(|e| !e.is_empty())
                .map(|e| e.to_ascii_lowercase()),
        );
    }
    extensions
}

/// A regular file the user may execute.
fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn names() {
        assert_eq!(tool_name("blender>=4.2"), "blender");
        assert_eq!(tool_name("blender >= 4.2"), "blender");
        assert_eq!(tool_name("ffprobe"), "ffprobe");
        assert_eq!(tool_name(" git "), "git");
        assert_eq!(tool_name("magick<8"), "magick");
        assert_eq!(tool_name("a-b_c!=1"), "a-b_c");
        assert_eq!(tool_name("x~=2.1"), "x");
        assert_eq!(tool_name("y==1"), "y");
    }

    #[test]
    fn env_vars() {
        assert_eq!(tool_env_var("blender"), "GRIDA_FX_TOOL_BLENDER");
        assert_eq!(tool_env_var("image-magick"), "GRIDA_FX_TOOL_IMAGE_MAGICK");
        assert_eq!(tool_env_var("ff_probe2"), "GRIDA_FX_TOOL_FF_PROBE2");
    }

    fn env_of(vars: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |name| vars.get(name).cloned()
    }

    fn executable(path: &Path) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn the_variable_wins_when_it_names_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("bin");
        std::fs::create_dir(&on_path).unwrap();
        executable(&on_path.join("blender"));
        let chosen = dir.path().join("my-blender");
        std::fs::write(&chosen, "").unwrap();
        let path = std::env::join_paths([&on_path]).unwrap();
        let path = path.to_str().unwrap().to_string();

        let env = env_of(HashMap::from([
            ("PATH".to_string(), path.clone()),
            (
                "GRIDA_FX_TOOL_BLENDER".to_string(),
                chosen.to_str().unwrap().to_string(),
            ),
        ]));
        assert_eq!(resolve_tool("blender", &env), Some(chosen));

        // Set but naming nothing: not found, even though PATH has one.
        let env = env_of(HashMap::from([
            ("PATH".to_string(), path.clone()),
            (
                "GRIDA_FX_TOOL_BLENDER".to_string(),
                dir.path().join("nope").to_str().unwrap().to_string(),
            ),
        ]));
        assert_eq!(resolve_tool("blender", &env), None);

        // A folder is not a file.
        let env = env_of(HashMap::from([
            ("PATH".to_string(), path.clone()),
            (
                "GRIDA_FX_TOOL_BLENDER".to_string(),
                on_path.to_str().unwrap().to_string(),
            ),
        ]));
        assert_eq!(resolve_tool("blender", &env), None);

        // Empty counts as unset.
        let env = env_of(HashMap::from([
            ("PATH".to_string(), path),
            ("GRIDA_FX_TOOL_BLENDER".to_string(), String::new()),
        ]));
        assert_eq!(resolve_tool("blender", &env), Some(on_path.join("blender")));
    }

    #[test]
    fn path_lookup_takes_the_first_executable() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        let third = dir.path().join("third");
        for d in [&first, &second, &third] {
            std::fs::create_dir(d).unwrap();
        }
        // In the first folder, a folder of that name; in the second, a file nobody may run.
        std::fs::create_dir(first.join("tool-x")).unwrap();
        std::fs::write(second.join("tool-x"), "").unwrap();
        executable(&third.join("tool-x"));
        let path = std::env::join_paths([&first, &second, &third]).unwrap();
        let env = env_of(HashMap::from([(
            "PATH".to_string(),
            path.to_str().unwrap().to_string(),
        )]));
        if cfg!(unix) {
            assert_eq!(resolve_tool("tool-x", &env), Some(third.join("tool-x")));
        }
        assert_eq!(resolve_tool("tool-y", &env), None);
        assert_eq!(resolve_tool("tool-x", &env_of(HashMap::new())), None);
    }
}
