//! Setting up a plan: targets, workflow lookup, builders, inputs, routes, takes, the registry.
//!
//! `make_planner` does, in this order (the order decides which error a broken setup shows):
//! 0. a builder target given workflow input flags is refused at once
//!    (`unknown flag <x>; a builder takes its arguments as --arg`), before any builder runs;
//! 1. the planning project: [`Project::find`] from the workflow file for a `.yaml`/`.yml` target,
//!    else from `cwd`;
//! 2. the workflow: a builder (`^(?P<file>.+\.py):(?P<name>[A-Za-z_][A-Za-z0-9_]*)$`, run through
//!    the host's `build` after [`NodeHost::open_project`] on the planning project, cwd = the
//!    working directory; `load_failed` → `no builder file …` / `… has no function …`,
//!    `build_failed` → its message; exit 2), else [`find_workflow`] and
//!    [`crate::docs::workflow::load_workflow`]. Builder documents are validated like files; their
//!    plan-digest source is `<path>:<function>` and their home is the planning project;
//! 3. the inputs schema ([`crate::inputs::compile_inputs`], `$ref` relative to the home);
//! 4. input flags from `rest`, then the inputs ([`crate::inputs::bind::load_inputs`]; `--inputs`
//!    files relative to `cwd`, named as typed);
//! 5. the home project (the nearest fx.yaml above the workflow file), its fx.lock, the registry
//!    (route defaults: the home's, overridden by the planning project's), and
//!    [`NodeHost::open_project`] on the home;
//! 6. the route catalog ([`crate::routes::load_catalog`]: the planning project's
//!    `route_tables`, then `--routes` files relative to `cwd`);
//! 7. the takes file ([`crate::docs::takes`]): next to the workflow file, or in the folder of the
//!    builder's `takes_anchor`; in the planning project's root when the home differs.
//!
//! Every error is fatal (exit 2). Planning writes nothing.

use crate::docs::lock::LockFile;
use crate::docs::project::Project;
use crate::docs::takes::Takes;
use crate::docs::workflow::LoadedWorkflow;
use crate::error::{Error, Result};
use crate::expand::NodeResult;
use crate::host::{HostFailure, NodeHost};
use crate::inputs::bind::RootInputs;
use crate::money::Usd;
use crate::registry::{Registry, relative_inside, resolve_path};
use crate::routes::RouteTable;
use grida_fx_protocol::BuildParams;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;

/// The project file's name, never a workflow.
const PROJECT_FILE: &str = "fx.yaml";

/// `file.py:function`.
static BUILDER_TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<file>.+\.py):(?P<name>[A-Za-z_][A-Za-z0-9_]*)$").expect("a valid pattern")
});

/// A target as written on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A workflow file (`*.yaml`, `*.yml`, `*.json`) or a workflow id.
    Workflow(String),
    /// `file.py:function`.
    Builder { file: String, function: String },
}

impl Target {
    /// Classifies a target: a builder only when the function part is an identifier and the file
    /// ends in `.py`; anything else is a workflow file or id (`b.py:bad-name` is looked up as an
    /// id).
    pub fn parse(text: &str) -> Target {
        match BUILDER_TARGET.captures(text) {
            Some(captures) => Target::Builder {
                file: captures["file"].to_string(),
                function: captures["name"].to_string(),
            },
            None => Target::Workflow(text.to_string()),
        }
    }
}

/// Whether a target names a workflow file by its suffix (`.yaml`, `.yml`, `.json`, case
/// sensitive).
fn is_workflow_file(target: &str) -> bool {
    matches!(
        Path::new(target).extension().and_then(|s| s.to_str()),
        Some("yaml" | "yml" | "json")
    )
}

/// `path` joined to `cwd` unless absolute.
fn absolute(cwd: &Path, path: &str) -> PathBuf {
    cwd.join(path)
}

