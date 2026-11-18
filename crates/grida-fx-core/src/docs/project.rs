//! The project file, `fx.yaml` (`fx: project/v1`; spec/schemas/fx-project-v1.schema.json).
//!
//! `Project::find(start)` walks up from `start` (a file's folder, or the folder itself) to the
//! first folder holding a regular file `fx.yaml`. With none, the project root is `start` and the
//! document is all defaults (`runs: runs`, `cache: .fx/cache`, `nodes: nodes`,
//! `workflows: [workflows]`). An invalid `fx.yaml` is an error (exit 2).
//!
//! The start is made absolute and its symbolic links are resolved (as far as it exists), as
//! Python's `Path.resolve()` does. Messages name the file `fx.yaml`. A `nodes:` folder with a
//! `..` segment reads `nodes is a folder inside the project`. A `workflows:`
//! folder that is the project root or a folder above it, lexically or once its symbolic links
//! are resolved, is refused when the file is found (spec/store.md, *Finding a workflow by id*):
//! searching it would read the project's own runs and store. `providers:` declares the project's
//! own providers, refused as [`crate::providers`] says.

use super::workflow::Budget;
use super::{Schema, check_kind, validate};
use crate::error::{Error, Result};
use crate::money::Usd;
use crate::providers::{Declaration, Declarations};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// The project file's name.
const PROJECT_FILE: &str = "fx.yaml";

/// A capability's default model selector in fx.yaml `routes:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDefault {
    pub route: String,
    pub concurrency: Option<u32>,
}

/// Project-wide presentation defaults, separate from workflow execution and recorded data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewerConfig {
    pub display: ViewerDisplay,
}

impl ViewerConfig {
    /// Rereads only the bound project's `fx.yaml`; removing it restores defaults instead of
    /// switching to a different ancestor project. An invalid or unreadable file is an error.
    pub fn read(root: &Path) -> Result<Self> {
        let path = root.join(PROJECT_FILE);
        match std::fs::metadata(&path) {
            Ok(_) => read_project(&path, PROJECT_FILE).map(|document| document.viewer),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::io(PROJECT_FILE, &error)),
        }
    }
}

/// Visible canvas content. Omitted settings preserve the viewer's normal presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewerDisplay {
    pub workflow_inputs: bool,
    pub workflow_outputs: bool,
    pub notes: bool,
}

impl Default for ViewerDisplay {
    fn default() -> Self {
        Self {
            workflow_inputs: true,
            workflow_outputs: true,
            notes: true,
        }
    }
}

/// The typed project file.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDoc {
    pub runs: String,
    pub cache: String,
    pub budget: Option<Budget>,
    pub routes: IndexMap<String, RouteDefault>,
    /// One explicitly preferred provider per capability (spec/model-selection.md).
    pub provider_preferences: IndexMap<String, String>,
    pub route_tables: Vec<String>,
    /// The providers it declares, in its order (spec/providers.md §1).
    pub providers: Vec<Declaration>,
    pub view_origins: Vec<String>,
    pub sources: Vec<String>,
    pub nodes: String,
    /// The folders searched for a workflow by id after the root's own files, as written:
    /// relative to the root, or absolute ([`crate::project::project_workflow_files`]).
    pub workflows: Vec<String>,
    /// Viewer defaults read from the current project, never executable workflow metadata.
    pub viewer: ViewerConfig,
}

impl Default for ProjectDoc {
    fn default() -> Self {
        Self {
            runs: "runs".into(),
            cache: ".fx/cache".into(),
            budget: None,
            routes: IndexMap::new(),
            provider_preferences: IndexMap::new(),
            route_tables: Vec::new(),
            providers: Vec::new(),
            view_origins: Vec::new(),
            sources: Vec::new(),
            nodes: "nodes".into(),
            workflows: vec!["workflows".into()],
            viewer: ViewerConfig::default(),
        }
    }
}

/// A project: its root (absolute, symlinks resolved) and its document.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    pub document: ProjectDoc,
    /// Whether an `fx.yaml` was found.
    pub has_file: bool,
}

impl Project {
    /// Validates a staged project file against the location where its folder will be installed.
    pub fn validate_installation(staging: &Path, destination: &Path) -> Result<()> {
        let document = read_project(&staging.join(PROJECT_FILE), PROJECT_FILE)?;
        refuse_holding_folders(destination, &document, PROJECT_FILE, Some(staging))
    }

