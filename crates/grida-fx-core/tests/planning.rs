//! Planning: the registry, plan setup, the lock and the plan around expansion.
//!
//! Every test builds its project in a temporary folder and answers `describe` and `build` with
//! [`FakeHost`]. Tests named `integrated_…` also need the documents, YAML, node specs, built-ins
//! and expansion; the others need only this module's own code.

use grida_fx_core::docs::lock::LockFile;
use grida_fx_core::docs::project::{Project, ProjectDoc};
use grida_fx_core::docs::takes::TakeChoice;
use grida_fx_core::docs::workflow::{LoadedWorkflow, Step, WorkflowDoc};
use grida_fx_core::error::ErrorKind;
use grida_fx_core::expand::{CallPrice, Expansion, Instance, NodeResult, PendingRepeat, State};
use grida_fx_core::host::{FakeHost, NoCache, PlanTimeRunner};
use grida_fx_core::inputs::bind::RootInputs;
use grida_fx_core::lock::lock;
use grida_fx_core::money::Usd;
use grida_fx_core::plan::{Estimate, PhaseSummary, Plan, make_plan};
use grida_fx_core::project::{
    PlanRequest, Planner, Target, find_workflow, load_target, make_planner, project_workflow_files,
};
use grida_fx_core::registry::{
    Registry, RegistryError, Resolved, ResolvedType, SourceDigests, TypeOrigin, read_project_bytes,
    relative_inside,
};
use grida_fx_core::routes::RouteTable;
use grida_fx_core::spec::{BodyKind, NodeSpec, Retry};
use grida_fx_core::{Error, Problem};
use grida_fx_protocol::{
    BuildResult, ClosureEntry, DescribedType, ErrorCode, ModuleDescription, RetryMode, RpcError,
    TypeSpec,
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

// ------------------------------------------------------------------ fixtures

/// A temporary project folder; `root` has its symbolic links resolved.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        Fixture { _dir: dir, root }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn project(&self) -> Project {
        Project {
            root: self.root.clone(),
            document: ProjectDoc::default(),
            has_file: true,
        }
    }

    fn registry(&self) -> Registry {
        self.registry_with(LockFile::default())
    }

    fn registry_with(&self, locks: LockFile) -> Registry {
        Registry::new(self.root.clone(), Vec::new(), locks, IndexMap::new())
    }

    fn closure(&self, labels: &[&str]) -> Vec<ClosureEntry> {
        labels
            .iter()
            .map(|label| ClosureEntry {
                label: (*label).to_string(),
                path: self.root.join(label).to_string_lossy().into_owned(),
            })
            .collect()
    }
}

fn type_spec(name: &str, version: Option<u64>, resources: &[&str]) -> TypeSpec {
    TypeSpec {
        name: name.to_string(),
        description: None,
        inputs: IndexMap::new(),
        params: IndexMap::from([("text".to_string(), json!({"type": "string"}))]),
        outputs: IndexMap::from([("text".to_string(), "text".to_string())]),
        judge: false,
        calls: IndexMap::new(),
        resources: resources.iter().map(|r| (*r).to_string()).collect(),
        tools: Vec::new(),
        view: None,
        version,
        retry: RetryMode::Service,
    }
}

fn described(
    path: &str,
    types: Vec<(&str, TypeSpec)>,
    closure: Vec<ClosureEntry>,
) -> ModuleDescription {
    ModuleDescription::Described {
        path: path.to_string(),
        types: types
            .into_iter()
            .map(|(attribute, spec)| DescribedType {
                attribute: attribute.to_string(),
                spec,
            })
            .collect(),
        closure,
    }
}

const N_PY: &str = "\"\"\"Two project node types.\"\"\"\n\nfrom nodes import helper\n";
const HELPER_PY: &str = "def frame(text):\n    return f\"[{text.strip()}]\"\n";

/// The local-identity / lock cases: `nodes/n.py` declares `echo` (unversioned, with the
/// resource `prompts/r.md`) and `pinned` (version 2), and imports `nodes/helper.py`.
fn local_project() -> (Fixture, FakeHost) {
    let fixture = Fixture::new();
    fixture.write("nodes/n.py", N_PY);
    fixture.write("nodes/helper.py", HELPER_PY);
    fixture.write("prompts/r.md", "Say it: ${{ text }}\n");
    let host = local_host(&fixture);
    (fixture, host)
}

fn local_host(fixture: &Fixture) -> FakeHost {
    FakeHost::new()
        .with_module(described(
            "nodes/n.py",
            vec![
                ("echo", type_spec("echo", None, &["prompts/r.md"])),
                ("pinned", type_spec("pinned", Some(2), &[])),
            ],
            fixture.closure(&["nodes/helper.py", "nodes/n.py"]),
        ))
        .with_module(described(
            "nodes/helper.py",
            Vec::new(),
            fixture.closure(&["nodes/helper.py"]),
        ))
}

fn problem(result: Result<Resolved, RegistryError>) -> String {
    match result {
        Err(RegistryError::Problem(text)) => text,
        other => panic!("expected a problem, got {other:?}"),
    }
}

fn node(result: Result<Resolved, RegistryError>) -> Rc<ResolvedType> {
    match result {
        Ok(Resolved::Node(resolved)) => resolved,
        other => panic!("expected a node type, got {other:?}"),
    }
}

// ------------------------------------------------------------- source digests

/// identity.md §14, "A node's source".
#[test]
fn a_nodes_source_matches_the_worked_example() {
    let fixture = Fixture::new();
    fixture.write("nodes/n.py", "x = 1\n");
    fixture.write("prompts/r.md", "Hello\n");
    let registry = fixture.registry();
    let source = registry
        .source_digests(&fixture.closure(&["nodes/n.py"]), &["prompts/r.md".into()])
        .unwrap();
    assert_eq!(
        source.files["nodes/n.py"],
        "9e26bf369911c45c243c684147b23fc9e1dcfcf257d299a1c632016a6fcd33f4"
    );
    assert_eq!(
        source.resources["prompts/r.md"],
        "66a045b452102c59d840ec097d59d9467e13a3f34f6494e539ffd32c1bb35f18"
    );
    assert_eq!(
        source.digest(),
        "f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3"
    );
    assert_eq!(
        source.to_value(),
        json!({
            "files": {"nodes/n.py": "9e26bf369911c45c243c684147b23fc9e1dcfcf257d299a1c632016a6fcd33f4"},
            "resources": {"prompts/r.md": "66a045b452102c59d840ec097d59d9467e13a3f34f6494e539ffd32c1bb35f18"},
        })
    );
}

#[test]
fn an_empty_source_still_names_its_kind() {
    let empty = SourceDigests::default();
    assert_eq!(empty.to_value(), json!({"files": {}, "resources": {}}));
    assert_eq!(
        empty.digest(),
        grida_fx_core::value::digest(
            &json!({"kind": "fx-node-source-v1", "files": {}, "resources": {}})
        )
    );
}

