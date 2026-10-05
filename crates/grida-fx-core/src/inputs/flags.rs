//! Workflow input flags on the command line.
//!
//! Each top-level input of type integer, number, boolean, string or file gets `--<kebab name>`
//! (a value), `files` gets a repeatable flag; lists, maps and nested objects get none.
//! `--flag value` and `--flag=value` are accepted. FX decisions: no prefix abbreviation; integers
//! and numbers are read with FX's number rules (identity.md §1; `nan`, `inf`, `1_000` refused);
//! booleans `true|yes|1` / `false|no|0` in any case.
//! Errors (exit 2): `bad input flag in <rest joined by " ">: <detail>` for a bad value or a
//! missing value; `unknown input flag <x>; lists and maps of <target> come from --inputs` for
//! anything else, a stray positional included; with a builder target any flag is
//! `unknown flag <x>; a builder takes its arguments as --arg` (checked by the caller).
//!
//! As with gnode's parser, a bad value is reported even when an unknown flag came before it, a
//! repeated value flag keeps its last value, and the result lists inputs in property order. A
//! number is written as YAML writes a plain number (yaml.md "Values"): a decimal integer, or a
//! decimal with a fraction or an exponent, no leading zero; an integer literal that reading would
//! round is refused. An integer flag takes any whole number (`2.0` and `1e3` are integers).
//! The details read as argparse wrote them: `argument --count: invalid int value: 'x'`,
//! `argument --ratio: invalid float value: 'x'`, `argument --loud: maybe is not true or false`,
//! `argument --count: expected one argument`.

use super::FILE_TAG;
use crate::error::{Error, Result};
use indexmap::IndexMap;
use serde_json::Value;

/// The value a flag takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagType {
    Integer,
    Number,
    Boolean,
    /// A string, and a `file` input's path.
    String,
    /// A `files` input: repeatable, collected into a list.
    Files,
}

/// One flag of a workflow's inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFlag {
    pub name: String,
    /// `--max-entities`.
    pub flag: String,
    pub ty: FlagType,
}

/// The flags of compiled inputs, in property order.
pub fn input_flags(schema: &Value) -> Vec<InputFlag> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut flags = Vec::new();
    for (name, field) in properties {
        let kind = match field.get("type") {
            Some(Value::String(t)) => Some(t.as_str()),
            Some(Value::Array(types)) => types
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null"),
            _ => None,
        };
        let many = field
            .get(FILE_TAG)
            .and_then(|tag| tag.get("many"))
            .is_some_and(crate::docs::truthy);
        let ty = match kind {
            Some("integer") => FlagType::Integer,
            Some("number") => FlagType::Number,
            Some("boolean") => FlagType::Boolean,
            Some("string") => FlagType::String,
            _ if many => FlagType::Files,
            _ => continue,
        };
        flags.push(InputFlag {
            name: name.clone(),
            flag: super::flag_name(name),
            ty,
        });
    }
    flags
}

