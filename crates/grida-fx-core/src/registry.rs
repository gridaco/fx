//! Resolving `uses:` to node types and used workflows, type identities, source digests, the
//! fx.lock check, and project and input files (identity.md §6; protocol.md §5.1).
//!
//! `uses` forms, tested in order (FX names):
//! - built-in `^fx/(?P<name>[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)@(?P<major>[0-9]+)$` →
//!   [`crate::builtins`]; missing: `no built-in node type {uses}; see grida-fx nodes`. Identity
//!   `fx/<name>@<major>.<version>` with the major as a number (`@01` is `@1`: FX normalizes).
//! - project type `^(?P<file>\.{1,2}/[^#]+)#(?P<attr>[A-Za-z_][A-Za-z0-9_]*)$`: the module is
//!   `home root / file`, which must exist (`{uses}: no file {basename}`) and lie inside the
//!   project once symbolic links are resolved (`{basename} is outside the project`); a `.py`
//!   module goes to the Python host (another suffix: `{uses}: no node host for <suffix> modules
//!   yet`). `describe` is called once per module (all attributes), its result cached; a failed
//!   module's error text is the problem; a missing attribute is `{uses}: {attr} is not declared
//!   with @node`. The spec is validated with [`crate::spec::NodeSpec::from_type_spec`]
//!   (`{uses}: {message}`).
//! - workflow `^(?P<file>\.{1,2}/.+\.ya?ml)$` → the file under the home root
//!   (`{uses}: no workflow file {file}`; FX: one outside the project is refused,
//!   `{uses}: {file} is outside the project`), loaded like a top-level workflow under its
//!   project-relative path; a document error is fatal (exit 2).
//! - reserved package `^[a-z0-9_-]+/[a-z0-9_.]+@[0-9]+$` → `{uses}: published node packages are
//!   not available yet`.
//! - anything else → `uses: {repr} is fx/<type>@<major>, ./<path>#<name> or ./<path>.yaml`.
//!
//! Source digests: the engine reads every closure file the host reported and every declared
//! resource (POSIX, relative to the root) and hashes them (identity.md §6). A declared resource
//! that is not a file inside the project is the problem `{uses}: declared resource {path} is not a
//! project file` (FX: a refusal on the step, conformance `resource-missing`). A versioned type's
//! identity is `<path>#<attr>@<version>` and its digest is checked against fx.lock: a mismatch
//! sets [`ResolvedType::drift`] to `{uses}: its source changed but version {v} did not; bump the
//! version, or confirm no change in behaviour with grida-fx lock --same {path}#{attr}`, which the
//! expander reports on the step (conformance `lock-drift`). An unversioned type's identity is
//! `source:` + `digest(source)`.
//!
//! Paths: `./` and `../` are relative to the home root (the folder of the nearest fx.yaml above
//! the workflow file), never to the workflow file. Every path is checked against the project
//! root after symbolic links are resolved, and no message names an absolute path the user did
//! not write.

use crate::builtins::BuiltinType;
use crate::docs::lock::LockFile;
use crate::docs::workflow::LoadedWorkflow;
use crate::error::{Error, Result, io_reason};
use crate::host::{HostFailure, NodeHost};
use crate::spec::{BodyKind, NodeSpec};
use crate::val::FileValue;
use grida_fx_protocol::{
    ClosureEntry, DescribeParams, DescribeTarget, DescribedType, ModuleDescription,
};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;

/// `fx/<name>@<major>`.
static BUILTIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^fx/(?P<name>[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)@(?P<major>[0-9]+)$")
        .expect("a valid pattern")
});

/// `./<path>#<attr>`.
static PROJECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<file>\.{1,2}/[^#]+)#(?P<attr>[A-Za-z_][A-Za-z0-9_]*)$")
        .expect("a valid pattern")
});

/// `./<path>.yaml`.
static WORKFLOW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<file>\.{1,2}/.+\.ya?ml)$").expect("a valid pattern"));

/// A published node package (reserved).
static PACKAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9_-]+/[a-z0-9_.]+@[0-9]+$").expect("a valid pattern"));