/// conformance `local-identity`: the source moves with the resource and with an imported module.
#[test]
fn a_source_moves_with_its_resource_and_its_imports() {
    let (fixture, mut host) = local_project();
    let source_of = |host: &mut FakeHost| {
        let mut registry = fixture.registry();
        let module = registry.describe_module("nodes/n.py", host).unwrap();
        registry.type_source(&module, "echo").unwrap().digest()
    };
    let first = source_of(&mut host);
    fixture.write("prompts/r.md", "Say it twice: ${{ text }}\n");
    let resource_edited = source_of(&mut host);
    fixture.write(
        "nodes/helper.py",
        "def frame(text):\n    return f\"<{text.strip()}>\"\n",
    );
    let import_edited = source_of(&mut host);
    assert_ne!(first, resource_edited);
    assert_ne!(resource_edited, import_edited);
    assert_ne!(first, import_edited);
}

/// conformance `resource-missing`: a refusal, never a crash.
#[test]
fn a_declared_resource_must_be_a_project_file() {
    let fixture = Fixture::new();
    fixture.write("nodes/n.py", "x = 1\n");
    std::fs::write(fixture.root.parent().unwrap().join("secret.md"), "s\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.root.parent().unwrap().join("secret.md"),
        fixture.root.join("escape.md"),
    )
    .unwrap();
    fixture.write("prompts/folder/keep.md", "k\n");
    let registry = fixture.registry();
    let closure = fixture.closure(&["nodes/n.py"]);
    for resource in ["prompts/missing.md", "../secret.md", "prompts/folder"] {
        assert_eq!(
            registry.source_digests(&closure, &[resource.into()]),
            Err(format!(
                "declared resource {resource} is not a project file"
            ))
        );
    }
    #[cfg(unix)]
    assert_eq!(
        registry.source_digests(&closure, &["escape.md".into()]),
        Err("declared resource escape.md is not a project file".into())
    );
}

#[test]
fn closure_files_are_read_and_named_by_their_label() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    assert_eq!(
        registry.source_digests(&fixture.closure(&["nodes/gone.py"]), &[]),
        Err("cannot read nodes/gone.py: no such file".into())
    );
    let bad = vec![ClosureEntry {
        label: "/abs/n.py".into(),
        path: "/abs/n.py".into(),
    }];
    assert!(registry.source_digests(&bad, &[]).is_err());
}

#[test]
fn modules_are_described_once() {
    let (fixture, mut host) = local_project();
    let mut registry = fixture.registry();
    let first = registry.describe_module("nodes/n.py", &mut host).unwrap();
    let again = registry.describe_module("nodes/n.py", &mut host).unwrap();
    assert_eq!(first, again);
    assert_eq!(host.describe_calls, 1);
    // A failed module is cached too.
    let failed = registry
        .describe_module("nodes/broken.py", &mut host)
        .unwrap();
    registry
        .describe_module("nodes/broken.py", &mut host)
        .unwrap();
    assert!(matches!(failed, ModuleDescription::Failed { .. }));
    assert_eq!(host.describe_calls, 2);
}

#[test]
fn type_source_serves_lock() {
    let (fixture, mut host) = local_project();
    let mut registry = fixture.registry();
    let module = registry.describe_module("nodes/n.py", &mut host).unwrap();
    let pinned = registry.type_source(&module, "pinned").unwrap();
    assert_eq!(
        pinned.files.keys().collect::<Vec<_>>(),
        ["nodes/helper.py", "nodes/n.py"]
    );
    assert!(pinned.resources.is_empty());
    assert_eq!(
        registry.type_source(&module, "nope"),
        Err("nodes/n.py: nope is not declared with @node".into())
    );
}

// ---------------------------------------------------------------- uses forms

#[test]
fn uses_that_resolve_to_nothing_are_problems() {
    let (fixture, mut host) = local_project();
    fixture.write("nodes/n.ts", "export {}\n");
    fixture.write("nodes/plain", "x\n");
    fixture.write("nodes/broken.py", "import not_a_module\n");
    std::fs::write(fixture.root.parent().unwrap().join("outside.py"), "x = 1\n").unwrap();
    let mut registry = fixture.registry();
    let cases = [
        (
            "./nodes/missing.py#x",
            "./nodes/missing.py#x: no file missing.py",
        ),
        ("../outside.py#x", "outside.py is outside the project"),
        (
            "./nodes/n.ts#x",
            "./nodes/n.ts#x: no node host for .ts modules yet",
        ),
        (
            "./nodes/plain#x",
            "./nodes/plain#x: no node host for modules without a suffix yet",
        ),
        (
            "./nodes/broken.py#x",
            "nodes/broken.py failed to import: ModuleNotFoundError: No module named 'nodes/broken.py'",
        ),
        (
            "./nodes/n.py#notnode",
            "./nodes/n.py#notnode: notnode is not declared with @node",
        ),
        (
            "./workflows/none.yaml",
            "./workflows/none.yaml: no workflow file ./workflows/none.yaml",
        ),
        (
            "fx/nosuch@1",
            "no built-in node type fx/nosuch@1; see grida-fx nodes",
        ),
        (
            "fx/9x@1",
            "fx/9x@1: published node packages are not available yet",
        ),
        (
            "acme/tools.x@2",
            "acme/tools.x@2: published node packages are not available yet",
        ),
        (
            "nodes/n.py#echo",
            "uses: 'nodes/n.py#echo' is fx/<type>@<major>, ./<path>#<name> or ./<path>.yaml",
        ),
    ];
    for (uses, want) in cases {
        assert_eq!(problem(registry.node_type(uses, &mut host)), want, "{uses}");
    }
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_out_of_the_project_is_outside() {
    let (fixture, mut host) = local_project();
    let outside = fixture.root.parent().unwrap().join("outside.py");
    std::fs::write(&outside, "x = 1\n").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.root.join("nodes/link.py")).unwrap();
    let mut registry = fixture.registry();
    assert_eq!(
        problem(registry.node_type("./nodes/link.py#x", &mut host)),
        "link.py is outside the project"
    );
    assert_eq!(host.describe_calls, 0);
}

#[test]
fn a_host_that_cannot_start_is_fatal() {
    let (fixture, _) = local_project();
    let mut registry = fixture.registry();
    let mut host = grida_fx_core::host::NoHost;
    match registry.node_type("./nodes/n.py#echo", &mut host) {
        Err(RegistryError::Fatal(error)) => assert_eq!(error.kind, ErrorKind::Host),
        other => panic!("expected a fatal error, got {other:?}"),
    }
}

// ------------------------------------------------------------ project files

