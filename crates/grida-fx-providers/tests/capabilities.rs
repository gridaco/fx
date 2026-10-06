//! The engine's capability table (`capabilities::CAPABILITIES`) agrees with spec/capabilities.md
//! and with the standard library's paid built-ins (spec/capabilities.md §14).

use grida_fx_core::builtins::{BuiltinType, builtins};
use grida_fx_core::kinds::family;
use grida_fx_core::spec::{BodyKind, Port, Shape as PortShape};
use grida_fx_providers::capabilities::{
    CAPABILITIES, Capability, MemberType, Shape, capability, check_request,
};
use grida_fx_providers::live::adapters;
use grida_fx_providers::testing::{setup, test_keys};
use grida_fx_providers::transport::replay::NoNetwork;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The capabilities the standard library declares but no adapter serves (spec/capabilities.md
/// §13).
const WITHOUT_ADAPTER: [&str; 3] = ["structured.review", "vision.annotate", "vision.review"];

/// Each capability's section of spec/capabilities.md. Adapters cite these numbers.
const SECTIONS: [(u32, &str); 11] = [
    (2, "image.generate"),
    (3, "image.edit"),
    (4, "structured.generate"),
    (5, "agent.turn"),
    (6, "video.generate"),
    (7, "mesh.generate"),
    (8, "mesh.rig"),
    (9, "music.generate"),
    (10, "sound.generate"),
    (11, "speech.generate"),
    (12, "background.remove"),
];

const ALL_TYPES: [MemberType; 10] = [
    MemberType::Text,
    MemberType::Number,
    MemberType::Integer,
    MemberType::Boolean,
    MemberType::Object,
    MemberType::List,
    MemberType::File,
    MemberType::Files,
    MemberType::FilesByName,
    MemberType::FileOrObject,
];

// --- the standard library ----------------------------------------------------------------------

/// What a paid built-in sends for one param or input port.
#[derive(Debug)]
struct Sent {
    name: String,
    ty: Result<MemberType, String>,
    required: bool,
    /// The param's `default`, if any.
    default: Option<Value>,
}

/// The member type a param's JSON Schema admits: `string` or a string `enum` is text, and
/// `number`, `integer` and `boolean` are themselves.
fn param_type(schema: &Value) -> Result<MemberType, String> {
    if let Some(values) = schema.get("enum") {
        return match values.as_array() {
            Some(values) if !values.is_empty() && values.iter().all(Value::is_string) => {
                Ok(MemberType::Text)
            }
            _ => Err(format!("an enum of {values} is no member type")),
        };
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Ok(MemberType::Text),
        Some("number") => Ok(MemberType::Number),
        Some("integer") => Ok(MemberType::Integer),
        Some("boolean") => Ok(MemberType::Boolean),
        other => Err(format!("a param of type {other:?} is no member type")),
    }
}

/// The member type a port's value has: one file (a `json` one may also be the object inline), a
/// list of files, or files by name.
fn port_type(port: &Port) -> MemberType {
    match port.shape {
        PortShape::One if family(&port.kind) == "json" => MemberType::FileOrObject,
        PortShape::One => MemberType::File,
        PortShape::List => MemberType::Files,
        PortShape::Keyed => MemberType::FilesByName,
    }
}

/// Every param except `vars`, then every input port, in the built-in's order.
fn sent_by(builtin: &BuiltinType) -> Vec<Sent> {
    let params = builtin
        .spec
        .params
        .iter()
        .filter(|(name, _)| name.as_str() != "vars")
        .map(|(name, schema)| Sent {
            name: name.clone(),
            ty: param_type(schema),
            required: schema.get("default").is_none()
                && schema.get("x-fx-optional") != Some(&Value::Bool(true)),
            default: schema.get("default").cloned(),
        });
    let ports = builtin.spec.inputs.iter().map(|(name, port)| Sent {
        name: name.clone(),
        ty: Ok(port_type(port)),
        required: !port.optional,
        default: None,
    });
    params.chain(ports).collect()
}

fn paid_builtins() -> Vec<(&'static BuiltinType, &'static str)> {
    builtins()
        .iter()
        .filter(|b| b.body == BodyKind::Capability)
        .map(|b| {
            let name = b
                .spec
                .capability
                .as_deref()
                .unwrap_or_else(|| panic!("{} has a capability body", b.uses));
            (b, name)
        })
        .collect()
}

/// A value of a member type, for requests built the way a built-in builds them.
fn sample(ty: MemberType) -> Value {
    let file = json!({"file": "a".repeat(64)});
    match ty {
        MemberType::Text => json!("text"),
        MemberType::Number => json!(1.5),
        MemberType::Integer => json!(2),
        MemberType::Boolean => json!(true),
        MemberType::Object => json!({}),
        MemberType::List => json!([]),
        MemberType::File => file,
        MemberType::Files => json!([file]),
        MemberType::FilesByName => json!({"front": file}),
        MemberType::FileOrObject => json!({"type": "object"}),
    }
}

#[test]
fn every_paid_builtin_sends_exactly_its_capability_s_members() {
    let mut checked = 0;
    for (builtin, name) in paid_builtins() {
        if WITHOUT_ADAPTER.contains(&name) {
            continue;
        }
        let capability = capability(name)
            .unwrap_or_else(|| panic!("{}: {name} is not in CAPABILITIES", builtin.uses));
        let sent = sent_by(builtin);
        let sent_names: Vec<&str> = sent.iter().map(|s| s.name.as_str()).collect();
        let members: Vec<&str> = capability.members.iter().map(|m| m.name).collect();
        assert_eq!(
            sent_names, members,
            "{}: the members, in order",
            builtin.uses
        );
        for (sent, member) in sent.iter().zip(capability.members) {
            let ty = sent
                .ty
                .as_ref()
                .unwrap_or_else(|reason| panic!("{} {}: {reason}", builtin.uses, sent.name));
            assert_eq!(*ty, member.ty, "{} {}: its type", builtin.uses, sent.name);
            assert_eq!(
                sent.required, member.required,
                "{} {}: required",
                builtin.uses, sent.name
            );
            if let Some(default) = &sent.default {
                assert!(
                    member.ty.admits(default),
                    "{} {}: its default {default} is not {}",
                    builtin.uses,
                    sent.name,
                    member.ty.word()
                );
            }
        }
        checked += 1;
    }
    assert_eq!(
        checked,
        CAPABILITIES.len() - 1,
        "every capability but agent.turn"
    );
}

#[test]
fn what_a_paid_builtin_sends_passes_the_capability_check() {
    for (builtin, name) in paid_builtins() {
        if WITHOUT_ADAPTER.contains(&name) {
            continue;
        }
        let sent = sent_by(builtin);
        // Only the required members: a param the step left out with no default, and an input
        // port with no value, are absent from the request (spec/capabilities.md §1, §14).
        let least: Map<String, Value> = sent
            .iter()
            .filter(|s| s.required)
            .map(|s| {
                let ty =
                    s.ty.as_ref()
                        .unwrap_or_else(|reason| panic!("{} {}: {reason}", builtin.uses, s.name));
                (s.name.clone(), sample(*ty))
            })
            .collect();
        assert_eq!(
            check_request(name, &Value::Object(least.clone())),
            Ok(()),
            "{}",
            builtin.uses
        );
        // A param whose value is missing or null goes as `null`, which counts as absent.
        let nulls: Map<String, Value> = sent
            .iter()
            .map(|s| {
                let value = match (&s.ty, s.required) {
                    (Ok(ty), true) => sample(*ty),
                    _ => Value::Null,
                };
                (s.name.clone(), value)
            })
            .collect();
        assert_eq!(
            check_request(name, &Value::Object(nulls)),
            Ok(()),
            "{}",
            builtin.uses
        );
        // Every member, defaults where the built-in has them.
        let most: Map<String, Value> = sent
            .iter()
            .map(|s| {
                let value = match (&s.default, &s.ty) {
                    (Some(default), _) => default.clone(),
                    (None, Ok(ty)) => sample(*ty),
                    (None, Err(reason)) => panic!("{} {}: {reason}", builtin.uses, s.name),
                };
                (s.name.clone(), value)
            })
            .collect();
        assert_eq!(
            check_request(name, &Value::Object(most)),
            Ok(()),
            "{}",
            builtin.uses
        );
        // Each required member, left out, is refused by name.
        for s in sent.iter().filter(|s| s.required) {
            let mut without = least.clone();
            without.remove(&s.name);
            assert_eq!(
                check_request(name, &Value::Object(without)),
                Err(format!("{name} needs {}", s.name)),
                "{}",
                builtin.uses
            );
        }
    }
}

#[test]
fn every_output_of_a_paid_builtin_takes_a_file_of_its_answer() {
    for (builtin, name) in paid_builtins() {
        let Some(capability) = capability(name) else {
            continue;
        };
        let mut taken = BTreeSet::new();
        for (port_name, port) in &builtin.spec.outputs {
            if family(&port.kind) == "json" {
                assert!(
                    capability.files.is_empty(),
                    "{}: {port_name} takes data.json, and the answer has no files",
                    builtin.uses
                );
                continue;
            }
            let (file, kind) = capability
                .files
                .iter()
                .find(|(file, _)| file == port_name)
                .unwrap_or_else(|| panic!("{}: no answer file {port_name}", builtin.uses));
            let agrees = if port.kind.contains('/') {
                port.kind == *kind
            } else {
                family(kind) == port.kind
            };
            assert!(
                agrees,
                "{} {port_name}: {} and {kind}",
                builtin.uses, port.kind
            );
            taken.insert(*file);
        }
        let files: BTreeSet<&str> = capability.files.iter().map(|(file, _)| *file).collect();
        assert_eq!(
            taken, files,
            "{}: every answer file is an output",
            builtin.uses
        );
    }
}

#[test]
fn the_judge_capabilities_are_exactly_the_ones_without_an_adapter() {
    let declared: BTreeSet<&str> = paid_builtins().into_iter().map(|(_, name)| name).collect();
    let shipped: BTreeSet<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
    let without: BTreeSet<&str> = declared.difference(&shipped).copied().collect();
    assert_eq!(without, WITHOUT_ADAPTER.into_iter().collect());
    // The engine builds agent.turn; every other capability FX ships has a built-in.
    let unsent: Vec<&str> = shipped.difference(&declared).copied().collect();
    assert_eq!(unsent, vec!["agent.turn"]);
    // An adapter serves every shipped capability, and none of the others.
    let registry = adapters(&setup(Arc::new(NoNetwork), test_keys()));
    let served: BTreeSet<String> = registry.served().into_iter().map(|(c, _)| c).collect();
    let served: BTreeSet<&str> = served.iter().map(String::as_str).collect();
    assert_eq!(served, shipped);
    for name in WITHOUT_ADAPTER {
        assert_eq!(
            check_request(name, &json!({})),
            Err(format!("{name} is not a capability FX ships"))
        );
    }
}

// --- spec/capabilities.md ----------------------------------------------------------------------

struct Section {
    number: u32,
    title: String,
    body: Vec<String>,
}

fn spec() -> String {
    let path = format!("{}/../../spec/capabilities.md", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The document's `## <n>. <title>` sections.
fn sections(text: &str) -> Vec<Section> {
    let heading = Regex::new(r"^## (\d+)\. (.+)$").unwrap();
    let mut sections: Vec<Section> = Vec::new();
    for line in text.lines() {
        if let Some(captures) = heading.captures(line) {
            sections.push(Section {
                number: captures[1].parse().unwrap(),
                title: captures[2].to_string(),
                body: Vec::new(),
            });
        } else if line.starts_with("## ") {
            panic!("an unnumbered section: {line}");
        } else if let Some(section) = sections.last_mut() {
            section.body.push(line.to_string());
        }
    }
    sections
}

/// The capability a section defines: its title is the capability's name in backticks.
fn section_capability(section: &Section) -> Option<&str> {
    let title = section.title.strip_prefix('`')?.strip_suffix('`')?;
    Some(title)
}

/// A table row's cells, with `\|` read as a literal `|`.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim().replace("\\|", "\u{0}");
    let inner = line
        .strip_prefix('|')
        .and_then(|l| l.strip_suffix('|'))
        .unwrap_or_else(|| panic!("not a table row: {line}"));
    inner
        .split('|')
        .map(|cell| cell.trim().replace('\u{0}', "|"))
        .collect()
}

/// The rows of the first table whose header starts with `header`, if any.
fn table(body: &[String], header: &str) -> Option<Vec<Vec<String>>> {
    let start = body.iter().position(|line| line.starts_with(header))?;
    let separator = body.get(start + 1).expect("a separator row");
    assert!(separator.starts_with("|---"), "{header}: {separator}");
    Some(
        body[start + 2..]
            .iter()
            .take_while(|line| line.starts_with('|'))
            .map(|line| cells(line))
            .collect(),
    )
}

/// `` `name` `` as `name`.
fn code(cell: &str) -> &str {
    cell.strip_prefix('`')
        .and_then(|c| c.strip_suffix('`'))
        .unwrap_or_else(|| panic!("{cell} is not a name in backticks"))
}

fn capability_sections() -> Vec<(Section, &'static Capability)> {
    sections(&spec())
        .into_iter()
        .filter_map(|section| {
            let name = section_capability(&section)?.to_string();
            let capability = capability(&name)
                .unwrap_or_else(|| panic!("§{} {name} is not in CAPABILITIES", section.number));
            Some((section, capability))
        })
        .collect()
}

#[test]
fn every_capability_has_its_numbered_section() {
    let found: Vec<(u32, &str)> = capability_sections()
        .iter()
        .map(|(section, capability)| (section.number, capability.name))
        .collect();
    assert_eq!(found, SECTIONS.to_vec());
    let names: BTreeSet<&str> = SECTIONS.iter().map(|(_, name)| *name).collect();
    let shipped: BTreeSet<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
    assert_eq!(names, shipped);
}

#[test]
fn every_member_table_is_the_capability_s_members() {
    for (section, capability) in capability_sections() {
        let rows = table(&section.body, "| Member | Type |")
            .unwrap_or_else(|| panic!("§{} has no member table", section.number));
        let found: Vec<(String, String, bool)> = rows
            .iter()
            .map(|row| {
                let required = match row[2].as_str() {
                    "required" => true,
                    "optional" => false,
                    other => panic!(
                        "§{}: {other} is neither required nor optional",
                        section.number
                    ),
                };
                (code(&row[0]).to_string(), row[1].clone(), required)
            })
            .collect();
        let expected: Vec<(String, String, bool)> = capability
            .members
            .iter()
            .map(|m| (m.name.to_string(), m.ty.notation().to_string(), m.required))
            .collect();
        assert_eq!(found, expected, "§{} {}", section.number, capability.name);
    }
}

#[test]
fn every_feature_list_is_the_capability_s_features() {
    for (section, capability) in capability_sections() {
        let found: Vec<String> = match table(&section.body, "| Feature | Meaning |") {
            Some(rows) => rows.iter().map(|row| code(&row[0]).to_string()).collect(),
            None => {
                assert!(
                    section.body.iter().any(|l| l == "**Features:** none."),
                    "§{} lists no features and does not say so",
                    section.number
                );
                Vec::new()
            }
        };
        let expected: Vec<String> = capability.features.iter().map(|f| f.to_string()).collect();
        assert_eq!(found, expected, "§{} {}", section.number, capability.name);
    }
}

/// The planner keeps its own copy of the feature vocabulary (core cannot import this crate):
/// `grida_fx_core::expand::bind::capability_features`, read from
/// `crates/grida-fx-core/src/expand/features.json`. The two copies never drift.
#[test]
fn the_planner_s_feature_vocabulary_is_this_table() {
    use grida_fx_core::expand::bind::{capability_features, renamed_feature};
    for capability in CAPABILITIES {
        let planner: Option<Vec<&str>> = capability_features(capability.name)
            .map(|features| features.iter().map(String::as_str).collect());
        assert_eq!(
            planner.as_deref(),
            Some(capability.features),
            "{}: features.json against capabilities::CAPABILITIES",
            capability.name
        );
    }
    // The planner knows no capability this table lacks: the judges' have no vocabulary (§13).
    for name in WITHOUT_ADAPTER {
        assert!(capability(name).is_none(), "{name}");
        assert_eq!(capability_features(name), None, "{name}");
    }
    let core: Value = serde_json::from_str(grida_fx_core::expand::bind::FEATURES_JSON).unwrap();
    let listed: BTreeSet<&str> = core["capabilities"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let ours: BTreeSet<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
    assert_eq!(listed, ours);
    // A renamed feature is FX's name for a feature of some capability, and no longer a feature.
    for (old, new) in core["renamed"].as_object().unwrap() {
        assert_eq!(renamed_feature(old), new.as_str());
        assert!(
            CAPABILITIES
                .iter()
                .all(|c| !c.features.contains(&old.as_str())),
            "{old} is still a feature"
        );
        assert!(
            CAPABILITIES
                .iter()
                .any(|c| c.features.contains(&new.as_str().unwrap())),
            "{new} is no feature"
        );
    }
}

#[test]
fn long_jobs_and_answer_files_are_as_the_sections_say() {
    for (section, capability) in capability_sections() {
        let text = section.body.join("\n");
        assert_eq!(
            text.contains("A long job."),
            capability.shape == Shape::LongJob,
            "§{} {}",
            section.number,
            capability.name
        );
        for (file, _) in capability.files {
            assert!(
                text.contains(&format!("the file `{file}`")),
                "§{} names no answer file {file}",
                section.number
            );
        }
        if capability.files.is_empty() {
            assert!(
                text.contains("**Answer:** no files."),
                "§{}",
                section.number
            );
        }
    }
}

#[test]
fn the_capabilities_without_an_adapter_are_listed() {
    let section = sections(&spec())
        .into_iter()
        .find(|s| s.number == 13)
        .expect("§13");
    assert_eq!(section.title, "Capabilities without an adapter");
    let name = Regex::new(r"`([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+)`").unwrap();
    let text = section.body.join("\n");
    let listed: BTreeSet<&str> = name
        .captures_iter(&text)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    assert_eq!(listed, WITHOUT_ADAPTER.into_iter().collect());
}

#[test]
fn the_type_words_are_the_refusals_words() {
    let text = spec();
    let line = text
        .lines()
        .find_map(|l| l.split_once("The type words are").map(|(_, words)| words))
        .expect("§1 lists the type words");
    let word = Regex::new(r"`([^`]+)`").unwrap();
    let listed: Vec<&str> = word
        .captures_iter(line)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    let words: Vec<&str> = ALL_TYPES.iter().map(|ty| ty.word()).collect();
    assert_eq!(listed, words);
    // And a refusal uses them: one member of each type a member has (no member is an object),
    // given a value of another type.
    let mut refused = 0;
    for ty in ALL_TYPES {
        let Some((capability, member)) = CAPABILITIES
            .iter()
            .find_map(|c| c.members.iter().find(|m| m.ty == ty).map(|m| (c, m)))
        else {
            continue;
        };
        refused += 1;
        let mut request: Map<String, Value> = capability
            .members
            .iter()
            .filter(|m| m.required)
            .map(|m| (m.name.to_string(), sample(m.ty)))
            .collect();
        let wrong = if ty == MemberType::Text {
            json!(1)
        } else {
            json!("x")
        };
        request.insert(member.name.to_string(), wrong);
        assert_eq!(
            check_request(capability.name, &Value::Object(request)),
            Err(format!("{} is {}", member.name, ty.word()))
        );
    }
    assert_eq!(refused, ALL_TYPES.len() - 1);
}
