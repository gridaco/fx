//! `nodes [TYPE]`: the built-ins sorted by name, each `format!("{uses:34} {kind}")`; with TYPE
//! (a name or `fx/<name>@<major>`) only that one, then `  input   <name>: <port>`,
//! `  setting <name>: <schema as JSON, keys sorted, ", "/": ">`, `  output  <name>: <port>`,
//! `  about   <description>` (FX addition, first line only) and for a capability type
//! `  routes  <ids serving it, sorted, ", " or "none installed">` from the project's catalog
//! (built-in table + fx.yaml `route_tables`). An unknown TYPE prints nothing.
//!
//! A setting's schema is written on one line as the predecessor wrote it (Python's
//! `json.dumps(schema, sort_keys=True)`): keys sorted, `", "` and `": "` between items, text
//! outside ASCII escaped as `\uXXXX`, numbers in their JCS form (`0`, not `0.0`). The route
//! catalog is read only when a capability type's `routes` line needs it.

use crate::cli::NodesArgs;
use crate::print::print_line;
use grida_fx_core::Error;
use grida_fx_core::builtins::{BuiltinType, builtins};
use grida_fx_core::docs::project::Project;
use grida_fx_core::routes::{RouteTable, load_catalog};
use serde_json::Value;

pub fn run(args: &NodesArgs) -> Result<u8, Error> {
    let mut sorted: Vec<&BuiltinType> = builtins().iter().collect();
    sorted.sort_by(|a, b| (&a.name, a.major).cmp(&(&b.name, b.major)));
    let mut catalog: Option<RouteTable> = None;
    for builtin in sorted {
        if let Some(wanted) = &args.type_
            && wanted != &builtin.name
            && wanted != &builtin.uses
        {
            continue;
        }
        let spec = &builtin.spec;
        print_line(&format!("{:34} {}", builtin.uses, spec.kind_word()));
        if args.type_.is_none() {
            continue;
        }
        for (name, port) in &spec.inputs {
            print_line(&format!("  input   {name}: {}", port.notation()));
        }
        for (name, schema) in &spec.params {
            print_line(&format!("  setting {name}: {}", inline_json(schema)));
        }
        for (name, port) in &spec.outputs {
            print_line(&format!("  output  {name}: {}", port.notation()));
        }
        if let Some(about) = spec
            .description
            .as_deref()
            .and_then(|d| d.lines().next())
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            print_line(&format!("  about   {about}"));
        }
        if let Some(capability) = &spec.capability {
            if catalog.is_none() {
                catalog = Some(project_catalog()?);
            }
            let served: Vec<String> = catalog
                .as_ref()
                .map(|table| table.routes_for(capability))
                .unwrap_or_default()
                .into_iter()
                .map(|route| route.id())
                .collect();
            let served = if served.is_empty() {
                "none installed".to_string()
            } else {
                served.join(", ")
            };
            print_line(&format!("  routes  {served}"));
        }
    }
    Ok(0)
}

/// The route catalog of the working directory's project: the built-in table and fx.yaml's
/// `route_tables`.
fn project_catalog() -> Result<RouteTable, Error> {
    let cwd = super::planning::working_directory()?;
    let project = Project::find(&cwd)?;
    load_catalog(&project, &[])
}

/// A JSON value on one line, as Python's `json.dumps(value, sort_keys=True)` writes it.
fn inline_json(value: &Value) -> String {
    let mut out = String::new();
    write_inline(&mut out, value);
    out
}

fn write_inline(out: &mut String, value: &Value) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_inline(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_ascii_string(out, key);
                out.push_str(": ");
                write_inline(out, item);
            }
            out.push('}');
        }
        Value::String(text) => write_ascii_string(out, text),
        other => out.push_str(&grida_fx_core::value::canon(other)),
    }
}

/// A JSON string with everything outside printable ASCII escaped (Python's `ensure_ascii`).
fn write_ascii_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_print_as_the_predecessor_printed_them() {
        assert_eq!(
            inline_json(&json!({"type": "string", "x-fx-template": true})),
            r#"{"type": "string", "x-fx-template": true}"#
        );
        assert_eq!(
            inline_json(&json!({"enum": ["opaque", "transparent", "auto"], "default": "auto"})),
            r#"{"default": "auto", "enum": ["opaque", "transparent", "auto"]}"#
        );
        assert_eq!(
            inline_json(&json!({"type": "object", "default": {}})),
            r#"{"default": {}, "type": "object"}"#
        );
        assert_eq!(
            inline_json(&json!({"type": "number", "default": 0})),
            r#"{"default": 0, "type": "number"}"#
        );
        assert_eq!(
            inline_json(&json!({"default": -16, "type": "number"})),
            r#"{"default": -16, "type": "number"}"#
        );
        assert_eq!(
            inline_json(&json!({"default": ["point", "points", "box"], "type": "array"})),
            r#"{"default": ["point", "points", "box"], "type": "array"}"#
        );
        assert_eq!(inline_json(&json!({"default": []})), r#"{"default": []}"#);
        assert_eq!(
            inline_json(&json!({"default": 0.5, "x": null})),
            r#"{"default": 0.5, "x": null}"#
        );
    }

    #[test]
    fn text_outside_ascii_is_escaped() {
        assert_eq!(
            inline_json(&json!({"default": "\u{e9} \"q\" \\ \n \u{1f600} \u{1}"})),
            "{\"default\": \"\\u00e9 \\\"q\\\" \\\\ \\n \\ud83d\\ude00 \\u0001\"}"
        );
    }
}