#[test]
fn project_files_are_read_by_content_once() {
    let fixture = Fixture::new();
    fixture.write("data/one.txt", "one\ntwo\n");
    let mut registry = fixture.registry();
    let file = registry.project_file("./data/one.txt").unwrap();
    assert_eq!(
        file.digest,
        "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8"
    );
    assert_eq!(file.name, "data/one.txt");
    assert_eq!(file.size, 8);
    assert_eq!(file.key, None);
    assert_eq!(file.content, None);
    assert_eq!(file.location, Some(fixture.root.join("data/one.txt")));
    // Read once: the cached value survives an edit.
    fixture.write("data/one.txt", "changed\n");
    assert_eq!(registry.project_file("./data/one.txt").unwrap(), file);
    assert_eq!(
        registry.project_file("./data/../data/one.txt").unwrap(),
        file
    );
}

#[test]
fn project_files_stay_in_the_project() {
    let fixture = Fixture::new();
    fixture.write("data/folder/x.txt", "x\n");
    std::fs::write(fixture.root.parent().unwrap().join("outside.txt"), "x\n").unwrap();
    let mut registry = fixture.registry();
    assert_eq!(
        registry.project_file("../outside.txt"),
        Err("../outside.txt is outside the project".into())
    );
    assert_eq!(
        registry.project_file("./data/missing.txt"),
        Err("no such file".into())
    );
    assert_eq!(
        registry.project_file("./data/folder"),
        Err("not a file".into())
    );
    let absolute = fixture.root.join("data/folder/x.txt");
    let refused = registry
        .project_file(absolute.to_str().unwrap())
        .unwrap_err();
    assert_eq!(
        refused,
        "an absolute path is not a project file; write it as ./<path> inside the project"
    );
}

#[test]
fn input_files_take_the_declared_kind_for_an_unknown_suffix() {
    let fixture = Fixture::new();
    fixture.write("data/blob.bin", "abc");
    let mut registry = fixture.registry();
    let file = registry.input_file("data/blob.bin", "image").unwrap();
    assert_eq!(file.kind, "image");
    assert_eq!(file.name, "blob.bin");
    assert_eq!(file.size, 3);
    assert_eq!(file.digest, grida_fx_core::value::file_digest(b"abc"));
    // A project file of the same bytes keeps its own name.
    let same = registry.project_file("./data/blob.bin").unwrap();
    assert_eq!(same.name, "data/blob.bin");
    assert_eq!(
        registry.input_file("data/none.bin", "file"),
        Err("no input file data/none.bin".into())
    );
}

#[test]
fn route_defaults_come_from_the_registry() {
    let fixture = Fixture::new();
    let registry = Registry::new(
        fixture.root.clone(),
        Vec::new(),
        LockFile::default(),
        IndexMap::from([("image.generate".to_string(), "img-a@acme".to_string())]),
    );
    assert_eq!(registry.route_default("image.generate"), Some("img-a@acme"));
    assert_eq!(registry.route_default("speech.generate"), None);
}

#[test]
fn paths_inside_the_project() {
    let fixture = Fixture::new();
    fixture.write("a/b.txt", "b");
    assert_eq!(
        relative_inside(&fixture.root, &fixture.root.join("a/./b.txt")),
        Some("a/b.txt".into())
    );
    assert_eq!(
        relative_inside(&fixture.root, &fixture.root.join("a/../../x")),
        None
    );
    assert_eq!(
        read_project_bytes(&fixture.root, "a/b.txt").unwrap(),
        Some(b"b".to_vec())
    );
    assert_eq!(
        read_project_bytes(&fixture.root, "a/none.txt").unwrap(),
        None
    );
    assert_eq!(
        read_project_bytes(&fixture.root, "../proj/a/b.txt").unwrap(),
        Some(b"b".to_vec())
    );
    assert_eq!(read_project_bytes(&fixture.root, "a").unwrap(), None);
}

// -------------------------------------------------------------------- lock

/// conformance `lock-write`, before any fx.lock exists.
#[test]
fn lock_names_unlocked_types_and_writes_them() {
    let (fixture, mut host) = local_project();
    let project = fixture.project();
    let checked = lock(&project, &[], true, &mut host).unwrap();
    assert_eq!(
        checked.lines,
        ["unlocked  nodes/n.py#pinned@2: run grida-fx lock"]
    );
    assert_eq!(checked.status, 1);
    assert_eq!(checked.write, None);
    assert_eq!(host.describe_calls, 1);

    let written = lock(&project, &[], false, &mut host).unwrap();
    assert_eq!(written.lines, ["fx.lock: 1 versioned node types"]);
    assert_eq!(written.status, 0);
    let digest = pinned_source(&fixture, &mut host);
    assert_eq!(
        written.write.unwrap().nodes,
        [("nodes/n.py#pinned@2".to_string(), digest)].into()
    );
}

fn pinned_source(fixture: &Fixture, host: &mut FakeHost) -> String {
    let mut registry = fixture.registry();
    let module = registry.describe_module("nodes/n.py", host).unwrap();
    registry.type_source(&module, "pinned").unwrap().digest()
}

#[test]
fn lock_without_node_modules_starts_no_host() {
    let fixture = Fixture::new();
    let mut host = FakeHost::new();
    let report = lock(&fixture.project(), &[], true, &mut host).unwrap();
    assert_eq!(
        report.lines,
        ["fx.lock: 0 versioned node types, all locked"]
    );
    assert_eq!(report.status, 0);
    assert_eq!(host.describe_calls, 0);
}

#[test]
fn lock_refuses_a_broken_module_and_a_missing_resource() {
    let (fixture, host) = local_project();
    fixture.write("nodes/z_broken.py", "import nothing\n");
    let mut broken = host.clone();
    let error = lock(&fixture.project(), &[], false, &mut broken).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Plan);
    assert!(
        error
            .message
            .starts_with("nodes/z_broken.py failed to import")
    );

    let fixture = Fixture::new();
    fixture.write("nodes/n.py", "x = 1\n");
    let mut host = FakeHost::new().with_module(described(
        "nodes/n.py",
        vec![("lost", type_spec("lost", Some(1), &["prompts/missing.md"]))],
        fixture.closure(&["nodes/n.py"]),
    ));
    let error = lock(&fixture.project(), &[], false, &mut host).unwrap_err();
    assert_eq!(
        error.message,
        "nodes/n.py#lost: declared resource prompts/missing.md is not a project file"
    );
}

const DRIFTED_LOCK: &str = "# fx.lock: the source behind each versioned node type; commit it\n\
fx: lock/v1\n\
nodes:\n  \"nodes/n.py#pinned@2\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n";

