//! Separating a planning verb's own options from workflow input flags, and `--arg NAME=VALUE`
//! pairs.
//!
//! `split_plan_args` walks the arguments after the verb: the verb's own options (`--inputs`,
//! `--routes`, `--arg`, `--max-usd` with a value, as `--x v` or `--x=v`; `--check`, `--json`,
//! `--expect-cached`; `-h`/`--help`) and the first positional (the target) stay for clap;
//! everything else goes to `rest`, in order, for `grida_fx_core::inputs::flags`. No prefix
//! abbreviation (FX decision). An input whose flag equals a verb option is shadowed by it.
//!
//! `--check`, `--json` and `--expect-cached` are options of `plan` only, as in the predecessor:
//! after `expand`, `identity` or `price` they are workflow input flags like any other. `run` takes
//! the planning verbs' value options plus its own: `--yes-up-to`, `--deliver` and `--run` with a
//! value, and the flag `--live`; `plan`'s flags are input flags after `run`. Arguments of any
//! other verb, and of no verb, all stay for clap.

use grida_fx_core::Error;
use indexmap::IndexMap;
use std::ffi::OsString;

/// The verbs whose extra arguments are workflow input flags.
const PLANNING_VERBS: [&str; 5] = ["plan", "expand", "identity", "price", "run"];
/// Options of every planning verb that take a value.
const VALUE_OPTIONS: [&str; 4] = ["--inputs", "--routes", "--arg", "--max-usd"];
/// Flags of `plan` alone.
const PLAN_FLAGS: [&str; 3] = ["--check", "--json", "--expect-cached"];
/// Options of `run` alone that take a value.
const RUN_VALUE_OPTIONS: [&str; 3] = ["--yes-up-to", "--deliver", "--run"];
/// Flags of `run` alone.
const RUN_FLAGS: [&str; 1] = ["--live"];
/// Help, for every verb.
const HELP_FLAGS: [&str; 2] = ["-h", "--help"];

/// `(arguments for clap, rest)`; `argv` starts with the program name and the verb.
pub fn split_plan_args(argv: &[OsString]) -> (Vec<OsString>, Vec<String>) {
    let verb = argv.get(1).and_then(|v| v.to_str());
    let Some(verb) = verb.filter(|v| PLANNING_VERBS.contains(v)) else {
        return (argv.to_vec(), Vec::new());
    };
    let (own_values, own_flags): (&[&str], &[&str]) = match verb {
        "plan" => (&[], &PLAN_FLAGS),
        "run" => (&RUN_VALUE_OPTIONS, &RUN_FLAGS),
        _ => (&[], &[]),
    };
    let mut for_clap: Vec<OsString> = argv[..2].to_vec();
    let mut rest = Vec::new();
    let mut target_seen = false;
    let mut arguments = argv[2..].iter();
    while let Some(argument) = arguments.next() {
        let Some(text) = argument.to_str() else {
            // Not text: clap refuses it with its own message.
            for_clap.push(argument.clone());
            continue;
        };
        let name = text.split_once('=').map_or(text, |(name, _)| name);
        if VALUE_OPTIONS.contains(&name) || own_values.contains(&name) {
            for_clap.push(argument.clone());
            if name == text {
                // `--x v`: the value is the next argument, whatever it looks like.
                if let Some(value) = arguments.next() {
                    for_clap.push(value.clone());
                }
            }
        } else if own_flags.contains(&text) || HELP_FLAGS.contains(&text) {
            for_clap.push(argument.clone());
        } else if !target_seen && !text.starts_with('-') {
            target_seen = true;
            for_clap.push(argument.clone());
        } else {
            rest.push(text.to_string());
        }
    }
    (for_clap, rest)
}