/// The `kind` of a node type's source object (identity.md §6).
pub const NODE_SOURCE_KIND: &str = "fx-node-source-v1";

/// The digests behind a project type (identity.md §6 `fx-node-source-v1` without its `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceDigests {
    /// Closure label → file digest.
    pub files: BTreeMap<String, String>,
    /// Declared resource path → file digest.
    pub resources: BTreeMap<String, String>,
}

impl SourceDigests {
    /// `{"files", "resources"}`: the graph's `types.<uses>.source` (fx-graph-v1).
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("files".into(), digests_object(&self.files));
        object.insert("resources".into(), digests_object(&self.resources));
        Value::Object(object)
    }

    /// `digest({"kind": "fx-node-source-v1", "files", "resources"})`.
    pub fn digest(&self) -> String {
        let mut object = Map::new();
        object.insert("kind".into(), Value::String(NODE_SOURCE_KIND.into()));
        object.insert("files".into(), digests_object(&self.files));
        object.insert("resources".into(), digests_object(&self.resources));
        crate::value::digest(&Value::Object(object))
    }
}

fn digests_object(digests: &BTreeMap<String, String>) -> Value {
    Value::Object(
        digests
            .iter()
            .map(|(name, digest)| (name.clone(), Value::String(digest.clone())))
            .collect(),
    )
}

/// Where a resolved type comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeOrigin {
    Builtin {
        name: String,
        major: u32,
    },
    /// `path` is POSIX, relative to the project root, without `./`.
    Project {
        path: String,
        attribute: String,
    },
}

/// A resolved node type.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedType {
    /// The `uses` as written.
    pub uses: String,
    /// The type identity (identity.md §6).
    pub identity: String,
    pub spec: Rc<NodeSpec>,
    pub origin: TypeOrigin,
    pub body: BodyKind,
    /// A project type's source digests (both versioned and unversioned); `None` for built-ins.
    pub source: Option<SourceDigests>,
    /// A versioned type whose source no longer matches fx.lock: the problem text.
    pub drift: Option<String>,
}

impl ResolvedType {
    /// Whether this is the engine's `fx/select@1`. FX keys native select on the built-in, not on
    /// any type named `select`.
    pub fn is_select(&self) -> bool {
        matches!(&self.origin, TypeOrigin::Builtin { name, .. } if name == "select")
    }
}

/// What a `uses` resolved to.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Node(Rc<ResolvedType>),
    Workflow(Rc<LoadedWorkflow>),
}

/// Why a `uses` did not resolve.
#[derive(Debug, Clone, PartialEq)]
pub enum RegistryError {
    /// A problem on the step (its text); the step is absent.
    Problem(String),
    /// The command stops (exit 2): a used workflow that is not a valid document, a host that
    /// cannot start.
    Fatal(Error),
}

/// The registry of one plan. It outlives expansions (a run expands again after every result) and
/// caches modules, types and files.
#[derive(Debug)]
pub struct Registry {
    /// The home project root (absolute).
    pub root: PathBuf,
    /// Declared source packages (fx.yaml `sources`).
    pub sources: Vec<String>,
    pub locks: LockFile,
    /// `{capability: route id}`: the planning project's fx.yaml routes over the home's.
    pub route_defaults: IndexMap<String, String>,
    /// Module descriptions by project-relative path, failed ones included.
    pub(crate) modules: HashMap<String, ModuleDescription>,
    /// Resolved node types by `uses` as written (built-ins also by their canonical `uses`).
    pub(crate) types: HashMap<String, Rc<ResolvedType>>,
    /// Used workflows by resolved path.
    pub(crate) workflows: HashMap<PathBuf, Rc<LoadedWorkflow>>,
    /// Project files read by content, by resolved path. `name` is the project-relative path;
    /// [`Registry::input_file`] names its files afresh.
    pub(crate) files: HashMap<PathBuf, FileValue>,
}