/// conformance `lock-drift`: `--check` and a plain lock refuse; `--same` confirms.
#[test]
fn integrated_lock_drift() {
    let (fixture, mut host) = local_project();
    fixture.write("fx.lock", DRIFTED_LOCK);
    let project = fixture.project();
    let old = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
    let changed = "changed   nodes/n.py#pinned: its source changed but version 2 did not; bump \
                   it, or confirm with --same nodes/n.py#pinned";

    let checked = lock(&project, &[], true, &mut host).unwrap();
    assert_eq!(checked.lines, [changed]);
    assert_eq!((checked.status, checked.write), (1, None));

    let plain = lock(&project, &[], false, &mut host).unwrap();
    assert_eq!(plain.lines, [changed, "fx.lock: 1 versioned node types"]);
    assert_eq!(plain.status, 1);
    assert_eq!(
        plain.write.unwrap().nodes,
        [("nodes/n.py#pinned@2".to_string(), old.clone())].into()
    );

    let same = ["nodes/n.py#pinned".to_string()];
    let checked_same = lock(&project, &same, true, &mut host).unwrap();
    assert_eq!(
        checked_same.lines,
        ["fx.lock: 1 versioned node types, all locked"]
    );
    assert_eq!((checked_same.status, checked_same.write), (0, None));

    let confirmed = lock(&project, &same, false, &mut host).unwrap();
    assert_eq!(confirmed.lines, ["fx.lock: 1 versioned node types"]);
    assert_eq!(confirmed.status, 0);
    let digest = pinned_source(&fixture, &mut host);
    assert_eq!(
        confirmed.write.unwrap().nodes,
        [("nodes/n.py#pinned@2".to_string(), digest)].into()
    );
}

/// Stale entries are kept and counted; a `--same` naming nothing is ignored.
#[test]
fn integrated_lock_keeps_stale_entries() {
    let (fixture, mut host) = local_project();
    let digest = pinned_source(&fixture, &mut host);
    fixture.write(
        "fx.lock",
        &format!(
            "fx: lock/v1\nnodes:\n  \"nodes/gone.py#old@1\": \"{0}\"\n  \"nodes/n.py#pinned@2\": \"{digest}\"\n",
            "a".repeat(64)
        ),
    );
    let report = lock(
        &fixture.project(),
        &["nodes/x.py#y".into()],
        true,
        &mut host,
    )
    .unwrap();
    assert_eq!(
        report.lines,
        ["fx.lock: 2 versioned node types, all locked"]
    );
    assert_eq!(report.status, 0);
}

// ---------------------------------------------------- resolved project types

/// identity.md §14: the unversioned type's identity is `source:` and the digest.
#[test]
fn integrated_an_unversioned_type_is_named_by_its_source() {
    let fixture = Fixture::new();
    fixture.write("nodes/n.py", "x = 1\n");
    fixture.write("prompts/r.md", "Hello\n");
    let mut host = FakeHost::new().with_module(described(
        "nodes/n.py",
        vec![("echo", type_spec("echo", None, &["prompts/r.md"]))],
        fixture.closure(&["nodes/n.py"]),
    ));
    let mut registry = fixture.registry();
    let resolved = node(registry.node_type("./nodes/n.py#echo", &mut host));
    assert_eq!(
        resolved.identity,
        "source:f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3"
    );
    assert_eq!(resolved.uses, "./nodes/n.py#echo");
    assert_eq!(resolved.body, BodyKind::Project);
    assert_eq!(
        resolved.origin,
        TypeOrigin::Project {
            path: "nodes/n.py".into(),
            attribute: "echo".into()
        }
    );
    assert_eq!(resolved.drift, None);
    assert_eq!(
        resolved.source.as_ref().unwrap().digest(),
        "f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3"
    );
    // Cached: no second describe.
    node(registry.node_type("./nodes/n.py#echo", &mut host));
    assert_eq!(host.describe_calls, 1);
}

/// conformance `local-identity`: the unversioned identity moves with the resource and the
/// helper module; the versioned one does not.
#[test]
fn integrated_local_identity() {
    let (fixture, mut host) = local_project();
    let identities = |host: &mut FakeHost| {
        let mut registry = fixture.registry();
        let echo = node(registry.node_type("./nodes/n.py#echo", host));
        let pinned = node(registry.node_type("./nodes/n.py#pinned", host));
        (echo.identity.clone(), pinned.identity.clone())
    };
    let (echo, pinned) = identities(&mut host);
    assert!(echo.starts_with("source:"));
    assert_eq!(pinned, "nodes/n.py#pinned@2");
    fixture.write("prompts/r.md", "Say it twice: ${{ text }}\n");
    let (echo_resource, pinned_resource) = identities(&mut host);
    fixture.write(
        "nodes/helper.py",
        "def frame(text):\n    return f\"<{text.strip()}>\"\n",
    );
    let (echo_import, pinned_import) = identities(&mut host);
    assert_ne!(echo, echo_resource);
    assert_ne!(echo_resource, echo_import);
    assert_eq!(pinned, pinned_resource);
    assert_eq!(pinned, pinned_import);
}

/// conformance `lock-drift` and `resource-missing` on the step's type.
#[test]
fn integrated_drift_and_missing_resources() {
    let (fixture, mut host) = local_project();
    let digest = pinned_source(&fixture, &mut host);
    let lock_of = |digest: &str| LockFile {
        nodes: [("nodes/n.py#pinned@2".to_string(), digest.to_string())].into(),
    };
    let mut drifted = fixture.registry_with(lock_of(&"0".repeat(64)));
    let resolved = node(drifted.node_type("./nodes/n.py#pinned", &mut host));
    assert_eq!(
        resolved.drift.as_deref(),
        Some(
            "./nodes/n.py#pinned: its source changed but version 2 did not; bump the version, \
             or confirm no change in behaviour with grida-fx lock --same nodes/n.py#pinned"
        )
    );
    assert_eq!(resolved.identity, "nodes/n.py#pinned@2");
    assert!(resolved.source.is_some());
    let mut locked = fixture.registry_with(lock_of(&digest));
    assert_eq!(
        node(locked.node_type("./nodes/n.py#pinned", &mut host)).drift,
        None
    );

    std::fs::remove_file(fixture.root.join("prompts/r.md")).unwrap();
    let mut registry = fixture.registry();
    assert_eq!(
        problem(registry.node_type("./nodes/n.py#echo", &mut host)),
        "./nodes/n.py#echo: declared resource prompts/r.md is not a project file"
    );
}

#[test]
fn integrated_an_invalid_spec_is_a_problem_on_the_step() {
    let fixture = Fixture::new();
    fixture.write("nodes/n.py", "x = 1\n");
    let mut host = FakeHost::new().with_module(described(
        "nodes/n.py",
        vec![("bad", type_spec("Bad Name", None, &[]))],
        fixture.closure(&["nodes/n.py"]),
    ));
    let mut registry = fixture.registry();
    let text = problem(registry.node_type("./nodes/n.py#bad", &mut host));
    assert!(text.starts_with("./nodes/n.py#bad: "), "{text}");
}

