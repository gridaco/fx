//! A planning project's own adapters while planning (spec/adapters.md §9; spec/identity.md §6,
//! §7).
//!
//! [`load_declared`] loads each adapter the planning project declares (`providers.<name>.adapter:
//! <path>#<attribute>`), in the declarations' order, with `adapter.load` on the planning host of
//! the module's language (spec/protocol.md §5.8), after pointing the host at the planning project.
//! It learns the adapter's version and each capability's kind, reads and hashes the source closure
//! the host reports itself ([`crate::registry::closure_digests`], no resources), and holds a
//! versioned adapter's digest to the planning project's fx.lock `adapters`: a mismatch is the
//! adapter's drift, which every step on a route it serves gets as a problem
//! ([`crate::routes::Route::drift`]). What it learns goes into the declaration
//! ([`LoadedAdapter`]), so the route tables read after it complete their contracts from it.
//!
//! An adapter that cannot be loaded refuses the command (exit 2), `fx.yaml:
//! providers.<name>.adapter: <sentence>`, the sentence one of, in the order they are found:
//! - `no file <path>`; `<path> is outside the project`, once symbolic links are resolved;
//! - a suffix refusal of spec/protocol.md §1 ([`Language::of_module`]: `no node host for .<suffix>
//!   modules`, …);
//! - the host's `load_failed` message (`<path> failed to import: …`, `<path>: <attribute> is not
//!   declared with adapter()`, the SDK to upgrade for a host that predates adapters), or why the
//!   host is unavailable;
//! - `<path>#<attribute> declares no capability`;
//! - a closure the engine cannot hash (`cannot read <label>: …`).

use crate::docs::project::Project;
use crate::error::{Error, Result};
use crate::host::{HostFailure, Language, NodeHost};
use crate::providers::{DeclaredAdapter, LoadedAdapter};
use crate::registry::relative_inside;
use grida_fx_protocol::AdapterLoadParams;
use std::collections::BTreeMap;
use std::path::Path;

/// The file a declaration writes its adapters' refusals against.
const PROJECT_FILE: &str = "fx.yaml";

/// Loads every adapter `project` declares into its declarations (module doc), judged against
/// its fx.lock. Reads the lock and points `host` at the project only when it declares one.
pub fn load_declared(project: &mut Project, host: &mut dyn NodeHost) -> Result<()> {
    if project
        .document
        .providers
        .iter()
        .all(|declaration| declaration.adapter.is_none())
    {
        return Ok(());
    }
    let adapter_locks = crate::project::read_lock_at(&project.root)?.adapters;
    load_judged(project, &adapter_locks, host)
}

/// [`load_declared`] with `adapter_locks`, the project's fx.lock `adapters`.
fn load_judged(
    project: &mut Project,
    adapter_locks: &BTreeMap<String, String>,
    host: &mut dyn NodeHost,
) -> Result<()> {
    host.open_project(&project.root, &project.document.sources)
        .map_err(Error::from)?;
    let root = project.root.clone();
    for declaration in &mut project.document.providers {
        let Some(adapter) = &mut declaration.adapter else {
            continue;
        };
        let mut loaded = load(&root, adapter, host).map_err(|sentence| {
            Error::plan(format!(
                "{PROJECT_FILE}: providers.{}.adapter: {sentence}",
                declaration.name
            ))
        })?;
        loaded.drift = drift(adapter, &loaded, adapter_locks);
        adapter.loaded = Some(loaded);
    }
    Ok(())
}

/// Loads one adapter of the project at `root` (module doc), its drift not judged. The error is
/// the sentence after `fx.yaml: providers.<name>.adapter: `.
pub fn load(
    root: &Path,
    adapter: &DeclaredAdapter,
    host: &mut dyn NodeHost,
) -> std::result::Result<LoadedAdapter, String> {
    let path = &adapter.path;
    let file = root.join(path);
    if !file.is_file() {
        return Err(format!("no file {path}"));
    }
    relative_inside(root, &file).ok_or_else(|| format!("{path} is outside the project"))?;
    Language::of_module(path)?;
    let answer = host
        .load_adapter(&AdapterLoadParams {
            path: path.clone(),
            attribute: adapter.attribute.clone(),
        })
        .map_err(|failure| match failure {
            HostFailure::Rpc(error) => error.message,
            HostFailure::Unavailable(message) => message,
        })?;
    if answer.adapter.capabilities.is_empty() {
        return Err(format!("{} declares no capability", adapter.name()));
    }
    let source = crate::registry::closure_digests(&answer.closure)?;
    Ok(LoadedAdapter {
        version: answer.adapter.version,
        capabilities: answer.adapter.capabilities,
        source,
        drift: None,
    })
}