impl Registry {
    pub fn new(
        root: PathBuf,
        sources: Vec<String>,
        locks: LockFile,
        route_defaults: IndexMap<String, String>,
    ) -> Registry {
        Registry {
            root,
            sources,
            locks,
            route_defaults,
            modules: HashMap::new(),
            types: HashMap::new(),
            workflows: HashMap::new(),
            files: HashMap::new(),
        }
    }

    /// Resolves a step's `uses` (module doc).
    pub fn node_type(
        &mut self,
        uses: &str,
        host: &mut dyn NodeHost,
    ) -> std::result::Result<Resolved, RegistryError> {
        if let Some(found) = self.types.get(uses) {
            return Ok(Resolved::Node(found.clone()));
        }
        if let Some(captures) = BUILTIN.captures(uses) {
            let name = &captures["name"];
            let builtin = captures["major"]
                .parse::<u32>()
                .ok()
                .and_then(|major| crate::builtins::builtin(name, major))
                .ok_or_else(|| {
                    RegistryError::Problem(format!(
                        "no built-in node type {uses}; see grida-fx nodes"
                    ))
                })?;
            let mut resolved = self.builtin_type(builtin);
            if resolved.uses != uses {
                resolved = Rc::new(ResolvedType {
                    uses: uses.to_string(),
                    ..(*resolved).clone()
                });
                self.types.insert(uses.to_string(), resolved.clone());
            }
            return Ok(Resolved::Node(resolved));
        }
        if let Some(captures) = PROJECT.captures(uses) {
            let resolved = self.project_type(uses, &captures["file"], &captures["attr"], host)?;
            let resolved = Rc::new(resolved);
            self.types.insert(uses.to_string(), resolved.clone());
            return Ok(Resolved::Node(resolved));
        }
        if let Some(captures) = WORKFLOW.captures(uses) {
            return self
                .workflow(uses, &captures["file"])
                .map(Resolved::Workflow);
        }
        if PACKAGE.is_match(uses) {
            return Err(RegistryError::Problem(format!(
                "{uses}: published node packages are not available yet"
            )));
        }
        Err(RegistryError::Problem(format!(
            "uses: {} is fx/<type>@<major>, ./<path>#<name> or ./<path>.yaml",
            crate::text::py_repr_str(uses)
        )))
    }

    /// A project type (`./<file>#<attr>`), not yet cached.
    fn project_type(
        &mut self,
        uses: &str,
        file: &str,
        attribute: &str,
        host: &mut dyn NodeHost,
    ) -> std::result::Result<ResolvedType, RegistryError> {
        let path = self.root.join(file);
        let basename = basename(file);
        if !path.is_file() {
            return Err(RegistryError::Problem(format!(
                "{uses}: no file {basename}"
            )));
        }
        let relative = relative_inside(&self.root, &path)
            .ok_or_else(|| RegistryError::Problem(format!("{basename} is outside the project")))?;
        match Path::new(&relative).extension().and_then(|s| s.to_str()) {
            Some("py") => {}
            Some(suffix) => {
                return Err(RegistryError::Problem(format!(
                    "{uses}: no node host for .{suffix} modules yet"
                )));
            }
            None => {
                return Err(RegistryError::Problem(format!(
                    "{uses}: no node host for modules without a suffix yet"
                )));
            }
        }
        let module = self
            .describe_module(&relative, host)
            .map_err(|failure| RegistryError::Fatal(Error::from(failure)))?;
        let (types, closure) = match &module {
            ModuleDescription::Failed { error, .. } => {
                return Err(RegistryError::Problem(error.clone()));
            }
            ModuleDescription::Described { types, closure, .. } => (types, closure),
        };
        let described = find_type(types, attribute).ok_or_else(|| {
            RegistryError::Problem(format!("{uses}: {attribute} is not declared with @node"))
        })?;
        let spec = NodeSpec::from_type_spec(&described.spec, None)
            .map_err(|message| RegistryError::Problem(format!("{uses}: {message}")))?;
        let source = self
            .source_digests(closure, &spec.resources)
            .map_err(|message| RegistryError::Problem(format!("{uses}: {message}")))?;
        let digest = source.digest();
        let (identity, drift) = match spec.version {
            None => (format!("source:{digest}"), None),
            Some(version) => {
                let identity = format!("{relative}#{attribute}@{version}");
                let drift = match self.locks.nodes.get(&identity) {
                    Some(locked) if *locked != digest => Some(format!(
                        "{uses}: its source changed but version {version} did not; bump the \
                         version, or confirm no change in behaviour with grida-fx lock --same \
                         {relative}#{attribute}"
                    )),
                    _ => None,
                };
                (identity, drift)
            }
        };
        Ok(ResolvedType {
            uses: uses.to_string(),
            identity,
            spec: Rc::new(spec),
            origin: TypeOrigin::Project {
                path: relative,
                attribute: attribute.to_string(),
            },
            body: BodyKind::Project,
            source: Some(source),
            drift,
        })
    }