/// Finds a workflow: a path with a `.yaml`/`.yml`/`.json` suffix, relative to `cwd`
/// (`no workflow file {target}`); else a workflow whose `id` is `target` among
/// [`project_workflow_files`] (`workflow id {repr} is declared twice: {relpaths}`;
/// `no workflow with id {repr} under {project folder name}`). Files that are not UTF-8, do not
/// mention `workflow/v1`, or do not parse are skipped while searching. When no file declares the
/// id, a skipped file that does not parse but whose text mentions both `workflow/v1` and the id
/// is reported instead, with the strict loader's error (the first such file in search order), so
/// a workflow that is refused is not called missing. A path found by id is returned as found
/// under the project root; a file target with its symbolic links resolved.
pub fn find_workflow(target: &str, project: &Project, cwd: &Path) -> Result<PathBuf> {
    if is_workflow_file(target) {
        let path = absolute(cwd, target);
        if !path.is_file() {
            return Err(Error::plan(format!("no workflow file {target}")));
        }
        return Ok(resolve_path(&path));
    }
    let mut matches = Vec::new();
    let mut refused: Option<Error> = None;
    for path in project_workflow_files(project) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if !text.contains("workflow/v1") {
            continue;
        }
        let label = relative_label(&project.root, &path);
        let document = match crate::yaml::load(&bytes, &label) {
            Ok(document) => document,
            Err(error) => {
                if refused.is_none() && text.contains(target) {
                    refused = Some(error.into());
                }
                continue;
            }
        };
        let declares = document.get("fx").and_then(Value::as_str) == Some("workflow/v1")
            && document.get("id").and_then(Value::as_str) == Some(target);
        if declares {
            matches.push(path);
        }
    }
    if matches.is_empty()
        && let Some(error) = refused
    {
        return Err(error);
    }
    let repr = crate::text::py_repr_str(target);
    match matches.len() {
        0 => Err(Error::plan(format!(
            "no workflow with id {repr} under {}",
            project
                .root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ))),
        1 => Ok(matches.remove(0)),
        _ => {
            let names: Vec<String> = matches
                .iter()
                .map(|path| relative_label(&project.root, path))
                .collect();
            Err(Error::plan(format!(
                "workflow id {repr} is declared twice: {}",
                names.join(", ")
            )))
        }
    }
}

/// A path under `root` as POSIX text relative to it (lexically), else its file name.
fn relative_label(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) => rest
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// The files searched for a workflow id: the root's own `*.yaml`/`*.yml` entries sorted, then
/// every folder under `workflows/` (symbolic links to folders not followed) in the sorted order
/// of their paths, each folder's entries sorted; never `fx.yaml`, never a name containing
/// `.takes.`, regular files only. Suffixes are case sensitive; `.json` files are never found by
/// id.
pub fn project_workflow_files(project: &Project) -> Vec<PathBuf> {
    let mut found = Vec::new();
    candidates(&project.root, &mut found);
    let workflows = project.root.join("workflows");
    if workflows.is_dir() {
        let mut folders = Vec::new();
        walk_folders(&workflows, &mut folders);
        folders.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        for folder in folders {
            candidates(&folder, &mut found);
        }
    }
    found
}

/// The workflow-file candidates directly in `folder`, sorted by name.
fn candidates(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let suffix = Path::new(name).extension().and_then(|s| s.to_str());
        let named = name != PROJECT_FILE && !name.contains(".takes.");
        if matches!(suffix, Some("yaml" | "yml")) && named && path.is_file() {
            found.push(path);
        }
    }
}

/// `folder` and every folder below it, without following symbolic links.
fn walk_folders(folder: &Path, folders: &mut Vec<PathBuf>) {
    folders.push(folder.to_path_buf());
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            walk_folders(&entry.path(), folders);
        }
    }
}

/// What a planning verb asked for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanRequest {
    pub target: String,
    /// The working directory, absolute.
    pub cwd: PathBuf,
    /// `--inputs` files as typed (relative to `cwd` unless absolute).
    pub input_files: Vec<String>,
    /// The arguments the verb did not take: workflow input flags.
    pub rest: Vec<String>,
    /// `--arg NAME=VALUE` pairs, already split; a repeated name keeps the last.
    pub arguments: IndexMap<String, String>,
    /// `--routes` files as typed.
    pub routes: Vec<String>,
    /// `--max-usd`.
    pub max_usd: Option<Usd>,
}

/// Everything a plan is made from. Owned, so expansion can run again (at-plan rounds; the
/// runner after each result) against the same registry.
#[derive(Debug)]
pub struct Planner {
    pub cwd: PathBuf,
    /// The project planning happens in (runs, cache, budget, route overrides).
    pub project: Project,
    /// The workflow's own project (`./` paths, node modules, fx.lock, sources).
    pub home: Project,
    pub workflow: Rc<LoadedWorkflow>,
    /// The compiled inputs schema.
    pub input_schema: Value,
    pub inputs: RootInputs,
    pub routes: RouteTable,
    pub takes: Takes,
    pub takes_path: PathBuf,
    pub registry: Registry,
    /// Results known while planning (at-plan results; the runner's results in step 3).
    pub results: IndexMap<String, NodeResult>,
    pub max_usd: Option<Usd>,
}

/// A workflow as loaded by steps 1 and 2 of [`make_planner`].
struct LoadedTarget {
    /// The planning project.
    project: Project,
    workflow: LoadedWorkflow,
    /// For a builder: the folder its takes file lives in.
    builder_takes: Option<PathBuf>,
}

