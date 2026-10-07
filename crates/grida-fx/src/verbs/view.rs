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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::io("viewer runtime", &error))?;
    runtime.block_on(async {
        let bound = if let Some(graph) = graph {
            grida_fx_viewer::bind_plan(graph, args.port).await
        } else {
            let run = args.run.as_ref().ok_or_else(|| {
                Error::usage("view needs a workflow target, --run DIRECTORY, or --plan FILE")
            })?;
            grida_fx_viewer::bind(Path::new(run), args.port).await
        };
        let server = bound.map_err(|error| Error::io("viewer", &error))?;
        let url = server.url();
        println!("{url}");
        println!("Press Ctrl-C to stop the viewer.");
        io::stdout()
            .flush()
            .map_err(|error| Error::io("viewer output", &error))?;
        if !args.no_open {
            launch_browser(url.to_string());
        }
        server
            .serve()
            .await
            .map_err(|error| Error::io("viewer server", &error))?;
        Ok(0)
    })
}

/// Opening a browser is best-effort. Reap the launcher without holding up serving,
/// including when a headless Linux opener waits for an interactive browser.
fn launch_browser(url: String) {
    let launched = std::thread::Builder::new()
        .name("grida-fx-browser".into())
        .spawn(move || {
            if open_browser(&url).is_err() {
                eprintln!("grida-fx: could not open a browser; open the printed URL manually");
            }
        });
    if launched.is_err() {
        eprintln!("grida-fx: could not open a browser; open the printed URL manually");
    }
}

fn open_browser(url: &str) -> io::Result<()> {
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
    if command.status()?.success() {
        Ok(())
    } else {
        Err(io::Error::other("browser launcher failed"))
    }
}