/// Parses the arguments the verb did not take. Values are JSON (paths stay strings; anchoring
/// happens in [`super::bind::load_inputs`]).
pub fn parse_input_flags(
    schema: &Value,
    rest: &[String],
    target: &str,
) -> Result<IndexMap<String, Value>> {
    if rest.is_empty() {
        return Ok(IndexMap::new());
    }
    let flags = input_flags(schema);
    let bad =
        |detail: String| Error::usage(format!("bad input flag in {}: {detail}", rest.join(" ")));
    let mut parsed: IndexMap<&str, Value> = IndexMap::new();
    let mut unknown: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        i += 1;
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if arg.starts_with("--") => (name, Some(value)),
            _ => (arg, None),
        };
        let found = arg
            .starts_with("--")
            .then(|| flags.iter().find(|f| f.flag == name))
            .flatten();
        let Some(flag) = found else {
            unknown.get_or_insert(arg);
            continue;
        };
        let text = match inline {
            Some(text) => text,
            None => match rest.get(i) {
                Some(next) if !looks_like_option(next) => {
                    i += 1;
                    next.as_str()
                }
                _ => {
                    return Err(bad(format!(
                        "argument {}: expected one argument",
                        flag.flag
                    )));
                }
            },
        };
        let value = match flag.ty {
            FlagType::Integer => number(text, true)
                .map_err(|detail| bad(format!("argument {}: {detail}", flag.flag)))?,
            FlagType::Number => number(text, false)
                .map_err(|detail| bad(format!("argument {}: {detail}", flag.flag)))?,
            FlagType::Boolean => match text.to_lowercase().as_str() {
                "true" | "yes" | "1" => Value::Bool(true),
                "false" | "no" | "0" => Value::Bool(false),
                _ => {
                    return Err(bad(format!(
                        "argument {}: {text} is not true or false",
                        flag.flag
                    )));
                }
            },
            FlagType::String => Value::String(text.to_string()),
            FlagType::Files => {
                let entry = parsed
                    .entry(flag.name.as_str())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(items) = entry {
                    items.push(Value::String(text.to_string()));
                }
                continue;
            }
        };
        parsed.insert(flag.name.as_str(), value);
    }
    if let Some(arg) = unknown {
        return Err(Error::usage(format!(
            "unknown input flag {arg}; lists and maps of {target} come from --inputs"
        )));
    }
    // Property order, as the parser's namespace lists them.
    let mut out = IndexMap::new();
    for flag in &flags {
        if let Some(value) = parsed.shift_remove(flag.name.as_str()) {
            out.insert(flag.name.clone(), value);
        }
    }
    Ok(out)
}

/// Whether an argument reads as an option rather than a value (argparse: it starts with `-`, is
/// longer than `-`, and is not a negative number).
fn looks_like_option(arg: &str) -> bool {
    arg.starts_with('-') && arg != "-" && !negative_number(arg)
}