#[test]
fn integrated_builtins_are_named_by_their_version() {
    let fixture = Fixture::new();
    let mut registry = fixture.registry();
    let mut host = FakeHost::new();
    let select = node(registry.node_type("fx/select@1", &mut host));
    assert_eq!(select.identity, "fx/select@1.1");
    assert!(select.is_select());
    assert_eq!(select.source, None);
    let leading_zero = node(registry.node_type("fx/select@01", &mut host));
    assert_eq!(leading_zero.uses, "fx/select@01");
    assert_eq!(leading_zero.identity, "fx/select@1.1");
    let image = node(registry.node_type("fx/image.generate@1", &mut host));
    assert_eq!(image.identity, "fx/image.generate@1.1");
    assert!(!image.is_select());
    assert_eq!(host.describe_calls, 0);
}

#[test]
fn integrated_used_workflows_load_once() {
    let fixture = Fixture::new();
    fixture.write(
        "workflows/inner.yaml",
        "fx: workflow/v1\nid: inner\ntitle: Inner\nsteps:\n  s:\n    uses: fx/select@1\n    with: { first_of: [1] }\n",
    );
    fixture.write("workflows/bad.yaml", "fx: workflow/v1\nid: Bad Id\n");
    let mut registry = fixture.registry();
    let mut host = FakeHost::new();
    let Ok(Resolved::Workflow(first)) = registry.node_type("./workflows/inner.yaml", &mut host)
    else {
        panic!("expected a workflow")
    };
    assert_eq!(first.source, "workflows/inner.yaml");
    assert_eq!(first.workflow.id, "inner");
    let Ok(Resolved::Workflow(again)) =
        registry.node_type("./workflows/../workflows/inner.yaml", &mut host)
    else {
        panic!("expected a workflow")
    };
    assert!(Rc::ptr_eq(&first, &again));
    assert!(matches!(
        registry.node_type("./workflows/bad.yaml", &mut host),
        Err(RegistryError::Fatal(_))
    ));
}

// --------------------------------------------------------- finding workflows

#[test]
fn workflow_files_are_searched_in_order() {
    let fixture = Fixture::new();
    for file in [
        "b.yaml",
        "a.yml",
        "fx.yaml",
        "case.takes.yaml",
        "notes.json",
        "workflows/z.yaml",
        "workflows/sub/y.yml",
        "workflows/sub-a/x.yaml",
        "workflows/sub/deeper/w.yaml",
        "other/v.yaml",
        "upper.YAML",
    ] {
        fixture.write(file, "fx: workflow/v1\n");
    }
    std::fs::create_dir_all(fixture.root.join("folder.yaml")).unwrap();
    let found: Vec<String> = project_workflow_files(&fixture.project())
        .iter()
        .map(|p| relative_inside(&fixture.root, p).unwrap())
        .collect();
    assert_eq!(
        found,
        [
            "a.yml",
            "b.yaml",
            "workflows/z.yaml",
            "workflows/sub/y.yml",
            "workflows/sub-a/x.yaml",
            "workflows/sub/deeper/w.yaml",
        ]
    );
}

#[test]
fn finding_a_workflow_that_is_not_there() {
    let fixture = Fixture::new();
    fixture.write("workflows/other.yaml", "fx: workflow/v1\nid: other\n");
    fixture.write("workflows/broken.yaml", "workflow/v1: [\n");
    let project = fixture.project();
    let error = find_workflow("nope.yaml", &project, &fixture.root).unwrap_err();
    assert_eq!(error.message, "no workflow file nope.yaml");
    assert_eq!(error.kind, ErrorKind::Plan);
    let error = find_workflow("nosuch", &project, &fixture.root).unwrap_err();
    assert_eq!(error.message, "no workflow with id 'nosuch' under proj");
    let error = find_workflow("x.py", &project, &fixture.root).unwrap_err();
    assert_eq!(error.message, "no workflow with id 'x.py' under proj");
    fixture.write("w/f.json", "{}");
    assert_eq!(
        find_workflow("w/f.json", &project, &fixture.root).unwrap(),
        fixture.root.join("w/f.json")
    );
}

#[test]
fn a_workflow_the_strict_loader_refuses_is_reported_not_missing() {
    let fixture = Fixture::new();
    fixture.write(
        "workflows/a.yaml",
        "fx: workflow/v1\nid: other\nratio: 4:3\n",
    );
    fixture.write(
        "workflows/case.yaml",
        "fx: workflow/v1\nid: case\nsteps:\n  a:\n    uses: fx/select@1\n    with:\n      aspect_ratio: 16:9\n",
    );
    fixture.write(
        "workflows/later.yaml",
        "fx: workflow/v1\nid: case\nid: case\n",
    );
    let project = fixture.project();
    // Both broken files mention `case`; the first in search order is reported.
    let error = find_workflow("case", &project, &fixture.root).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Yaml);
    assert_eq!(
        error.message,
        "workflows/case.yaml:7:21: plain scalar '16:9' is ambiguous (a sexagesimal number); quote it"
    );
    // A broken file that does not mention the id is still skipped in silence.
    let error = find_workflow("nosuch", &project, &fixture.root).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Plan);
    assert_eq!(error.message, "no workflow with id 'nosuch' under proj");
    // A file that declares the id wins over a broken one that mentions it.
    fixture.write(
        "workflows/b.yaml",
        "fx: workflow/v1\nid: case\ntitle: T\nsteps:\n  a:\n    uses: fx/select@1\n",
    );
    assert_eq!(
        find_workflow("case", &project, &fixture.root).unwrap(),
        fixture.root.join("workflows/b.yaml")
    );
}

#[test]
fn integrated_finding_a_workflow_by_id() {
    let fixture = Fixture::new();
    let wf = |id: &str| {
        format!("fx: workflow/v1\nid: {id}\ntitle: T\nsteps:\n  a:\n    uses: fx/select@1\n")
    };
    fixture.write("dup1.yaml", &wf("enum-opt"));
    fixture.write("workflows/enum.yaml", &wf("enum-opt"));
    fixture.write("workflows/sub/dup2.yml", &wf("enum-opt"));
    fixture.write("workflows/one.yaml", &wf("one"));
    fixture.write("workflows/one.takes.yaml", &wf("one"));
    fixture.write(
        "workflows/broken.yaml",
        "fx: workflow/v1\nid: one\nid: two\n",
    );
    let project = fixture.project();
    let error = find_workflow("enum-opt", &project, &fixture.root).unwrap_err();
    assert_eq!(
        error.message,
        "workflow id 'enum-opt' is declared twice: dup1.yaml, workflows/enum.yaml, workflows/sub/dup2.yml"
    );
    assert_eq!(
        find_workflow("one", &project, &fixture.root).unwrap(),
        fixture.root.join("workflows/one.yaml")
    );
}

