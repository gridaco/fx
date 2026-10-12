//! Embeds every agent skill under `skills/` for `init` (spec/service.md §2): each folder there
//! with a `SKILL.md` is a skill, and every file in it goes to `.agents/skills/<skill>/` of a new
//! project. A new skill or file is picked up by the next build, with no list to keep.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let skills = manifest.join("../../skills");
    println!("cargo:rerun-if-changed={}", skills.display());
    let mut files = Vec::new();
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&skills)
        .expect("skills/ is missing")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("SKILL.md").is_file())
        .collect();
    folders.sort();
    assert!(!folders.is_empty(), "skills/ holds no skill");
    for folder in &folders {
        let name = folder.file_name().unwrap().to_str().unwrap().to_string();
        collect(folder, Path::new(""), &name, &mut files);
    }
    files.sort();
    let mut source = String::from("/// Every skill's files: `(project path, contents)`.\n");
    source.push_str("pub const SKILLS: &[(&str, &[u8])] = &[\n");
    for (target, path) in &files {
        writeln!(
            source,
            "    ({target:?}, include_bytes!({:?})),",
            path.display()
        )
        .unwrap();
    }
    source.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("skills.rs");
    std::fs::write(out, source).unwrap();
}

/// Every file under `folder`, with the project path it goes to, skipping what tools leave there.
fn collect(folder: &Path, relative: &Path, skill: &str, files: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", folder.join(relative).display());
    for entry in std::fs::read_dir(folder.join(relative)).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_str().unwrap().to_string();
        if name.starts_with('.') || name == "__pycache__" || name == "node_modules" {
            continue;
        }
        let path = relative.join(&name);
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            collect(folder, &path, skill, files);
        } else if kind.is_file() {
            let parts: Vec<&str> = path.iter().map(|part| part.to_str().unwrap()).collect();
            let target = format!(".agents/skills/{skill}/{}", parts.join("/"));
            files.push((target, entry.path().canonicalize().unwrap()));
        }
    }
}
