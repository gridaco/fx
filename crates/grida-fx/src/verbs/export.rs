//! Offline export of one exact saved run. Only publishing display defaults come from the
//! current project's configuration; no workflow source, provider or host is started.
use crate::cli::ExportArgs;
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_viewer::export::{ExportError, ExportOptions, run_export_with_viewer};

pub fn run(args: &ExportArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let viewer = Project::find(&cwd).map(|project| project.document.viewer);
    let result = viewer.map_err(|_| ExportError {
        kind: "fx-run-export-error-v1",
        code: "invalid_viewer_config",
        message: "The publishing project's fx.yaml cannot be read or is invalid; check it with doctor.",
    }).and_then(|viewer| run_export_with_viewer(
        &cwd.join(&args.run),
        &cwd.join(&args.out),
        &args.out,
        ExportOptions {
            include_values: args.include_values,
            include_diagnostics: args.include_diagnostics,
        },
        viewer,
    ));
    match result {
        Ok(result) => {
            if args.json {
                crate::print::print_json(
                    &serde_json::to_value(&result).expect("serializable export result"),
                );
            } else {
                println!(
                    "Exported {} files ({} bytes) to {}",
                    result.files, result.bytes, result.output
                );
                println!("Manifest: {}", result.manifest_sha256);
                if !result.omitted_artifacts.is_empty() {
                    println!(
                        "Omitted {} recorded artifacts; see manifest.json",
                        result.omitted_artifacts.len()
                    );
                }
            }
            Ok(0)
        }
        Err(error) => {
            if args.json {
                crate::print::print_json(
                    &serde_json::to_value(&error).expect("serializable export refusal"),
                );
            } else {
                grida_fx_runtime::stderr::line(format_args!(
                    "{}: {}: {}",
                    crate::invoker::command(),
                    error.code,
                    error.message
                ));
            }
            Ok(2)
        }
    }
}
