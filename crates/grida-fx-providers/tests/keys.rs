//! The allowlisted key loader and the base-URL endpoints (spec/providers.md §3).
//!
//! Every `.env` here is written into a temporary directory from inline text; no test reads a real
//! `.env` or the process environment (the process is a closure over made-up pairs), and every
//! value is made up.

use grida_fx_providers::keys::{
    ALLOWED, DISABLE_DOTENV, Environment, KeyName, KeySource, Keys, parse_dotenv, read_dotenv,
};
use grida_fx_providers::setup::Endpoints;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A process environment of made-up pairs.
fn process(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    move |name: &str| map.get(name).cloned()
}

/// A temporary directory holding a `.env` with `text`.
fn dotenv(text: impl AsRef<[u8]>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join(".env");
    std::fs::write(&path, text).expect("the .env is written");
    (dir, path)
}

fn read(pairs: &[(&str, &str)], path: Option<&Path>) -> Result<Environment, String> {
    Environment::read(&process(pairs), path)
}

/// The refusal of a `.env` with `text`, checked to hold none of `values`.
fn refusal(text: &str, values: &[&str]) -> String {
    let (_dir, path) = dotenv(text);
    let error = read(&[], Some(&path)).expect_err("the .env is refused");
    for value in values {
        assert!(!error.contains(value), "{error:?} shows {value:?}");
    }
    error
}

// ---------------------------------------------------------------------------------------------
// Precedence and the allowlist

#[test]
fn only_allowlisted_values_are_loaded_from_the_dotenv() {
    let (_dir, path) = dotenv(
        "OPENAI_API_KEY=file-openai\n\
         OPENROUTER_API_KEY=file-openrouter\n\
         FAL_KEY='file-fal'\n\
         ELEVENLABS_API_KEY=file-elevenlabs\n\
         STAGE_GEN_IMAGE_MODEL=ignored/from-file\n\
         UNRELATED_SECRET=ignored-value\n\
         OPENAI_BASE_URL=https://proxy.example.test/v1\n",
    );
    let environment = read(&[], Some(&path)).expect("a valid .env");
    assert_eq!(environment.get("OPENAI_API_KEY"), Some("file-openai"));
    assert_eq!(
        environment.get("OPENROUTER_API_KEY"),
        Some("file-openrouter")
    );
    assert_eq!(environment.get("FAL_KEY"), Some("file-fal"));
    assert_eq!(
        environment.get("ELEVENLABS_API_KEY"),
        Some("file-elevenlabs")
    );
    assert_eq!(
        environment.get("OPENAI_BASE_URL"),
        Some("https://proxy.example.test/v1")
    );
    assert_eq!(environment.get("TRIPO_API_KEY"), None);
    assert_eq!(environment.get("STAGE_GEN_IMAGE_MODEL"), None);
    assert_eq!(environment.get("UNRELATED_SECRET"), None);
    assert_eq!(environment.source("FAL_KEY"), Some(KeySource::DotEnv));
    let shown = format!("{environment:?}");
    for value in [
        "file-openai",
        "file-openrouter",
        "file-fal",
        "file-elevenlabs",
        "ignored-value",
    ] {
        assert!(!shown.contains(value), "{shown}");
    }
    let keys = Keys::from_environment(&environment);
    assert_eq!(
        keys.present(),
        [
            KeyName::OpenAi,
            KeyName::OpenRouter,
            KeyName::Fal,
            KeyName::ElevenLabs
        ]
    );
    assert_eq!(keys.source(KeyName::OpenAi), Some(KeySource::DotEnv));
    assert!(!format!("{keys:?}").contains("file-openai"));
}