// ------------------------------------------------------------ make_planner

const CASE: &str = "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  a:\n    uses: ./nodes/n.py#echo\n    with: { text: hello }\n";

fn request(fixture: &Fixture, target: &str) -> PlanRequest {
    PlanRequest {
        target: target.to_string(),
        cwd: fixture.root.clone(),
        ..PlanRequest::default()
    }
}

#[test]
fn a_builder_refuses_input_flags_before_it_runs() {
    let fixture = Fixture::new();
    let mut host = FakeHost::new();
    let mut asked = request(&fixture, "builders/b.py:build");
    asked.rest = vec!["--count".into(), "3".into()];
    let error = make_planner(&asked, &mut host).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Usage);
    assert_eq!(
        error.message,
        "unknown flag --count; a builder takes its arguments as --arg"
    );
    assert_eq!(host.build_calls, 0);
}

#[test]
fn integrated_make_planner_reports_the_first_broken_step() {
    let fixture = Fixture::new();
    fixture.write("fx.yaml", "fx: project/v1\n");
    let mut host = FakeHost::new();
    // 2: the workflow before the inputs and routes.
    let mut asked = request(&fixture, "nosuch");
    asked.input_files = vec!["missing-inputs.yaml".into()];
    asked.routes = vec!["missing-routes.yaml".into()];
    let error = make_planner(&asked, &mut host).unwrap_err();
    assert_eq!(error.message, "no workflow with id 'nosuch' under proj");
    // 2: a builder file that does not exist.
    let error = make_planner(&request(&fixture, "builders/b.py:build"), &mut host).unwrap_err();
    assert_eq!(error.message, "no builder file builders/b.py");
    assert_eq!(host.build_calls, 0);
    // 2: a builder that fails gives the host's message.
    fixture.write("builders/b.py", "def build(): ...\n");
    host.builds.insert(
        "builders/b.py:build".into(),
        Err(RpcError::new(
            ErrorCode::BuildFailed,
            "build: ValueError: no levels",
        )),
    );
    let error = make_planner(&request(&fixture, "builders/b.py:build"), &mut host).unwrap_err();
    assert_eq!(
        (error.kind, error.message.as_str()),
        (ErrorKind::Plan, "build: ValueError: no levels")
    );
    let error = make_planner(&request(&fixture, "builders/b.py:other"), &mut host).unwrap_err();
    assert_eq!(error.message, "builders/b.py has no function other");
    // 4: the inputs before the routes.
    fixture.write("workflows/case.yaml", CASE);
    let error = make_planner(
        &asked_with(
            &fixture,
            "case",
            &["missing-inputs.yaml"],
            &["missing-routes.yaml"],
        ),
        &mut host,
    )
    .unwrap_err();
    assert!(
        error.message.contains("missing-inputs.yaml"),
        "{}",
        error.message
    );
    // 6: then the routes.
    let error = make_planner(
        &asked_with(&fixture, "case", &[], &["missing-routes.yaml"]),
        &mut host,
    )
    .unwrap_err();
    assert!(
        error.message.contains("missing-routes.yaml"),
        "{}",
        error.message
    );
    // 7: then the takes file.
    fixture.write("routes.yaml", "fx: routes/v1\nroutes: []\n");
    fixture.write("workflows/case.takes.yaml", "a: { take: 0 }\n");
    let error = make_planner(
        &asked_with(&fixture, "case", &[], &["routes.yaml"]),
        &mut host,
    )
    .unwrap_err();
    assert!(
        error.message.contains("case.takes.yaml"),
        "{}",
        error.message
    );
}

fn asked_with(fixture: &Fixture, target: &str, inputs: &[&str], routes: &[&str]) -> PlanRequest {
    PlanRequest {
        input_files: inputs.iter().map(|s| (*s).to_string()).collect(),
        routes: routes.iter().map(|s| (*s).to_string()).collect(),
        ..request(fixture, target)
    }
}

#[test]
fn integrated_a_builder_builds_through_the_host() {
    let fixture = Fixture::new();
    fixture.write(
        "fx.yaml",
        "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n",
    );
    fixture.write("builders/b.py", "def build(level): ...\n");
    fixture.write("builders/lib/levels.py", "x = 1\n");
    let document = json!({
        "fx": "workflow/v1",
        "id": "levels",
        "title": "Levels",
        "steps": {"a": {"uses": "fx/select@1", "with": {"first_of": [1]}}},
    });
    let mut host = FakeHost::new();
    host.builds.insert(
        "builders/b.py:build".into(),
        Ok(BuildResult {
            document: document.clone(),
            takes_anchor: "builders/lib/levels.py".into(),
        }),
    );
    let mut asked = request(&fixture, "builders/b.py:build");
    asked.arguments = IndexMap::from([("level".to_string(), "2".to_string())]);
    asked.max_usd = Some(Usd(1_500_000));
    let planner = make_planner(&asked, &mut host).unwrap();
    assert_eq!(host.build_calls, 1);
    assert_eq!(planner.workflow.source, "builders/b.py:build");
    assert_eq!(planner.workflow.document, document);
    assert_eq!(planner.workflow.workflow.id, "levels");
    assert_eq!(planner.workflow.path, fixture.root.join("builders/b.py"));
    assert_eq!(planner.home.root, planner.project.root);
    assert_eq!(
        planner.takes_path,
        fixture.root.join("builders/lib/levels.takes.yaml")
    );
    assert_eq!(planner.max_usd, Some(Usd(1_500_000)));
    assert_eq!(
        planner.registry.route_default("image.generate"),
        Some("img-a@acme")
    );
}

#[test]
fn integrated_a_workflow_found_by_id_plans_from_its_home() {
    let fixture = Fixture::new();
    fixture.write(
        "fx.yaml",
        "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n",
    );
    fixture.write("workflows/case.yaml", CASE);
    fixture.write("workflows/case.takes.yaml", "a: { take: 2 }\n");
    let mut host = FakeHost::new();
    let planner = make_planner(&request(&fixture, "case"), &mut host).unwrap();
    assert_eq!(planner.workflow.source, "workflows/case.yaml");
    assert_eq!(
        planner.takes_path,
        fixture.root.join("workflows/case.takes.yaml")
    );
    assert_eq!(
        planner.takes["a"],
        TakeChoice {
            take: 2,
            result: None
        }
    );
    assert_eq!(planner.home, planner.project);
    assert_eq!(host.describe_calls, 0);

    let (project, workflow) = load_target(
        "workflows/case.yaml",
        &fixture.root,
        &IndexMap::new(),
        &mut host,
    )
    .unwrap();
    assert_eq!(project.root, fixture.root);
    assert_eq!(workflow.source, "workflows/case.yaml");
}

