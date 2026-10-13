//! What the core needs from outside: node hosts, the plan-time runner, and the result cache.
//!
//! - [`Language`]: the language of a project module, chosen by its file suffix (protocol.md §1):
//!   `.py` is Python; `.ts`, `.mts`, `.js` and `.mjs` are TypeScript. Builders and stand-in files
//!   choose their host the same way.
//! - [`NodeHost`]: `describe` and `build` over the node protocol (protocol.md §5.1, §5.2). The
//!   runtime's hosts implement it, one per language behind one that dispatches by suffix (each
//!   starts its process lazily, on the first call for a module of its language, so a project
//!   without node modules needs neither Python nor Node). Tests use [`FakeHost`].
//! - [`PlanTimeRunner`]: runs an `at: plan` instance's body while planning. The runtime's
//!   runner implements it; without one, a plan that needs it gets the problem
//!   `<step>: running an at: plan step needs a plan-time runner, and planning has none`.
//! - [`ResultCache`]: whether the store holds a trusted result for an instance (plan `cached`,
//!   store.md §4: the identity and the instance's read set). The runtime's store implements it;
//!   [`NoCache`] holds nothing.

use crate::error::Error;
use crate::expand::{Instance, NodeResult};
use crate::project::Planner;
use grida_fx_protocol::{
    BuildParams, BuildResult, DescribeParams, DescribeResult, ModuleDescription, RpcError,
};
use indexmap::IndexMap;
use std::path::Path;

/// The language of a project module, and so of the node host that runs it (protocol.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    Python,
    TypeScript,
}

impl Language {
    /// Every language, in the order hosts are asked.
    pub const ALL: [Language; 2] = [Language::Python, Language::TypeScript];

    /// The language of the module at `path` (POSIX or native, relative or absolute), by its
    /// suffix: `.py` is Python; `.ts`, `.mts`, `.js` and `.mjs` are TypeScript, all ES modules.
    /// `Err` is the refusal that follows `<uses>: ` (or `<file>:<function>: `):
    /// `no node host for .tsx modules` for `.tsx`, `.cts` and `.cjs`, and for a TypeScript
    /// declaration file (`.d.ts`, `.d.mts`), which is never a node module;
    /// `no node host for .rb modules yet` for any other suffix, and `no node host for modules
    /// without a suffix yet`. Suffixes are case sensitive (`X.TS` has none of these).
    pub fn of_module(path: &str) -> Result<Language, String> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        if let Some(suffix) = declaration_suffix(name) {
            return Err(format!("no node host for .{suffix} modules"));
        }
        match Path::new(name).extension().and_then(|s| s.to_str()) {
            Some("py") => Ok(Language::Python),
            Some("ts" | "mts" | "js" | "mjs") => Ok(Language::TypeScript),
            Some(suffix @ ("tsx" | "cts" | "cjs")) => {
                Err(format!("no node host for .{suffix} modules"))
            }
            Some(suffix) => Err(format!("no node host for .{suffix} modules yet")),
            None => Err("no node host for modules without a suffix yet".into()),
        }
    }

    /// `initialize`'s `host.language`: `python` or `typescript`.
    pub fn name(self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::TypeScript => "typescript",
        }
    }

    /// The language's name in sentences: `Python`, `TypeScript` (`the Python node host`).
    pub fn title(self) -> &'static str {
        match self {
            Language::Python => "Python",
            Language::TypeScript => "TypeScript",
        }
    }

    /// The SDK that holds the language's host, as sentences name it (`upgrade grida`, `upgrade
    /// @grida/fx`).
    pub fn sdk(self) -> &'static str {
        match self {
            Language::Python => "grida",
            Language::TypeScript => "@grida/fx",
        }
    }

    /// Whether the language's host can name the declared source package `name` (fx.yaml
    /// `sources`; protocol.md §2): Python takes a package name (`^[A-Za-z_][A-Za-z0-9_]*$`),
    /// TypeScript an npm package name (`^(?:@[a-z0-9][a-z0-9._~-]*/)?[a-z0-9][a-z0-9._~-]*$`).
    /// The engine sends each host only the names its language can name (`acme_lib` goes to
    /// both).
    pub fn accepts_source(self, name: &str) -> bool {
        match self {
            Language::Python => {
                let mut bytes = name.bytes();
                bytes
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                    && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
            }
            Language::TypeScript => {
                // One npm name segment: a lowercase letter or digit, then lowercase letters,
                // digits and `._~-`.
                let segment = |text: &str| {
                    let mut bytes = text.bytes();
                    bytes
                        .next()
                        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                        && bytes.all(|b| {
                            b.is_ascii_lowercase()
                                || b.is_ascii_digit()
                                || matches!(b, b'.' | b'_' | b'~' | b'-')
                        })
                };
                match name.strip_prefix('@') {
                    Some(scoped) => scoped
                        .split_once('/')
                        .is_some_and(|(scope, rest)| segment(scope) && segment(rest)),
                    None => segment(name),
                }
            }
        }
    }

    /// The refusal of an attribute a module does not declare as a node type, after `<uses>: `
    /// or `<path>: `: `x is not declared with @node` (Python), `x is not an export declared
    /// with node()` (TypeScript).
    pub fn not_a_node(self, attribute: &str) -> String {
        match self {
            Language::Python => format!("{attribute} is not declared with @node"),
            Language::TypeScript => format!("{attribute} is not an export declared with node()"),
        }
    }
}

