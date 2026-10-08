//! A foreground viewer for a static workflow plan or one recorded run. Source targets
//! use normal offline planning before serving; saved files never load author code.
//! The command's signal handler stops this process; it owns no workflow run or daemon.

use crate::cli::{PlanArgs, ViewArgs};
use grida_fx_core::Error;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

pub fn run(args: &ViewArgs) -> Result<u8, Error> {
    if args.target.is_none() && !args.rest.is_empty() {
        return Err(Error::usage(
            "workflow input flags require a workflow target; --run and --plan only read recorded data",
        ));
    }
    // Finish planning, including the planning runtime and Python host lifetime, before
    // entering the viewer's runtime. No refresh can import or replan the source target.
    let graph = if let Some(target) = &args.target {
        Some(super::planning::graph(&PlanArgs {
            target: target.clone(),
            inputs: args.inputs.clone(),
            routes: args.routes.clone(),
            arg: args.arg.clone(),
            max_usd: args.max_usd.clone(),
            rest: args.rest.clone(),
            ..PlanArgs::default()
        })?)
    } else if let Some(path) = &args.plan {
        let json =
            std::fs::read_to_string(path).map_err(|error| Error::io("saved plan", &error))?;
        Some(
            grida_fx_core::value::parse_json(&json)
                .map_err(|_| Error::usage("saved plan is not valid JSON"))?,
        )
    } else {
        None
    };
    serve(
        graph,
        args.run.as_deref().map(Path::new),
        args.port,
        args.open,
    )
}

/// Independent inspection used by explicit standalone commands and legacy `view`.
pub(crate) fn serve(
    graph: Option<serde_json::Value>,
    run: Option<&Path>,
    port: u16,
    open: bool,
) -> Result<u8, Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::io("viewer runtime", &error))?;
    runtime.block_on(async {
        let bound = if let Some(graph) = graph {
            grida_fx_viewer::bind_plan(graph, port).await
        } else {
            let run = run.ok_or_else(|| {
                Error::usage("view needs a workflow target, --run DIRECTORY, or --plan FILE")
            })?;
            grida_fx_viewer::bind(run, port).await
        };
        let server = bound.map_err(|error| Error::io("viewer", &error))?;
        let url = server.url();
        println!("{url}");
        println!("Press Ctrl-C to stop the viewer.");
        io::stdout()
            .flush()
            .map_err(|error| Error::io("viewer output", &error))?;
        if open {
            launch_browser(url.to_string());
        }
        server
            .serve()
            .await
            .map_err(|error| Error::io("viewer server", &error))?;
        Ok(0)
    })
}

/// Viewing is optional. A missing service or registration failure preserves the
/// execution/inspection result and never produces a URL that was not verified.
pub(crate) fn report_service(result: Result<Option<String>, Error>, requested: bool) {
    match result {
        Ok(Some(url)) => {
            crate::print::print_line(&crate::print::labelled("view", &url));
            if requested {
                launch_browser(url);
            }
        }
        Ok(None) if requested => crate::print::print_error(&Error::usage(
            "FX service is not running. Start this project with `grida-fx start --background`, then use `grida-fx inspect RUN --open` or `grida-fx plan TARGET --open`.",
        )),
        Ok(None) => {}
        Err(error) => crate::print::print_error(&Error::usage(format!(
            "FX inspection unavailable; the command continues: {}",
            error.message
        ))),
    }
}

/// Opening a browser is best-effort. Reap the launcher without holding up serving,
/// including when a headless Linux opener waits for an interactive browser.
pub(crate) fn launch_browser(url: String) {
    let mut child = match browser_command(&url).spawn() {
        Ok(child) => child,
        Err(_) => {
            crate::print::print_error(&Error::usage(
                "could not open a browser; open the printed URL manually",
            ));
            return;
        }
    };
    // Spawn before returning: short-lived plan/inspect/start commands may exit
    // immediately after this call, before a background thread would get to run.
    let launched = std::thread::Builder::new()
        .name("grida-fx-browser".into())
        .spawn(move || {
            if !child.wait().is_ok_and(|status| status.success()) {
                crate::print::print_error(&Error::usage(
                    "could not open a browser; open the printed URL manually",
                ));
            }
        });
    if launched.is_err() {
        crate::print::print_error(&Error::usage(
            "could not open a browser; open the printed URL manually",
        ));
    }
}

fn browser_command(url: &str) -> Command {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for name in [
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
        "OPENAI_BASE_URL",
        "OPENROUTER_BASE_URL",
        "FAL_BASE_URL",
        "ELEVENLABS_BASE_URL",
    ] {
        command.env_remove(name);
    }
    command
}