#[test]
fn a_process_value_takes_precedence_over_the_dotenv() {
    let (_dir, path) = dotenv(
        "OPENAI_API_KEY=file-openai\n\
         OPENROUTER_API_KEY=file-openrouter\n\
         FAL_KEY=file-fal\n\
         ELEVENLABS_API_KEY=file-elevenlabs\n",
    );
    let environment = read(
        &[
            ("OPENAI_API_KEY", " process-openai "),
            ("OPENROUTER_API_KEY", "process-openrouter"),
            // Blank counts as unset: the file's value is used.
            ("FAL_KEY", "   "),
            ("TRIPO_API_KEY", "process-tripo"),
            ("HOME", "/home/acme"),
        ],
        Some(&path),
    )
    .expect("a valid .env");
    assert_eq!(environment.get("OPENAI_API_KEY"), Some("process-openai"));
    assert_eq!(
        environment.source("OPENAI_API_KEY"),
        Some(KeySource::Environment)
    );
    assert_eq!(
        environment.get("OPENROUTER_API_KEY"),
        Some("process-openrouter")
    );
    assert_eq!(environment.get("FAL_KEY"), Some("file-fal"));
    assert_eq!(environment.source("FAL_KEY"), Some(KeySource::DotEnv));
    assert_eq!(environment.get("TRIPO_API_KEY"), Some("process-tripo"));
    assert_eq!(environment.get("HOME"), None);
    let shown = format!("{environment:?}");
    assert!(!shown.contains("process-openai"), "{shown}");
    assert!(!shown.contains("file-fal"), "{shown}");
}

#[test]
fn the_disable_switch_keeps_the_dotenv_unread() {
    // Malformed on purpose: a file that is read at all would be refused.
    let (_dir, path) = dotenv("OPENAI_API_KEY=file-openai\nFAL_KEY without-equals\n");
    let environment = read(&[(DISABLE_DOTENV, "1")], Some(&path)).expect("the file is off");
    assert_eq!(environment, Environment::default());
    let with_process = read(
        &[(DISABLE_DOTENV, "1"), ("FAL_KEY", "process-fal")],
        Some(&path),
    )
    .expect("the file is off");
    assert_eq!(with_process.get("FAL_KEY"), Some("process-fal"));
    assert_eq!(with_process.get("OPENAI_API_KEY"), None);
    // Any other value leaves the file on.
    assert!(read(&[(DISABLE_DOTENV, "0")], Some(&path)).is_err());
}

#[test]
fn no_dotenv_path_reads_the_process_only() {
    let environment = read(&[("FAL_KEY", "process-fal")], None).expect("no file");
    assert_eq!(environment.get("FAL_KEY"), Some("process-fal"));
    assert_eq!(environment.source("FAL_KEY"), Some(KeySource::Environment));
}

#[test]
fn a_missing_dotenv_is_fine() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join(".env");
    assert_eq!(read(&[], Some(&path)), Ok(Environment::default()));
    assert_eq!(read_dotenv(&path, &ALLOWED), Ok(BTreeMap::new()));
    // A path through a file that is not a directory is missing too.
    let (_other, file) = dotenv("FAL_KEY=x\n");
    assert_eq!(
        read_dotenv(&file.join(".env"), &ALLOWED),
        Ok(BTreeMap::new())
    );
}

// ---------------------------------------------------------------------------------------------
// Refusals (stage-gen's test_config_rejects_unsafe_allowlisted_dotenv_entries, and more)

#[test]
fn a_duplicate_key_is_refused() {
    assert_eq!(
        refusal(
            "OPENROUTER_API_KEY=first\nOPENROUTER_API_KEY=second\n",
            &["first", "second"]
        ),
        ".env contains duplicate key: OPENROUTER_API_KEY"
    );
}

#[test]
fn a_malformed_assignment_is_refused() {
    assert_eq!(
        refusal("FAL_KEY without-equals\n", &["without-equals"]),
        ".env contains malformed assignment for FAL_KEY on line 1"
    );
    assert_eq!(
        refusal("# comment\n\nexport  TRIPO_API_KEY\n", &[]),
        ".env contains malformed assignment for TRIPO_API_KEY on line 3"
    );
    assert_eq!(
        refusal("FAL_KEY-x: secret-dash\n", &["secret-dash"]),
        ".env contains malformed assignment for FAL_KEY on line 1"
    );
}

