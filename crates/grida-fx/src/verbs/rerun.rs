//! Allocate a fresh execution for a saved run after its plan has been verified.

use crate::print::print_line;
use grida_fx_core::{Error, project::Planner};
use grida_fx_runtime::{
    attempts::{self, Allocation, Attempt},
    folder::RunFolder,
    runner, stderr,
};
use std::path::{Path, PathBuf};

pub(super) struct Rerun<'a> {
    pub source: &'a Path,
    pub attempt: Attempt,
    pub request_digest: Option<String>,
    pub pinned: bool,
}

/// Serializes allocation with other reruns and removals until the start event is durable.
pub(super) struct RerunAllocation {
    pub folder: PathBuf,
    pub start: runner::RerunStart,
    pub name: Option<String>,
    _project_lock: std::fs::File,
    _source_lock: RunFolder,
}

pub(super) enum Launch {
    Duplicate(PathBuf),
    Fresh(RerunAllocation),
}

impl Rerun<'_> {
    pub(super) fn allocate(
        &self,
        planner: &Planner,
        plan: &grida_fx_core::plan::Plan,
        invocation: &str,
    ) -> Result<Launch, Error> {
        let root = &planner.project.root;
        let project_lock = grida_fx_runtime::run_index::allocate_attempt(root)
            .map_err(|error| Error::io("project run index", &error))?;
        let run_id = &self.attempt.run_id;
        if let Some(folder) = super::history::accepted(
            root,
            run_id,
            self.request_digest.as_deref(),
            self.pinned.then_some(self.attempt.attempt_number),
        )? {
            return Ok(Launch::Duplicate(folder));
        }
        let source_lock = RunFolder::lock_existing(self.source, "rerun source")
            .map_err(|error| Error::usage(error.0))?;
        let events = super::read_log(self.source, "rerun source", true)?;
        if super::history::recorded(self.source)?.as_ref() != Some(&self.attempt)
            || events
                .iter()
                .rfind(|event| event["event"] == "run_started")
                .and_then(|event| event["invocation_id"].as_str())
                != Some(invocation)
        {
            return Err(Error::usage(
                "the source changed while preparing to rerun; inspect it and try again",
            ));
        }
        let digest = grida_fx_core::plan::output::plan_digest(plan, planner)
            .ok_or_else(|| Error::usage("cannot verify the plan for the rerun source"))?;
        let recorded = super::read_plan(self.source, "rerun source")?;
        if recorded["plan"].as_str() != Some(&digest) {
            return Err(Error::usage(
                "the source workflow or inputs changed; the recorded plan no longer matches",
            ));
        }

        let legacy_created = std::fs::metadata(self.source.join("plan.json"))
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let (name, run_created_at) =
            grida_fx_runtime::events::run_metadata(&events, legacy_created);
        let mut allocations = super::history::journal(root, run_id)?;
        let pending = self.request_digest.as_ref().and_then(|key| {
            allocations
                .iter()
                .find(|entry| entry.request_digest.as_ref() == Some(key))
        });
        let (folder, attempt_number) = if let Some(pending) = pending {
            if pending.source_attempt != self.attempt.attempt_number {
                return Err(Error::usage(format!(
                    "retry this request with its original source, attempt {}",
                    pending.source_attempt
                )));
            }
            let folder = pending
                .existing_folder(root)
                .map_err(|error| Error::io("run history", &error))?
                .ok_or_else(|| Error::usage("the previously allocated attempt was removed"))?;
            (folder, pending.attempt_number)
        } else {
            let number = allocations
                .last()
                .map_or(self.attempt.attempt_number, |entry| {
                    entry.attempt_number.max(self.attempt.attempt_number)
                })
                .checked_add(1)
                .ok_or_else(|| Error::usage("the run has exhausted its attempt numbers"))?;
            let base = planner
                .project
                .runs_dir()
                .join(&planner.workflow.workflow.id);
            let folder = base.join(format!("attempt-{run_id}-{number}"));
            std::fs::create_dir_all(&base)
                .and_then(|_| match std::fs::create_dir(&folder) {
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && std::fs::symlink_metadata(&folder)
                                .is_ok_and(|meta| meta.is_dir())
                            && std::fs::read_dir(&folder)
                                .is_ok_and(|mut files| files.next().is_none()) =>
                    {
                        Ok(())
                    }
                    result => result,
                })
                .map_err(|error| Error::io("rerun attempt folder", &error))?;
            let allocation = Allocation::new(
                root,
                &folder,
                number,
                self.attempt.attempt_number,
                self.request_digest.clone(),
            )
            .map_err(|error| Error::io("run history", &error))?;
            let folder = allocation.path(root);
            allocations.push(allocation);
            attempts::save(root, run_id, &allocations)
                .map_err(|error| Error::io("run history", &error))?;
            (folder, number)
        };
        Ok(Launch::Fresh(RerunAllocation {
            folder,
            start: runner::RerunStart {
                identity: Attempt {
                    run_id: run_id.clone(),
                    attempt_number,
                },
                run_created_at,
            },
            name,
            _project_lock: project_lock,
            _source_lock: source_lock,
        }))
    }
}

pub(super) fn report_accepted(folder: &Path, root: &Path) -> Result<(), Error> {
    let identity = super::history::recorded(folder)?
        .ok_or_else(|| Error::usage("the previously accepted attempt is unavailable"))?;
    print_line(&format!("run       {}", folder.display()));
    print_line(&format!("run ID    {}", identity.run_id));
    print_line(&format!("attempt   {}", identity.attempt_number));
    print_line("result    already accepted");
    if let Some(root) = super::run::sdk_root(root)? {
        stderr::line(format_args!("resume_project	{root}"));
    }
    Ok(())
}