    /// Finds the project of `start` (module doc).
    pub fn find(start: &Path) -> Result<Project> {
        let mut here = crate::inputs::bind::resolve(start);
        if here.is_file()
            && let Some(parent) = here.parent()
        {
            here = parent.to_path_buf();
        }
        for folder in here.ancestors() {
            let candidate = folder.join(PROJECT_FILE);
            if candidate.is_file() {
                let document = read_project(&candidate, PROJECT_FILE)?;
                refuse_holding_folders(folder, &document, PROJECT_FILE, None)?;
                return Ok(Project {
                    root: folder.to_path_buf(),
                    document,
                    has_file: true,
                });
            }
        }
        Ok(Project {
            root: here,
            document: ProjectDoc::default(),
            has_file: false,
        })
    }

    /// The store root, `root / cache`.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(&self.document.cache)
    }

    /// The run folders' root, `root / runs`.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join(&self.document.runs)
    }

    /// `{capability: route id}` from `routes:`.
    pub fn route_defaults(&self) -> IndexMap<String, String> {
        self.document
            .routes
            .iter()
            .map(|(k, v)| (k.clone(), v.route.clone()))
            .collect()
    }

    /// FX's provider declarations, then this project's (spec/providers.md §1).
    pub fn declarations(&self) -> Declarations {
        Declarations::with(&self.document.providers)
    }
}

/// Reads and types a project file. `label` names it in messages.
fn read_project(path: &Path, label: &str) -> Result<ProjectDoc> {
    let value = crate::yaml::load_file(path, label)?;
    parse_project(&value, label)
}

/// Refuses a `workflows:` entry that is the project root `root` or a folder above it, compared
/// lexically and with symbolic links resolved (module doc).
fn refuse_holding_folders(
    root: &Path,
    document: &ProjectDoc,
    label: &str,
    staging: Option<&Path>,
) -> Result<()> {
    let root = crate::inputs::bind::resolve(root);
    for entry in &document.workflows {
        let listed = root.join(entry);
        let holds = |folder: PathBuf| root.starts_with(folder);
        let resolved = || match staging {
            None => crate::inputs::bind::resolve(&listed),
            Some(staging) => crate::inputs::bind::resolve_with(&listed, |path| {
                canonicalize_installation(path, &root, staging, &mut 40)
            }),
        };
        if holds(crate::inputs::lexical(&listed)) || holds(resolved()) {
            return Err(Error::document(format!(
                "{label}: workflows: {entry} is the project or holds it; list the folders under it \
                 that hold workflows"
            )));
        }
    }
    Ok(())
}

