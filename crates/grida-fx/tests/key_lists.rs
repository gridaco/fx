//! The lists of provider variables kept outside the engine stay in step with its own
//! (spec/providers.md §1): the tools, examples and SDK tests that keep keys out of the runs they
//! start name exactly the keys of FX's built-in declarations, or every variable they read. An
//! example's check stands alone and the SDKs' tests start an engine, so none can ask the engine
//! for the list; this test holds each to it instead.

use grida_fx_core::providers::{Declarations, built_in};
use std::collections::BTreeSet;
use std::path::Path;

/// Which of the engine's lists a file keeps.
enum Kept {
    /// The built-in declarations' keys.
    Keys,
    /// Their keys and base-URL variables.
    Variables,
}

/// Every file that keeps a list, by its path from the repository root.
const FILES: [(&str, Kept); 17] = [
    ("tools/check_control.py", Kept::Keys),
    ("tools/check_examples.py", Kept::Keys),
    ("tools/check_host.py", Kept::Keys),
    ("tools/check_observation.py", Kept::Keys),
    ("tools/check_showcase_examples.py", Kept::Keys),
    ("tools/check_viewer.py", Kept::Keys),
    ("tools/check_viewer_fixtures.py", Kept::Keys),
    ("tools/package_release.py", Kept::Keys),
    ("tools/check_llms.py", Kept::Variables),
    ("tools/demo_viewer.py", Kept::Variables),
    ("examples/portrait-motion/check.py", Kept::Keys),
    ("examples/terrain/check_cover.py", Kept::Keys),
    ("examples/terrain/check_cover_motion.py", Kept::Keys),
    ("python/tests/test_record.py", Kept::Keys),
    ("python/tests/test_runs.py", Kept::Keys),
    ("python/tests/test_stand_in_engine.py", Kept::Keys),
    ("js/fx/test/record.test.ts", Kept::Keys),
];

/// The words in `text` that look like a provider's key or base-URL variable: upper case, and
/// ending in `_KEY` or `_BASE_URL`.
fn named(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| {
            word.starts_with(|c: char| c.is_ascii_uppercase())
                && word
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && (word.ends_with("_KEY") || word.ends_with("_BASE_URL"))
        })
        .map(String::from)
        .collect()
}

#[test]
fn every_list_of_provider_variables_is_the_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let keys: BTreeSet<String> = built_in()
        .iter()
        .filter_map(|d| d.credential.as_ref().map(|c| c.variable.clone()))
        .collect();
    let variables: BTreeSet<String> = Declarations::built_in().variables().into_iter().collect();
    for (path, kept) in FILES {
        let text = std::fs::read_to_string(root.join(path)).unwrap();
        let expected = match kept {
            Kept::Keys => &keys,
            Kept::Variables => &variables,
        };
        assert_eq!(&named(&text), expected, "{path}");
    }
}