impl LoadedTarget {
    fn is_builder(&self) -> bool {
        self.builder_takes.is_some()
    }

    /// The workflow's home project: the planning project for a builder, else the nearest
    /// fx.yaml above the workflow file.
    fn home(&self) -> Result<Project> {
        if self.is_builder() {
            Ok(self.project.clone())
        } else {
            Project::find(&self.workflow.path)
        }
    }
}

/// Steps 1 and 2: the planning project and the workflow.
fn load(
    target: &str,
    cwd: &Path,
    arguments: &IndexMap<String, String>,
    host: &mut dyn NodeHost,
) -> Result<LoadedTarget> {
    let parsed = Target::parse(target);
    let start = match &parsed {
        Target::Workflow(text)
            if matches!(
                Path::new(text).extension().and_then(|s| s.to_str()),
                Some("yaml" | "yml")
            ) =>
        {
            absolute(cwd, text)
        }
        _ => cwd.to_path_buf(),
    };
    let project = Project::find(&start)?;
    match parsed {
        Target::Builder { file, function } => {
            host.open_project(&project.root, &project.document.sources)
                .map_err(Error::from)?;
            let path = absolute(cwd, &file);
            if !path.is_file() {
                return Err(Error::plan(format!("no builder file {file}")));
            }
            let relative = relative_inside(&project.root, &path).ok_or_else(|| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| file.clone());
                Error::plan(format!("{name} is outside the project"))
            })?;
            let built = host
                .build(&BuildParams {
                    path: relative.clone(),
                    function: function.clone(),
                    arguments: arguments.clone(),
                    cwd: cwd.to_string_lossy().into_owned(),
                })
                .map_err(|failure| match failure {
                    HostFailure::Rpc(error) => Error::plan(error.message),
                    unavailable => Error::from(unavailable),
                })?;
            let source = format!("{relative}:{function}");
            let document = crate::docs::workflow::parse_workflow(&built.document, &source)?;
            let resolved = resolve_path(&path);
            let anchor = project.root.join(&built.takes_anchor);
            let anchor = if !built.takes_anchor.is_empty()
                && !Path::new(&built.takes_anchor).has_root()
                && relative_inside(&project.root, &anchor).is_some()
            {
                anchor
            } else {
                resolved.clone()
            };
            let takes_folder = anchor
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| project.root.clone());
            Ok(LoadedTarget {
                project,
                workflow: LoadedWorkflow {
                    document: built.document,
                    workflow: Rc::new(document),
                    source,
                    path: resolved,
                },
                builder_takes: Some(takes_folder),
            })
        }
        Target::Workflow(text) => {
            let path = find_workflow(&text, &project, cwd)?;
            let label = if is_workflow_file(&text) {
                text.clone()
            } else {
                relative_label(&project.root, &path)
            };
            let workflow = crate::docs::workflow::load_workflow(&path, &label, &label)?;
            Ok(LoadedTarget {
                project,
                workflow,
                builder_takes: None,
            })
        }
    }
}