/// The fx.lock key of a versioned adapter: `<path>#<attribute>@<version>`.
pub fn lock_key(adapter: &DeclaredAdapter, version: u64) -> String {
    format!("{}@{version}", adapter.name())
}

/// The drift of a versioned adapter whose source no longer matches its fx.lock entry (module
/// doc); `None` without a version or an entry, or when it matches.
fn drift(
    adapter: &DeclaredAdapter,
    loaded: &LoadedAdapter,
    adapter_locks: &BTreeMap<String, String>,
) -> Option<String> {
    let version = loaded.version?;
    let locked = adapter_locks.get(&lock_key(adapter, version))?;
    let name = adapter.name();
    (*locked != loaded.source.digest()).then(|| {
        format!(
            "{name}: its source changed but version {version} did not; bump the version, or \
             confirm no change in behaviour with grida-fx lock --same {name}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FakeHost;
    use grida_fx_protocol::{
        AdapterDeclaration, AdapterLoadResult, CallKind, ClosureEntry, ErrorCode, RpcError,
    };
    use indexmap::IndexMap;

    /// A project at a new folder with `fx.yaml` and `adapters/acme.py`.
    fn project(providers: &str) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(
            root.join("fx.yaml"),
            format!("fx: project/v1\nproviders:\n{providers}"),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("adapters")).unwrap();
        std::fs::write(root.join("adapters/acme.py"), "acme = adapter()\n").unwrap();
        let project = Project::find(&root).unwrap();
        (dir, project)
    }

    const ACME: &str = "  acme:\n    base_url: { default: \"https://api.acme.test\" }\n    \
                        adapter: adapters/acme.py#acme\n";

    fn answer(root: &Path, version: Option<u64>, kinds: &[(&str, CallKind)]) -> AdapterLoadResult {
        AdapterLoadResult {
            adapter: AdapterDeclaration {
                version,
                capabilities: kinds
                    .iter()
                    .map(|(capability, kind)| (capability.to_string(), *kind))
                    .collect::<IndexMap<_, _>>(),
            },
            closure: vec![ClosureEntry {
                label: "adapters/acme.py".into(),
                path: root.join("adapters/acme.py").to_string_lossy().into_owned(),
            }],
        }
    }

    fn refusal(project: &mut Project, host: &mut FakeHost) -> String {
        load_judged(project, &BTreeMap::new(), host)
            .unwrap_err()
            .message
    }

    #[test]
    fn a_declared_adapter_is_loaded_into_its_declaration() {
        let (_dir, mut project) = project(ACME);
        let root = project.root.clone();
        let mut host = FakeHost::new().with_adapter(
            "adapters/acme.py#acme",
            Ok(answer(
                &root,
                Some(2),
                &[
                    ("image.generate", CallKind::Request),
                    ("video.generate", CallKind::Job),
                ],
            )),
        );
        load_judged(&mut project, &BTreeMap::new(), &mut host).unwrap();
        assert_eq!(host.adapter_calls, 1);
        let loaded = project.document.providers[0]
            .adapter
            .as_ref()
            .unwrap()
            .loaded
            .clone()
            .unwrap();
        assert_eq!(loaded.version, Some(2));
        assert_eq!(loaded.capabilities["video.generate"], CallKind::Job);
        let digest = crate::value::file_digest(b"acme = adapter()\n");
        assert_eq!(loaded.source.files["adapters/acme.py"], digest);
        assert!(loaded.source.resources.is_empty());
        assert_eq!(loaded.drift, None);
        assert_eq!(loaded.behavior(), serde_json::json!(2));
        // The declarations carry it, so the route tables read next complete contracts from it.
        let declarations = project.declarations();
        let declared = declarations.get("acme").unwrap();
        assert!(declared.adapter.as_ref().unwrap().loaded.is_some());
    }

    #[test]
    fn a_project_without_adapters_asks_no_host() {
        let (_dir, mut project) =
            project("  acme:\n    base_url: { default: \"https://api.acme.test\" }\n");
        let mut host = FakeHost::new();
        load_declared(&mut project, &mut host).unwrap();
        assert_eq!(host.adapter_calls, 0);
    }

    #[test]
    fn a_locked_adapter_whose_source_changed_drifts() {
        let (_dir, mut project) = project(ACME);
        let root = project.root.clone();
        let mut host = FakeHost::new().with_adapter(
            "adapters/acme.py#acme",
            Ok(answer(
                &root,
                Some(2),
                &[("image.generate", CallKind::Request)],
            )),
        );
        let locks: BTreeMap<String, String> =
            [("adapters/acme.py#acme@2".to_string(), "0".repeat(64))].into();
        load_judged(&mut project, &locks, &mut host).unwrap();
        let loaded = project.document.providers[0].adapter.as_ref().unwrap();
        assert_eq!(
            loaded.loaded.as_ref().unwrap().drift.as_deref(),
            Some(
                "adapters/acme.py#acme: its source changed but version 2 did not; bump the \
                 version, or confirm no change in behaviour with grida-fx lock --same \
                 adapters/acme.py#acme"
            )
        );
        // An entry of another version, or the same digest, is no drift.
        let (_dir, mut project) = self::project(ACME);
        let root = project.root.clone();
        let mut host = FakeHost::new().with_adapter(
            "adapters/acme.py#acme",
            Ok(answer(
                &root,
                Some(2),
                &[("image.generate", CallKind::Request)],
            )),
        );
        let digest = crate::registry::SourceDigests {
            files: [(
                "adapters/acme.py".to_string(),
                crate::value::file_digest(b"acme = adapter()\n"),
            )]
            .into(),
            resources: BTreeMap::new(),
        }
        .digest();
        let locks: BTreeMap<String, String> = [
            ("adapters/acme.py#acme@1".to_string(), "0".repeat(64)),
            ("adapters/acme.py#acme@2".to_string(), digest),
        ]
        .into();
        load_judged(&mut project, &locks, &mut host).unwrap();
        let loaded = project.document.providers[0].adapter.as_ref().unwrap();
        assert_eq!(loaded.loaded.as_ref().unwrap().drift, None);
    }

    #[test]
    fn an_adapter_that_cannot_be_loaded_refuses_the_command() {
        let at = "fx.yaml: providers.acme.adapter: ";
        let (_dir, mut project) = project(
            "  acme:\n    base_url: { default: \"https://api.acme.test\" }\n    \
             adapter: adapters/missing.py#acme\n",
        );
        assert_eq!(
            refusal(&mut project, &mut FakeHost::new()),
            format!("{at}no file adapters/missing.py")
        );
        let (dir, mut project) = self::project(
            "  acme:\n    base_url: { default: \"https://api.acme.test\" }\n    \
             adapter: adapters/acme.rb#acme\n",
        );
        std::fs::write(dir.path().join("adapters/acme.rb"), "").unwrap();
        assert_eq!(
            refusal(&mut project, &mut FakeHost::new()),
            format!("{at}no node host for .rb modules yet")
        );
        let (_dir, mut project) = self::project(ACME);
        assert_eq!(
            refusal(&mut project, &mut FakeHost::new()),
            format!("{at}adapters/acme.py: acme is not declared with adapter()")
        );
        let mut old = FakeHost::new().with_adapter(
            "adapters/acme.py#acme",
            Err(HostFailure::Rpc(RpcError::new(
                ErrorCode::LoadFailed,
                "the project's grida 0.6.1 cannot load adapters: upgrade grida",
            ))),
        );
        assert_eq!(
            refusal(&mut project, &mut old),
            format!("{at}the project's grida 0.6.1 cannot load adapters: upgrade grida")
        );
        let mut unavailable = FakeHost::new().with_adapter(
            "adapters/acme.py#acme",
            Err(HostFailure::Unavailable(
                "the node host answered adapter.load with something FX cannot read: x".into(),
            )),
        );
        assert_eq!(
            refusal(&mut project, &mut unavailable),
            format!("{at}the node host answered adapter.load with something FX cannot read: x")
        );
        let root = project.root.clone();
        let mut empty =
            FakeHost::new().with_adapter("adapters/acme.py#acme", Ok(answer(&root, None, &[])));
        assert_eq!(
            refusal(&mut project, &mut empty),
            format!("{at}adapters/acme.py#acme declares no capability")
        );
        let mut gone = answer(&root, None, &[("image.generate", CallKind::Request)]);
        gone.closure[0].path = root.join("adapters/gone.py").to_string_lossy().into_owned();
        gone.closure[0].label = "adapters/gone.py".into();
        let mut unreadable = FakeHost::new().with_adapter("adapters/acme.py#acme", Ok(gone));
        assert_eq!(
            refusal(&mut project, &mut unreadable),
            format!("{at}cannot read adapters/gone.py: no such file")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_adapter_outside_the_project_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("acme.py"), "").unwrap();
        let (dir, mut project) = project(
            "  acme:\n    base_url: { default: \"https://api.acme.test\" }\n    \
             adapter: linked/acme.py#acme\n",
        );
        std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
        assert_eq!(
            refusal(&mut project, &mut FakeHost::new()),
            "fx.yaml: providers.acme.adapter: linked/acme.py is outside the project"
        );
    }
}