#[test]
fn an_unterminated_quote_is_refused() {
    assert_eq!(
        refusal("OPENROUTER_API_KEY=\"unterminated\n", &["unterminated"]),
        ".env contains malformed quoted value for OPENROUTER_API_KEY on line 1"
    );
    assert_eq!(
        refusal("FAL_KEY='unterminated\n", &["unterminated"]),
        ".env contains malformed quoted value for FAL_KEY on line 1"
    );
    assert_eq!(
        refusal("FAL_KEY=\"quoted-a\" # trailing\n", &["quoted-a"]),
        ".env contains malformed quoted value for FAL_KEY on line 1"
    );
    assert_eq!(
        refusal("\nFAL_KEY=ab\"cd\n", &["ab\"cd"]),
        ".env contains malformed quoted value for FAL_KEY on line 2",
        "an unquoted value holds no quote"
    );
    assert_eq!(
        refusal("FAL_KEY=it's\n", &["it's"]),
        ".env contains malformed quoted value for FAL_KEY on line 1"
    );
}

#[test]
fn a_json_escaped_line_feed_is_refused_as_unsafe() {
    assert_eq!(
        refusal("FAL_KEY=\"line\\nfeed\"\n", &["line", "feed"]),
        ".env contains an unsafe value for FAL_KEY"
    );
    assert_eq!(
        refusal("FAL_KEY=\"tab\\there\"\n", &["tab", "here"]),
        ".env contains an unsafe value for FAL_KEY"
    );
    assert_eq!(
        refusal("FAL_KEY='del\u{7f}x'\n", &["del"]),
        ".env contains an unsafe value for FAL_KEY"
    );
}

#[test]
fn a_blank_value_is_refused() {
    for text in [
        "FAL_KEY=\n",
        "FAL_KEY=   \n",
        "FAL_KEY=''\n",
        "FAL_KEY=\"\"\n",
    ] {
        assert_eq!(
            refusal(text, &[]),
            ".env contains an empty value for FAL_KEY",
            "{text:?}"
        );
    }
    // stage-gen's .env.example shape: blank keys are refused, not skipped.
    assert_eq!(
        refusal("OPENAI_API_KEY=\nFAL_KEY=x\n", &[]),
        ".env contains an empty value for OPENAI_API_KEY"
    );
}

#[test]
fn a_non_utf8_dotenv_is_refused() {
    let (_dir, path) = dotenv(b"FAL_KEY=\xff\xfe\n".as_slice());
    assert_eq!(
        read(&[], Some(&path)),
        Err(".env must be valid UTF-8".to_string())
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_dotenv_is_refused() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let target = dir.path().join("real.env");
    std::fs::write(&target, "FAL_KEY=linked-secret\n").expect("written");
    let link = dir.path().join(".env");
    std::os::unix::fs::symlink(&target, &link).expect("a symlink");
    assert_eq!(
        read(&[], Some(&link)),
        Err(".env must be a regular file".to_string())
    );
    // Dangling too.
    let dangling = dir.path().join("dangling.env");
    std::os::unix::fs::symlink(dir.path().join("nowhere"), &dangling).expect("a symlink");
    assert_eq!(
        read(&[], Some(&dangling)),
        Err(".env must be a regular file".to_string())
    );
}

#[test]
fn a_directory_is_not_a_dotenv() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join(".env");
    std::fs::create_dir(&path).expect("a directory");
    assert_eq!(
        read(&[], Some(&path)),
        Err(".env must be a regular file".to_string())
    );
}

// ---------------------------------------------------------------------------------------------
// The value forms

fn parsed(text: &str) -> BTreeMap<String, String> {
    parse_dotenv(text, &ALLOWED).expect("a valid .env")
}

#[test]
fn an_export_prefix_is_accepted() {
    let values = parsed("export FAL_KEY=exported\n  export\tOPENAI_API_KEY = spaced  \n");
    assert_eq!(values["FAL_KEY"], "exported");
    assert_eq!(values["OPENAI_API_KEY"], "spaced");
}

