//! A new model reaches planning through imported source alone, without keys or network access.

use grida_fx_providers::models::{ASSETS, testing};
use std::path::Path;
use std::process::{Command, Output};

fn command(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command.current_dir(root).args(args).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("HOME", root)
        .env("NO_COLOR", "1")
        .env("GRIDA_FX_NETWORK", "off")
        .env("GRIDA_FX_DISABLE_DOTENV", "1");
    command.output().unwrap()
}

fn out(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn release(root: &Path) {
    std::fs::create_dir_all(root.join("release")).unwrap();
    for (name, bytes) in testing::files() {
        std::fs::write(root.join("release").join(name), bytes).unwrap();
    }
}

fn import(root: &Path, cap: &str) -> Output {
    command(
        root,
        &[
            "models",
            "import",
            testing::MODEL,
            "--provider",
            "gg",
            "--cap-usd",
            cap,
            "--from",
            "release",
        ],
    )
}

#[test]
fn an_offline_import_plans_a_new_model_and_reimport_is_idempotent() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    release(root);
    std::fs::write(root.join("fx.yaml"), format!(
        "# keep this project comment\nfx: project/v1\nroutes:\n  image.generate: {}\nprovider_preferences:\n  image.generate: gg\n", testing::MODEL,
    )).unwrap();
    std::fs::write(root.join("case.yaml"), "fx: workflow/v1\nid: case\ntitle: Model import fixture\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    with:\n      prompt: An original copper lantern.\n      size: 1024x1024\n      background: transparent\noutputs:\n  image: ${{ steps.draw.outputs.image }}\n").unwrap();
    let before = command(root, &["plan", "case", "--check"]);
    assert!(!before.status.success(), "{}", out(&before));
    let imported = import(root, "3");
    assert!(imported.status.success(), "{}", out(&imported));
    assert!(out(&imported).contains(testing::TAG));
    let config = std::fs::read(root.join("fx.yaml")).unwrap();
    assert!(String::from_utf8_lossy(&config).contains("# keep this project comment"));
    let planned = command(root, &["plan", "case", "--check", "--json"]);
    assert!(planned.status.success(), "{}", out(&planned));
    assert!(out(&planned).contains("acme/img-a@gg"));
    let repeated = import(root, "3");
    assert!(repeated.status.success(), "{}", out(&repeated));
    assert_eq!(std::fs::read(root.join("fx.yaml")).unwrap(), config);
    assert_eq!(
        std::fs::read_dir(root.join("routes/models"))
            .unwrap()
            .count(),
        1
    );
    let changed = import(root, "4");
    assert!(changed.status.success(), "{}", out(&changed));
    assert_eq!(
        std::fs::read_dir(root.join("routes/models"))
            .unwrap()
            .count(),
        2
    );
    let planned = command(root, &["plan", "case", "--check"]);
    assert!(planned.status.success(), "{}", out(&planned));
    assert!(out(&planned).contains("$0.00 – $4.00"), "{}", out(&planned));
    // Reusing an older immutable import must restore its precedence, not merely find its file.
    let restored = import(root, "3");
    assert!(restored.status.success(), "{}", out(&restored));
    let planned = command(root, &["plan", "case", "--check"]);
    assert!(planned.status.success(), "{}", out(&planned));
    assert!(out(&planned).contains("$0.00 – $3.00"), "{}", out(&planned));
    assert_eq!(
        std::fs::read_dir(root.join("routes/models"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn refusals_do_not_change_project_files() {
    for case in ["checksum", "layout", "profile", "release"] {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        release(root);
        let config = if case == "layout" {
            "{fx: project/v1}\n"
        } else {
            "# keep\nfx: project/v1\n"
        };
        std::fs::write(root.join("fx.yaml"), config).unwrap();
        if case == "checksum" {
            std::fs::write(root.join("release/models.v1.json"), "{}").unwrap();
        } else if case == "profile" {
            std::fs::write(root.join("release/gg.operations.json"), "{}").unwrap();
        }
        let output = if case == "release" {
            command(
                root,
                &[
                    "models",
                    "import",
                    testing::MODEL,
                    "--provider",
                    "gg",
                    "--cap-usd",
                    "3",
                    "--from",
                    "release",
                    "--release",
                    "v1.20260101.1",
                ],
            )
        } else {
            import(root, "3")
        };
        assert_eq!(output.status.code(), Some(2), "{case}: {}", out(&output));
        assert_eq!(
            std::fs::read_to_string(root.join("fx.yaml")).unwrap(),
            config
        );
        assert!(!root.join("routes").exists(), "{case}");
    }
}

#[test]
fn only_explicit_import_can_use_network_and_caps_are_checked_first() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let output = command(
        root,
        &[
            "models",
            "import",
            testing::MODEL,
            "--provider",
            "gg",
            "--cap-usd",
            "3",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(out(&output).contains("network is off"), "{}", out(&output));
    let output = import(root, "0");
    assert!(
        out(&output).contains("positive per-call"),
        "{}",
        out(&output)
    );
    assert!(!root.join("fx.yaml").exists());
    assert!(!root.join("routes").exists());
    // A missing optional asset is a local refusal, never a network fallback.
    release(root);
    std::fs::remove_file(root.join("release").join(ASSETS[2])).unwrap();
    let output = import(root, "3");
    assert!(!output.status.success());
    assert!(!out(&output).contains("network is off"));
}

#[cfg(unix)]
#[test]
fn imported_paths_cannot_follow_symlinks_outside_the_project() {
    for config_symlink in [false, true] {
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = folder.path();
        release(root);
        let config = "fx: project/v1\n";
        std::fs::write(outside.path().join("fx.yaml"), config).unwrap();
        if config_symlink {
            std::os::unix::fs::symlink(outside.path().join("fx.yaml"), root.join("fx.yaml"))
                .unwrap();
        } else {
            std::fs::write(root.join("fx.yaml"), config).unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("routes")).unwrap();
        }
        let output = import(root, "3");
        assert_eq!(output.status.code(), Some(2), "{}", out(&output));
        assert!(out(&output).contains("inside the project"));
        assert_eq!(
            std::fs::read_to_string(outside.path().join("fx.yaml")).unwrap(),
            config
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    }
}