    /// A workflow used as a step (`./<file>.yaml`), loaded once per file.
    fn workflow(
        &mut self,
        uses: &str,
        file: &str,
    ) -> std::result::Result<Rc<LoadedWorkflow>, RegistryError> {
        let path = self.root.join(file);
        if !path.is_file() {
            return Err(RegistryError::Problem(format!(
                "{uses}: no workflow file {file}"
            )));
        }
        let resolved = resolve_path(&path);
        if let Some(found) = self.workflows.get(&resolved) {
            return Ok(found.clone());
        }
        let relative = relative_inside(&self.root, &resolved).ok_or_else(|| {
            RegistryError::Problem(format!("{uses}: {file} is outside the project"))
        })?;
        let loaded = crate::docs::workflow::load_workflow(&resolved, &relative, &relative)
            .map_err(RegistryError::Fatal)?;
        let loaded = Rc::new(loaded);
        self.workflows.insert(resolved, loaded.clone());
        Ok(loaded)
    }

    /// The type of a built-in.
    pub fn builtin_type(&mut self, builtin: &'static BuiltinType) -> Rc<ResolvedType> {
        if let Some(found) = self.types.get(&builtin.uses) {
            return found.clone();
        }
        let resolved = Rc::new(ResolvedType {
            uses: builtin.uses.clone(),
            identity: builtin.identity(),
            spec: Rc::new(builtin.spec.clone()),
            origin: TypeOrigin::Builtin {
                name: builtin.name.clone(),
                major: builtin.major,
            },
            body: builtin.body,
            source: None,
            drift: None,
        });
        self.types.insert(builtin.uses.clone(), resolved.clone());
        resolved
    }

    /// Describes a project module (all its types), once per registry. `path` is POSIX, relative
    /// to the root.
    pub fn describe_module(
        &mut self,
        path: &str,
        host: &mut dyn NodeHost,
    ) -> std::result::Result<ModuleDescription, HostFailure> {
        if let Some(found) = self.modules.get(path) {
            return Ok(found.clone());
        }
        let result = host.describe(&DescribeParams {
            targets: vec![DescribeTarget {
                path: path.to_string(),
                attribute: None,
            }],
            builtins: false,
        })?;
        let module = result.modules.into_iter().next().ok_or_else(|| {
            HostFailure::Unavailable(format!("the node host answered describe without {path}"))
        })?;
        self.modules.insert(path.to_string(), module.clone());
        Ok(module)
    }