/// A workflow under a nested fx.yaml: its home is the nested project, its takes stay in the
/// planning project, and the planning project's route defaults win.
#[test]
fn integrated_a_nested_home() {
    let fixture = Fixture::new();
    fixture.write(
        "fx.yaml",
        "fx: project/v1\nroutes:\n  image.generate: img-b@acme\n",
    );
    fixture.write(
        "workflows/pkg/fx.yaml",
        "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n  speech.generate: voice-a@acme\n",
    );
    fixture.write("workflows/pkg/case.yaml", CASE);
    let mut host = FakeHost::new();
    let planner = make_planner(&request(&fixture, "case"), &mut host).unwrap();
    assert_eq!(planner.project.root, fixture.root);
    assert_eq!(planner.home.root, fixture.root.join("workflows/pkg"));
    assert_eq!(planner.workflow.source, "case.yaml");
    assert_eq!(planner.takes_path, fixture.root.join("case.takes.yaml"));
    assert_eq!(planner.registry.root, fixture.root.join("workflows/pkg"));
    assert_eq!(
        planner.registry.route_default("image.generate"),
        Some("img-b@acme")
    );
    assert_eq!(
        planner.registry.route_default("speech.generate"),
        Some("voice-a@acme")
    );
}

// ---------------------------------------------------------- make_plan

const AT_PLAN: &str = "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  a:\n    uses: ./nodes/n.py#echo\n    at: plan\n    with: { text: hello }\n";

fn at_plan_project() -> (Fixture, FakeHost) {
    let (fixture, host) = local_project();
    fixture.write("fx.yaml", "fx: project/v1\nbudget: { max_usd: 2 }\n");
    fixture.write("workflows/case.yaml", AT_PLAN);
    (fixture, host)
}

#[test]
fn integrated_an_at_plan_step_without_a_runner() {
    let (fixture, mut host) = at_plan_project();
    let mut planner = make_planner(&request(&fixture, "case"), &mut host).unwrap();
    let plan = make_plan(&mut planner, &mut host, None, &NoCache).unwrap();
    assert_eq!(
        plan.problems,
        [Problem::new(
            "a",
            "running an at: plan step is not available until the runner lands"
        )]
    );
    assert_eq!(plan.ceiling, Some(Usd(2_000_000)));
    assert!(plan.cached.is_empty());
    assert!(planner.results.is_empty());
}

struct Runner {
    result: NodeResult,
    runs: usize,
}

impl PlanTimeRunner for Runner {
    fn run(&mut self, _: &Instance, _: &Planner) -> Result<NodeResult, Error> {
        self.runs += 1;
        Ok(self.result.clone())
    }
}

#[test]
fn integrated_an_at_plan_step_with_a_runner() {
    let (fixture, mut host) = at_plan_project();
    let mut planner = make_planner(&request(&fixture, "case"), &mut host).unwrap();
    let mut runner = Runner {
        result: NodeResult {
            status: grida_fx_core::expand::ResultStatus::Failed,
            outputs: IndexMap::new(),
            facts: IndexMap::new(),
            error: Some("RuntimeError: boom".into()),
        },
        runs: 0,
    };
    let plan = make_plan(&mut planner, &mut host, Some(&mut runner), &NoCache).unwrap();
    assert_eq!(runner.runs, 1);
    assert_eq!(
        plan.problems,
        [Problem::new(
            "a",
            "failed while planning: RuntimeError: boom"
        )]
    );
    assert!(planner.results.contains_key("a#1"));
    let mut asked = request(&fixture, "case");
    asked.max_usd = Some(Usd::ZERO);
    let mut planner = make_planner(&asked, &mut host).unwrap();
    let plan = make_plan(&mut planner, &mut host, None, &NoCache).unwrap();
    assert_eq!(plan.ceiling, Some(Usd::ZERO));
}

#[test]
fn integrated_make_plan_counts_cached_identities() {
    struct All;
    impl grida_fx_core::host::ResultCache for All {
        fn has_result(&self, _: &grida_fx_core::expand::Instance) -> bool {
            true
        }
    }
    let (fixture, mut host) = local_project();
    fixture.write("fx.yaml", "fx: project/v1\n");
    fixture.write("workflows/case.yaml", CASE);
    let mut planner = make_planner(&request(&fixture, "case"), &mut host).unwrap();
    let plan = make_plan(&mut planner, &mut host, None, &All).unwrap();
    assert!(plan.ok(), "{:?}", plan.problems);
    assert_eq!(plan.cached, BTreeSet::from(["a#1".to_string()]));
    assert_eq!(plan.known(), 1);
    assert_eq!(plan.ceiling, None);
}

// ------------------------------------------------- estimate, phases, warnings

fn spec() -> NodeSpec {
    NodeSpec {
        name: "t".into(),
        description: None,
        inputs: IndexMap::new(),
        params: IndexMap::new(),
        outputs: IndexMap::new(),
        judge: false,
        capability: None,
        calls: IndexMap::new(),
        resources: Vec::new(),
        tools: Vec::new(),
        view: None,
        version: Some(1),
        retry: Retry::Service,
    }
}

fn usd(micros: i64) -> Usd {
    Usd(micros)
}

/// One instance; `prices` are `(capability, route, calls, low, high)` in micro-dollars.
fn inst(path: &str, phase: u32, state: State, prices: &[(&str, &str, u32, i64, i64)]) -> Instance {
    Instance {
        id: format!("{path}#1"),
        path: path.to_string(),
        step: path.to_string(),
        takes: vec![1],
        uses: "fx/t@1".into(),
        ty: Rc::new(ResolvedType {
            uses: "fx/t@1".into(),
            identity: "fx/t@1.1".into(),
            spec: Rc::new(spec()),
            origin: TypeOrigin::Builtin {
                name: "t".into(),
                major: 1,
            },
            body: BodyKind::None,
            source: None,
            drift: None,
        }),
        with: IndexMap::new(),
        needs: Vec::new(),
        state,
        identity: Some(format!("identity-of-{path}")),
        routes: IndexMap::new(),
        prices: prices
            .iter()
            .map(|(capability, route, calls, low, high)| CallPrice {
                capability: (*capability).into(),
                route: (*route).into(),
                calls: *calls,
                low: usd(*low),
                high: usd(*high),
            })
            .collect(),
        phase,
        key: None,
        judges: None,
        judge_policy: None,
        judged_by: Vec::new(),
        view: Value::Bool(false),
        at_plan: false,
        budget: None,
        concurrency_group: None,
        concurrency: None,
        timeout_s: None,
        reason: None,
        reads: BTreeSet::new(),
    }
}

fn pending(path: &str, max: u32, high: i64, phase: u32) -> PendingRepeat {
    PendingRepeat {
        path: path.to_string(),
        max,
        waiting_on: BTreeSet::new(),
        per_instance_low: Usd::ZERO,
        per_instance_high: usd(high),
        phase,
    }
}

