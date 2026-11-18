//! Project setup with `init` and `add` (spec/service.md §2). Nothing reaches beyond this
//! machine: GitHub archives are stood in for by archives built here, sometimes served on loopback.
#![cfg(unix)]

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The starter's own files, in the order they are written; the skills follow.
const STARTER: [&str; 6] = [
    "fx.yaml",
    "README.md",
    "AGENTS.md",
    ".gitignore",
    ".env.example",
    "workflows/icon.yaml",
];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The engine in `root`, with no keys, no `.env`, no proxy and the network off.
fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .current_dir(root)
        .env_clear()
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("GRIDA_FX_NETWORK", "off");
    command
}

fn execute(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn refused(output: &Output, says: &str) {
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(says), "{stderr}");
}

fn validator(schema: &str) -> jsonschema::Validator {
    let path = repository().join(format!("spec/schemas/{schema}.schema.json"));
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

fn conforms(schema: &str, document: &Value) {
    let validator = validator(schema);
    let problems: Vec<String> = validator
        .iter_errors(document)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    assert!(problems.is_empty(), "{problems:#?}\n{document:#}");
}

/// Every file under `folder`, relative, `/`-separated and sorted.
fn files(folder: &Path) -> Vec<String> {
    fn walk(folder: &Path, relative: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(folder.join(relative)).unwrap() {
            let entry = entry.unwrap();
            let path = relative.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                walk(folder, &path, found);
            } else {
                let parts: Vec<&str> = path
                    .components()
                    .map(|part| part.as_os_str().to_str().unwrap())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(folder, Path::new(""), &mut found);
    found.sort();
    found
}

fn skills() -> Vec<String> {
    let mut skills: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(repository().join("skills")).unwrap() {
        let folder = entry.unwrap().path();
        if folder.join("SKILL.md").is_file() {
            let name = folder.file_name().unwrap().to_str().unwrap();
            skills.extend(
                files(&folder)
                    .into_iter()
                    .map(|file| format!(".agents/skills/{name}/{file}")),
            );
        }
    }
    skills.sort();
    skills
}

fn index() -> Vec<Value> {
    let text = std::fs::read_to_string(repository().join("examples/templates.json")).unwrap();
    let index: Value = serde_json::from_str(&text).unwrap();
    index["templates"].as_array().unwrap().clone()
}

#[test]
fn the_starter_holds_the_starter_files_and_every_skill() {
    let root = tempfile::tempdir().unwrap();
    let initialized = success(execute(root.path(), &["init", "--json"]));
    conforms("fx-project-init-v1", &initialized);
    assert_eq!(initialized["created"], true);
    assert_eq!(
        initialized["template"],
        json!({"name": "starter", "source": "built_in"})
    );
    let mut expected: Vec<String> = STARTER.iter().map(|name| name.to_string()).collect();
    expected.extend(skills());
    assert_eq!(initialized["files_created"], json!(expected));
    expected.sort();
    assert_eq!(files(root.path()), expected);
    // The skill a project gets is the repository's, byte for byte.
    assert_eq!(
        std::fs::read(root.path().join(".agents/skills/grida-fx/SKILL.md")).unwrap(),
        std::fs::read(repository().join("skills/grida-fx/SKILL.md")).unwrap()
    );
    // The starter plans offline; its one paid step waits for the user's route.
    let plan = execute(root.path(), &["plan", "icon"]);
    assert!(plan.status.success());
    assert!(String::from_utf8_lossy(&plan.stdout).contains("no route for image.generate"));
}

#[test]
fn an_existing_project_is_checked_and_left_as_it_is() {
    let root = tempfile::tempdir().unwrap();
    let source = "# Authored configuration\nfx: project/v1\nruns: history\n";
    std::fs::write(root.path().join("fx.yaml"), source).unwrap();
    let repeated = success(execute(root.path(), &["init", "--json"]));
    conforms("fx-project-init-v1", &repeated);
    assert_eq!(repeated["created"], false);
    assert_eq!(repeated["files_created"], json!([]));
    assert_eq!(repeated["template"], Value::Null);
    refused(
        &execute(root.path(), &["init", "--template", "hello"]),
        "already a project",
    );
    std::fs::write(root.path().join("fx.yaml"), "invalid: configuration\n").unwrap();
    assert!(!execute(root.path(), &["init", "--json"]).status.success());
    assert_eq!(files(root.path()), vec!["fx.yaml"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("fx.yaml")).unwrap(),
        "invalid: configuration\n"
    );
}

#[test]
fn a_folder_with_files_and_no_project_is_refused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes.txt"), "mine\n").unwrap();
    refused(
        &execute(root.path(), &["init", "--json"]),
        "is not empty and has no fx.yaml",
    );
    assert_eq!(files(root.path()), vec!["notes.txt"]);
    // A fresh `git init` still counts as empty, and a named directory is created.
    let fresh = tempfile::tempdir().unwrap();
    std::fs::create_dir(fresh.path().join(".git")).unwrap();
    assert_eq!(
        success(execute(fresh.path(), &["init", "--json"]))["created"],
        true
    );
    let named = success(execute(root.path(), &["init", "assets", "--json"]));
    assert_eq!(named["files_created"][0], "fx.yaml");
    assert!(root.path().join("assets/workflows/icon.yaml").is_file());
}

#[test]
fn the_listing_is_the_starter_then_the_index() {
    let root = tempfile::tempdir().unwrap();
    let listed = success(execute(
        root.path(),
        &["init", "--list-templates", "--json"],
    ));
    conforms("fx-project-templates-v1", &listed);
    let names: Vec<&str> = listed["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|template| template["name"].as_str().unwrap())
        .collect();
    let mut expected = vec!["starter"];
    let index = index();
    expected.extend(
        index
            .iter()
            .map(|template| template["name"].as_str().unwrap()),
    );
    assert_eq!(names, expected);
    assert!(listed["templates"][0].get("workflow_entry").is_none());
    for (entry, template) in listed["templates"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .zip(index)
    {
        for field in ["workflow_entry", "inputs_file", "prerequisites"] {
            assert_eq!(entry.get(field), template["starter"].get(field));
        }
        assert!(entry.get("missing_inputs").is_none());
    }
    refused(
        &execute(root.path(), &["init", "x", "--template", "nope"]),
        "no template \"nope\"; the templates: starter,",
    );
    assert!(!root.path().join("x").exists());
}

#[test]
fn every_indexed_template_starts_from_the_checkout() {
    let checkout = repository().canonicalize().unwrap();
    for template in index() {
        let name = template["name"].as_str().unwrap();
        let folder = checkout.join(template["path"].as_str().unwrap());
        let root = tempfile::tempdir().unwrap();
        let started = success(execute(
            root.path(),
            &[
                "init",
                "project",
                "--template",
                name,
                "--template-source",
                checkout.to_str().unwrap(),
                "--json",
            ],
        ));
        conforms("fx-project-init-v1", &started);
        assert_eq!(
            started["template"],
            json!({"name": name, "source": "local"})
        );
        let project = root.path().join("project");
        let written = files(&project);
        assert_eq!(started["files_created"], json!(written), "{name}");
        // The template's committed files arrive unchanged, but for .gitignore, which gains the
        // starter's lines; what a checkout's runs leave behind does not come along.
        for file in files(&folder) {
            if [
                ".fx/",
                "runs/",
                ".venv/",
                "__pycache__/",
                ".showcase-inputs/",
            ]
            .iter()
            .any(|skipped| file.starts_with(skipped) || file.contains(&format!("/{skipped}")))
            {
                assert!(!project.join(&file).exists(), "{name}: {file}");
                continue;
            }
            if file == ".gitignore" {
                continue;
            }
            assert_eq!(
                std::fs::read(project.join(&file)).unwrap(),
                std::fs::read(folder.join(&file)).unwrap(),
                "{name}: {file}"
            );
        }
        let ignore = std::fs::read_to_string(project.join(".gitignore")).unwrap();
        for line in ["runs/", ".fx/", ".env", ".venv/", "node_modules/"] {
            assert!(ignore.lines().any(|own| own == line), "{name}: {ignore}");
        }
        for file in [
            "AGENTS.md",
            ".env.example",
            ".agents/skills/grida-fx/SKILL.md",
        ] {
            assert!(project.join(file).is_file(), "{name}: {file}");
        }
        // Nothing of the unpacking is left behind.
        assert!(!std::fs::read_dir(&project).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".fx-init")
        }));
    }
}

#[test]
fn a_template_s_own_gitignore_keeps_its_lines() {
    let checkout = repository().canonicalize().unwrap();
    let own = std::fs::read_to_string(checkout.join("examples/image-recolor/.gitignore")).unwrap();
    let root = tempfile::tempdir().unwrap();
    success(execute(
        root.path(),
        &[
            "init",
            "--template",
            "image-recolor",
            "--template-source",
            checkout.to_str().unwrap(),
            "--json",
        ],
    ));
    let merged = std::fs::read_to_string(root.path().join(".gitignore")).unwrap();
    assert!(merged.starts_with(&own), "{merged}");
    assert!(
        merged.ends_with("runs/\n.fx/\n.env\n.venv/\n__pycache__/\nnode_modules/\n"),
        "{merged}"
    );
}

// --- Archives -----------------------------------------------------------------------------------

/// An entry of an archive built here.
#[derive(Clone, Copy)]
enum Item<'a> {
    File(&'a str, &'a [u8]),
    Link(&'a str, &'a str),
    /// A file whose name is written as given, `..` included.
    Raw(&'a str, &'a [u8]),
}