/// Canonicalizes an existing path in the layout after publication. A currently dangling
/// symlink can become valid when its target is supplied by the staged project tree.
fn canonicalize_installation(
    path: &Path,
    destination: &Path,
    staging: &Path,
    links_left: &mut usize,
) -> Option<PathBuf> {
    if let Ok(resolved) = path.canonicalize() {
        return Some(resolved);
    }
    let on_disk = |path: &Path| match path.strip_prefix(destination) {
        Ok(inside) => staging.join(inside),
        Err(_) => path.to_path_buf(),
    };
    let mut resolved = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                let physical = on_disk(&resolved);
                let metadata = std::fs::symlink_metadata(&physical).ok()?;
                if metadata.is_symlink() {
                    *links_left = links_left.checked_sub(1)?;
                    let target = std::fs::read_link(&physical).ok()?;
                    let target = resolved.parent()?.join(target);
                    resolved =
                        canonicalize_installation(&target, destination, staging, links_left)?;
                    if components.peek().is_some() && !on_disk(&resolved).is_dir() {
                        return None;
                    }
                } else if components.peek().is_some() && !metadata.is_dir() {
                    return None;
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    // Preserve trailing-separator behavior from the local filesystem's canonicalizer.
    if path
        .as_os_str()
        .as_encoded_bytes()
        .last()
        .is_some_and(|byte| std::path::is_separator(char::from(*byte)))
    {
        let mut with_separator = on_disk(&resolved).into_os_string();
        with_separator.push(std::path::MAIN_SEPARATOR_STR);
        Path::new(&with_separator).canonicalize().ok()?;
    }
    Some(resolved)
}

/// Validates and types a project document (fx-project-v1).
pub(crate) fn parse_project(value: &Value, label: &str) -> Result<ProjectDoc> {
    check_kind(Schema::Project, value, label)?;
    if let Some(Value::String(nodes)) = value.get("nodes")
        && nodes.split('/').any(|segment| segment == "..")
    {
        return Err(Error::document(format!(
            "{label}: nodes: nodes is a folder inside the project"
        )));
    }
    validate(Schema::Project, value, label)?;
    let refuse =
        |where_: &str, message: &str| Error::document(format!("{label}: {where_}: {message}"));
    let text = |key: &str, default: &str| -> Result<String> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(default.to_string()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(refuse(key, "expected text")),
        }
    };
    let texts = |key: &str| -> Result<Vec<String>> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    item.as_str()
                        .map(String::from)
                        .ok_or_else(|| refuse(&format!("{key}.{i}"), "expected text"))
                })
                .collect(),
            Some(_) => Err(refuse(key, "expected a list")),
        }
    };
    let budget = match value.get("budget") {
        None | Some(Value::Null) => None,
        Some(budget) => {
            let amount = budget
                .get("max_usd")
                .ok_or_else(|| refuse("budget.max_usd", "a budget names max_usd"))?;
            let max_usd =
                Usd::from_value(amount).map_err(|message| refuse("budget.max_usd", &message))?;
            Some(Budget { max_usd })
        }
    };
    let mut routes = IndexMap::new();
    if let Some(Value::Object(entries)) = value.get("routes") {
        for (capability, entry) in entries {
            let where_ = format!("routes.{capability}");
            let default = match entry {
                Value::String(route) => RouteDefault {
                    route: route.clone(),
                    concurrency: None,
                },
                Value::Object(map) => RouteDefault {
                    route: map
                        .get("route")
                        .and_then(Value::as_str)
                        .ok_or_else(|| refuse(&where_, "a route default names its route"))?
                        .to_string(),
                    concurrency: match map.get("concurrency") {
                        None | Some(Value::Null) => None,
                        Some(Value::Number(n)) => {
                            let x = crate::value::as_f64(n);
                            if x.fract() != 0.0 || !(1.0..=1024.0).contains(&x) {
                                return Err(refuse(
                                    &format!("{where_}.concurrency"),
                                    "out of range",
                                ));
                            }
                            Some(x as u32)
                        }
                        Some(_) => {
                            return Err(refuse(
                                &format!("{where_}.concurrency"),
                                "expected a number",
                            ));
                        }
                    },
                },
                _ => return Err(refuse(&where_, "a route default is a route or a mapping")),
            };
            routes.insert(capability.clone(), default);
        }
    }
    Ok(ProjectDoc {
        runs: text("runs", "runs")?,
        cache: text("cache", ".fx/cache")?,
        budget,
        routes,
        provider_preferences: value
            .get("provider_preferences")
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(capability, provider)| {
                        (
                            capability.clone(),
                            provider
                                .as_str()
                                .expect("validated provider name")
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        route_tables: texts("route_tables")?,
        providers: crate::providers::parse(value.get("providers"), label)?,
        view_origins: texts("view_origins")?,
        sources: texts("sources")?,
        nodes: text("nodes", "nodes")?,
        workflows: match value.get("workflows") {
            None | Some(Value::Null) => ProjectDoc::default().workflows,
            Some(_) => texts("workflows")?,
        },
        viewer: value
            .get("viewer")
            .map(|viewer| {
                serde_json::from_value(viewer.clone()).expect("validated viewer settings")
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_project_document_is_typed() {
        let document = json!({
            "fx": "project/v1",
            "runs": "out/runs",
            "routes": {
                "image.generate": "img-a@acme",
                "vision.review": {"route": "vlm-c@other", "concurrency": 2},
            },
            "route_tables": ["routes.yaml"],
            "view_origins": ["https://example.test"],
            "sources": ["acme_nodes"],
            "nodes": "src/nodes",
            "workflows": ["library", "../shared", "/abs/flows"],
        });
        let project = parse_project(&document, "fx.yaml").unwrap();
        assert_eq!(project.runs, "out/runs");
        assert_eq!(project.cache, ".fx/cache");
        assert_eq!(project.nodes, "src/nodes");
        assert_eq!(project.workflows, ["library", "../shared", "/abs/flows"]);
        assert_eq!(project.route_tables, ["routes.yaml"]);
        assert_eq!(project.sources, ["acme_nodes"]);
        assert_eq!(project.view_origins, ["https://example.test"]);
        assert_eq!(
            project.routes["vision.review"],
            RouteDefault {
                route: "vlm-c@other".into(),
                concurrency: Some(2)
            }
        );
        assert_eq!(
            project.routes["image.generate"],
            RouteDefault {
                route: "img-a@acme".into(),
                concurrency: None
            }
        );
        let defaults = parse_project(&json!({"fx": "project/v1"}), "fx.yaml").unwrap();
        assert_eq!(defaults, ProjectDoc::default());
        assert_eq!(defaults.workflows, ["workflows"]);
        // The setting replaces the default; an empty list searches only the root's own files.
        let none = parse_project(&json!({"fx": "project/v1", "workflows": []}), "fx.yaml");
        assert!(none.unwrap().workflows.is_empty());
    }

    #[test]
    fn project_refusals() {
        let message = |document: Value| parse_project(&document, "fx.yaml").unwrap_err().message;
        assert_eq!(
            message(json!({"fx": "workflow/v1"})),
            "fx.yaml: a project file starts with fx: project/v1"
        );
        assert_eq!(
            message(json!({"fx": "project/v1", "nodes": "a/../b"})),
            "fx.yaml: nodes: nodes is a folder inside the project"
        );
        assert!(
            message(json!({"fx": "project/v1", "routes": {"image.generate": "nope@"}}))
                .starts_with("fx.yaml: routes.image.generate: ")
        );
        assert!(
            message(json!({"fx": "project/v1", "colour": 1})).starts_with("fx.yaml: (document): ")
        );
        assert!(
            message(json!({"fx": "project/v1", "route_tables": ["a", "a"]}))
                .starts_with("fx.yaml: route_tables: ")
        );
        for workflows in [
            json!(["a", "a"]),
            json!([""]),
            json!(["a", 1]),
            json!("a"),
            json!(null),
        ] {
            let found = message(json!({"fx": "project/v1", "workflows": workflows}));
            assert!(found.starts_with("fx.yaml: workflows"), "{found}");
        }
    }

    #[test]
    fn model_defaults_and_provider_preferences_are_separate() {
        let project = parse_project(
            &json!({
                "fx": "project/v1",
                "routes": {
                    "image.generate": "acme/img-a",
                    "mesh.generate": {"route": "mesh-a@acme", "concurrency": 2}
                },
                "provider_preferences": {"image.generate": "other"}
            }),
            "fx.yaml",
        )
        .unwrap();
        assert_eq!(project.routes["image.generate"].route, "acme/img-a");
        assert_eq!(project.routes["mesh.generate"].concurrency, Some(2));
        assert_eq!(project.provider_preferences["image.generate"], "other");
        for preference in [json!(""), json!("Upper"), json!(["acme", "other"])] {
            assert!(
                parse_project(
                    &json!({
                        "fx": "project/v1",
                        "provider_preferences": {"image.generate": preference}
                    }),
                    "fx.yaml"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn viewer_display_defaults_are_independent_and_strict() {
        let project = parse_project(
            &json!({"fx":"project/v1", "viewer":{"display":{"workflow_inputs":false,"notes":false}}}),
            "fx.yaml",
        ).unwrap();
        assert_eq!(
            project.viewer.display,
            ViewerDisplay {
                workflow_inputs: false,
                workflow_outputs: true,
                notes: false,
            }
        );
        for viewer in [
            json!(null),
            json!({"display":null}),
            json!({"display":{"notes":"false"}}),
            json!({"display":{"workflow_outputs":0}}),
            json!({"display":{"step_media_previews":false}}),
            json!({"display":{"show_workflow_inputs":false}}),
            json!({"unknown":true}),
        ] {
            assert!(parse_project(&json!({"fx":"project/v1","viewer":viewer}), "fx.yaml").is_err());
        }
    }

    #[test]
    fn bound_viewer_config_reloads_only_its_own_project() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("project");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(
            directory.path().join("fx.yaml"),
            "fx: project/v1\nviewer: {display: {notes: false}}\n",
        )
        .unwrap();
        assert_eq!(ViewerConfig::read(&child).unwrap(), ViewerConfig::default());
        std::fs::write(
            child.join("fx.yaml"),
            "fx: project/v1\nviewer: {display: {workflow_outputs: false}}\n",
        )
        .unwrap();
        assert!(!ViewerConfig::read(&child).unwrap().display.workflow_outputs);
        std::fs::write(
            child.join("fx.yaml"),
            "fx: project/v1\nviewer: {display: {notes: invalid}}\n",
        )
        .unwrap();
        assert!(ViewerConfig::read(&child).is_err());
        std::fs::remove_file(child.join("fx.yaml")).unwrap();
        assert!(ViewerConfig::read(&child).unwrap().display.notes);
    }

    #[test]
    fn a_project_declares_providers() {
        let document = json!({
            "fx": "project/v1",
            "providers": {
                "acme": {
                    "credential": {"variable": "ACME_API_KEY"},
                    "base_url": {"default": "https://api.acme.test/v1"},
                    "family": "openai-compatible",
                },
            },
        });
        let project = parse_project(&document, "fx.yaml").unwrap();
        assert_eq!(project.providers.len(), 1);
        assert_eq!(project.providers[0].name, "acme");
        let message = |providers: Value| {
            parse_project(
                &json!({"fx": "project/v1", "providers": providers}),
                "fx.yaml",
            )
            .unwrap_err()
            .message
        };
        let base = json!({"default": "https://api.acme.test"});
        // The schema's refusals come first, then the declarations' own.
        for bad in [
            json!({"Acme": {"base_url": base}}),
            json!({"acme": {}}),
            json!({"acme": {"base_url": base, "family": "acme-wire"}}),
            json!({"acme": {"base_url": base, "credential": {"variable": "1KEY"}}}),
            json!({"acme": {"base_url": base, "download_hosts": ["*.test"]}}),
            json!({"acme": {"base_url": base, "credential": {"variable": "K", "header": "X-Key"}}}),
        ] {
            let found = message(bad.clone());
            assert!(found.starts_with("fx.yaml: providers"), "{bad}: {found}");
        }
        assert_eq!(
            message(json!({"fal": {"base_url": base}})),
            "fx.yaml: providers.fal: fal is a provider FX ships"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_workflows_folder_that_holds_the_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("proj");
        std::fs::create_dir_all(root.join("library/thing")).unwrap();
        std::fs::create_dir_all(root.join("ok")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("ok/up")).unwrap();
        let refused = |entry: &str| {
            let document = ProjectDoc {
                workflows: vec!["library".into(), entry.into()],
                ..ProjectDoc::default()
            };
            refuse_holding_folders(&root, &document, "fx.yaml", None).map_err(|error| error.message)
        };
        let above = root.parent().unwrap().to_string_lossy().into_owned();
        let itself = root.to_string_lossy().into_owned();
        for entry in [
            ".",
            "./",
            "..",
            "../..",
            "library/..",
            "library/thing/../..",
            "../proj",
            "ok/up",
            "/",
            above.as_str(),
            itself.as_str(),
        ] {
            assert_eq!(
                refused(entry),
                Err(format!(
                    "fx.yaml: workflows: {entry} is the project or holds it; list the folders \
                     under it that hold workflows"
                )),
                "{entry}"
            );
        }
        let inside = root.join("library").to_string_lossy().into_owned();
        for entry in [
            "library/thing",
            "missing",
            "../sibling",
            "../proj/library",
            "ok/up/library",
            "/elsewhere/flows",
            inside.as_str(),
        ] {
            assert_eq!(refused(entry), Ok(()), "{entry}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_staged_regular_file_does_not_resolve_through_a_directory_link() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let staging = root.join("staged");
        let destination = root.join("fox");
        let external = root.join("external");
        std::fs::create_dir(&staging).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::write(
            staging.join("fx.yaml"),
            "fx: project/v1\nworkflows: [\"../external/alias/..\"]\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("../fox/fx.yaml/", external.join("alias")).unwrap();

        let before = Project::validate_installation(&staging, &destination);
        std::fs::rename(&staging, &destination).unwrap();
        let after = Project::find(&destination).map(|_| ());
        assert_eq!(before, after);
    }

    /// Budgets are read as money (they need [`crate::money`]).
    mod with_money {
        use super::*;

        #[test]
        fn a_project_budget_is_micro_dollars() {
            let document = json!({"fx": "project/v1", "budget": {"max_usd": 2}});
            let project = parse_project(&document, "fx.yaml").unwrap();
            assert_eq!(
                project.budget,
                Some(Budget {
                    max_usd: Usd(2_000_000)
                })
            );
        }
    }

    #[test]
    fn find_walks_up_and_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // No fx.yaml anywhere in a fresh temporary folder (its ancestors are system folders).
        let bare = Project::find(&root).unwrap();
        assert_eq!(bare.root, root);
        assert!(!bare.has_file);
        assert_eq!(bare.document, ProjectDoc::default());
        // A folder named fx.yaml is skipped; a file under it starts at its folder.
        std::fs::create_dir_all(root.join("a/fx.yaml")).unwrap();
        std::fs::write(root.join("a/w.yaml"), "x").unwrap();
        let skipped = Project::find(&root.join("a/w.yaml")).unwrap();
        assert_eq!(skipped.root, root.join("a"));
        assert!(!skipped.has_file);
        // A start that does not exist is still walked from.
        let missing = Project::find(&root.join("a/missing/deeper")).unwrap();
        assert_eq!(missing.root, root.join("a/missing/deeper"));
        assert_eq!(missing.cache_dir(), root.join("a/missing/deeper/.fx/cache"));
    }
}
