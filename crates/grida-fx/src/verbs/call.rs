//! The in-memory one-step image workflow (spec/call.md).

use super::planning::{amount, working_directory};
use super::run::{INTERRUPTED, StandInSource, start_engine};
use crate::cli::CallArgs;
use crate::print::{labelled, print_json, print_line, shown_path};
use grida_fx_core::Error;
use grida_fx_core::host::NodeHost;
use grida_fx_core::money::Usd;
use grida_fx_core::plan::{Estimate, output::forecast_document};
use grida_fx_core::project::{ImageCall, PlanRequest, Planner, make_image_call_planner};
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::folder::{new_folder, output_files, same_file};
use grida_fx_runtime::runner::{self, RunError, RunOptions};
use grida_fx_runtime::stderr;
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Runs one managed capability and returns its CLI status.
pub fn run(args: &CallArgs) -> Result<u8, Error> {
    let mut report = Report::default();
    let status = match execute(args, &mut report) {
        Ok(status) => status,
        Err(error) if args.json => {
            report.failure("error", None, &error.message, Value::Null);
            2
        }
        Err(error) => return Err(error),
    };
    if args.json {
        print_json(&report.document(args));
    } else {
        report.print();
    }
    Ok(status)
}

fn execute(args: &CallArgs, report: &mut Report) -> Result<u8, Error> {
    if args.prompt.trim().is_empty() {
        return Err(Error::usage("--prompt needs non-blank text"));
    }
    if args.fresh && !args.live && args.stand_in.is_none() {
        return Err(Error::usage("--fresh needs --live or --stand-in"));
    }
    let size = args.size.as_deref().map(Value::from);
    grida_fx_providers::wire::parse_size(size.as_ref())
        .map_err(|message| Error::usage(format!("--{message}")))?;
    let cwd = working_directory()?;
    let max_usd = args
        .max_usd
        .as_deref()
        .map(|s| amount("--max-usd", s))
        .transpose()?;
    let stand_in = args
        .stand_in
        .as_deref()
        .map(|source| StandInSource::parse(source, &cwd))
        .transpose()?;
    let delivery = args
        .out
        .as_deref()
        .map(|path| Delivery::prepare(&cwd, path))
        .transpose()?;
    let mut host = crate::print::planning_host("", &cwd);
    let request = PlanRequest {
        cwd,
        max_usd,
        builtin_routes: crate::engine::builtin_routes()?,
        ..PlanRequest::default()
    };
    let call = ImageCall {
        prompt: &args.prompt,
        route: args.route.as_deref(),
        background: args.background.as_deref(),
        size: args.size.as_deref(),
    };
    let mut planner = make_image_call_planner(&request, call, &mut host)?;
    if args.route.is_none()
        && !planner
            .project
            .document
            .routes
            .contains_key(&args.capability)
    {
        report.failure(
            "refused",
            Some("no_route"),
            "select a route with --route or set routes.image.generate in fx.yaml",
            Value::Null,
        );
        return Ok(1);
    }
    if args.fresh {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| Error::internal("cannot create a fresh call identity"))?;
        planner.fresh_id = Some(random.iter().map(|byte| format!("{byte:02x}")).collect());
    }
    let prepared =
        crate::models::prepare(&mut planner, &mut host, None, false, stand_in.is_none())?;
    let runtime = crate::engine::runtime()?;
    let engine = start_engine(&runtime, &planner, args.live, stand_in, prepared.settings)?;
    let ran = run_prepared(
        args,
        &engine,
        &mut planner,
        &mut host,
        prepared.facts,
        delivery,
        report,
    );
    crate::engine::shutdown(&runtime, &engine);
    ran
}

