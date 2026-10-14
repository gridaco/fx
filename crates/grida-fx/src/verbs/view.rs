//! A foreground viewer for a static workflow plan or one recorded run. Source targets
//! use normal offline planning before serving; saved files never load author code.
//! The command's signal handler stops this process; it owns no workflow run or daemon.

use crate::cli::{PlanArgs, ViewArgs};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::providers::Declarations;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

pub fn run(args: &ViewArgs) -> Result<u8, Error> {
    if args.target.is_none() && !args.rest.is_empty() {
        return Err(Error::usage(
            "workflow input flags require a workflow target; --run and --plan only read recorded data",
        ));
    }
    // Finish planning, including the planning runtime and node host lifetimes, before
    // entering the viewer's runtime. No refresh can import or replan the source target.
    let (graph, declarations) = if let Some(target) = &args.target {
        let (graph, project) = super::planning::graph(&PlanArgs {
            target: target.clone(),
            inputs: args.inputs.clone(),
            routes: args.routes.clone(),
            arg: args.arg.clone(),
            max_usd: args.max_usd.clone(),
            rest: args.rest.clone(),
            ..PlanArgs::default()
        })?;
        (Some(graph), project.declarations())
    } else if let Some(path) = &args.plan {
        let json =
            std::fs::read_to_string(path).map_err(|error| Error::io("saved plan", &error))?;
        let graph = grida_fx_core::value::parse_json(&json)
            .map_err(|_| Error::usage("saved plan is not valid JSON"))?;
        (Some(graph), declarations_here())
    } else {
        (None, declarations_here())
    };
    serve(
        graph,
        args.run.as_deref().map(Path::new),
        args.port,
        args.open,
        &declarations,
    )
}

/// The provider declarations of the project of the current directory, for `--run` and
/// `--plan`, which read no project (spec/providers.md §1): FX's own alone when that project's
/// `fx.yaml` cannot be read.
fn declarations_here() -> Declarations {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| Project::find(&cwd).ok())
        .map_or_else(Declarations::built_in, |project| project.declarations())
}

/// Independent inspection used by explicit standalone commands and legacy `view`. The browser
/// opener withholds `declarations`' variables.
pub(crate) fn serve(
    graph: Option<serde_json::Value>,
    run: Option<&Path>,
    port: u16,
    open: bool,
    declarations: &Declarations,
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
            launch_browser(url.to_string(), declarations);
        }
        server
            .serve()
            .await
            .map_err(|error| Error::io("viewer server", &error))?;
        Ok(0)
    })
}

/// Viewing is optional. A missing service or registration failure preserves the
/// execution/inspection result and never produces a URL that was not verified. The browser
/// opener withholds `declarations`' variables.
pub(crate) fn report_service(
    result: Result<Option<String>, Error>,
    requested: bool,
    declarations: &Declarations,
) {
    match result {
        Ok(Some(url)) => {
            crate::print::print_line(&crate::print::labelled("view", &url));
            if requested {
                launch_browser(url, declarations);
            }
        }
        Ok(None) if requested => {
            let command = crate::invoker::command();
            crate::print::print_error(&Error::usage(format!(
                "FX service is not running. Start this project with `{command} start --background`, then use `{command} inspect RUN --open` or `{command} plan TARGET --open`."
            )))
        }
        Ok(None) => {}
        Err(error) => crate::print::print_error(&Error::usage(format!(
            "FX inspection unavailable; the command continues: {}",
            error.message
        ))),
    }
}

/// Opening a browser is best-effort. Reap the launcher without holding up serving,
/// including when a headless Linux opener waits for an interactive browser. The opener never
/// inherits a variable of `declarations`, those of the project the command works in
/// (spec/providers.md §1).
pub(crate) fn launch_browser(url: String, declarations: &Declarations) {
    let mut child = match browser_command(&url, declarations).spawn() {
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

fn browser_command(url: &str, declarations: &Declarations) -> Command {
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
    for name in declarations.variables() {
        command.env_remove(name);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_opener_inherits_no_declared_variable() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("fx.yaml"),
            "fx: project/v1\nproviders:\n  acme:\n    credential: { variable: ACME_API_KEY }\n    \
             base_url: { variable: ACME_BASE_URL, default: \"https://api.acme.test/v1\" }\n",
        )
        .unwrap();
        let project = Project::find(root.path()).unwrap();
        let command = browser_command("http://127.0.0.1:1/", &project.declarations());
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in Declarations::built_in()
            .variables()
            .into_iter()
            .chain(["ACME_API_KEY".into(), "ACME_BASE_URL".into()])
        {
            assert!(removed.contains(&name), "{name}: {removed:?}");
        }
    }
}
