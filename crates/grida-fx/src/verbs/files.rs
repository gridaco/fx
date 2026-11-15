//! Read-only file discovery and private native resolution; never starts a host or service.
use crate::cli::{FilesArgs, FilesVerb};
use grida_fx_core::Error;
use grida_fx_viewer::files::{self, FileError};
use serde_json::Value;

pub fn run(args: &FilesArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let (result, json) = match &args.verb {
        FilesVerb::List(args) => (
            files::list(&cwd.join(&args.target), args.instance.as_deref()),
            args.json,
        ),
        FilesVerb::Resolve(args) => {
            let project = args.project.as_ref().map(|path| cwd.join(path));
            let source_root = args.source_root.as_ref().map(|path| cwd.join(path));
            let reference = (args.reference.len() <= 1024 * 1024)
                .then(|| grida_fx_core::value::parse_json(&args.reference).ok())
                .flatten();
            (
                reference.map_or_else(
                    || {
                        Err(FileError {
                            kind: "fx-file-error-v1",
                            code: "invalid_request",
                            message: "The file reference is malformed.",
                        })
                    },
                    |reference| {
                        files::resolve(
                            &cwd.join(&args.target),
                            &reference,
                            project.as_deref(),
                            source_root.as_deref(),
                        )
                    },
                ),
                args.json,
            )
        }
    };
    match result {
        Ok(value) => {
            if json {
                crate::print::print_json(&value);
            } else if let Some(files) = value["files"].as_array() {
                for file in files {
                    let target = &file["target"];
                    println!(
                        "{}  {}",
                        file["role"].as_str().unwrap_or("file"),
                        label(target)
                    );
                }
            } else if let Some(path) = value["local_path"].as_str() {
                // This operation explicitly returns private paths to its local native caller.
                println!("{path}");
            } else {
                println!(
                    "{}",
                    value["reason"].as_str().unwrap_or("File unavailable.")
                );
            }
            Ok(0)
        }
        Err(error) => {
            if json {
                crate::print::print_json(
                    &serde_json::to_value(error).expect("serializable file error"),
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

fn label(target: &Value) -> &str {
    target["source"]
        .as_str()
        .or_else(|| target["artifact"]["name"].as_str())
        .unwrap_or("file")
}