/// A `.tar.gz` like the release archive: a pax global header first (as `git archive` writes),
/// then `items`.
fn archive(items: &[Item]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let comment = b"52 comment=0123456789abcdef0123456789abcdef01234567\n";
    let mut global = tar::Header::new_ustar();
    global.set_path("pax_global_header").unwrap();
    global.set_entry_type(tar::EntryType::XGlobalHeader);
    global.set_size(comment.len() as u64);
    global.set_cksum();
    builder.append(&global, &comment[..]).unwrap();
    for item in items {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        match item {
            Item::File(path, contents) => {
                header.set_size(contents.len() as u64);
                builder.append_data(&mut header, path, *contents).unwrap();
            }
            Item::Link(path, target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder.append_link(&mut header, path, target).unwrap();
            }
            Item::Raw(path, contents) => {
                let name = &mut header.as_old_mut().name;
                name[..path.len()].copy_from_slice(path.as_bytes());
                header.set_size(contents.len() as u64);
                header.set_cksum();
                builder.append(&header, *contents).unwrap();
            }
        }
    }
    builder.into_inner().unwrap().finish().unwrap()
}

const HELLO: [Item<'static>; 3] = [
    Item::File("fx-9.9.9/README.md", b"# FX\n"),
    Item::File("fx-9.9.9/examples/hello/fx.yaml", b"fx: project/v1\n"),
    Item::File(
        "fx-9.9.9/examples/hello/workflows/hello.yaml",
        b"fx: workflow/v1\n",
    ),
];

