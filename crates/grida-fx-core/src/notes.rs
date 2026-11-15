//! Static workflow documentation. Source capture is presentation-only (spec/notes.md).

use crate::docs::workflow::{LoadedWorkflow, Steps};
use crate::registry::Registry;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
#[cfg(unix)]
use std::path::Component;
use std::path::Path;

/// Maximum UTF-8 bytes per note, before removing an optional BOM.
pub const MAX_NOTE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Markdown,
    Text,
}

/// Validated authored note. Exactly one source is present.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredNote {
    pub text: Option<String>,
    pub file: Option<String>,
    #[serde(default)]
    pub format: Format,
    pub about: Option<String>,
}

pub type Notes = IndexMap<String, AuthoredNote>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: String,
    pub digest: String,
}

/// A captured declaration, including declarations in not-yet-expanded groups. The private
/// registry retains these across expansions; plan.json retains them across resumptions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub workflow: String,
    pub container: String,
    pub name: String,
    pub about: Option<String>,
    pub format: Format,
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

/// One visible occurrence of a captured declaration; never an executable instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub id: String,
    pub name: String,
    pub scope_id: Option<String>,
    pub about: Option<String>,
    pub format: Format,
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl Registry {
    /// Capture every declared note once, including groups whose execution is still pending.
    pub fn capture_notes(&mut self, loaded: &LoadedWorkflow) {
        if self.notes_frozen || !self.note_workflows.insert(loaded.source.clone()) {
            return;
        }
        self.capture_container(
            &loaded.source,
            "",
            &loaded.workflow.notes,
            &loaded.workflow.steps,
        );
    }

    fn capture_container(&mut self, workflow: &str, container: &str, notes: &Notes, steps: &Steps) {
        for (name, authored) in notes {
            let (text, source, mut diagnostic) = match &authored.file {
                Some(path) => match read_note(&self.root, path) {
                    Ok((text, source)) => (Some(text), Some(source), None),
                    Err(message) => (None, None, Some(message.to_string())),
                },
                None => (authored.text.clone(), None, None),
            };
            if authored
                .about
                .as_ref()
                .is_some_and(|about| !steps.contains_key(about))
            {
                diagnostic = Some(match diagnostic {
                    Some(message) => format!("{message}; associated step is unavailable"),
                    None => "associated step is unavailable".into(),
                });
            }
            self.note_templates.push(Template {
                workflow: workflow.into(),
                container: container.into(),
                name: name.clone(),
                about: authored.about.clone(),
                format: authored.format,
                text,
                source,
                diagnostic,
            });
        }
        for (name, step) in steps.iter() {
            if let Some(children) = &step.steps {
                let child = if container.is_empty() {
                    name.clone()
                } else {
                    format!("{container}.{name}")
                };
                self.capture_container(workflow, &child, &step.notes, children);
            }
        }
    }

    /// The immutable presentation source ledger of this preparation.
    pub fn note_templates(&self) -> &[Template] {
        &self.note_templates
    }

    /// Restore the original run's note evidence. An empty/legacy snapshot stays empty, even
    /// when current authored notes were added under the same executable plan digest.
    pub fn restore_note_templates(&mut self, templates: Vec<Template>) {
        self.note_templates = templates;
        self.notes_frozen = true;
    }

    pub(crate) fn notes_for(
        &self,
        workflow: &str,
        container: &str,
        scope_id: Option<&str>,
        prefix: &str,
    ) -> Vec<Note> {
        self.note_templates
            .iter()
            .filter(|template| template.workflow == workflow && template.container == container)
            .map(|template| Note {
                id: format!("note:{}:{}", scope_id.unwrap_or("root"), template.name),
                name: template.name.clone(),
                scope_id: scope_id.map(str::to_string),
                about: template
                    .about
                    .as_ref()
                    .map(|name| format!("{prefix}{name}")),
                format: template.format,
                text: template.text.clone(),
                source: template.source.clone(),
                diagnostic: template.diagnostic.clone(),
            })
            .collect()
    }
}

/// Strip presentation notes only at the declared workflow/group positions. A task parameter or
/// arbitrary table named `notes` remains executable authored data.
pub fn execution_document(document: &Value) -> Value {
    let mut document = document.clone();
    strip_container(&mut document);
    document
}