fn negative_number(arg: &str) -> bool {
    let Some(digits) = arg.strip_prefix('-') else {
        return false;
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match fraction {
        None => !whole.is_empty() && all_digits(whole),
        Some(fraction) => all_digits(whole) && !fraction.is_empty() && all_digits(fraction),
    }
}

/// A flag's number, by FX's rules. `whole` asks for an integer.
fn number(text: &str, whole: bool) -> std::result::Result<Value, String> {
    let invalid = || {
        format!(
            "invalid {} value: {}",
            if whole { "int" } else { "float" },
            crate::text::py_repr_str(text)
        )
    };
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let (integer, fraction) = match mantissa.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (mantissa, None),
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = digits(integer)
        && fraction.is_none_or(digits)
        && !(integer.is_empty() && fraction.is_none_or(str::is_empty))
        && !(integer.len() > 1 && integer.starts_with('0'));
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    if !mantissa_ok || !exponent_ok {
        return Err(invalid());
    }
    let value = if fraction.is_none() && exponent.is_none() {
        crate::value::integer_literal(text).map_err(|refused| refused.message)?
    } else {
        let x: f64 = text.parse().map_err(|_| invalid())?;
        crate::value::number(x).map_err(|_| invalid())?
    };
    if whole
        && !value
            .as_number()
            .is_some_and(|n| crate::value::as_f64(n).fract() == 0.0)
    {
        return Err(invalid());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_entities": {"type": "integer"},
                "ratio": {"default": null, "type": ["number", "null"]},
                "loud": {"type": "boolean"},
                "name": {"type": "string"},
                "brief": {"type": "string", "x-fx-file": {"kind": "text/plain"}},
                "shots": {"type": "array", "items": {"type": "string"},
                          "x-fx-file": {"kind": "image", "many": true, "glob": false}},
                "tags": {"type": "array", "items": {"type": "string"}},
                "style": {"type": "object", "properties": {}},
            },
        })
    }

    fn args(text: &str) -> Vec<String> {
        text.split(' ').map(String::from).collect()
    }

    fn parse(text: &str) -> Result<IndexMap<String, Value>> {
        parse_input_flags(&schema(), &args(text), "flags")
    }

    #[test]
    fn flags_follow_the_property_order() {
        let flags = input_flags(&schema());
        let names: Vec<(&str, FlagType)> = flags.iter().map(|f| (f.flag.as_str(), f.ty)).collect();
        assert_eq!(
            names,
            [
                ("--max-entities", FlagType::Integer),
                ("--ratio", FlagType::Number),
                ("--loud", FlagType::Boolean),
                ("--name", FlagType::String),
                ("--brief", FlagType::String),
                ("--shots", FlagType::Files),
            ]
        );
    }

    #[test]
    fn values_both_ways_repeats_and_order() {
        let parsed =
            parse("--shots a.png --loud YES --name=Ada --max-entities 3 --shots=b.png --name Bo")
                .unwrap();
        assert_eq!(
            Value::Object(parsed.into_iter().collect()),
            json!({"max_entities": 3, "loud": true, "name": "Bo", "shots": ["a.png", "b.png"]})
        );
        assert_eq!(parse("--ratio -0.5").unwrap()["ratio"], json!(-0.5));
        assert_eq!(
            parse("--max-entities -3").unwrap()["max_entities"],
            json!(-3)
        );
        assert_eq!(
            parse("--max-entities 2.0").unwrap()["max_entities"],
            json!(2)
        );
        assert_eq!(
            parse("--max-entities 1e3").unwrap()["max_entities"],
            json!(1000)
        );
        assert_eq!(parse("--name -").unwrap()["name"], json!("-"));
        assert_eq!(parse("--name=").unwrap()["name"], json!(""));
        assert_eq!(parse("--loud 0").unwrap()["loud"], json!(false));
        assert!(
            parse_input_flags(&schema(), &[], "flags")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn bad_values() {
        let message = |text: &str| parse(text).unwrap_err().message;
        assert_eq!(
            message("--loud maybe"),
            "bad input flag in --loud maybe: argument --loud: maybe is not true or false"
        );
        assert_eq!(
            message("--max-entities"),
            "bad input flag in --max-entities: argument --max-entities: expected one argument"
        );
        assert_eq!(
            message("--name --loud yes"),
            "bad input flag in --name --loud yes: argument --name: expected one argument"
        );
        for refused in [
            "nan", "inf", "1_000", "017", "+", "1e", ".", "0x10", " 3", "1.5",
        ] {
            let error =
                parse_input_flags(&schema(), &["--max-entities".into(), refused.into()], "f")
                    .unwrap_err();
            assert_eq!(error.kind, crate::ErrorKind::Usage);
            assert!(
                error
                    .message
                    .starts_with(&format!("bad input flag in --max-entities {refused}: ")),
                "{refused}: {}",
                error.message
            );
        }
        assert!(
            message("--max-entities 12345678901234567891")
                .contains("is beyond the integers a number holds exactly"),
            "{}",
            message("--max-entities 12345678901234567891")
        );
        // A bad value wins over an unknown flag before it, as argparse reports it.
        assert!(message("--tags a --loud maybe").starts_with("bad input flag in "));
    }

    #[test]
    fn unknown_flags() {
        let message = |text: &str| parse(text).unwrap_err().message;
        assert_eq!(
            message("--tags a"),
            "unknown input flag --tags; lists and maps of flags come from --inputs"
        );
        assert_eq!(
            message("extra"),
            "unknown input flag extra; lists and maps of flags come from --inputs"
        );
        assert_eq!(
            message("--style=x --name a"),
            "unknown input flag --style=x; lists and maps of flags come from --inputs"
        );
        // No prefix abbreviation.
        assert_eq!(
            message("--nam Ada"),
            "unknown input flag --nam; lists and maps of flags come from --inputs"
        );
        assert_eq!(
            message("-n Ada"),
            "unknown input flag -n; lists and maps of flags come from --inputs"
        );
    }

    #[test]
    fn numbers_follow_fx_rules() {
        assert_eq!(number("0.5", false), Ok(json!(0.5)));
        assert_eq!(number(".5", false), Ok(json!(0.5)));
        assert_eq!(number("5.", false), Ok(json!(5)));
        assert_eq!(number("-0", true), Ok(json!(0)));
        assert_eq!(number("+3", true), Ok(json!(3)));
        assert_eq!(number("1E2", false), Ok(json!(100)));
        assert!(number("1e400", false).is_err());
        assert!(number("01.5", false).is_err());
        assert!(number("1.5", true).is_err());
    }
}