const PET: [Item<'static>; 3] = [
    Item::File(
        "fx-9.9.9/examples/pet/fx.yaml",
        include_bytes!("../../../examples/pet/fx.yaml"),
    ),
    Item::File(
        "fx-9.9.9/examples/pet/workflows/pet.yaml",
        include_bytes!("../../../examples/pet/workflows/pet.yaml"),
    ),
    Item::File(
        "fx-9.9.9/examples/pet/inputs/pebble.yaml",
        include_bytes!("../../../examples/pet/inputs/pebble.yaml"),
    ),
];

fn write_test_archive(root: &Path, items: &[Item]) {
    std::fs::write(root.join("release.tar.gz"), archive(items)).unwrap();
}

#[test]
fn add_installs_pet_as_its_own_project_and_preserves_local_edits() {
    let root = tempfile::tempdir().unwrap();
    let parent_config = "fx: project/v1\nroutes:\n  image.generate: imagegen@openai\n";
    std::fs::write(root.path().join("fx.yaml"), parent_config).unwrap();
    write_test_archive(root.path(), &PET);
    let args = [
        "add",
        "pet",
        "--template-source",
        "release.tar.gz",
        "--json",
    ];
    let result = success(execute(root.path(), &args));
    conforms("fx-project-init-v1", &result);
    let validator = validator("fx-project-init-v1");
    for (field, invalid) in [
        ("created", json!(false)),
        ("template", Value::Null),
        ("template", json!({"name": "pet", "source": "built_in"})),
    ] {
        let mut broken = result.clone();
        broken[field] = invalid;
        assert!(!validator.is_valid(&broken), "accepted {broken:#}");
    }
    let project = root.path().join("pet").canonicalize().unwrap();
    assert_eq!(result["project_root"], json!(project));
    assert_eq!(
        result["template"],
        json!({"name": "pet", "source": "local"})
    );
    assert_eq!(result["workflow_entry"], "workflows/pet.yaml");
    assert_eq!(result["inputs_file"], "inputs/pebble.yaml");
    assert_eq!(result["missing_inputs"], json!([]));
    assert_eq!(result["prerequisites"].as_array().unwrap().len(), 2);
    assert_eq!(result["files_created"], json!(files(&project)));
    assert!(project.join(".agents/skills/grida-fx/SKILL.md").is_file());

    let inputs = project.join("inputs/pebble.yaml");
    std::fs::write(&inputs, "id: edited\n").unwrap();
    // Even with no network, the collision is diagnosed before a template is acquired.
    refused(
        &execute(root.path(), &["add", "pet", "--json"]),
        "already exists",
    );
    assert_eq!(std::fs::read_to_string(inputs).unwrap(), "id: edited\n");
    assert_eq!(
        std::fs::read_to_string(root.path().join("fx.yaml")).unwrap(),
        parent_config
    );
}

