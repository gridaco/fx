//! The YAML vectors of `spec/vectors/yaml/` (see its README): every `accept/<name>.yaml` loads
//! to the value of `accept/<name>.json`, compared by canonical JSON; every `refuse/<name>.yaml`
//! is refused, for the rule its `refuse/<name>.txt` names. The files are read as raw bytes.

use grida_fx_core::value::{canon, parse_json};
use grida_fx_core::yaml::{self, YamlError};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn vectors(kind: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors/yaml")
        .join(kind)
}

/// The `.yaml` files of a vector folder, sorted, with their names.
fn cases(kind: &str) -> Vec<(String, PathBuf)> {
    let mut cases: Vec<_> = std::fs::read_dir(vectors(kind))
        .unwrap_or_else(|error| panic!("{kind} vectors: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, path)
        })
        .collect();
    cases.sort();
    cases
}

/// The `rule` values that implement the yaml.md rule a refuse vector's `.txt` names.
fn rules_for(reason: &str) -> &'static [&'static str] {
    let table: &[(&str, &[&str])] = &[
        ("Encoding: a stream is UTF-8", &["invalid_utf8"]),
        (
            "Encoding: one leading byte-order mark",
            &["byte_order_mark"],
        ),
        ("Line breaks:", &["line_break"]),
        ("One document:", &["multiple_documents"]),
        ("Not supported: anchors and aliases", &["anchor", "alias"]),
        ("Not supported: merge keys", &["merge_key"]),
        ("Not supported: tags", &["tag"]),
        ("Not supported: directives", &["directive"]),
        ("Not supported: complex keys", &["complex_key"]),
        (
            "Not supported: a key that is itself a collection",
            &["collection_key"],
        ),
        ("Mapping keys: two equal keys", &["duplicate_key"]),
        ("Values: an ambiguous plain scalar", &["ambiguous_scalar"]),
        (
            "Values: a decimal integer is refused",
            &["integer_out_of_range"],
        ),
        (
            "Values: a decimal float that overflows",
            &["number_overflow"],
        ),
        ("Nesting:", &["invalid_yaml"]),
    ];
    table
        .iter()
        .find(|(prefix, _)| reason.starts_with(prefix))
        .map(|(_, rules)| *rules)
        .unwrap_or_else(|| panic!("no rule is known for the reason {reason:?}"))
}

#[test]
fn accept_vectors_load_to_their_values() {
    let cases = cases("accept");
    assert!(cases.len() >= 40, "only {} accept vectors", cases.len());
    let mut failures = Vec::new();
    for (name, path) in &cases {
        let bytes = std::fs::read(path).unwrap();
        let expected_text = std::fs::read_to_string(path.with_extension("json"))
            .unwrap_or_else(|error| panic!("{name}.json: {error}"));
        let expected: Value = parse_json(&expected_text)
            .unwrap_or_else(|error| panic!("{name}.json: {}", error.message));
        match yaml::load(&bytes, &format!("{name}.yaml")) {
            Ok(value) if canon(&value) == canon(&expected) => {}
            Ok(value) => failures.push(format!(
                "{name}: loaded {} but expected {}",
                canon(&value),
                canon(&expected)
            )),
            Err(error) => failures.push(format!("{name}: refused: {error}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn refuse_vectors_are_refused_for_their_rule() {
    let cases = cases("refuse");
    assert!(cases.len() >= 80, "only {} refuse vectors", cases.len());
    let mut failures = Vec::new();
    for (name, path) in &cases {
        let bytes = std::fs::read(path).unwrap();
        let reason = std::fs::read_to_string(path.with_extension("txt"))
            .unwrap_or_else(|error| panic!("{name}.txt: {error}"));
        let rules = rules_for(reason.trim());
        let label = format!("refuse/{name}.yaml");
        match yaml::load(&bytes, &label) {
            Ok(value) => failures.push(format!("{name}: loaded {}", canon(&value))),
            Err(YamlError { rule, .. }) if !rules.contains(&rule) => failures.push(format!(
                "{name}: refused for {rule}, expected one of {rules:?}"
            )),
            Err(error) => {
                // Every message names the file; an ambiguous scalar's says to quote it.
                let shown = error.to_string();
                if !shown.starts_with(&format!("{label}:")) {
                    failures.push(format!(
                        "{name}: the message does not name the file: {shown}"
                    ));
                }
                if error.rule == "ambiguous_scalar" && !shown.ends_with("; quote it") {
                    failures.push(format!("{name}: no \"quote it\": {shown}"));
                }
                // Every refused file but the encoding ones has a position.
                let whole_stream = ["invalid_utf8"].contains(&error.rule);
                if whole_stream != (error.line == 0) {
                    failures.push(format!("{name}: position {}:{}", error.line, error.column));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn refuse_vectors_point_at_the_offending_text() {
    // A sample of positions, 1-based line and column.
    for (name, line, column) in [
        ("alias", 1, 7),
        ("anchor_on_mapping", 1, 7),
        ("tag_core", 1, 7),
        ("tag_on_collection", 1, 8),
        ("complex_key", 1, 1),
        ("directive_yaml", 1, 1),
        ("duplicate_key", 2, 1),
        ("duplicate_key_flow", 1, 20),
        ("duplicate_key_nested_same_value", 3, 3),
        ("merge_key", 2, 3),
        ("key_flow_mapping", 1, 1),
        ("stream_second_document", 2, 1),
        ("stream_second_explicit_document", 3, 1),
        ("stream_bom_twice", 1, 1),
        ("ambiguous_on", 1, 10),
        ("ambiguous_in_block_sequence", 3, 5),
        ("ambiguous_in_flow_sequence", 1, 15),
        ("int_2p60_exact_digits", 2, 4),
        ("float_overflow", 1, 8),
        ("line_break_ls_plain", 1, 11),
        ("line_break_nel_comment", 1, 7),
        ("line_break_nel_double_quoted", 1, 12),
        ("line_break_ps_block", 2, 4),
        ("nesting_too_deep", 1, 1027),
    ] {
        let path = vectors("refuse").join(format!("{name}.yaml"));
        let error = yaml::load(&std::fs::read(&path).unwrap(), name).unwrap_err();
        assert_eq!(
            (error.line, error.column),
            (line, column),
            "{name}: {error}"
        );
    }
}

#[test]
fn accept_vectors_round_trip_through_the_writer() {
    for (name, path) in cases("accept") {
        let value = yaml::load(&std::fs::read(&path).unwrap(), &name).unwrap();
        let written = yaml::write_yaml(&value);
        let back = yaml::load(written.as_bytes(), &name)
            .unwrap_or_else(|error| panic!("{name}: {error}\n{written}"));
        assert_eq!(back, value, "{name}:\n{written}");
    }
}