#[test]
fn comments_and_blank_lines_are_skipped() {
    let values = parsed(
        "# FAL_KEY=commented-out\n\
         \n\
         \x20  # indented comment\n\
         FAL_KEY=abc # trailing comment\n\
         OPENAI_API_KEY=abc#not-a-comment\n\
         TRIPO_API_KEY='a # kept'\n\
         ELEVENLABS_API_KEY=\"json \\u00e9 \\\"q\\\"\"\n",
    );
    assert_eq!(values["FAL_KEY"], "abc");
    assert_eq!(values["OPENAI_API_KEY"], "abc#not-a-comment");
    assert_eq!(values["TRIPO_API_KEY"], "a # kept");
    assert_eq!(values["ELEVENLABS_API_KEY"], "json é \"q\"");
    assert_eq!(values.len(), 4);
}

#[test]
fn lines_off_the_allowlist_are_never_parsed() {
    let values = parsed(
        "OTHER=\"unterminated\n\
         OTHER=\"unterminated\n\
         JUST_A_WORD\n\
         FAL_KEYS=not-the-key\n\
         OPENAI_API_KEY_2=nope\n\
         FAL_KEY=yes\n",
    );
    assert_eq!(values.len(), 1);
    assert_eq!(values["FAL_KEY"], "yes");
}

#[test]
fn every_line_break_counts_and_crlf_is_one() {
    let values = parsed("FAL_KEY=a\r\nOPENAI_API_KEY=b\rTRIPO_API_KEY=c\n");
    assert_eq!(values["FAL_KEY"], "a");
    assert_eq!(values["OPENAI_API_KEY"], "b");
    assert_eq!(values["TRIPO_API_KEY"], "c");
    assert_eq!(
        parse_dotenv("\r\n\r\nFAL_KEY nope\r\n", &ALLOWED),
        Err(".env contains malformed assignment for FAL_KEY on line 3".to_string())
    );
    assert_eq!(
        parse_dotenv("A=1\u{2028}FAL_KEY nope", &ALLOWED),
        Err(".env contains malformed assignment for FAL_KEY on line 2".to_string())
    );
}

#[test]
fn the_allowed_names_are_the_callers() {
    let values = parse_dotenv("FAL_KEY=a\nOPENAI_API_KEY=b\n", &["FAL_KEY"]).expect("valid");
    assert_eq!(values.len(), 1);
    assert_eq!(
        parse_dotenv("FAL_KEY nope\n", &[]),
        Ok(BTreeMap::new()),
        "nothing allowed, nothing refused"
    );
}

// ---------------------------------------------------------------------------------------------
// Endpoints (as stage-gen's endpoint tests hold them)

fn endpoints(pairs: &[(&str, &str)]) -> Result<Endpoints, String> {
    let environment = Environment::from_pairs(pairs);
    Endpoints::from_environment(&environment, &Keys::from_environment(&environment))
}

#[test]
fn defaults_hold_without_base_urls() {
    assert_eq!(endpoints(&[]), Ok(Endpoints::default()));
    assert_eq!(
        Endpoints::default(),
        Endpoints {
            openai: "https://api.openai.com/v1".into(),
            openrouter: "https://openrouter.ai/api/v1".into(),
            fal_run: "https://fal.run".into(),
            fal_queue: "https://queue.fal.run".into(),
            tripo: "https://openapi.tripo3d.ai/v3".into(),
            elevenlabs: "https://api.elevenlabs.io/v1".into(),
        }
    );
}

#[test]
fn a_base_url_refuses_cleartext_to_a_non_loopback_host() {
    for value in [
        "http://provider.example.test/v1",
        "http://192.0.2.1:8080/v1",
    ] {
        assert_eq!(
            endpoints(&[("OPENAI_BASE_URL", value)]),
            Err(
                "OPENAI_BASE_URL: OpenAI base_url must use HTTPS unless it targets a loopback host"
                    .to_string()
            ),
            "{value}"
        );
    }
}

