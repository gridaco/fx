//! GitHub sources and their declared or discoverable starter workflows.

use super::{DOWNLOAD_TIMEOUT, MAX_DOWNLOAD, Starter, refused};
use grida_fx_core::{Error, ErrorKind};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

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
        format!(
            "https://codeload.github.com/{}/{}/tar.gz/{}",
            self.owner,
            self.name,
            self.scoped_ref()
        )
    }

    fn scoped_ref(&self) -> String {
        if self.reference.len() == 40 && self.reference.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            self.reference.to_ascii_lowercase()
        } else {
            format!("refs/tags/{}", self.reference)
        }
    }

    /// GitHub's API takes a single path parameter for a fully qualified ref.
    fn archive_endpoint(&self) -> String {
        let reference = self.scoped_ref().replace('/', "%2F").replace('+', "%2B");
        format!("repos/{}/{}/tarball/{reference}", self.owner, self.name)
    }

    /// Run the bounded fetch inside the installer's cooperative interrupt scope.
    pub(super) fn authenticated_archive(&self) -> Result<Vec<u8>, Error> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Error::io("the download", &error))?;
        runtime.block_on(fetch_with_gh(&self.archive_endpoint()))
    }
}

async fn fetch_with_gh(endpoint: &str) -> Result<Vec<u8>, Error> {
    crate::interrupt::check_template_staging()?;
    // Let gh apply its own authentication and follow the archive redirect. Its diagnostics may
    // contain local paths or signed URLs, so only the archive is read back into FX.
    let mut child = tokio::process::Command::new("gh")
        .args(["api", "--hostname", "github.com", endpoint])
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env_remove("GH_DEBUG")
        .env_remove("GH_FORCE_TTY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            let message = if error.kind() == io::ErrorKind::NotFound {
                "--github-auth requires GitHub CLI (gh). Install gh and run gh auth login, then retry."
            } else {
                "could not start GitHub CLI (gh). Check that gh is executable and retry."
            };
            Error::new(ErrorKind::Io, message)
        })?;

    let failed = || {
        Error::new(
            ErrorKind::Io,
            "could not download the template with gh. Check your GitHub CLI sign-in (gh auth login), repository read access, and the tag or commit.",
        )
    };
    let read_archive = async {
        let mut archive = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(failed)?
            .take(MAX_DOWNLOAD + 1)
            .read_to_end(&mut archive)
            .await
            .map_err(|_| failed())?;
        if archive.len() as u64 > MAX_DOWNLOAD {
            return Err(Error::new(
                ErrorKind::Io,
                "the template download exceeds 64 MiB",
            ));
        }
        if !child.wait().await.is_ok_and(|status| status.success()) {
            return Err(failed());
        }
        Ok(archive)
    };
    let result = tokio::select! {
        result = tokio::time::timeout(DOWNLOAD_TIMEOUT, read_archive) => result.unwrap_or_else(|_| {
            Err(Error::new(ErrorKind::Io, "the template download with gh timed out"))
        }),
        error = wait_for_interrupt() => Err(error),
    };
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

async fn wait_for_interrupt() -> Error {
    loop {
        if let Err(error) = crate::interrupt::check_template_staging() {
            return error;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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
        for (reference, path, api_ref) in [
            (
                "v1.2.3+build",
                "refs/tags/v1.2.3+build",
                "refs%2Ftags%2Fv1.2.3%2Bbuild",
            ),
            (
                "release/spring",
                "refs/tags/release/spring",
                "refs%2Ftags%2Frelease%2Fspring",
            ),
            ("0123456", "refs/tags/0123456", "refs%2Ftags%2F0123456"),
            (&sha.to_ascii_uppercase(), sha, sha),
        ] {
            let source = format!("acme/example@{reference}");
            let repository = Repository::parse(&source).unwrap();
            assert_eq!(
                repository.archive_url(),
                format!("https://codeload.github.com/acme/example/tar.gz/{path}")
            );
            assert_eq!(
                repository.archive_endpoint(),
                format!("repos/acme/example/tarball/{api_ref}")
            );
        }
    }
}