#[test]
fn add_rejects_a_broken_starter_before_publishing() {
    let root = tempfile::tempdir().unwrap();
    let args = [
        "add",
        "pet",
        "fox-pet",
        "--template-source",
        "release.tar.gz",
        "--json",
    ];
    let cases = [
        (
            [
                Item::File("fx-9.9.9/examples/pet/fx.yaml", b"fx: wrong/v1\n"),
                PET[1],
                PET[2],
            ],
            "fx.yaml",
        ),
        (
            [
                PET[0],
                Item::File(
                    "fx-9.9.9/examples/pet/workflows/pet.yaml",
                    b"fx: wrong/v1\n",
                ),
                PET[2],
            ],
            "workflows/pet.yaml",
        ),
        (
            [PET[0], PET[1], Item::File("fx-9.9.9/README.md", b"")],
            "starter input preset",
        ),
    ];
    for (items, says) in cases {
        write_test_archive(root.path(), &items);
        refused(&execute(root.path(), &args), says);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
    refused(
        &execute(root.path(), &["add", "starter"]),
        "no add template",
    );
    assert!(!root.path().join("starter").exists());
    refused(
        &execute(root.path(), &["add", "pet", "absent/fox"]),
        "destination parent",
    );
    assert!(!root.path().join("absent").exists());
}

#[test]
fn add_refuses_workflow_search_that_would_cover_the_installed_project() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().canonicalize().unwrap().join("fox-pet");
    let config = format!("fx: project/v1\nworkflows: [{}]\n", json!(target));
    write_test_archive(
        root.path(),
        &[
            Item::File("fx-9.9.9/examples/pet/fx.yaml", config.as_bytes()),
            PET[1],
            PET[2],
        ],
    );
    let outcome = execute(
        root.path(),
        &[
            "add",
            "pet",
            "fox-pet",
            "--template-source",
            "release.tar.gz",
        ],
    );
    refused(&outcome, "is the project or holds it");
    assert!(!target.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn add_checks_search_links_against_the_installed_project_tree() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    let target = root.join("fox-pet");
    symlink("fox-pet", root.join("project-link")).unwrap();
    symlink(target.join("workflows"), root.join("child-link")).unwrap();
    symlink("child-link", root.join("chain")).unwrap();
    symlink("fox-pet/missing", root.join("missing-link")).unwrap();
    std::fs::create_dir(root.join("external")).unwrap();

    let install = |workflows: &[&str]| {
        let config = format!("fx: project/v1\nworkflows: {}\n", json!(workflows));
        write_test_archive(
            &root,
            &[
                Item::File("fx-9.9.9/examples/pet/fx.yaml", config.as_bytes()),
                PET[1],
                PET[2],
            ],
        );
        execute(
            &root,
            &[
                "add",
                "pet",
                "fox-pet",
                "--template-source",
                "release.tar.gz",
                "--json",
            ],
        )
    };

    for entry in [
        "../project-link",
        "../chain/..",
        "../project-link/missing/..",
    ] {
        refused(&install(&[entry]), "is the project or holds it");
        assert!(!target.exists(), "{entry}");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 6);
    }
    success(install(&[
        "workflows",
        "../chain",
        "../missing-link",
        "../external",
    ]));
    let validated = success(execute(&target, &["init", "--json"]));
    assert_eq!(validated["created"], false);
}