fn run_prepared(
    args: &CallArgs,
    engine: &Arc<Engine>,
    planner: &mut Planner,
    host: &mut dyn NodeHost,
    resolved_models: Value,
    delivery: Option<Delivery>,
    report: &mut Report,
) -> Result<u8, Error> {
    let plan = crate::engine::plan(engine, planner, host)?;
    report.route = plan
        .expansion
        .ordered()
        .find_map(|instance| instance.routes.get(&args.capability))
        .map(|route| route.id());
    report.forecast = Some(plan.estimate());
    report.ceiling = plan.ceiling;
    if let Some(message) = runner::refused(&plan, engine.live, &engine.adapters.plan_providers()) {
        report.failure("refused", None, &message, Value::Null);
        return Ok(1);
    }
    if args.dry_run {
        report.status = "planned";
        return Ok(0);
    }
    if args.live && report.ceiling.is_none() {
        report.ceiling = Some(Usd::ZERO);
    }
    let runs = planner.project.runs_dir();
    let id = &planner.workflow.workflow.id;
    let cwd = planner.cwd.clone();
    let folder = new_folder(&runs, id)
        .map_err(|error| Error::io(&shown_path(&runs.join(id), &cwd), &error))?;
    let label = shown_path(&folder, &cwd);
    let options = RunOptions {
        attempt: None,
        folder: folder.clone(),
        label: label.clone(),
        resolved_models: resolved_models["routes"]
            .as_array()
            .filter(|routes| !routes.is_empty())
            .map(|_| resolved_models.clone()),
        ..RunOptions::default()
    };
    let ran =
        runner::run_with_control_ready(Arc::clone(engine), planner, host, plan, options, |ready| {
            crate::interrupt::runner_started();
            report.run = Some(label.clone());
            report.invocation = Some(ready.invocation_id.clone());
            stderr::line(format_args!("run       {label}"));
            stderr::line(format_args!("invocation {}", ready.invocation_id));
            stderr::line(if ready.control_available {
                "control   available"
            } else {
                "control   unavailable"
            });
        });
    crate::interrupt::runner_ended();
    if ran.is_err() {
        let _ = fs::remove_dir(&folder);
    }
    let outcome = match ran {
        Ok(outcome) => outcome,
        Err(RunError::Refused(message)) => {
            report.failure("refused", None, &message, Value::Null);
            return Ok(1);
        }
        Err(RunError::Fatal(error)) => return Err(error),
    };
    report.charged = Some(outcome.charged);
    if outcome.cancelled {
        report.status = "cancelled";
        return Ok(INTERRUPTED);
    }
    let recorded = super::read_log(&folder, &label, true)?;
    report.read_execution(&recorded);
    if !outcome.ok {
        if report.error.is_null() {
            let reason = outcome
                .stopped
                .as_deref()
                .unwrap_or("the run did not complete");
            report.failure("failed", None, reason, Value::Null);
        }
        return Ok(1);
    }
    let Some((path, digest)) = outcome
        .outputs
        .get("image")
        .and_then(|image| output_files("image", image).into_iter().next())
    else {
        report.failure(
            "failed",
            Some("missing_output"),
            "the run did not produce an image",
            Value::Null,
        );
        return Ok(1);
    };
    let artifact = folder.join(path);
    let artifact_label = shown_path(&artifact, &cwd);
    report.artifact = Some(artifact_label.clone());
    if let Some(delivery) = delivery {
        match delivery.publish(&artifact, &digest) {
            Ok(output) => report.output = Some(output),
            Err(error) => {
                report.failure(
                    "failed",
                    Some("delivery_failed"),
                    &error.message,
                    Value::Null,
                );
                return Ok(1);
            }
        }
    } else {
        report.output = Some(artifact_label);
    }
    report.status = "ok";
    Ok(0)
}

/// A destination checked before the call, with no-clobber publication after success.
struct Delivery {
    requested: PathBuf,
    target: PathBuf,
    label: String,
}