/// The suffix of a TypeScript declaration file name (`d.ts`, `d.mts`), with a name before it.
fn declaration_suffix(name: &str) -> Option<&'static str> {
    ["d.ts", "d.mts"].into_iter().find(|suffix| {
        name.strip_suffix(suffix)
            .and_then(|stem| stem.strip_suffix('.'))
            .is_some_and(|stem| !stem.is_empty())
    })
}

/// Why a host call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum HostFailure {
    /// The host could not be started or broke the protocol: the command stops (exit 2). The
    /// message says what to do (`set GRIDA_FX_PYTHON to a Python that has the grida package`,
    /// `set GRIDA_FX_NODE to Node 24 or later`).
    Unavailable(String),
    /// The host answered the request with an error (`load_failed`, `build_failed`, …).
    Rpc(RpcError),
}

impl From<HostFailure> for Error {
    fn from(failure: HostFailure) -> Self {
        match failure {
            HostFailure::Unavailable(message) => Error::host(message),
            HostFailure::Rpc(error) => Error::host(error.message),
        }
    }
}

/// A node host, as the core sees it.
pub trait NodeHost {
    /// Points the host at a project: `root` (absolute) is its working directory and
    /// `initialize`'s `project_root`, `sources` its declared source packages. `make_planner` and
    /// `lock` call it before any `describe` or `build`; a host already started for another root
    /// is shut down and starts again on its next call.
    fn open_project(&mut self, root: &Path, sources: &[String]) -> Result<(), HostFailure>;
    /// `describe` (protocol.md §5.1).
    fn describe(&mut self, params: &DescribeParams) -> Result<DescribeResult, HostFailure>;
    /// `build` (protocol.md §5.2).
    fn build(&mut self, params: &BuildParams) -> Result<BuildResult, HostFailure>;
}

/// A host for callers that have none: every call is `Unavailable`.
#[derive(Debug, Default)]
pub struct NoHost;

impl NodeHost for NoHost {
    fn open_project(&mut self, _: &Path, _: &[String]) -> Result<(), HostFailure> {
        Ok(())
    }

    fn describe(&mut self, _: &DescribeParams) -> Result<DescribeResult, HostFailure> {
        Err(HostFailure::Unavailable("no node host is available".into()))
    }

    fn build(&mut self, _: &BuildParams) -> Result<BuildResult, HostFailure> {
        Err(HostFailure::Unavailable("no node host is available".into()))
    }
}