#[test]
fn add_refuses_to_stage_inside_its_checkout_source() {
    let root = tempfile::tempdir().unwrap();
    let example = root.path().join("examples/pet");
    std::fs::create_dir_all(&example).unwrap();
    let link = root.path().join("example-link");
    std::os::unix::fs::symlink(&example, &link).unwrap();
    let outcome = execute(
        root.path(),
        &["add", "pet", "example-link/fox", "--template-source", "."],
    );
    refused(&outcome, "destination is inside the template source");
    assert!(std::fs::read_dir(example).unwrap().next().is_none());
}

#[test]
fn add_reports_a_missing_declared_input_without_creating_it() {
    let root = tempfile::tempdir().unwrap();
    write_test_archive(
        root.path(),
        &[
            Item::File(
                "fx-9.9.9/examples/image-recolor/fx.yaml",
                b"fx: project/v1\n",
            ),
            Item::File(
                "fx-9.9.9/examples/image-recolor/workflows/image-recolor.yaml",
                include_bytes!("../../../examples/image-recolor/workflows/image-recolor.yaml"),
            ),
            Item::File(
                "fx-9.9.9/examples/image-recolor/inputs/example.yaml",
                include_bytes!("../../../examples/image-recolor/inputs/example.yaml"),
            ),
        ],
    );
    let result = success(execute(
        root.path(),
        &[
            "add",
            "image-recolor",
            "--template-source",
            "release.tar.gz",
            "--json",
        ],
    ));
    assert_eq!(result["missing_inputs"], json!(["inputs/source.png"]));
    assert!(!root.path().join("image-recolor/inputs/source.png").exists());
}

const COMMUNITY: &str = "acme/studio@v1.2.0";
const COMMUNITY_PROJECT: Item<'static> = Item::File("studio-1.2.0/fx.yaml", b"fx: project/v1\n");
const COMMUNITY_WORKFLOW: &[u8] = b"fx: workflow/v1
id: posters
title: Studio posters
steps:
  draw:
    uses: fx/image.generate@1
    with: { prompt: A colorful poster }
";

fn community_add(root: &Path, options: &[&str]) -> Output {
    let mut args = vec![
        "add",
        COMMUNITY,
        "--template-source",
        "release.tar.gz",
        "--json",
    ];
    args.extend_from_slice(options);
    execute(root, &args)
}