/// `--arg` pairs: split at the first `=`; no `=` or an empty name is
/// `--arg <pair>: write NAME=VALUE` (exit 2); a repeated name keeps the last value.
pub fn parse_arguments(pairs: &[String]) -> Result<IndexMap<String, String>, Error> {
    let mut arguments = IndexMap::new();
    for pair in pairs {
        match pair.split_once('=') {
            Some((name, value)) if !name.is_empty() => {
                arguments.insert(name.to_string(), value.to_string());
            }
            _ => return Err(Error::usage(format!("--arg {pair}: write NAME=VALUE"))),
        }
    }
    Ok(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(args: &[&str]) -> (Vec<String>, Vec<String>) {
        let argv: Vec<OsString> = std::iter::once("grida-fx")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect();
        let (for_clap, rest) = split_plan_args(&argv);
        let for_clap = for_clap
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        (for_clap, rest)
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn verb_options_stay_and_input_flags_go() {
        let (for_clap, rest) = split(&[
            "plan",
            "case",
            "--routes",
            "routes.yaml",
            "--name",
            "Ada",
            "--inputs=inputs.yaml",
            "--check",
            "--loud=yes",
            "--max-usd",
            "1.5",
            "--json",
            "--expect-cached",
            "--arg",
            "n=3",
        ]);
        assert_eq!(
            for_clap,
            strings(&[
                "grida-fx",
                "plan",
                "case",
                "--routes",
                "routes.yaml",
                "--inputs=inputs.yaml",
                "--check",
                "--max-usd",
                "1.5",
                "--json",
                "--expect-cached",
                "--arg",
                "n=3",
            ])
        );
        assert_eq!(rest, strings(&["--name", "Ada", "--loud=yes"]));
    }

    #[test]
    fn plan_flags_belong_to_plan_alone() {
        let (for_clap, rest) = split(&["expand", "case", "--check", "--json", "--max-usd=2"]);
        assert_eq!(
            for_clap,
            strings(&["grida-fx", "expand", "case", "--max-usd=2"])
        );
        assert_eq!(rest, strings(&["--check", "--json"]));
        let (for_clap, rest) = split(&["identity", "case", "--expect-cached"]);
        assert_eq!(for_clap, strings(&["grida-fx", "identity", "case"]));
        assert_eq!(rest, strings(&["--expect-cached"]));
    }

    #[test]
    fn no_prefix_abbreviation() {
        let (for_clap, rest) = split(&["plan", "case", "--max", "5", "--rout", "r.yaml"]);
        assert_eq!(for_clap, strings(&["grida-fx", "plan", "case"]));
        assert_eq!(rest, strings(&["--max", "5", "--rout", "r.yaml"]));
    }

    #[test]
    fn the_first_positional_is_the_target_and_later_ones_are_rest() {
        let (for_clap, rest) = split(&["price", "--inputs", "-", "case", "extra"]);
        assert_eq!(
            for_clap,
            strings(&["grida-fx", "price", "--inputs", "-", "case"])
        );
        assert_eq!(rest, strings(&["extra"]));
    }

    #[test]
    fn a_value_option_takes_the_next_argument_even_when_it_looks_like_a_flag() {
        let (for_clap, rest) = split(&["plan", "case", "--routes", "--weird.yaml"]);
        assert_eq!(
            for_clap,
            strings(&["grida-fx", "plan", "case", "--routes", "--weird.yaml"])
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn a_value_option_at_the_end_is_left_to_clap() {
        let (for_clap, rest) = split(&["plan", "case", "--max-usd"]);
        assert_eq!(
            for_clap,
            strings(&["grida-fx", "plan", "case", "--max-usd"])
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn help_stays_for_clap() {
        let (for_clap, rest) = split(&["expand", "-h", "--help"]);
        assert_eq!(for_clap, strings(&["grida-fx", "expand", "-h", "--help"]));
        assert!(rest.is_empty());
    }

    #[test]
    fn other_verbs_and_no_verb_go_to_clap_whole() {
        let (for_clap, rest) = split(&["lock", "--check", "--name", "x"]);
        assert_eq!(
            for_clap,
            strings(&["grida-fx", "lock", "--check", "--name", "x"])
        );
        assert!(rest.is_empty());
        let (for_clap, rest) = split(&["--version"]);
        assert_eq!(for_clap, strings(&["grida-fx", "--version"]));
        assert!(rest.is_empty());
        let (for_clap, rest) = split(&[]);
        assert_eq!(for_clap, strings(&["grida-fx"]));
        assert!(rest.is_empty());
    }

    #[test]
    fn run_keeps_its_own_options_and_gives_the_rest_to_the_workflow() {
        let (for_clap, rest) = split(&[
            "run",
            "case",
            "--name",
            "Ada",
            "--live",
            "--max-usd",
            "3",
            "--yes-up-to",
            "-1",
            "--deliver",
            "all=out/all.txt",
            "--deliver=each=out/{key}.txt",
            "--run",
            "runs/one",
            "--inputs=inputs.yaml",
            "--loud=yes",
        ]);
        assert_eq!(
            for_clap,
            strings(&[
                "grida-fx",
                "run",
                "case",
                "--live",
                "--max-usd",
                "3",
                "--yes-up-to",
                "-1",
                "--deliver",
                "all=out/all.txt",
                "--deliver=each=out/{key}.txt",
                "--run",
                "runs/one",
                "--inputs=inputs.yaml",
            ])
        );
        assert_eq!(rest, strings(&["--name", "Ada", "--loud=yes"]));
    }

    #[test]
    fn plan_flags_are_input_flags_after_run_and_run_options_after_plan() {
        let (for_clap, rest) = split(&["run", "case", "--check", "--json", "--live=yes"]);
        assert_eq!(for_clap, strings(&["grida-fx", "run", "case"]));
        assert_eq!(rest, strings(&["--check", "--json", "--live=yes"]));
        let (for_clap, rest) = split(&["plan", "case", "--live", "--run", "r", "--deliver", "x"]);
        assert_eq!(for_clap, strings(&["grida-fx", "plan", "case"]));
        assert_eq!(rest, strings(&["--live", "--run", "r", "--deliver", "x"]));
    }

    #[test]
    fn a_run_value_option_takes_the_next_argument_whatever_it_looks_like() {
        let (for_clap, rest) = split(&["run", "--run", "--odd", "case", "--deliver", "--x=y"]);
        assert_eq!(
            for_clap,
            strings(&[
                "grida-fx",
                "run",
                "--run",
                "--odd",
                "case",
                "--deliver",
                "--x=y"
            ])
        );
        assert!(rest.is_empty());
        let (for_clap, rest) = split(&["run", "case", "--yes-up-to"]);
        assert_eq!(
            for_clap,
            strings(&["grida-fx", "run", "case", "--yes-up-to"])
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn the_other_run_verbs_go_to_clap_whole() {
        for argv in [
            &["reroll", "runs/one", "draw", "--live"][..],
            &["pick", "runs/one", "draw", "-1"],
            &["takes", "list", "case", "--x"],
            &["jobs", "--forget", "k"],
            &["project", "runs/one"],
            &["inspect", "case", "--verify", "--json"],
        ] {
            let (for_clap, rest) = split(argv);
            let mut expected = vec!["grida-fx"];
            expected.extend_from_slice(argv);
            assert_eq!(for_clap, strings(&expected));
            assert!(rest.is_empty(), "{argv:?}");
        }
    }

    #[test]
    fn arguments_split_at_the_first_equals_sign() {
        let pairs = strings(&["name=Ada", "expr=a=b", "empty=", "name=Bo"]);
        let arguments = parse_arguments(&pairs).unwrap();
        assert_eq!(
            arguments.into_iter().collect::<Vec<_>>(),
            vec![
                ("name".to_string(), "Bo".to_string()),
                ("expr".to_string(), "a=b".to_string()),
                ("empty".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn an_argument_needs_a_name_and_an_equals_sign() {
        for pair in ["novalue", "=x"] {
            let error = parse_arguments(&strings(&[pair])).unwrap_err();
            assert_eq!(error.kind, grida_fx_core::ErrorKind::Usage);
            assert_eq!(error.message, format!("--arg {pair}: write NAME=VALUE"));
        }
    }
}
