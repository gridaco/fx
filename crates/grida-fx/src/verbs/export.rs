//! Offline export of one exact saved run; never loads a project or starts a host.
use crate::cli::ExportArgs;
use grida_fx_core::Error;
use grida_fx_viewer::export::{ExportOptions, run_export};

pub fn run(args: &ExportArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let result = run_export(
        &cwd.join(&args.run),
        &cwd.join(&args.out),
        &args.out,
        ExportOptions {
            include_values: args.include_values,
            include_diagnostics: args.include_diagnostics,
        },
    );
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
                eprintln!(
                    "{}: {}: {}",
                    crate::invoker::command(),
                    error.code,
                    error.message
                );
            }
            Ok(2)
        }
    }
}