fn plan_of(instances: Vec<Instance>, repeats: Vec<PendingRepeat>) -> Plan {
    Plan {
        expansion: Expansion {
            instances: instances.into_iter().map(|i| (i.id.clone(), i)).collect(),
            pending: repeats,
            ..Expansion::default()
        },
        problems: Vec::new(),
        ceiling: None,
        cached: BTreeSet::new(),
    }
}

/// conformance `refusals`: four live steps in phase 1, and the unbounded repeat in phase 2.
#[test]
fn refusals_estimate_and_phases() {
    let plan = plan_of(
        vec![
            inst(
                "draw",
                1,
                State::Planned,
                &[("image.generate", "img-a@acme", 1, 10_000, 40_000)],
            ),
            inst(
                "write",
                1,
                State::Planned,
                &[("structured.generate", "llm-a@acme", 1, 1_000, 2_000)],
            ),
            inst(
                "review",
                1,
                State::Planned,
                &[("vision.review", "llm-a@other", 1, 2_000, 4_000)],
            ),
            inst("lost", 1, State::Planned, &[]),
        ],
        vec![pending("unbounded", 1, 0, 2)],
    );
    assert_eq!(
        plan.estimate(),
        Estimate {
            low: usd(13_000),
            high: usd(46_000)
        }
    );
    assert_eq!(
        plan.phases(),
        [
            PhaseSummary {
                phase: 1,
                steps: 4,
                calls_low: 3,
                calls_high: 3,
                low: usd(13_000),
                high: usd(46_000),
                pending: Vec::new(),
            },
            PhaseSummary {
                phase: 2,
                steps: 0,
                calls_low: 0,
                calls_high: 0,
                low: Usd::ZERO,
                high: Usd::ZERO,
                pending: vec!["unbounded (up to 1)".into()],
            },
        ]
    );
    assert_eq!(plan.known(), 4);
}

/// conformance `phase`: a free step in phase 1, then `draw` waits on its list (up to 6).
#[test]
fn phase_estimate_and_phases() {
    let plan = plan_of(
        vec![inst("split", 1, State::Planned, &[])],
        vec![pending("draw", 6, 40_000, 2)],
    );
    assert_eq!(
        plan.estimate(),
        Estimate {
            low: Usd::ZERO,
            high: usd(240_000)
        }
    );
    let phases = plan.phases();
    assert_eq!(phases.len(), 2);
    assert_eq!(
        phases[0],
        PhaseSummary {
            phase: 1,
            steps: 1,
            calls_low: 0,
            calls_high: 0,
            low: Usd::ZERO,
            high: Usd::ZERO,
            pending: Vec::new(),
        }
    );
    assert_eq!(phases[1].steps, 0);
    assert_eq!(phases[1].high, usd(240_000));
    assert_eq!(phases[1].pending, ["draw (up to 6)"]);
}

/// Maybe counts towards high only, cached and done never count, phase 1 always exists.
#[test]
fn maybe_cached_and_done_instances() {
    let image = [("image.generate", "img-a@acme", 2, 10_000, 40_000)];
    let mut plan = plan_of(
        vec![
            inst("a", 2, State::Planned, &image),
            inst("b", 2, State::Maybe, &image),
            inst("c", 2, State::Planned, &image),
            inst("d", 3, State::Done, &image),
            inst("e", 3, State::Absent, &image),
        ],
        Vec::new(),
    );
    plan.cached.insert("c#1".into());
    assert_eq!(
        plan.estimate(),
        Estimate {
            low: usd(10_000),
            high: usd(80_000)
        }
    );
    let phases = plan.phases();
    assert_eq!(phases.iter().map(|p| p.phase).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(phases[0].steps, 0);
    assert_eq!(
        (phases[1].steps, phases[1].calls_low, phases[1].calls_high),
        (3, 2, 4)
    );
    assert_eq!((phases[1].low, phases[1].high), (usd(10_000), usd(80_000)));
    assert_eq!(plan.live().count(), 3);
    // Known: identity known and not absent (done included).
    assert_eq!(plan.known(), 4);
    plan.expansion.instances.get_mut("a#1").unwrap().identity = None;
    assert_eq!(plan.known(), 3);
}

fn hand_planner(takes: &[&str]) -> (Fixture, Planner) {
    let fixture = Fixture::new();
    let workflow = WorkflowDoc {
        id: "case".into(),
        title: "Case".into(),
        description: None,
        inputs: IndexMap::new(),
        tables: IndexMap::new(),
        let_: IndexMap::new(),
        budget: None,
        asserts: Vec::new(),
        steps: Rc::new(IndexMap::<String, Rc<Step>>::new()),
        outputs: IndexMap::new(),
        view: None,
    };
    let planner = Planner {
        cwd: fixture.root.clone(),
        project: fixture.project(),
        home: fixture.project(),
        workflow: Rc::new(LoadedWorkflow {
            document: json!({}),
            workflow: Rc::new(workflow),
            source: "workflows/case.yaml".into(),
            path: fixture.root.join("workflows/case.yaml"),
        }),
        input_schema: json!({}),
        inputs: RootInputs::default(),
        routes: RouteTable::default(),
        takes: takes
            .iter()
            .map(|step| {
                (
                    (*step).to_string(),
                    TakeChoice {
                        take: 1,
                        result: None,
                    },
                )
            })
            .collect(),
        takes_path: fixture.root.join("workflows/case.takes.yaml"),
        registry: fixture.registry(),
        results: IndexMap::new(),
        max_usd: None,
    };
    (fixture, planner)
}

#[test]
fn takes_entries_that_name_no_step_are_noted() {
    let (_fixture, planner) = hand_planner(&["zeta", "draw['a']", "alpha", "kept"]);
    let plan = plan_of(
        vec![
            inst("kept", 1, State::Planned, &[]),
            inst("draw['a']", 1, State::Absent, &[]),
        ],
        Vec::new(),
    );
    assert_eq!(
        plan.warnings(&planner),
        [
            "the takes file names alpha, which no step is any more; move it with grida-fx takes mv case \"alpha\" <new path>",
            "the takes file names zeta, which no step is any more; move it with grida-fx takes mv case \"zeta\" <new path>",
        ]
    );
}

#[test]
fn targets_parse() {
    assert_eq!(
        Target::parse("b.py:build"),
        Target::Builder {
            file: "b.py".into(),
            function: "build".into()
        }
    );
    assert_eq!(Target::parse("case"), Target::Workflow("case".into()));
}

#[test]
fn a_plan_with_no_problems_is_ok() {
    let mut plan = plan_of(Vec::new(), Vec::new());
    assert!(plan.ok());
    assert_eq!(plan.phases().len(), 1);
    assert_eq!(plan.estimate(), Estimate::default());
    plan.problems.push(Problem::new("a", "x"));
    assert!(!plan.ok());
}