    /// Hashes a closure and declared resources. Errors are problem texts (a resource that is not
    /// a project file; a closure file that cannot be read).
    pub fn source_digests(
        &self,
        closure: &[ClosureEntry],
        resources: &[String],
    ) -> std::result::Result<SourceDigests, String> {
        let mut digests = SourceDigests::default();
        for entry in closure {
            if !is_relative_label(&entry.label) {
                return Err(format!(
                    "the node host named a source file {}, which is not a relative path",
                    crate::text::py_repr_str(&entry.label)
                ));
            }
            let bytes = std::fs::read(&entry.path)
                .map_err(|error| format!("cannot read {}: {}", entry.label, io_reason(&error)))?;
            digests
                .files
                .insert(entry.label.clone(), crate::value::file_digest(&bytes));
        }
        for resource in resources {
            let bytes = match read_project_bytes(&self.root, resource) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => {
                    return Err(format!(
                        "declared resource {resource} is not a project file"
                    ));
                }
                Err(error) => return Err(format!("cannot read {}", error.message)),
            };
            digests
                .resources
                .insert(resource.clone(), crate::value::file_digest(&bytes));
        }
        Ok(digests)
    }

    /// A project file named by a `./` or `../` path in `with:` (identity.md §8): relative to the
    /// home root, inside the project
    /// (`{value} is outside the project`), read and hashed once; `name` is the project-relative
    /// POSIX path; no content. An absolute path is refused (FX). Errors are the text after
    /// `cannot read {value}: ` (without any absolute path).
    pub fn project_file(&mut self, relative: &str) -> std::result::Result<FileValue, String> {
        if Path::new(relative).has_root() {
            return Err(
                "an absolute path is not a project file; write it as ./<path> inside the project"
                    .into(),
            );
        }
        let path = self.root.join(relative);
        let name = relative_inside(&self.root, &path)
            .ok_or_else(|| format!("{relative} is outside the project"))?;
        let resolved = resolve_path(&path);
        if let Some(found) = self.files.get(&resolved) {
            return Ok(found.clone());
        }
        let bytes = read_file(&resolved)?;
        let value = FileValue {
            digest: crate::value::file_digest(&bytes),
            kind: crate::kinds::kind_of(&name).to_string(),
            name,
            size: bytes.len() as u64,
            key: None,
            content: None,
            location: Some(resolved.clone()),
        };
        self.files.insert(resolved, value.clone());
        Ok(value)
    }

    /// A file a workflow gives a used workflow's file input as a path: relative to the home
    /// root and inside the project (FX: an absolute path is refused); `no input file {path}` when
    /// missing; kind from the suffix, else `declared_kind`; named by its file name, no content.
    pub fn input_file(
        &mut self,
        path: &str,
        declared_kind: &str,
    ) -> std::result::Result<FileValue, String> {
        if Path::new(path).has_root() {
            return Err(format!(
                "{path} is an absolute path; name an input file relative to the project"
            ));
        }
        let target = self.root.join(path);
        if !target.is_file() {
            return Err(format!("no input file {path}"));
        }
        let relative = relative_inside(&self.root, &target)
            .ok_or_else(|| format!("{path} is outside the project"))?;
        let resolved = resolve_path(&target);
        let known = match self.files.get(&resolved) {
            Some(found) => found.clone(),
            None => {
                let bytes = read_file(&resolved).map_err(|reason| format!("{path}: {reason}"))?;
                let value = FileValue {
                    digest: crate::value::file_digest(&bytes),
                    kind: crate::kinds::kind_of(&relative).to_string(),
                    name: relative.clone(),
                    size: bytes.len() as u64,
                    key: None,
                    content: None,
                    location: Some(resolved.clone()),
                };
                self.files.insert(resolved, value.clone());
                value
            }
        };
        let found = crate::kinds::kind_of(&relative);
        Ok(FileValue {
            kind: if found == "file" {
                declared_kind.to_string()
            } else {
                found.to_string()
            },
            name: basename(&relative).to_string(),
            ..known
        })
    }

    /// The default route for a capability (fx.yaml `routes:`).
    pub fn route_default(&self, capability: &str) -> Option<&str> {
        self.route_defaults.get(capability).map(String::as_str)
    }

    /// The source digests of a versioned or unversioned type in a module, for `grida-fx lock`.
    pub fn type_source(
        &mut self,
        module: &ModuleDescription,
        attribute: &str,
    ) -> std::result::Result<SourceDigests, String> {
        match module {
            ModuleDescription::Failed { error, .. } => Err(error.clone()),
            ModuleDescription::Described {
                path,
                types,
                closure,
            } => {
                let described = find_type(types, attribute)
                    .ok_or_else(|| format!("{path}: {attribute} is not declared with @node"))?;
                self.source_digests(closure, &described.spec.resources)
            }
        }
    }
}

