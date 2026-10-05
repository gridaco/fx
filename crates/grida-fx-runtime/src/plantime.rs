//! Running `at: plan` steps while planning (`grida_fx_core::host::PlanTimeRunner`).
//!
//! `make_plan` calls [`PlanTime::run`] on the command's own thread for each ready `at: plan`
//! instance. It builds the instance's job (`executor::InstanceJob::from_instance`) and makes
//! **one** attempt with `executor::execute` on the engine's runtime (`handle.block_on`, legal
//! because the command's thread is not a runtime thread), with `Services::planning`: not live, no
//! ledger, no events, so nothing is billed and nothing is logged. Its result record is written to
//! the store as for any run (a later plan answers it from the result cache). A failed attempt is an
//! `Ok` result with status failed (planning reports `failed while planning: <error>`); an attempt
//! that must stop (`Attempt::stop`) is an `Err` (exit 2). At-plan steps are never retried.
//!
//! An instance whose job cannot be built (a with-value still pending) is a failed result too, with
//! the sentence `InstanceJob::from_instance` gives. Tools resolve over the process environment.
//! The attempt runs under a token nobody cancels: planning has no Ctrl-C handling of its own, and
//! a person who stops planning ends the process.
//!
//! [`PlanTime::run`] must not be called from a thread of the engine's runtime (`block_on` would
//! panic there); the CLI plans on its own command thread.

use crate::engine::{Cancel, Engine, Services};
use crate::executor::{InstanceJob, execute, failed};
use grida_fx_core::Error;
use grida_fx_core::expand::{Instance, NodeResult};
use grida_fx_core::host::PlanTimeRunner;
use grida_fx_core::project::Planner;
use std::sync::Arc;

/// The plan-time runner of one command.
pub struct PlanTime {
    pub services: Arc<Services>,
}

impl PlanTime {
    pub fn new(engine: Arc<Engine>) -> PlanTime {
        PlanTime {
            services: Arc::new(Services::planning(engine)),
        }
    }
}

impl PlanTimeRunner for PlanTime {
    fn run(&mut self, instance: &Instance, planner: &Planner) -> Result<NodeResult, Error> {
        let env = |name: &str| std::env::var_os(name).and_then(|v| v.into_string().ok());
        let job = match InstanceJob::from_instance(instance, planner, &env) {
            Ok(job) => job,
            Err(message) => return Ok(failed(&message)),
        };
        let services = Arc::clone(&self.services);
        let handle = services.engine.handle.clone();
        let attempt = handle.block_on(execute(services, Arc::new(job), Cancel::new()));
        match attempt.stop {
            Some(reason) => Err(Error::internal(reason)),
            None => Ok(attempt.result),
        }
    }
}