impl Delivery {
    fn prepare(cwd: &Path, typed: &Path) -> Result<Self, Error> {
        let label = typed
            .to_str()
            .ok_or_else(|| Error::usage("--out needs a UTF-8 file path"))?
            .to_owned();
        if matches!(label.rsplit('/').next(), Some("" | "." | "..")) {
            return Err(Error::usage("--out needs a file path"));
        }
        let requested = cwd.join(typed);
        let name = requested
            .file_name()
            .ok_or_else(|| Error::usage("--out needs a file path"))?;
        let parent = fs::canonicalize(requested.parent().expect("absolute file path has a parent"))
            .map_err(|error| Error::io(&label, &error))?;
        let target = parent.join(name);
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(&label, &error)),
            Ok(_) => return Err(Error::usage(format!("{label}: output already exists"))),
        }
        crate::interrupt::with_delivery_staging(&parent, tempfile::NamedTempFile::close)
            .map_err(|error| Error::io(&label, &error))?;
        Ok(Self {
            requested,
            target,
            label,
        })
    }

    fn publish(self, artifact: &Path, digest: &str) -> Result<String, Error> {
        let io_error = |error: std::io::Error| Error::io(&self.label, &error);
        let bytes = fs::read(artifact).map_err(io_error)?;
        if grida_fx_core::value::file_digest(&bytes) != digest {
            return Err(Error::internal(
                "the recorded image does not match its digest",
            ));
        }
        let parent = self
            .target
            .parent()
            .expect("canonical output path has a parent");
        let requested_parent = self
            .requested
            .parent()
            .expect("absolute file path has a parent");
        crate::interrupt::with_delivery_staging(parent, |mut staged| {
            staged.write_all(&bytes)?;
            staged.as_file().sync_all()?;
            if fs::canonicalize(requested_parent)? != parent {
                return Err(io::Error::other("output parent changed during the run"));
            }
            let published = staged
                .persist_noclobber(&self.target)
                .map_err(|error| error.error)?;
            if !same_file(&published, &self.requested) {
                return Err(io::Error::other("output path changed during publication"));
            }
            Ok(())
        })
        .map_err(io_error)?;
        Ok(self.label)
    }
}

#[derive(Default)]
struct Report {
    status: &'static str,
    route: Option<String>,
    cache: Option<&'static str>,
    run: Option<String>,
    invocation: Option<String>,
    artifact: Option<String>,
    output: Option<String>,
    charged: Option<Usd>,
    ceiling: Option<Usd>,
    forecast: Option<Estimate>,
    error: Value,
}

impl Report {
    fn failure(
        &mut self,
        status: &'static str,
        code: Option<&str>,
        message: &str,
        diagnostic: Value,
    ) {
        self.status = status;
        self.error = json!({ "code": code, "message": message, "diagnostic": diagnostic });
    }

    fn read_execution(&mut self, events: &[Value]) {
        for event in events {
            match super::event_name(event) {
                Some("call") => {
                    self.cache = event["cached"]
                        .as_bool()
                        .map(|cached| if cached { "call" } else { "miss" })
                }
                Some("node_finished") if event["cache"] == "hit" => self.cache = Some("step"),
                Some("node_failed") => self.failure(
                    "failed",
                    super::text(event, "code"),
                    super::text(event, "error").unwrap_or("the image step failed"),
                    event["diagnostic"].clone(),
                ),
                _ => {}
            }
        }
    }

    fn document(&self, args: &CallArgs) -> Value {
        json!({
            "kind": "fx-call-result-v1",
            "status": self.status,
            "capability": args.capability,
            "route": self.route,
            "cache": self.cache,
            "run": self.run,
            "invocation_id": self.invocation,
            "artifact": self.artifact,
            "output": self.output,
            "charged_usd": self.charged.map(Usd::to_value),
            "ceiling_usd": self.ceiling.map(Usd::to_value),
            "forecast": self.forecast.map(|estimate| {
                let mut value = forecast_document(estimate.low, estimate.high, estimate.unpriced_calls);
                value["unpriced_calls"] = Value::from(estimate.unpriced_calls);
                value
            }),
            "stand_in": args.stand_in.is_some(),
            "error": self.error,
        })
    }

    fn print(&self) {
        if let Some(route) = &self.route {
            print_line(&labelled("route", route));
        }
        if let Some(cache) = self.cache {
            print_line(&labelled("cache", cache));
        }
        if let Some(estimate) = self.forecast.filter(|_| self.status == "planned") {
            let range = format!(
                "{} to {}",
                estimate.low.dollars_2(),
                estimate.high.dollars_2()
            );
            let forecast = if estimate.unpriced_calls == 0 {
                range
            } else {
                format!("{range} known, {} unpriced", estimate.unpriced_calls)
            };
            print_line(&labelled("forecast", &forecast));
            if let Some(ceiling) = self.ceiling {
                print_line(&labelled("ceiling", &ceiling.dollars_2()));
            }
        }
        if let Some(error) = self.error["message"].as_str() {
            print_line(&labelled(self.status, error));
        } else {
            print_line(&labelled("result", self.status));
        }
        if let Some(charged) = self.charged {
            print_line(&labelled("booked", &charged.dollars_2()));
        }
        if let Some(output) = &self.output {
            print_line(&labelled("output", output));
        } else if let Some(artifact) = &self.artifact {
            print_line(&labelled("artifact", artifact));
        }
    }
}