/// The plan-digest source of a workflow file: its path relative to its home, POSIX.
fn workflow_source(home: &Project, path: &Path) -> String {
    relative_inside(&home.root, path).unwrap_or_else(|| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// Reads `<root>/fx.lock`; an absent file is an empty lock (and is not opened).
pub(crate) fn read_lock_at(root: &Path) -> Result<LockFile> {
    if std::fs::symlink_metadata(root.join(crate::docs::lock::LOCK_FILE)).is_err() {
        return Ok(LockFile::default());
    }
    crate::docs::lock::read_lock(root)
}

/// Sets up a plan (module doc).
pub fn make_planner(request: &PlanRequest, host: &mut dyn NodeHost) -> Result<Planner> {
    let cwd = request.cwd.as_path();
    // 0. A builder takes no input flags; refuse before running it.
    if let Target::Builder { .. } = Target::parse(&request.target) {
        if let Some(first) = request.rest.first() {
            return Err(Error::usage(format!(
                "unknown flag {first}; a builder takes its arguments as --arg"
            )));
        }
    }
    // 1, 2. The planning project and the workflow.
    let loaded = load(&request.target, cwd, &request.arguments, host)?;
    // 3. The inputs schema; `$ref` files are relative to the home.
    let schema = {
        let mut home_root: Option<PathBuf> =
            loaded.is_builder().then(|| loaded.project.root.clone());
        let workflow_path = loaded.workflow.path.clone();
        let mut resolve = |reference: &str| -> Result<Value> {
            let root = match &home_root {
                Some(root) => root.clone(),
                None => {
                    let root = Project::find(&workflow_path)?.root;
                    home_root = Some(root.clone());
                    root
                }
            };
            crate::inputs::resolve_ref(&root, reference)
        };
        crate::inputs::compile_inputs(&loaded.workflow.workflow.inputs, Some(&mut resolve))?
    };
    // 4. Input flags, then the inputs.
    let flags = if loaded.is_builder() {
        IndexMap::new()
    } else {
        crate::inputs::flags::parse_input_flags(&schema, &request.rest, &request.target)?
    };
    let files: Vec<(PathBuf, String)> = request
        .input_files
        .iter()
        .map(|file| (absolute(cwd, file), file.clone()))
        .collect();
    let inputs = crate::inputs::bind::load_inputs(&schema, &files, flags, cwd)?;
    // 5. The home, its lock, the registry.
    let home = loaded.home()?;
    let LoadedTarget {
        project,
        mut workflow,
        builder_takes,
    } = loaded;
    if builder_takes.is_none() {
        workflow.source = workflow_source(&home, &workflow.path);
    }
    let locks = read_lock_at(&home.root)?;
    let mut route_defaults = home.route_defaults();
    route_defaults.extend(project.route_defaults());
    let registry = Registry::new(
        home.root.clone(),
        home.document.sources.clone(),
        locks,
        route_defaults,
    );
    host.open_project(&home.root, &home.document.sources)
        .map_err(Error::from)?;
    // 6. The route catalog.
    let extra: Vec<(PathBuf, String)> = request
        .routes
        .iter()
        .map(|file| (absolute(cwd, file), file.clone()))
        .collect();
    let routes = crate::routes::load_catalog(&project, &extra)?;
    // 7. The takes file.
    let folder = match builder_takes {
        Some(folder) => folder,
        None if home.root != project.root => project.root.clone(),
        None => workflow
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.root.clone()),
    };
    let takes_path = crate::docs::takes::takes_path(&folder, &workflow.workflow.id);
    let takes_label = relative_inside(&project.root, &takes_path).unwrap_or_else(|| {
        takes_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let takes = crate::docs::takes::read_takes(&takes_path, &takes_label)?;
    Ok(Planner {
        cwd: cwd.to_path_buf(),
        project,
        home,
        workflow: Rc::new(workflow),
        input_schema: schema,
        inputs,
        routes,
        takes,
        takes_path,
        registry,
        results: IndexMap::new(),
        max_usd: request.max_usd,
    })
}

/// Loads only the target's workflow, as `schema` and `doctor` need it (no inputs): steps 1 and 2
/// of [`make_planner`]. Returns the planning project and the workflow, whose `source` is its
/// plan-digest key. The workflow's home is the planning project for a builder, else
/// `Project::find(&workflow.path)`; the host is pointed at that home.
pub fn load_target(
    target: &str,
    cwd: &Path,
    arguments: &IndexMap<String, String>,
    host: &mut dyn NodeHost,
) -> Result<(Project, Rc<LoadedWorkflow>)> {
    let loaded = load(target, cwd, arguments, host)?;
    let home = loaded.home()?;
    let LoadedTarget {
        project,
        mut workflow,
        builder_takes,
    } = loaded;
    if builder_takes.is_none() {
        workflow.source = workflow_source(&home, &workflow.path);
    }
    host.open_project(&home.root, &home.document.sources)
        .map_err(Error::from)?;
    Ok((project, Rc::new(workflow)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(
            Target::parse("builders/b.py:build"),
            Target::Builder {
                file: "builders/b.py".into(),
                function: "build".into()
            }
        );
        assert_eq!(
            Target::parse("b.py:bad-name"),
            Target::Workflow("b.py:bad-name".into())
        );
        assert_eq!(Target::parse("x.py"), Target::Workflow("x.py".into()));
        assert_eq!(
            Target::parse("workflows/f.yaml:x"),
            Target::Workflow("workflows/f.yaml:x".into())
        );
        assert_eq!(Target::parse("case"), Target::Workflow("case".into()));
        assert_eq!(
            Target::parse("a:b.py:_f1"),
            Target::Builder {
                file: "a:b.py".into(),
                function: "_f1".into()
            }
        );
    }

    #[test]
    fn workflow_file_suffixes() {
        assert!(is_workflow_file("a.yaml"));
        assert!(is_workflow_file("w/a.yml"));
        assert!(is_workflow_file("a.json"));
        assert!(!is_workflow_file("a.YAML"));
        assert!(!is_workflow_file(".yaml"));
        assert!(!is_workflow_file("case"));
    }
}
