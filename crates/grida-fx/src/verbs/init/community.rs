//! Public GitHub sources and their declared or discoverable starter workflows.

use super::{Starter, refused};
use grida_fx_core::Error;
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

const MANIFEST: &str = "fx-template.json";

pub(super) struct Repository<'a> {
    owner: &'a str,
    pub(super) name: &'a str,
    reference: &'a str,
}

impl<'a> Repository<'a> {
    pub(super) fn parse(source: &'a str) -> Result<Self, Error> {
        let invalid = || {
            Error::usage("a GitHub template must be OWNER/REPO@REF with a tag or full commit SHA")
        };
        let (repository, reference) = source.split_once('@').ok_or_else(invalid)?;
        let (owner, name) = repository.split_once('/').ok_or_else(invalid)?;
        let owner_ok = !owner.is_empty()
            && owner.len() <= 39
            && !owner.starts_with('-')
            && !owner.ends_with('-')
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        let name_ok = !matches!(name, "" | "." | "..")
            && name.len() <= 100
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte));
        let ref_ok = reference.len() <= 255
            && !reference.ends_with('.')
            && !reference.contains("..")
            && reference.split('/').all(|part| {
                !part.is_empty()
                    && !part.starts_with('.')
                    && !part.ends_with(".lock")
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-+".contains(&byte))
            });
        if !owner_ok || !name_ok || !ref_ok {
            return Err(invalid());
        }
        Ok(Self {
            owner,
            name,
            reference,
        })
    }

    pub(super) fn archive_url(&self) -> String {
        let scoped_ref = if self.reference.len() == 40
            && self.reference.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            self.reference.to_ascii_lowercase()
        } else {
            format!("refs/tags/{}", self.reference)
        };
        format!(
            "https://codeload.github.com/{}/{}/tar.gz/{scoped_ref}",
            self.owner, self.name
        )
    }
}

/// The optional manifest chooses the default. Without it, exactly one matching workflow is
/// needed. Selecting the manifest's workflow keeps its authored preset and prerequisites.
pub(super) fn starter(project: &Path, selection: Option<&str>) -> Result<Starter, Error> {
    let manifest = read_manifest(project)?;
    let declared_entry = manifest.as_ref().map(|entry| entry.workflow_entry.as_str());
    if selection.is_none()
        && let Some(manifest) = manifest
    {
        return Ok(manifest);
    }

    let mut candidates = BTreeSet::new();
    let mut unselected = Vec::new();
    if let Some(entry) = declared_entry {
        candidates.insert(entry.to_owned());
    }
    for directory in ["workflows", "fx"] {
        let folder = project.join(directory);
        if !folder.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&folder).map_err(|error| Error::io(directory, &error))? {
            crate::interrupt::check_template_staging()?;
            let entry = entry.map_err(|error| Error::io(directory, &error))?;
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yaml" | "yml")
            ) || !entry
                .file_type()
                .map_err(|error| Error::io(directory, &error))?
                .is_file()
            {
                continue;
            }
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| refused("a template path is not UTF-8"))?;
            let relative = format!("{directory}/{name}");
            if selection.is_some_and(|name| workflow_name(&relative) != Some(name)) {
                unselected.push(relative);
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|error| Error::io(&relative, &error))?;
            if grida_fx_core::yaml::declares_kind(&bytes, "workflow/v1") {
                candidates.insert(relative);
            }
        }
    }

    let matches = |entry: &&String| selection.is_none_or(|name| workflow_name(entry) == Some(name));
    let mut matching = candidates.iter().filter(matches);
    if let Some(entry) = matching.next()
        && matching.next().is_none()
    {
        if declared_entry == Some(entry)
            && let Some(manifest) = manifest
        {
            return Ok(manifest);
        }
        return Ok(Starter {
            workflow_entry: entry.clone(),
            inputs_file: None,
            required_inputs: Vec::new(),
            prerequisites: Vec::new(),
        });
    }
    if candidates.is_empty() && unselected.is_empty() {
        return Err(refused("no starter workflow found in workflows/ or fx/"));
    }
    candidates.extend(unselected);
    let available = candidates.into_iter().collect::<Vec<_>>().join(", ");
    let why = match selection {
        None => format!("multiple starter workflows; use --workflow NAME; available: {available}"),
        Some(name) => format!(
            "--workflow {name:?} did not identify exactly one workflow; YAML files: {available}"
        ),
    };
    Err(refused(why))
}

fn workflow_name(entry: &str) -> Option<&str> {
    Path::new(entry).file_stem().and_then(|stem| stem.to_str())
}

fn read_manifest(project: &Path) -> Result<Option<Starter>, Error> {
    let bytes = match std::fs::read(project.join(MANIFEST)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io(MANIFEST, &error)),
    };
    let entry = serde_json::from_slice(&bytes)
        .map_err(|error| refused(format!("{MANIFEST} is not valid JSON: {error}")))?;
    Starter::parse(&entry)
        .map(Some)
        .ok_or_else(|| refused(format!("{MANIFEST} is not a valid starter declaration")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls_use_the_tag_namespace_or_the_full_commit() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        for (reference, path) in [
            ("v1.2.3+build", "refs/tags/v1.2.3+build"),
            ("release/spring", "refs/tags/release/spring"),
            ("0123456", "refs/tags/0123456"),
            (sha, sha),
        ] {
            let source = format!("acme/example@{reference}");
            let repository = Repository::parse(&source).unwrap();
            assert_eq!(
                repository.archive_url(),
                format!("https://codeload.github.com/acme/example/tar.gz/{path}")
            );
        }
    }
}
