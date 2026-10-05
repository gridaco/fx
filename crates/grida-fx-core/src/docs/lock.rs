//! The lock file, `fx.lock` (`fx: lock/v1`; spec/schemas/fx-lock-v1.schema.json; identity.md
//! §6). Keys are `<path>#<attr>@<version>`, values the bare 64-hex `digest(source)`.
//!
//! Written as `# fx.lock: the source behind each versioned node type; commit it\n` followed by
//! [`crate::yaml::write_yaml`] of `{fx: lock/v1, nodes: {sorted}}` (digests quoted when
//! ambiguous). Reading refuses a lock outside its schema (exit 2).

use super::{Schema, read_document};
use crate::error::{Error, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// The lock file's name.
pub const LOCK_FILE: &str = "fx.lock";

/// The first line of a lock file.
const HEADER: &str = "# fx.lock: the source behind each versioned node type; commit it\n";

/// The entries of fx.lock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockFile {
    pub nodes: BTreeMap<String, String>,
}

/// Reads `<root>/fx.lock`; an absent file is an empty lock.
pub fn read_lock(root: &Path) -> Result<LockFile> {
    let path = root.join(LOCK_FILE);
    match std::fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LockFile::default());
        }
        Err(error) => return Err(Error::io(LOCK_FILE, &error)),
        Ok(_) => {}
    }
    let document = read_document(&path, LOCK_FILE, Schema::Lock)?;
    let mut nodes = BTreeMap::new();
    if let Some(Value::Object(entries)) = document.get("nodes") {
        for (key, digest) in entries {
            let digest = digest.as_str().ok_or_else(|| {
                Error::document(format!("{LOCK_FILE}: nodes.{key}: a digest is text"))
            })?;
            nodes.insert(key.clone(), digest.to_string());
        }
    }
    Ok(LockFile { nodes })
}

/// The text of a lock file (header comment and YAML).
pub fn render_lock(lock: &LockFile) -> String {
    let nodes: Map<String, Value> = lock
        .nodes
        .iter()
        .map(|(key, digest)| (key.clone(), Value::String(digest.clone())))
        .collect();
    let mut document = Map::new();
    document.insert("fx".into(), Value::from("lock/v1"));
    document.insert("nodes".into(), Value::Object(nodes));
    format!(
        "{HEADER}{}",
        crate::yaml::write_yaml(&Value::Object(document))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_lock_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_lock(dir.path()).unwrap(), LockFile::default());
    }

    #[test]
    fn the_header_comes_first() {
        let text = render_lock(&LockFile::default());
        assert!(text.starts_with(HEADER), "{text}");
    }
}
