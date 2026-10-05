//! What the core needs from outside: node hosts, the plan-time runner, and the result cache.
//!
//! - [`NodeHost`]: `describe` and `build` over the node protocol (protocol.md §5.1, §5.2). The
//!   runtime's Python host implements it (it starts its process lazily, on the first call, so a
//!   project without node modules never needs Python). Tests use [`FakeHost`].
//! - [`PlanTimeRunner`]: runs an `at: plan` instance's body while planning. The step-3 runner implements it; in step 2 none exists and a plan that needs one gets
//!   the problem `<step>: running an at: plan step is not available until the runner lands`.
//! - [`ResultCache`]: whether the store holds a trusted result for a step identity (plan
//!   `cached`, store.md §4). Step 2 has no store: [`NoCache`].

use crate::error::Error;
use crate::expand::{Instance, NodeResult};
use crate::project::Planner;
use grida_fx_protocol::{
    BuildParams, BuildResult, DescribeParams, DescribeResult, ModuleDescription, RpcError,
};
use indexmap::IndexMap;
use std::path::Path;

/// Why a host call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum HostFailure {
    /// The host could not be started or broke the protocol: the command stops (exit 2). The
    /// message says what to do (`set GRIDA_FX_PYTHON to a Python that has the grida package`).
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
    /// Whether a trusted result record exists for the step identity.
    fn has_result(&self, identity: &str) -> bool;
}

/// No store: nothing is cached.
#[derive(Debug, Default)]
pub struct NoCache;

impl ResultCache for NoCache {
    fn has_result(&self, _: &str) -> bool {
        false
    }
}
