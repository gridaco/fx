//! The lock file, `fx.lock` (`fx: lock/v1`; spec/schemas/fx-lock-v1.schema.json; identity.md
//! §6). Keys are `<path>#<attr>@<version>`, values the bare 64-hex `digest(source)`: under
//! `nodes`, each versioned node type's; under `adapters`, each versioned project adapter's
//! (spec/adapters.md §9).
//!
//! Written as `# fx.lock: the source behind each versioned node type; commit it\n` followed by
//! [`crate::yaml::write_yaml`] of `{fx: lock/v1, nodes: {sorted}}`, with `adapters: {sorted}`
//! after `nodes` only when there is one (digests quoted when ambiguous). Reading refuses a lock
//! outside its schema (exit 2).

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
    pub adapters: BTreeMap<String, String>,
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
    Ok(LockFile {
        nodes: entries(&document, "nodes")?,
        adapters: entries(&document, "adapters")?,
    })
}

/// A lock's `member` entries, key to digest.
fn entries(document: &Value, member: &str) -> Result<BTreeMap<String, String>> {
    let mut found = BTreeMap::new();
    if let Some(Value::Object(entries)) = document.get(member) {
        for (key, digest) in entries {
            let digest = digest.as_str().ok_or_else(|| {
                Error::document(format!("{LOCK_FILE}: {member}.{key}: a digest is text"))
            })?;
            found.insert(key.clone(), digest.to_string());
        }
    }
    Ok(found)
}

/// The text of a lock file (header comment and YAML).
pub fn render_lock(lock: &LockFile) -> String {
    let object = |entries: &BTreeMap<String, String>| -> Value {
        entries
            .iter()
            .map(|(key, digest)| (key.clone(), Value::String(digest.clone())))
            .collect::<Map<String, Value>>()
            .into()
    };
    let mut document = Map::new();
    document.insert("fx".into(), Value::from("lock/v1"));
    document.insert("nodes".into(), object(&lock.nodes));
    if !lock.adapters.is_empty() {
        document.insert("adapters".into(), object(&lock.adapters));
    }
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
        assert!(!text.contains("adapters"), "{text}");
    }

    #[test]
    fn adapters_are_written_and_read_back_when_there_are_some() {
        let dir = tempfile::tempdir().unwrap();
        let lock = LockFile {
            nodes: [("nodes/a.py#a@1".to_string(), "a".repeat(64))].into(),
            adapters: [("adapters/acme.py#acme@2".to_string(), "b".repeat(64))].into(),
        };
        let text = render_lock(&lock);
        assert!(
            text.ends_with(&format!(
                "adapters:\n  adapters/acme.py#acme@2: {}\n",
                "b".repeat(64)
            )),
            "{text}"
        );
        std::fs::write(dir.path().join(LOCK_FILE), text).unwrap();
        assert_eq!(read_lock(dir.path()).unwrap(), lock);
    }
}