/// A scripted host for tests: modules by path, builds by `path:function`. `describe` answers each
/// target from `modules` (filtered to the attribute when one is given; an unknown module is a
/// `Failed` entry `<path> failed to import: ModuleNotFoundError`) and counts calls.
#[derive(Debug, Default, Clone)]
pub struct FakeHost {
    pub modules: IndexMap<String, ModuleDescription>,
    pub builds: IndexMap<String, Result<BuildResult, RpcError>>,
    pub describe_calls: usize,
    pub build_calls: usize,
}

impl FakeHost {
    pub fn new() -> FakeHost {
        FakeHost::default()
    }

    /// Adds a module description.
    pub fn with_module(mut self, description: ModuleDescription) -> FakeHost {
        let path = match &description {
            ModuleDescription::Described { path, .. } | ModuleDescription::Failed { path, .. } => {
                path.clone()
            }
        };
        self.modules.insert(path, description);
        self
    }
}

impl NodeHost for FakeHost {
    fn open_project(&mut self, _: &Path, _: &[String]) -> Result<(), HostFailure> {
        Ok(())
    }

    fn describe(&mut self, params: &DescribeParams) -> Result<DescribeResult, HostFailure> {
        self.describe_calls += 1;
        let modules = params
            .targets
            .iter()
            .map(|target| match self.modules.get(&target.path) {
                Some(ModuleDescription::Described {
                    path,
                    types,
                    closure,
                }) => {
                    let types = types
                        .iter()
                        .filter(|t| target.attribute.as_ref().is_none_or(|a| &t.attribute == a))
                        .cloned()
                        .collect::<Vec<_>>();
                    match &target.attribute {
                        Some(attribute) if types.is_empty() => ModuleDescription::Failed {
                            path: path.clone(),
                            attribute: Some(attribute.clone()),
                            error: format!("{path}: {attribute} is not a node type"),
                        },
                        _ => ModuleDescription::Described {
                            path: path.clone(),
                            types,
                            closure: closure.clone(),
                        },
                    }
                }
                Some(failed) => failed.clone(),
                None => ModuleDescription::Failed {
                    path: target.path.clone(),
                    attribute: target.attribute.clone(),
                    error: format!(
                        "{} failed to import: ModuleNotFoundError: No module named '{}'",
                        target.path, target.path
                    ),
                },
            })
            .collect();
        Ok(DescribeResult {
            modules,
            builtins: Vec::new(),
        })
    }

    fn build(&mut self, params: &BuildParams) -> Result<BuildResult, HostFailure> {
        self.build_calls += 1;
        match self
            .builds
            .get(&format!("{}:{}", params.path, params.function))
        {
            Some(Ok(result)) => Ok(result.clone()),
            Some(Err(error)) => Err(HostFailure::Rpc(error.clone())),
            None => Err(HostFailure::Rpc(RpcError::new(
                grida_fx_protocol::ErrorCode::LoadFailed,
                format!("{} has no function {}", params.path, params.function),
            ))),
        }
    }
}

/// Runs an `at: plan` instance while planning, so its results can feed the rest of the plan. A body that fails is
/// an `Ok` result with status failed; `Err` is reserved for an environment that cannot run it.
pub trait PlanTimeRunner {
    fn run(&mut self, instance: &Instance, planner: &Planner) -> Result<NodeResult, Error>;
}

/// The store's result records, as planning sees them.
pub trait ResultCache {
    /// Whether a trusted result record exists for the instance: its identity (known) and its read
    /// set (spec/store.md §4).
    fn has_result(&self, instance: &Instance) -> bool;
}

/// No store: nothing is cached.
#[derive(Debug, Default)]
pub struct NoCache;

