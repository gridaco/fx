//! Inspect or answer one run's durable requests without starting a workflow.

use crate::cli::{InspectArgs, RespondArgs};
use crate::print::{print_json, print_line};
use grida_fx_core::{Error, value::parse_json};
use grida_fx_runtime::waiting::{self, Request, ResponseChannel};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};

fn exact_run(run: &str) -> Result<PathBuf, Error> {
    let cwd = super::planning::working_directory()?;
    let folder = super::control::exact_folder(run, &cwd, None)
        .map_err(|(_, message)| Error::usage(message))?;
    super::read_plan(&folder, run)?;
    Ok(folder)
}

pub(super) fn requests(folder: &Path) -> Result<Vec<Request>, Error> {
    waiting::list(folder).map_err(|error| Error::input(error.to_string()))
}

pub fn inspect(args: &InspectArgs) -> Result<u8, Error> {
    let folder = exact_run(&args.run)?;
    let plan_path = folder.join("plan.json");
    let plan_file = std::fs::File::open(&plan_path).map_err(|e| Error::io("recorded plan", &e))?;
    let requests = requests(&folder)?;
    let viewer_url = if args.register {
        let project =
            grida_fx_core::docs::project::Project::find(&super::planning::working_directory()?)?;
        super::service::register_run(&project, &folder, false)?
    } else if args.viewer_url {
        super::service::existing_run_url(&folder)
    } else {
        None
    };
    if !grida_fx_runtime::folder::same_file(&plan_file, &plan_path) {
        return Err(Error::input(
            "The run was replaced during request inspection.",
        ));
    }
    if args.json {
        let mut result = json!({"requests": requests});
        if args.viewer_url || args.register {
            result["viewer_url"] = json!(viewer_url);
        }
        print_json(&result);
    } else if requests.is_empty() {
        print_line("no answer requests");
    } else {
        for request in requests {
            let line = format!(
                "{}  {}  {}",
                request.id,
                request.state.status(),
                request.spec.step
            );
            if let Some(message) = &request.spec.message {
                print_line(&format!("{line}  {message}"));
            } else {
                print_line(&line);
            }
        }
    }
    if !args.json
        && let Some(url) = viewer_url
    {
        print_line(&url);
    }
    Ok(0)
}

fn answer_json(input: &str) -> Result<Value, Error> {
    const LIMIT: usize = 1 << 20;
    let mut buffer = String::new();
    let input = if input == "-" {
        std::io::stdin()
            .take(LIMIT as u64 + 1)
            .read_to_string(&mut buffer)
            .map_err(|e| Error::io("answer on standard input", &e))?;
        &buffer
    } else {
        input
    };
    if input.len() > LIMIT {
        return Err(Error::input("the answer exceeds 1 MiB"));
    }
    parse_json(input).map_err(|_| Error::input("the answer must be valid JSON"))
}

pub fn respond(args: &RespondArgs) -> Result<u8, Error> {
    let channel = if args.source == "sdk" {
        ResponseChannel::Sdk
    } else {
        ResponseChannel::Cli
    };
    let (folder, answer) =
        match exact_run(&args.run).and_then(|folder| Ok((folder, answer_json(&args.json)?))) {
            Ok(inputs) => inputs,
            Err(error) => {
                print_json(&json!({"code": "invalid_input", "message": error.message}));
                return Ok(2);
            }
        };
    match waiting::respond(&folder, &args.request, answer, channel) {
        Ok(request) => {
            print_json(&json!({"request": request}));
            Ok(0)
        }
        Err(error) => {
            let mut receipt = json!({"code": error.code(), "message": error.to_string()});
            if let Some(request) = error.request() {
                receipt["request"] = json!(request);
            }
            print_json(&receipt);
            Ok(match error.code() {
                "answer_conflict" | "expired" | "not_found" | "invalid_answer" => 1,
                _ => 2,
            })
        }
    }
}