fn strip_container(document: &mut Value) {
    let Some(map) = document.as_object_mut() else {
        return;
    };
    map.remove("notes");
    if let Some(steps) = map.get_mut("steps").and_then(Value::as_object_mut) {
        for step in steps.values_mut() {
            if step.get("steps").is_some_and(Value::is_object) {
                strip_container(step);
            }
        }
    }
}

fn read_note(root: &Path, path: &str) -> Result<(String, Source), &'static str> {
    if Path::new(path).is_absolute() || path.contains('\\') || path.contains(':') {
        return Err("note source is not a project-relative file");
    }
    let root = root
        .canonicalize()
        .map_err(|_| "note source is unavailable")?;
    let candidate = root
        .join(path)
        .canonicalize()
        .map_err(|_| "note source is unavailable")?;
    let relative = candidate
        .strip_prefix(&root)
        .map_err(|_| "note source is outside the project")?;
    let label = relative
        .to_str()
        .ok_or("note source label is not UTF-8")?
        .replace('\\', "/");
    if label.is_empty() || label.chars().any(char::is_control) {
        return Err("note source is unavailable");
    }
    let file = open_note(&root, &label).map_err(|_| "note source is unavailable")?;
    let mut bytes = Vec::new();
    file.take((MAX_NOTE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "note source could not be read")?;
    if bytes.len() > MAX_NOTE_BYTES {
        return Err("note source exceeds 256 KiB");
    }
    let digest = crate::value::file_digest(&bytes);
    let text = crate::text::decode_text(&bytes, "note source")
        .map_err(|_| "note source is not valid UTF-8")?;
    Ok((
        text,
        Source {
            path: label,
            digest,
        },
    ))
}