impl ResultCache for NoCache {
    fn has_result(&self, _: &Instance) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modules_language_is_its_suffix() {
        for path in ["nodes/a.py", "./a.py", "a.b.py", "/w/acme/nodes/x.py"] {
            assert_eq!(Language::of_module(path), Ok(Language::Python), "{path}");
        }
        for path in [
            "nodes/a.ts",
            "a.mts",
            "lib/a.js",
            "a.mjs",
            "a.b.ts",
            "nodes\\a.ts",
        ] {
            assert_eq!(
                Language::of_module(path),
                Ok(Language::TypeScript),
                "{path}"
            );
        }
        // Refused for good: no "yet".
        for (path, suffix) in [
            ("nodes/a.tsx", "tsx"),
            ("a.cts", "cts"),
            ("a.cjs", "cjs"),
            ("nodes/types.d.ts", "d.ts"),
            ("types.d.mts", "d.mts"),
        ] {
            assert_eq!(
                Language::of_module(path),
                Err(format!("no node host for .{suffix} modules")),
                "{path}"
            );
        }
        // Any other suffix may get a host one day.
        assert_eq!(
            Language::of_module("nodes/a.rb"),
            Err("no node host for .rb modules yet".into())
        );
        assert_eq!(
            Language::of_module("nodes/X.TS"),
            Err("no node host for .TS modules yet".into())
        );
        assert_eq!(
            Language::of_module("nodes/X.PY"),
            Err("no node host for .PY modules yet".into())
        );
        for path in ["nodes/a", "nodes/.py", "Makefile"] {
            assert_eq!(
                Language::of_module(path),
                Err("no node host for modules without a suffix yet".into()),
                "{path}"
            );
        }
    }

    #[test]
    fn declaration_files_are_never_modules() {
        for (path, suffix) in [
            ("types.d.ts", "d.ts"),
            ("nodes/x.d.mts", "d.mts"),
            ("a.b.d.ts", "d.ts"),
        ] {
            assert_eq!(
                Language::of_module(path),
                Err(format!("no node host for .{suffix} modules")),
                "{path}"
            );
        }
        for path in ["d.ts", "nodes/d.ts", "x.ts", "x.d.js"] {
            assert_eq!(
                Language::of_module(path),
                Ok(Language::TypeScript),
                "{path}"
            );
        }
    }

    #[test]
    fn names_titles_and_sdks() {
        assert_eq!(Language::Python.name(), "python");
        assert_eq!(Language::TypeScript.name(), "typescript");
        assert_eq!(Language::Python.title(), "Python");
        assert_eq!(Language::TypeScript.title(), "TypeScript");
        assert_eq!(Language::Python.sdk(), "grida");
        assert_eq!(Language::TypeScript.sdk(), "@grida/fx");
        assert_eq!(
            Language::Python.not_a_node("shout"),
            "shout is not declared with @node"
        );
        assert_eq!(
            Language::TypeScript.not_a_node("shout"),
            "shout is not an export declared with node()"
        );
    }

    #[test]
    fn each_language_names_its_own_source_packages() {
        let python = |name| Language::Python.accepts_source(name);
        let typescript = |name| Language::TypeScript.accepts_source(name);
        // Both.
        for name in ["acme_lib", "acme", "lib2"] {
            assert!(python(name) && typescript(name), "{name}");
        }
        // Python only.
        for name in ["AcmeLib", "_private", "Acme"] {
            assert!(python(name) && !typescript(name), "{name}");
        }
        // npm only.
        for name in [
            "@acme/shapes",
            "acme-shapes",
            "acme.shapes",
            "2d",
            "@acme/x~y",
            "a_b-c",
        ] {
            assert!(!python(name) && typescript(name), "{name}");
        }
        // Neither.
        for name in [
            "",
            "@acme",
            "@acme/",
            "@/x",
            "@Acme/x",
            "acme/x",
            "@acme/x/y",
            "-acme",
            ".acme",
            "acme lib",
            "ácme",
        ] {
            assert!(!python(name) && !typescript(name), "{name}");
        }
    }
}
