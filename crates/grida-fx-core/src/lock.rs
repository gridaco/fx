//! `grida-fx lock [--check] [--same <path>#<attr>]…` (identity.md §6), computed here and written
//! by the CLI.
//!
//! Every `*.py` under the project's `nodes` folder (recursively, symbolic links to folders not
//! followed, sorted by path) is described in one `describe` call; each versioned type
//! `<rel>#<attr>@<version>` gets its source digest, in module order and then in the order the
//! host reports the module's types. With `check`, a type with no entry prints
//! `unlocked  <key>: run grida-fx lock` (status 1). A type whose entry differs prints
//! `changed   <rel>#<attr>: its source changed but version <v> did not; bump it, or confirm with
//! --same <rel>#<attr>` (status 1) unless `--same` names it (exactly `<rel>#<attr>`), and keeps
//! its old entry. Stale entries are kept and counted. With `check` and status 0:
//! `fx.lock: <n> versioned node types, all locked`; without `check` the lock is written (even
//! with status 1) and `fx.lock: <n> versioned node types` printed. A module that fails to load,
//! or a missing declared resource, is an error (exit 2) naming it, never a crash. A project with
//! no node modules never starts a host.

use crate::docs::lock::LockFile;
use crate::docs::project::Project;
use crate::error::{Error, Result};
use crate::host::NodeHost;
use crate::registry::Registry;
use grida_fx_protocol::{DescribeParams, DescribeTarget, ModuleDescription};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

/// What `lock` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockReport {
    /// Lines for stdout, in order.
    pub lines: Vec<String>,
    /// 0 or 1.
    pub status: i32,
    /// The lock to write; `None` with `--check`.
    pub write: Option<LockFile>,
}

/// Computes the lock (module doc).
pub fn lock(
    project: &Project,
    same: &[String],
    check: bool,
    host: &mut dyn NodeHost,
) -> Result<LockReport> {
    let mut locked = crate::project::read_lock_at(&project.root)?.nodes;
    let mut registry = Registry::new(
        project.root.clone(),
        project.document.sources.clone(),
        LockFile::default(),
        IndexMap::new(),
    );
    let modules = node_modules(&project.root, &project.document.nodes);
    let mut lines = Vec::new();
    let mut status = 0;
    if !modules.is_empty() {
        host.open_project(&project.root, &project.document.sources)
            .map_err(Error::from)?;
        let described = host
            .describe(&DescribeParams {
                targets: modules
                    .iter()
                    .map(|path| DescribeTarget {
                        path: path.clone(),
                        attribute: None,
                    })
                    .collect(),
                builtins: false,
            })
            .map_err(Error::from)?;
        if described.modules.len() != modules.len() {
            return Err(Error::host(format!(
                "the node host described {} of {} modules",
                described.modules.len(),
                modules.len()
            )));
        }
        for (relative, module) in modules.iter().zip(&described.modules) {
            let types = match module {
                ModuleDescription::Failed { error, .. } => {
                    return Err(Error::plan(error.clone()));
                }
                ModuleDescription::Described { types, .. } => types,
            };
            for described_type in types {
                let Some(version) = described_type.spec.version else {
                    continue;
                };
                let attribute = &described_type.attribute;
                let node = format!("{relative}#{attribute}");
                let key = format!("{node}@{version}");
                let source = registry
                    .type_source(module, attribute)
                    .map_err(|message| Error::plan(format!("{node}: {message}")))?;
                let digest = source.digest();
                if check && !locked.contains_key(&key) {
                    lines.push(format!("unlocked  {key}: run grida-fx lock"));
                    status = 1;
                    continue;
                }
                let differs = locked.get(&key).is_some_and(|old| *old != digest);
                if differs && !same.contains(&node) {
                    lines.push(format!(
                        "changed   {node}: its source changed but version {version} did not; \
                         bump it, or confirm with --same {node}"
                    ));
                    status = 1;
                    continue;
                }
                locked.insert(key, digest);
            }
        }
    }
    let count = locked.len();
    if check {
        if status == 0 {
            lines.push(format!(
                "{}: {count} versioned node types, all locked",
                crate::docs::lock::LOCK_FILE
            ));
        }
        return Ok(LockReport {
            lines,
            status,
            write: None,
        });
    }
    lines.push(format!(
        "{}: {count} versioned node types",
        crate::docs::lock::LOCK_FILE
    ));
    Ok(LockReport {
        lines,
        status,
        write: Some(LockFile { nodes: locked }),
    })
}

/// Every `*.py` file under `root/nodes`, as POSIX paths relative to the root, sorted by path
/// component. Symbolic links to folders are not followed; a `nodes` that is not a folder holds
/// nothing.
fn node_modules(root: &Path, nodes: &str) -> Vec<String> {
    let folder = root.join(nodes);
    if !folder.is_dir() {
        return Vec::new();
    }
    let mut found: Vec<Vec<String>> = Vec::new();
    walk(&folder, &mut found);
    let base: Vec<String> = Path::new(nodes)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .filter(|part| part != ".")
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|parts| {
            base.iter()
                .chain(parts.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// Collects the `*.py` files under `folder` as path parts relative to it.
fn walk(folder: &Path, found: &mut Vec<Vec<String>>) {
    fn visit(folder: &Path, prefix: &[String], found: &mut Vec<Vec<String>>) {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path: PathBuf = entry.path();
            let mut parts = prefix.to_vec();
            parts.push(name.clone());
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => visit(&path, &parts, found),
                Ok(_) => {
                    let python =
                        Path::new(&name).extension().and_then(|s| s.to_str()) == Some("py");
                    if python && path.is_file() {
                        found.push(parts);
                    }
                }
                Err(_) => {}
            }
        }
    }
    visit(folder, &[], found);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_are_found_sorted_by_component() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for file in [
            "nodes/b.py",
            "nodes/a/z.py",
            "nodes/a.py",
            "nodes/a-b.py",
            "nodes/readme.md",
            "nodes/sub/__init__.py",
            "nodes/x.PY",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        assert_eq!(
            node_modules(root, "nodes"),
            vec![
                "nodes/a/z.py",
                "nodes/a-b.py",
                "nodes/a.py",
                "nodes/b.py",
                "nodes/sub/__init__.py"
            ]
        );
        assert!(node_modules(root, "missing").is_empty());
    }
}