/// Descriptor-relative, regular-file reads keep presentation sources confined even when a
/// directory entry is replaced during preparation; nonblocking avoids hanging on a FIFO.
#[cfg(unix)]
fn open_note(root: &Path, relative: &str) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let bad = || std::io::Error::from(std::io::ErrorKind::NotFound);
    #[cfg(target_os = "linux")]
    let access = OFlags::PATH;
    // Darwin O_SEARCH = O_EXEC (0x40000000) | O_DIRECTORY (sys/fcntl.h).
    #[cfg(target_os = "macos")]
    let access = OFlags::from_bits_retain(0x40000000);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let access = OFlags::RDONLY;
    let flags = access | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", flags, Mode::empty())?;
    for part in root.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(part) => directory = openat(&directory, part, flags, Mode::empty())?,
            _ => return Err(bad()),
        }
    }
    let parts = Path::new(relative)
        .components()
        .map(|part| match part {
            Component::Normal(part) => Ok(part),
            _ => Err(bad()),
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let Some((name, parents)) = parts.split_last() else {
        return Err(bad());
    };
    for part in parents {
        directory = openat(&directory, *part, flags, Mode::empty())?;
    }
    let file = std::fs::File::from(openat(
        &directory,
        *name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    if !file.metadata()?.is_file() {
        return Err(bad());
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_note(_root: &Path, _relative: &str) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "confined note reading is unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docs::lock::LockFile;
    use crate::docs::workflow::parse_workflow;
    use serde_json::json;
    use std::rc::Rc;

    fn registry(root: &Path) -> Registry {
        Registry::new(
            root.to_path_buf(),
            vec![],
            LockFile::default(),
            IndexMap::new(),
        )
    }

    #[test]
    fn only_declared_note_positions_are_removed_from_identity() {
        let before = json!({"notes":{"a":{"text":"hello"}},"tables":{"notes":"data"},"steps":{
            "draw":{"uses":"acme/image@1","with":{"notes":"input"}},
            "group":{"steps":{"leaf":{"uses":"acme/image@1","with":{"notes":"nested input"}}},"notes":{"b":{"text":"nested"}}}
        }});
        let expected = json!({"tables":{"notes":"data"},"steps":{
            "draw":{"uses":"acme/image@1","with":{"notes":"input"}},
            "group":{"steps":{"leaf":{"uses":"acme/image@1","with":{"notes":"nested input"}}}}
        }});
        assert_eq!(execution_document(&before), expected);
        assert_eq!(execution_document(&expected), expected);
    }

    #[test]
    fn note_authoring_is_exclusive_bounded_and_group_only() {
        let base = json!({"fx":"workflow/v1","id":"case","title":"Notes","steps":{"draw":{"uses":"fx/image.generate@1","with":{"prompt":"x"}}}});
        for note in [
            json!({}),
            json!({"text":"x","file":"./x.md"}),
            json!({"text":"x","format":"html"}),
            json!({"text":"x","about":"a.b"}),
        ] {
            let mut document = base.clone();
            document["notes"] = json!({"a":note});
            assert!(parse_workflow(&document, "case.yaml").is_err());
        }
        let mut document = base.clone();
        document["notes"] = json!({"a":{"text":"é".repeat(MAX_NOTE_BYTES/2+1)}});
        assert!(
            parse_workflow(&document, "case.yaml")
                .unwrap_err()
                .message
                .contains("256 KiB")
        );
        document["notes"] = json!({"a":{"text":"${{ inputs.example }}","format":"text"}});
        assert!(parse_workflow(&document, "case.yaml").is_ok());
        document["steps"] =
            json!({"draw":{"uses":"fx/image.generate@1","notes":{"x":{"text":"wrong scope"}}}});
        assert!(parse_workflow(&document, "case.yaml").is_err());
    }

    #[test]
    fn local_sources_are_literal_bounded_and_captured_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(
            root.join("setup.md"),
            b"\xef\xbb\xbf# Setup\r\n${{ inputs.example }}\r\n<!-- literal -->",
        )
        .unwrap();
        let document = json!({"fx":"workflow/v1","id":"case","title":"Notes","steps":{"draw":{"uses":"fx/image.generate@1","with":{"prompt":"x"}}},"notes":{"a":{"file":"./setup.md"},"broken":{"file":"./missing.txt","about":"missing"}}});
        let loaded = LoadedWorkflow {
            document: document.clone(),
            workflow: Rc::new(parse_workflow(&document, "case.yaml").unwrap()),
            source: "workflows/case.yaml".into(),
            path: root.join("workflows/case.yaml"),
        };
        let mut registry = registry(&root);
        registry.capture_notes(&loaded);
        assert_eq!(
            registry.note_templates()[0].text.as_deref(),
            Some("# Setup\r\n${{ inputs.example }}\r\n<!-- literal -->")
        );
        assert_eq!(
            registry.note_templates()[0].source.as_ref().unwrap().path,
            "setup.md"
        );
        assert!(
            registry.note_templates()[1]
                .diagnostic
                .as_ref()
                .unwrap()
                .contains("associated step")
        );
        assert!(registry.note_templates()[1].text.is_none());
        std::fs::write(root.join("setup.md"), "changed").unwrap();
        registry.capture_notes(&loaded);
        assert!(
            registry.note_templates()[0]
                .text
                .as_ref()
                .unwrap()
                .starts_with("# Setup")
        );
        let snapshot = registry.note_templates().to_vec();
        let mut resumed = self::registry(&root);
        resumed.restore_note_templates(snapshot.clone());
        resumed.capture_notes(&loaded);
        assert_eq!(resumed.note_templates(), snapshot);
        std::fs::write(root.join("large.md"), vec![b'x'; MAX_NOTE_BYTES + 1]).unwrap();
        assert_eq!(
            read_note(&root, "large.md").unwrap_err(),
            "note source exceeds 256 KiB"
        );
        std::fs::write(root.join("invalid.txt"), [0xff]).unwrap();
        assert_eq!(
            read_note(&root, "invalid.txt").unwrap_err(),
            "note source is not valid UTF-8"
        );
        assert!(read_note(&root, "https://example.test/note.md").is_err());
        assert!(read_note(&root, "/").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn note_sources_refuse_escape_special_files_and_private_labels() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private.md"), "outside").unwrap();
        symlink(outside.path(), root.join("escape")).unwrap();
        assert_eq!(
            read_note(&root, "escape/private.md").unwrap_err(),
            "note source is outside the project"
        );
        std::fs::write(root.join("bad\u{1}.md"), "text").unwrap();
        assert!(read_note(&root, "bad\u{1}.md").is_err());
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        assert!(read_note(&root, "pipe").is_err());
    }
}