#[test]
fn community_add_uses_the_root_declaration_and_preserves_local_edits() {
    let root = tempfile::tempdir().unwrap();
    write_test_archive(
        root.path(),
        &[
            COMMUNITY_PROJECT,
            Item::File("studio-1.2.0/recipes/posters.yaml", COMMUNITY_WORKFLOW),
            Item::File("studio-1.2.0/workflows/labels.yaml", COMMUNITY_WORKFLOW),
            Item::File("studio-1.2.0/inputs/start.yaml", b"{}\n"),
            Item::File("studio-1.2.0/README.md", b"# Studio\n"),
            Item::File("studio-1.2.0/.env.local", b"# Local configuration\n"),
            Item::File("studio-1.2.0/.env.example", b"# Studio environment\n"),
            Item::File(
                "studio-1.2.0/fx-template.json",
                br#"{
                    "workflow_entry": "recipes/posters.yaml",
                    "inputs_file": "inputs/start.yaml",
                    "required_inputs": ["inputs/reference.png"],
                    "prerequisites": ["An image provider for live drawing"]
                }"#,
            ),
        ],
    );

    let install = success(community_add(root.path(), &[]));
    conforms("fx-project-init-v1", &install);
    let project = root.path().join("studio");
    assert_eq!(
        install["project_root"],
        json!(project.canonicalize().unwrap())
    );
    assert_eq!(
        install["template"],
        json!({"name": COMMUNITY, "source": "github"})
    );
    assert_eq!(install["workflow_entry"], "recipes/posters.yaml");
    assert_eq!(install["inputs_file"], "inputs/start.yaml");
    assert_eq!(install["missing_inputs"], json!(["inputs/reference.png"]));
    assert_eq!(
        install["prerequisites"],
        json!(["An image provider for live drawing"])
    );
    assert_eq!(install["files_created"], json!(files(&project)));
    assert_eq!(
        std::fs::read(project.join("README.md")).unwrap(),
        b"# Studio\n"
    );
    assert!(!project.join(".env.local").exists());
    assert_eq!(
        std::fs::read(project.join(".env.example")).unwrap(),
        b"# Studio environment\n"
    );

    let selection = success(community_add(
        root.path(),
        &["other", "--workflow", "posters"],
    ));
    assert_eq!(selection["inputs_file"], "inputs/start.yaml");
    let selection = success(community_add(
        root.path(),
        &["alternate", "--workflow", "labels"],
    ));
    assert_eq!(selection["workflow_entry"], "workflows/labels.yaml");
    assert!(selection.get("inputs_file").is_none());

    let preset = project.join("inputs/start.yaml");
    std::fs::write(&preset, "# edited\n").unwrap();
    refused(&execute(root.path(), &["add", COMMUNITY]), "already exists");
    assert_eq!(std::fs::read(&preset).unwrap(), b"# edited\n");
}

#[test]
fn community_add_discovers_workflow_files_and_requires_an_unambiguous_selection() {
    let root = tempfile::tempdir().unwrap();
    let posters = Item::File("studio-1.2.0/fx/posters.yml", COMMUNITY_WORKFLOW);
    let labels = Item::File("studio-1.2.0/workflows/labels.yaml", COMMUNITY_WORKFLOW);
    let other_yaml = Item::File("studio-1.2.0/workflows/settings.yaml", b"theme: ink\n");
    let nested = Item::File(
        "studio-1.2.0/workflows/nested/labels.yaml",
        COMMUNITY_WORKFLOW,
    );
    write_test_archive(
        root.path(),
        &[COMMUNITY_PROJECT, posters, other_yaml, nested],
    );
    let install = success(community_add(root.path(), &["single"]));
    assert_eq!(install["workflow_entry"], "fx/posters.yml");
    assert_eq!(install["missing_inputs"], json!([]));

    write_test_archive(root.path(), &[COMMUNITY_PROJECT, posters, labels]);
    for options in [&[][..], &["--workflow", "does-not-exist"][..]] {
        let outcome = community_add(root.path(), options);
        refused(&outcome, "workflows/labels.yaml");
        assert!(String::from_utf8_lossy(&outcome.stderr).contains("fx/posters.yml"));
        assert!(!root.path().join("studio").exists());
    }
    let malformed = Item::File(
        "studio-1.2.0/workflows/draft.yaml",
        b"fx: workflow/v1\nsteps: [\n",
    );
    write_test_archive(
        root.path(),
        &[COMMUNITY_PROJECT, posters, labels, malformed],
    );
    let install = success(community_add(
        root.path(),
        &["selected", "--workflow", "labels"],
    ));
    assert_eq!(install["workflow_entry"], "workflows/labels.yaml");

    let same_stem = Item::File("studio-1.2.0/workflows/posters.yaml", COMMUNITY_WORKFLOW);
    write_test_archive(root.path(), &[COMMUNITY_PROJECT, posters, same_stem]);
    let outcome = community_add(root.path(), &["--workflow", "posters"]);
    refused(&outcome, "workflows/posters.yaml");
    assert!(String::from_utf8_lossy(&outcome.stderr).contains("fx/posters.yml"));
    assert!(!root.path().join("studio").exists());
}