/// The type a module declares under `attribute`.
fn find_type<'a>(types: &'a [DescribedType], attribute: &str) -> Option<&'a DescribedType> {
    types.iter().find(|t| t.attribute == attribute)
}

/// The last name of a path as written (`./nodes/x.py` → `x.py`).
fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// A closure label: POSIX, relative, with no empty, `.` or `..` segment.
fn is_relative_label(label: &str) -> bool {
    !label.is_empty()
        && !label.contains('\\')
        && label
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Reads a regular file; the error is a short reason without its path.
fn read_file(path: &Path) -> std::result::Result<Vec<u8>, String> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Err("not a file".into()),
        Ok(_) => {}
        Err(error) => return Err(io_reason(&error)),
    }
    std::fs::read(path).map_err(|error| io_reason(&error))
}

/// `path` with every symbolic link resolved, as far as it exists; the part that does not exist
/// is kept as written, `.` dropped and `..` taken lexically (Python's `Path.resolve()`).
pub(crate) fn resolve_path(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => {
                out.push(name);
                if let Ok(resolved) = std::fs::canonicalize(&out) {
                    out = resolved;
                }
            }
        }
    }
    out
}

/// `path` as a POSIX path relative to `root`, when it lies inside it (no symlink escapes:
/// compare canonical paths).
pub fn relative_inside(root: &Path, path: &Path) -> Option<String> {
    let root = resolve_path(root);
    let path = resolve_path(path);
    let rest = path.strip_prefix(&root).ok()?;
    let parts: Vec<String> = rest
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// Reads a project file's bytes for hashing (an absent file is `None`). A path that leaves the
/// project, once symbolic links are resolved, or that is not a regular file, is absent; a file
/// that exists and cannot be read is an error naming `relative`.
pub fn read_project_bytes(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    if Path::new(relative).has_root() {
        return Ok(None);
    }
    let path = root.join(relative);
    if relative_inside(root, &path).is_none() || !path.is_file() {
        return Ok(None);
    }
    std::fs::read(&path)
        .map(Some)
        .map_err(|error| Error::io(relative, &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        assert!(BUILTIN.is_match("fx/image.generate@1"));
        assert!(BUILTIN.is_match("fx/select@01"));
        assert!(!BUILTIN.is_match("fx/9x@1"));
        assert!(!BUILTIN.is_match("fx/select@١"));
        assert!(PROJECT.is_match("./nodes/n.py#echo"));
        assert!(PROJECT.is_match("../n.ts#echo"));
        assert!(!PROJECT.is_match("nodes/n.py#echo"));
        assert!(!PROJECT.is_match("./nodes/n.py#9"));
        assert!(WORKFLOW.is_match("./workflows/a.yaml"));
        assert!(WORKFLOW.is_match("../a.yml"));
        assert!(!WORKFLOW.is_match("./a.json"));
        assert!(PACKAGE.is_match("fx/9x@1"));
        assert!(PACKAGE.is_match("acme/tools.x@2"));
    }

    #[test]
    fn basenames() {
        assert_eq!(basename("./nodes/x.py"), "x.py");
        assert_eq!(basename("../x.py"), "x.py");
        assert_eq!(basename("./nodes/"), "nodes");
    }

    #[test]
    fn labels() {
        assert!(is_relative_label("nodes/n.py"));
        assert!(is_relative_label("pkg/__init__.py"));
        assert!(!is_relative_label("/abs/n.py"));
        assert!(!is_relative_label("../n.py"));
        assert!(!is_relative_label("a//b.py"));
        assert!(!is_relative_label(""));
    }

    #[test]
    fn resolving_a_path_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            resolve_path(&root.join("a/./b/../c.txt")),
            root.join("a/c.txt")
        );
        assert_eq!(
            relative_inside(&root, &root.join("./x/../y.md")),
            Some("y.md".into())
        );
        assert_eq!(relative_inside(&root, &root.join("../y.md")), None);
    }
}