#[test]
fn a_base_url_allows_cleartext_to_a_loopback_host() {
    for value in [
        "http://localhost:8765/v1/",
        "http://127.0.0.1:8765/v1/",
        "http://[::1]:8765/v1/",
    ] {
        let found = endpoints(&[("OPENAI_BASE_URL", value)]).expect("loopback is allowed");
        assert_eq!(found.openai, value.trim_end_matches('/'));
        let setup = grida_fx_providers::testing::setup(
            std::sync::Arc::new(grida_fx_providers::transport::replay::NoNetwork),
            Keys::none(),
        );
        let client = setup.client(KeyName::OpenAi, &found.openai);
        assert_eq!(
            client.url("images/generations"),
            format!("{}/images/generations", value.trim_end_matches('/'))
        );
    }
}

#[test]
fn a_base_url_refuses_credentials_a_query_or_a_fragment() {
    for value in [
        "https://user:password@provider.example.test/v1",
        "https://provider.example.test/v1?credential=secret",
        "https://provider.example.test/v1#secret",
    ] {
        let error = endpoints(&[("OPENROUTER_BASE_URL", value)]).unwrap_err();
        assert_eq!(
            error,
            "OPENROUTER_BASE_URL: OpenRouter base_url must be an HTTP(S) URL without credentials, \
             query, or fragment"
        );
        assert!(!error.contains("secret") && !error.contains("password"));
    }
}

#[test]
fn a_base_url_refuses_an_invalid_port() {
    for value in [
        "https://provider.example.test:bad/v1",
        "https://provider.example.test:99999/v1",
    ] {
        assert_eq!(
            endpoints(&[("ELEVENLABS_BASE_URL", value)]),
            Err("ELEVENLABS_BASE_URL: ElevenLabs base_url must use a valid network port".into()),
            "{value}"
        );
    }
}

#[test]
fn a_base_url_holding_a_key_is_refused_without_showing_either() {
    for value in [
        "https://proxy.example.test/made-up-openrouter-key/v1",
        "https://proxy.example.test/v1?key=made-up-openrouter-key",
    ] {
        let error = endpoints(&[
            ("OPENROUTER_API_KEY", "made-up-openrouter-key"),
            ("OPENROUTER_BASE_URL", value),
        ])
        .unwrap_err();
        assert_eq!(error, "OPENROUTER_BASE_URL holds a credential");
    }
    // Another provider's key counts too.
    assert_eq!(
        endpoints(&[
            ("FAL_KEY", "made-up-fal-key"),
            (
                "OPENAI_BASE_URL",
                "https://proxy.example.test/made-up-fal-key"
            ),
        ]),
        Err("OPENAI_BASE_URL holds a credential".to_string())
    );
}

#[test]
fn fal_base_url_moves_the_run_host_only() {
    let found = endpoints(&[("FAL_BASE_URL", " https://fal-proxy.example.test/ ")])
        .expect("a valid base URL");
    assert_eq!(found.fal_run, "https://fal-proxy.example.test");
    assert_eq!(found.fal_queue, "https://queue.fal.run");
    assert_eq!(
        endpoints(&[("FAL_BASE_URL", "http://fal-proxy.example.test")]),
        Err("FAL_BASE_URL: fal base_url must use HTTPS unless it targets a loopback host".into())
    );
}

#[test]
fn base_urls_come_from_the_dotenv_too() {
    let (_dir, path) = dotenv(
        "ELEVENLABS_BASE_URL=https://audio-proxy.example.test/v1/\n\
         ELEVENLABS_API_KEY=file-elevenlabs\n",
    );
    let environment = read(&[], Some(&path)).expect("a valid .env");
    let keys = Keys::from_environment(&environment);
    let found = Endpoints::from_environment(&environment, &keys).expect("valid");
    assert_eq!(found.elevenlabs, "https://audio-proxy.example.test/v1");
    assert_eq!(found.openai, Endpoints::default().openai);
}
