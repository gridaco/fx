//! `lock [where] [--check] [--same NODE]…`: [`grida_fx_core::lock::lock`] for the project found
//! from `where` (relative to the working directory; a folder or a file in the project; default the
//! working directory itself); prints its lines; writes fx.lock atomically (temp file in the root,
//! then rename) unless `--check`; exits with its status. A `where` that is not there is refused
//! (exit 2) instead of falling back to a project above it.
//!
//! The file is written before the lines are printed, so a lock that cannot be written stops the
//! command (exit 2) instead of reporting a count it did not write.

use crate::cli::LockArgs;
use crate::print::print_line;
use grida_fx_core::Error;
use grida_fx_core::docs::lock::{LOCK_FILE, render_lock};
use grida_fx_core::docs::project::Project;
use std::io::Write;
use std::path::Path;

pub fn run(args: &LockArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let start = match &args.r#where {
        Some(place) => {
            let start = cwd.join(place);
            std::fs::metadata(&start).map_err(|error| Error::io(place, &error))?;
            start
        }
        None => cwd,
    };
    let project = Project::find(&start)?;
    let mut host = crate::print::host();
    let report = grida_fx_core::lock::lock(&project, &args.same, args.check, &mut host)?;
    if let (false, Some(lock)) = (args.check, &report.write) {
        write_atomically(&project.root, LOCK_FILE, &render_lock(lock))?;
    }
    for line in &report.lines {
        print_line(line);
    }
    Ok(u8::try_from(report.status).unwrap_or(1))
}

/// Writes `<folder>/<name>` through a temporary file in the same folder, renamed over it, so a
/// reader sees the old file or the new one, never a part. Errors name the file as `name`.
fn write_atomically(folder: &Path, name: &str, text: &str) -> Result<(), Error> {
    let target = folder.join(name);
    let temporary = folder.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, &target));
    written.map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::io(name, &error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces_a_file_whole() {
        let folder = tempfile::tempdir().unwrap();
        write_atomically(folder.path(), "fx.lock", "one\n").unwrap();
        write_atomically(folder.path(), "fx.lock", "two\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.path().join("fx.lock")).unwrap(),
            "two\n"
        );
        let names: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["fx.lock"], "no temporary file is left behind");
    }

    #[test]
    fn a_folder_that_is_not_there_names_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let missing = folder.path().join("nowhere");
        let error = write_atomically(&missing, "fx.lock", "x").unwrap_err();
        assert_eq!(error.kind, grida_fx_core::ErrorKind::Io);
        assert_eq!(error.message, "fx.lock: no such file");
    }
}
