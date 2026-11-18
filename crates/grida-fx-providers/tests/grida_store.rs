#![cfg(any(target_os = "macos", target_os = "linux"))]

use grida_fx_providers::grida_store::{self, StoreError};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const EMPTY: &str = "version = 1\n[migration]\nstate = 'unstarted'\nremoved = []\n[providers]\n";

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::named("home")
    }

    fn named(home_name: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().canonicalize().unwrap().join(home_name);
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
        let directory = home.join("providers");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temporary: temporary,
            home,
        }
    }

    fn file(&self) -> PathBuf {
        self.home.join("providers/credentials.toml")
    }

    fn write(&self, bytes: impl AsRef<[u8]>) {
        fs::write(self.file(), bytes).unwrap();
        fs::set_permissions(self.file(), fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn value(&self, id: &str) -> Option<String> {
        grida_store::read(&self.home, &[id])
            .unwrap()
            .get(id)
            .map(|secret| secret.expose().to_owned())
    }
}

fn input(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/grida-provider-v1")
            .join(format!("{name}.toml")),
    )
    .unwrap()
}

fn record(id: &str, key: &str) -> String {
    format!(
        "{EMPTY}[providers.{id}]\napi_key = {}\n",
        serde_json::to_string(key).unwrap()
    )
}

#[test]
fn pinned_protocol_inputs_preserve_exact_values_and_errors() {
    let fixture = Fixture::new();
    for (name, expected) in [
        ("empty", None),
        ("tombstone", None),
        ("escaping", Some(r#"  fx-"key"\path-λ-🧪  "#)),
    ] {
        let bytes = input(name);
        fixture.write(&bytes);
        assert_eq!(fixture.value("acme").as_deref(), expected);
        assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    }
    for (name, code) in [
        ("pending", StoreError::MigrationPending),
        ("unknown-version", StoreError::UnsupportedVersion),
        ("unknown-field", StoreError::InvalidStore),
    ] {
        let bytes = input(name);
        fixture.write(&bytes);
        assert_eq!(
            grida_store::read(&fixture.home, &["acme"]).unwrap_err(),
            code
        );
        assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    }
}

#[test]
fn reads_only_named_subset_and_keeps_stored_whitespace() {
    let fixture = Fixture::new();
    let document = format!(
        "{}[providers.gg]\napi_key = '  synthetic-gg  '\n[providers.other]\napi_key = 'synthetic-other'\n",
        record("example", "synthetic-α-🙂")
    );
    fixture.write(&document);
    let found = grida_store::read(&fixture.home, &["gg", "absent", "gg"]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found["gg"].expose(), "  synthetic-gg  ");
    assert!(!format!("{found:?}").contains("synthetic"));
    assert_eq!(fs::read(fixture.file()).unwrap(), document.as_bytes());
}

#[test]
fn missing_home_or_provider_directory_creates_nothing() {
    let fixture = Fixture::new();
    let missing = fixture.home.join("absent/home");
    assert!(grida_store::read(&missing, &["gg"]).unwrap().is_empty());
    assert!(!fixture.home.join("absent").exists());
    fs::remove_dir(fixture.home.join("providers")).unwrap();
    assert!(
        grida_store::read(&fixture.home, &["gg"])
            .unwrap()
            .is_empty()
    );
    assert!(!fixture.home.join("providers").exists());
}

#[test]
fn existing_empty_directory_uses_private_stable_lock_without_credential_record() {
    let fixture = Fixture::new();
    assert!(
        grida_store::read(&fixture.home, &["gg"])
            .unwrap()
            .is_empty()
    );
    assert!(!fixture.file().exists());
    let path = fixture.home.join("providers/profile.lock.sqlite");
    let before = fs::metadata(&path).unwrap();
    assert_eq!(before.len(), 0);
    assert_eq!(before.mode() & 0o7777, 0o600);
    assert!(
        grida_store::read(&fixture.home, &["gg"])
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::metadata(path).unwrap().ino(), before.ino());
}

#[test]
fn first_lock_has_private_mode_under_restrictive_umask() {
    let fixture = Fixture::new();
    fixture.write(record("acme", "fx-fixture-umask-key"));
    let output = Command::new("/bin/sh")
        .env_clear()
        .env("GRIDA_FX_TEST_STORE_UMASK_HOME", &fixture.home)
        .args(["-c", r#"umask 0200; exec "$@""#, "sh"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "restricted_umask_reader_child"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let lock = fixture.home.join("providers/profile.lock.sqlite");
    assert_eq!(fs::metadata(lock).unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn restricted_umask_reader_child() {
    let Some(home) = std::env::var_os("GRIDA_FX_TEST_STORE_UMASK_HOME") else {
        return;
    };
    let found = grida_store::read(Path::new(&home), &["acme"]).unwrap();
    assert_eq!(found["acme"].expose(), "fx-fixture-umask-key");
}

#[test]
fn malformed_inputs_fail_without_changing_credential_bytes() {
    let fixture = Fixture::new();
    let malformed = [
        vec![0xc3, 0x28],
        format!("\u{feff}{EMPTY}").into_bytes(),
        EMPTY.replace("version = 1", "version = 1.0").into_bytes(),
        EMPTY.replace("version = 1", "version = '1'").into_bytes(),
        format!("version = 1\n{EMPTY}").into_bytes(),
        EMPTY
            .replace("version = 1", "version = 1\nunknown = true")
            .into_bytes(),
        EMPTY
            .replace("state = 'unstarted'", "state = 'unknown'")
            .into_bytes(),
        EMPTY
            .replace("removed = []", "removed = ['gg', 'gg']")
            .into_bytes(),
        EMPTY
            .replace("state = 'unstarted'", "state = 'complete'")
            .replace("removed = []", "removed = ['gg']")
            .into_bytes(),
        record("gg", "synthetic")
            .replace("removed = []", "removed = ['gg']")
            .into_bytes(),
        record("Uppercase", "synthetic").into_bytes(),
        record("gg", "").into_bytes(),
        record("gg", "synthetic\u{85}").into_bytes(),
        record("gg", "synthetic\n").into_bytes(),
        format!(
            "{}refresh_token = 'synthetic-secret'\n",
            record("gg", "synthetic")
        )
        .into_bytes(),
        format!("{EMPTY}[providers.gg]\napi_key = [[[[['synthetic']]]]]\n").into_bytes(),
        format!("{EMPTY}[providers.gg]\napi_key = true\n").into_bytes(),
        record("gg", &"α".repeat(8193)).into_bytes(),
        vec![b' '; 1_048_577],
    ];
    for bytes in malformed {
        fixture.write(&bytes);
        assert_eq!(
            grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
            StoreError::InvalidStore
        );
        assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    }
    let mut too_many = EMPTY.to_owned();
    for index in 0..129 {
        too_many.push_str(&format!("[providers.p{index}]\napi_key = 'synthetic'\n"));
    }
    fixture.write(too_many);
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::InvalidStore
    );
}

#[test]
fn validates_unselected_entries_and_utf8_key_byte_boundary() {
    let fixture = Fixture::new();
    fixture.write(record("other", ""));
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::InvalidStore
    );
    let key = "α".repeat(8192);
    fixture.write(record("gg", &key));
    assert_eq!(fixture.value("gg"), Some(key));
}

#[test]
fn invalid_locations_and_provider_ids_do_not_open_storage() {
    let fixture = Fixture::new();
    for path in [
        "relative",
        "/",
        "/a/../b",
        "/a/./b",
        "/a//b",
        "/a/",
        "/a\nb",
        "/a\u{7f}b",
    ] {
        assert_eq!(
            grida_store::read(Path::new(path), &["gg"]).unwrap_err(),
            StoreError::InvalidInput
        );
    }
    for id in ["", "GG", "../gg", "g g"] {
        assert_eq!(
            grida_store::read(&fixture.home, &[id]).unwrap_err(),
            StoreError::InvalidInput
        );
    }
    assert!(!fixture.home.join("providers/profile.lock.sqlite").exists());
}

#[test]
fn allows_non_ascii_controls_in_home_path() {
    let fixture = Fixture::named("home\u{85}acme");
    fixture.write(record("acme", "fx-fixture-path-key"));
    assert_eq!(
        fixture.value("acme").as_deref(),
        Some("fx-fixture-path-key")
    );
}

#[test]
fn unsafe_files_and_aliases_are_refused_without_permission_repair() {
    let fixture = Fixture::new();
    fixture.write(record("gg", "synthetic"));
    fs::set_permissions(fixture.file(), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
    assert_eq!(fs::metadata(fixture.file()).unwrap().mode() & 0o7777, 0o644);
    fs::set_permissions(fixture.file(), fs::Permissions::from_mode(0o600)).unwrap();
    let alias = fixture.home.join("alias");
    symlink(&fixture.home, &alias).unwrap();
    assert_eq!(
        grida_store::read(&alias, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
    fs::hard_link(fixture.file(), fixture.home.join("copy")).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
    fs::remove_file(fixture.home.join("copy")).unwrap();
    fs::remove_file(fixture.file()).unwrap();
    symlink(fixture.home.join("does-not-exist"), fixture.file()).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
}

#[test]
fn unsafe_ancestors_are_refused_even_when_the_store_is_absent() {
    let fixture = Fixture::new();
    fs::remove_dir(fixture.home.join("providers")).unwrap();
    fs::set_permissions(&fixture.home, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
}

#[test]
fn cleanup_removes_only_valid_private_unpublished_orphans() {
    let fixture = Fixture::new();
    fixture.write(record("gg", "synthetic"));
    let directory = fixture.home.join("providers");
    let orphan = directory.join("credentials.toml.01234567-89ab-4def-8123-456789abcdef.tmp");
    fs::write(&orphan, "synthetic-unpublished").unwrap();
    fs::set_permissions(&orphan, fs::Permissions::from_mode(0o600)).unwrap();
    let unrelated = directory.join("credentials.toml.unrelated.tmp");
    fs::write(&unrelated, "synthetic-unrelated").unwrap();
    assert_eq!(fixture.value("gg").as_deref(), Some("synthetic"));
    assert!(!orphan.exists());
    assert!(unrelated.exists());
    fs::write(&orphan, "synthetic-unpublished").unwrap();
    fs::set_permissions(&orphan, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
    assert!(orphan.exists());
}

#[test]
fn rejects_wal_and_nonempty_lock_without_replacing_them() {
    let fixture = Fixture::new();
    fixture.write(EMPTY);
    let lock = fixture.home.join("providers/profile.lock.sqlite");
    fs::write(&lock, "synthetic-invalid-lock").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
    assert_eq!(fs::read(&lock).unwrap(), b"synthetic-invalid-lock");
    fs::write(&lock, []).unwrap();
    let sidecar = fixture.home.join("providers/profile.lock.sqlite-wal");
    fs::write(sidecar, []).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
        StoreError::StorageFailed
    );
}

#[test]
fn unsafe_existing_lock_is_refused_without_repair() {
    let fixture = Fixture::new();
    let lock = fixture.home.join("providers/profile.lock.sqlite");
    fs::write(&lock, []).unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o400)).unwrap();
    let before = fs::metadata(&lock).unwrap();
    assert_eq!(
        grida_store::read(&fixture.home, &["acme"]).unwrap_err(),
        StoreError::StorageFailed
    );
    let after = fs::metadata(lock).unwrap();
    assert_eq!(after.mode() & 0o7777, 0o400);
    assert_eq!(after.ino(), before.ino());
}

#[cfg(target_os = "macos")]
#[test]
fn refuses_macos_acl_grants_on_files_and_ancestors() {
    let fixture = Fixture::new();
    fixture.write(record("gg", "synthetic"));
    for path in [fixture.file(), fixture.home.clone()] {
        assert!(
            Command::new("/bin/chmod")
                .arg("+a")
                .arg("everyone allow read")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            grida_store::read(&fixture.home, &["gg"]).unwrap_err(),
            StoreError::StorageFailed
        );
        assert!(
            Command::new("/bin/chmod")
                .arg("-N")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(
        Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone deny write")
            .arg(fixture.file())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fixture.value("gg").as_deref(), Some("synthetic"));
    assert!(
        Command::new("/bin/chmod")
            .arg("-N")
            .arg(fixture.file())
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn errors_expose_only_fixed_codes() {
    let fixture = Fixture::new();
    fixture.write("synthetic-secret-not-toml");
    let start = Instant::now();
    let error = grida_store::read(&fixture.home, &["gg"]).unwrap_err();
    assert_eq!(error.code(), "invalid_store");
    assert_eq!(
        error.to_string(),
        "Grida provider credentials: invalid_store. Check the credential file format with grida ai providers --help."
    );
    assert!(!format!("{error:?}").contains("synthetic"));
    assert!(start.elapsed() < Duration::from_secs(5));
}