#[test]
fn community_add_rejects_invalid_sources_before_network_or_writes() {
    let root = tempfile::tempdir().unwrap();
    for source in [
        "acme/studio",
        "acme/studio@",
        "acme/studio@../v1",
        "acme/studio@v1?zip",
    ] {
        let outcome = execute(root.path(), &["add", source]);
        refused(&outcome, "OWNER/REPO@REF");
        assert!(outcome.stdout.is_empty());
    }
    for source in [
        COMMUNITY,
        "acme/studio@0123456789abcdef0123456789abcdef01234567",
    ] {
        refused(
            &execute(root.path(), &["add", source]),
            "GRIDA_FX_NETWORK=off",
        );
    }
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn community_add_refuses_unsafe_archive_entries_without_publishing() {
    let root = tempfile::tempdir().unwrap();
    let workflow = Item::File("studio-1.2.0/workflows/posters.yaml", COMMUNITY_WORKFLOW);
    for item in [
        Item::Link("studio-1.2.0/escape", "../outside"),
        Item::Raw("studio-1.2.0/../outside", b"escape"),
        Item::Raw(r"studio-1.2.0/fx\outside", b"escape"),
    ] {
        write_test_archive(root.path(), &[COMMUNITY_PROJECT, workflow, item]);
        refused(&community_add(root.path(), &[]), "template was refused");
        assert_eq!(files(root.path()), vec!["release.tar.gz"]);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn community_add_refuses_an_invalid_root_declaration_instead_of_falling_back_to_scan() {
    let root = tempfile::tempdir().unwrap();
    write_test_archive(
        root.path(),
        &[
            COMMUNITY_PROJECT,
            Item::File("studio-1.2.0/workflows/posters.yaml", COMMUNITY_WORKFLOW),
            Item::File(
                "studio-1.2.0/fx-template.json",
                br#"{"workflow_entry":"../posters.yaml"}"#,
            ),
        ],
    );
    refused(&community_add(root.path(), &[]), "fx-template.json");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn community_add_counts_implicit_archive_directories_against_the_limit() {
    let root = tempfile::tempdir().unwrap();
    let paths: Vec<String> = (0..501)
        .map(|n| format!("studio-1.2.0/assets/{n}/child/source.txt"))
        .collect();
    let mut items = vec![COMMUNITY_PROJECT];
    items.extend(paths.iter().map(|path| Item::File(path, b"source\n")));
    write_test_archive(root.path(), &items);
    refused(
        &community_add(root.path(), &[]),
        "more than 1000 directories",
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_add_has_one_complete_winner() {
    use std::process::Stdio;
    let root = tempfile::tempdir().unwrap();
    write_test_archive(root.path(), &PET);
    let mut call = command(root.path());
    call.args([
        "add",
        "pet",
        "fox-pet",
        "--template-source",
        "release.tar.gz",
        "--json",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let first = call.spawn().unwrap();
    let second = call.spawn().unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    let (winner, loser) = if first.status.success() {
        (first, second)
    } else {
        (second, first)
    };
    refused(&loser, "already exists");
    let winner = success(winner);
    assert_eq!(
        winner["files_created"],
        json!(files(&root.path().join("fox-pet")))
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

fn from_archive(root: &Path, bytes: &[u8]) -> Output {
    let path = root.join("release.tar.gz");
    std::fs::write(&path, bytes).unwrap();
    execute(
        root,
        &[
            "init",
            "project",
            "--template",
            "hello",
            "--template-source",
            path.to_str().unwrap(),
            "--json",
        ],
    )
}

#[test]
fn an_archive_yields_only_its_template_s_folder() {
    let root = tempfile::tempdir().unwrap();
    let started = success(from_archive(root.path(), &archive(&HELLO)));
    assert_eq!(
        started["template"],
        json!({"name": "hello", "source": "local"})
    );
    let project = root.path().join("project");
    assert_eq!(
        std::fs::read_to_string(project.join("workflows/hello.yaml")).unwrap(),
        "fx: workflow/v1\n"
    );
    // The repository's own README stays in the archive; the starter's is added instead.
    assert!(
        std::fs::read_to_string(project.join("README.md"))
            .unwrap()
            .starts_with("# FX project")
    );
}

#[test]
fn an_unsafe_or_wrong_archive_is_refused_and_writes_nothing() {
    let many: Vec<String> = (0..1001)
        .map(|n| format!("fx-9.9.9/examples/hello/many/{n}.txt"))
        .collect();
    let mut crowded = vec![Item::File(
        "fx-9.9.9/examples/hello/fx.yaml",
        b"fx: project/v1\n",
    )];
    crowded.extend(many.iter().map(|path| Item::File(path, b"x")));
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (
            archive(&[
                Item::File("fx-9.9.9/examples/hello/fx.yaml", b"fx: project/v1\n"),
                Item::Link("fx-9.9.9/examples/hello/escape", "/etc/passwd"),
            ]),
            "escape is a link or a special file",
        ),
        (
            archive(&[
                Item::File("fx-9.9.9/examples/hello/fx.yaml", b"fx: project/v1\n"),
                Item::Raw("fx-9.9.9/examples/hello/../../../evil", b"x"),
            ]),
            "leaves its folder",
        ),
        (
            archive(&[Item::Raw("/examples/hello/fx.yaml", b"fx: project/v1\n")]),
            "leaves its folder",
        ),
        (archive(&crowded), "more than 1000 files"),
        (
            archive(&[Item::File(
                "fx-9.9.9/examples/other/fx.yaml",
                b"fx: project/v1\n",
            )]),
            "is not in the source",
        ),
        (
            archive(&[Item::File("fx-9.9.9/examples/hello/README.md", b"# hi\n")]),
            "holds no fx.yaml",
        ),
        (
            archive(&[
                Item::File("fx-9.9.9/examples/hello/fx.yaml", b"fx: project/v1\n"),
                Item::File("other/examples/hello/x.yaml", b"x"),
            ]),
            "more than one top-level folder",
        ),
        (b"not an archive".to_vec(), "not a readable .tar.gz"),
    ];
    for (bytes, says) in cases {
        let root = tempfile::tempdir().unwrap();
        refused(&from_archive(root.path(), &bytes), says);
        assert!(!root.path().join("project").exists(), "{says}");
        assert!(!root.path().parent().unwrap().join("evil").exists());
    }
}

// --- Downloads ----------------------------------------------------------------------------------

/// Serves one HTTP response on loopback and returns its URL and the request it received.
fn serve_once(status: &str, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/gridaco/fx/tar.gz/v9.9.9",
        listener.local_addr().unwrap().port()
    );
    let status = status.to_string();
    let served = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        let head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/x-gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (url, served)
}

fn download(root: &Path, url: &str, network: bool) -> Output {
    let mut command = command(root);
    if network {
        command.env_remove("GRIDA_FX_NETWORK");
    }
    command
        .args([
            "init",
            "project",
            "--template",
            "hello",
            "--template-source",
            url,
            "--json",
        ])
        .output()
        .unwrap()
}

#[test]
fn a_template_downloads_once_without_a_credential() {
    let root = tempfile::tempdir().unwrap();
    let (url, served) = serve_once("200 OK", archive(&HELLO));
    let started = success(download(root.path(), &url, true));
    assert_eq!(started["template"]["source"], "local");
    assert!(root.path().join("project/workflows/hello.yaml").is_file());
    let request = served.join().unwrap().to_ascii_lowercase();
    assert!(
        request.starts_with("get /gridaco/fx/tar.gz/v9.9.9 "),
        "{request}"
    );
    assert!(!request.contains("authorization"), "{request}");
    assert!(!request.contains("cookie"), "{request}");
}

#[test]
fn a_failed_download_is_refused_and_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (url, served) = serve_once("404 Not Found", b"no such tag".to_vec());
    refused(&download(root.path(), &url, true), "returned HTTP 404");
    served.join().unwrap();
    assert!(!root.path().join("project").exists());
    // With the network off nothing is sent at all, not even to loopback.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/x.tar.gz",
        listener.local_addr().unwrap().port()
    );
    listener.set_nonblocking(true).unwrap();
    refused(&download(root.path(), &url, false), "GRIDA_FX_NETWORK=off");
    assert!(listener.accept().is_err());
    // Plain http goes only to this machine.
    refused(
        &download(root.path(), "http://example.com/fx.tar.gz", true),
        "must use https unless it targets a loopback host",
    );
    assert!(!root.path().join("project").exists());
}
