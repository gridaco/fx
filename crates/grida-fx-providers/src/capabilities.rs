//! The capabilities FX ships, as data (spec/capabilities.md): each one's canonical request members,
//! the features a route of it may declare, and what its answer carries. Adapters call
//! [`check_request`] first (spec/providers.md §5 step 2), so every route of a capability refuses
//! the same malformed request with the same sentence. `tests/capabilities.rs` holds this table to
//! spec/capabilities.md (every member table and feature list) and to the standard library's paid
//! built-ins; `tests/routes.rs` holds the built-in route table's features to [`Capability::features`].
//!
//! Members are listed in the order a paid built-in sends them: its params without `vars`, then its
//! input ports (spec/capabilities.md §14). `agent.turn`, which no built-in sends, lists them in
//! the order of spec/protocol.md §6.2.
//!
//! Rules [`check_request`] applies (spec/capabilities.md §1), in this order:
//! - the request is a JSON object: `a request for <capability> is a JSON object`;
//! - a member the capability does not define is refused: `<capability> takes no member <name>`;
//! - then each member in the table's order: a member whose value is `null` counts as absent (it
//!   stays in the call key); a required member that is absent: `<capability> needs <name>`; a
//!   present member of the wrong type: `<name> is <a type word>` (`prompt is text`, `duration is a
//!   number`, `references is a list of files`, `views is files by name`, `schema is a JSON file or
//!   object`).
//!
//! Value checks beyond the type (non-blank text, ranges, enums) are the adapter's, with the
//! sentences spec/providers.md §9 gives per provider.

use serde_json::Value;

/// What a member holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberType {
    Text,
    Number,
    /// A number with no fractional part.
    Integer,
    Boolean,
    Object,
    List,
    /// `{"file": digest}`.
    File,
    /// A list of file values.
    Files,
    /// An object of file values by name.
    FilesByName,
    /// A file value, or an inline JSON object.
    FileOrObject,
}

impl MemberType {
    /// How a refusal names the type.
    pub fn word(self) -> &'static str {
        match self {
            MemberType::Text => "text",
            MemberType::Number => "a number",
            MemberType::Integer => "a whole number",
            MemberType::Boolean => "true or false",
            MemberType::Object => "an object",
            MemberType::List => "a list",
            MemberType::File => "a file",
            MemberType::Files => "a list of files",
            MemberType::FilesByName => "files by name",
            MemberType::FileOrObject => "a JSON file or object",
        }
    }

    /// How spec/capabilities.md's member tables write the type.
    pub fn notation(self) -> &'static str {
        match self {
            MemberType::Text => "text",
            MemberType::Number => "number",
            MemberType::Integer => "integer",
            MemberType::Boolean => "boolean",
            MemberType::Object => "object",
            MemberType::List => "list",
            MemberType::File => "file",
            MemberType::Files => "file[]",
            MemberType::FilesByName => "file{}",
            MemberType::FileOrObject => "file | object",
        }
    }

    /// Whether `value` (not `null`) has this type.
    pub fn admits(self, value: &Value) -> bool {
        match self {
            MemberType::Text => value.is_string(),
            MemberType::Number => value.is_number(),
            MemberType::Integer => value
                .as_f64()
                .is_some_and(|x| x.fract() == 0.0 && x.is_finite()),
            MemberType::Boolean => value.is_boolean(),
            MemberType::Object => value.is_object(),
            MemberType::List => value.is_array(),
            MemberType::File => is_file_value(value),
            MemberType::Files => value
                .as_array()
                .is_some_and(|items| items.iter().all(is_file_value)),
            MemberType::FilesByName => value
                .as_object()
                .is_some_and(|items| items.values().all(is_file_value)),
            MemberType::FileOrObject => value.is_object(),
        }
    }
}

/// `{"file": "<digest>"}` and nothing else.
pub fn is_file_value(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == 1 && map.get("file").is_some_and(Value::is_string))
}

/// One request member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    pub name: &'static str,
    pub ty: MemberType,
    pub required: bool,
}

const fn req(name: &'static str, ty: MemberType) -> Member {
    Member {
        name,
        ty,
        required: true,
    }
}

const fn opt(name: &'static str, ty: MemberType) -> Member {
    Member {
        name,
        ty,
        required: false,
    }
}

/// How a capability is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One request, answered on its response (`RequestAdapter`).
    Request,
    /// Submitted once, collected by handle (`LongJob`).
    LongJob,
}

/// One capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    pub shape: Shape,
    /// The canonical request's members, in the order a paid built-in sends them (module doc).
    pub members: &'static [Member],
    /// The features a route of this capability may declare, in spec/capabilities.md's order.
    /// Only these names have a meaning for it; every route of the built-in table declares only
    /// these.
    pub features: &'static [&'static str],
    /// The answer's files: `(name, kind)`, the kind spec/capabilities.md names for the file. A
    /// family instead (`model`, of `mesh.generate`) means the file's bytes decide its kind
    /// within it. A stand-in's file without a kind takes this one ([`crate::stand_in`]).
    pub files: &'static [(&'static str, &'static str)],
}

use MemberType::*;

/// The features of `image.generate` (spec/capabilities.md §2).
const IMAGE_GENERATE_FEATURES: &[&str] = &[
    "text_to_image",
    "alpha",
    "opaque_background",
    "auto_background",
    "exact_size",
    "custom_exact_size",
    "flexible_size",
    "image_input",
    "hosted_url_reference_input",
    "png_output",
    "jpeg_output",
    "webp_output",
    "maximum_quality",
    "authored_prompt_passthrough",
];

/// The features of `image.edit` (spec/capabilities.md §3): those of `image.generate` without
/// `text_to_image`, plus `mask`.
const IMAGE_EDIT_FEATURES: &[&str] = &[
    "alpha",
    "opaque_background",
    "auto_background",
    "exact_size",
    "custom_exact_size",
    "flexible_size",
    "image_input",
    "hosted_url_reference_input",
    "png_output",
    "jpeg_output",
    "webp_output",
    "maximum_quality",
    "authored_prompt_passthrough",
    "mask",
];

/// Every capability FX ships (spec/capabilities.md §2–§12), sorted by name.
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "agent.turn",
        shape: Shape::Request,
        members: &[
            req("system", Text),
            req("messages", List),
            req("tools", List),
            req("tool_choice", Text),
            opt("max_tokens", Integer),
        ],
        features: &["tool_use", "image_input"],
        files: &[],
    },
    Capability {
        name: "background.remove",
        shape: Shape::Request,
        members: &[req("image", File)],
        features: &[],
        files: &[("image", "image/png")],
    },
    Capability {
        name: "image.edit",
        shape: Shape::Request,
        members: &[
            req("prompt", Text),
            opt("size", Text),
            opt("background", Text),
            req("image", File),
            opt("mask", File),
            opt("references", Files),
        ],
        features: IMAGE_EDIT_FEATURES,
        files: &[("image", "image/png")],
    },
    Capability {
        name: "image.generate",
        shape: Shape::Request,
        members: &[
            req("prompt", Text),
            opt("size", Text),
            opt("background", Text),
            opt("references", Files),
        ],
        features: IMAGE_GENERATE_FEATURES,
        files: &[("image", "image/png")],
    },
    Capability {
        name: "mesh.generate",
        shape: Shape::LongJob,
        members: &[
            opt("face_limit", Integer),
            opt("quad", Boolean),
            opt("texture", Boolean),
            opt("pbr", Boolean),
            req("views", FilesByName),
        ],
        features: &["multiview", "textured_mesh", "quad", "pbr"],
        files: &[("model", "model")],
    },
    Capability {
        name: "mesh.rig",
        shape: Shape::LongJob,
        members: &[
            opt("rig_type", Text),
            opt("skeleton", Text),
            opt("allow_negative_check", Boolean),
            req("model", File),
        ],
        features: &["biped", "glb_input", "rig_check"],
        files: &[("model", "model/gltf-binary")],
    },
    Capability {
        name: "music.generate",
        shape: Shape::Request,
        members: &[req("prompt", Text), opt("duration", Number)],
        features: &[],
        files: &[("audio", "audio/mpeg")],
    },
    Capability {
        name: "sound.generate",
        shape: Shape::Request,
        members: &[
            req("prompt", Text),
            opt("duration", Number),
            opt("prompt_influence", Number),
            opt("loop", Boolean),
        ],
        features: &["exact_duration"],
        files: &[("audio", "audio/mpeg")],
    },
    Capability {
        name: "speech.generate",
        shape: Shape::Request,
        members: &[
            req("text", Text),
            req("voice", Text),
            opt("stability", Number),
            opt("language_code", Text),
            opt("max_chars", Integer),
        ],
        features: &["audio_tags", "stability"],
        files: &[("audio", "audio/mpeg")],
    },
    Capability {
        name: "structured.generate",
        shape: Shape::Request,
        members: &[
            req("prompt", Text),
            opt("system", Text),
            opt("matte", Text),
            opt("max_tokens", Integer),
            req("schema", FileOrObject),
            opt("context", Files),
        ],
        features: &["structured_output", "image_input"],
        files: &[],
    },
    Capability {
        name: "video.generate",
        shape: Shape::LongJob,
        members: &[
            req("prompt", Text),
            opt("duration", Number),
            opt("resolution", Text),
            opt("aspect_ratio", Text),
            opt("first_frame", File),
            opt("last_frame", File),
        ],
        features: &["first_last_frame"],
        files: &[("video", "video/mp4")],
    },
];

/// The capability of this name.
pub fn capability(name: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|c| c.name == name)
}

/// Checks a canonical request against its capability (module doc).
pub fn check_request(capability_name: &str, request: &Value) -> Result<(), String> {
    let Some(capability) = capability(capability_name) else {
        return Err(format!("{capability_name} is not a capability FX ships"));
    };
    let Some(members) = request.as_object() else {
        return Err(format!("a request for {capability_name} is a JSON object"));
    };
    for name in members.keys() {
        if !capability.members.iter().any(|m| m.name == name) {
            return Err(format!("{capability_name} takes no member {name}"));
        }
    }
    for member in capability.members {
        match members.get(member.name) {
            None | Some(Value::Null) if member.required => {
                return Err(format!("{capability_name} needs {}", member.name));
            }
            None | Some(Value::Null) => {}
            Some(value) if !member.ty.admits(value) => {
                return Err(format!("{} is {}", member.name, member.ty.word()));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_table_is_sorted_and_unique() {
        let names: Vec<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted);
        for capability in CAPABILITIES {
            let mut members: Vec<&str> = capability.members.iter().map(|m| m.name).collect();
            members.sort();
            members.dedup();
            assert_eq!(
                members.len(),
                capability.members.len(),
                "{}",
                capability.name
            );
            let mut features = capability.features.to_vec();
            features.sort();
            features.dedup();
            assert_eq!(
                features.len(),
                capability.features.len(),
                "{}",
                capability.name
            );
        }
    }

    #[test]
    fn an_edit_route_has_the_features_of_a_generate_route_but_drawing_from_nothing() {
        let generate = capability("image.generate").unwrap().features;
        let edit = capability("image.edit").unwrap().features;
        let mut expected: Vec<&str> = generate
            .iter()
            .copied()
            .filter(|f| *f != "text_to_image")
            .collect();
        expected.push("mask");
        assert_eq!(edit, expected.as_slice());
    }

    #[test]
    fn every_type_has_a_notation_and_a_word() {
        let types = [
            (Text, "text", "text"),
            (Number, "number", "a number"),
            (Integer, "integer", "a whole number"),
            (Boolean, "boolean", "true or false"),
            (Object, "object", "an object"),
            (List, "list", "a list"),
            (File, "file", "a file"),
            (Files, "file[]", "a list of files"),
            (FilesByName, "file{}", "files by name"),
            (FileOrObject, "file | object", "a JSON file or object"),
        ];
        for (ty, notation, word) in types {
            assert_eq!((ty.notation(), ty.word()), (notation, word));
        }
    }

    #[test]
    fn types_admit_their_values_only() {
        let file = json!({"file": "a".repeat(64)});
        assert!(Integer.admits(&json!(3)));
        assert!(Integer.admits(&json!(3.0)));
        assert!(!Integer.admits(&json!(3.5)));
        assert!(!Integer.admits(&json!("3")));
        assert!(!Number.admits(&json!(true)));
        assert!(File.admits(&file));
        assert!(!File.admits(&json!({"file": "a", "kind": "image/png"})));
        assert!(!File.admits(&json!({"file": 3})));
        assert!(Files.admits(&json!([])));
        assert!(!Files.admits(&json!([file, null])));
        assert!(FilesByName.admits(&json!({"front": file})));
        assert!(!FilesByName.admits(&json!([file])));
        assert!(FileOrObject.admits(&file));
        assert!(FileOrObject.admits(&json!({"type": "object"})));
        assert!(!FileOrObject.admits(&json!("{}")));
    }

    #[test]
    fn requests_are_checked_against_their_capability() {
        let file = json!({"file": "a".repeat(64)});
        assert_eq!(
            check_request(
                "image.edit",
                &json!({"prompt": "p", "image": file, "size": null, "background": "auto"})
            ),
            Ok(())
        );
        assert_eq!(
            check_request("image.generate", &json!({"prompt": "p", "quality": "low"})),
            Err("image.generate takes no member quality".into())
        );
        assert_eq!(
            check_request("image.edit", &json!({"prompt": "p", "image": null})),
            Err("image.edit needs image".into())
        );
        assert_eq!(
            check_request("sound.generate", &json!({"prompt": "p", "duration": "1"})),
            Err("duration is a number".into())
        );
        assert_eq!(
            check_request(
                "image.generate",
                &json!({"prompt": "p", "references": [file, 3]})
            ),
            Err("references is a list of files".into())
        );
        assert_eq!(
            check_request(
                "mesh.generate",
                &json!({"views": {"front": file}, "face_limit": 10.5})
            ),
            Err("face_limit is a whole number".into())
        );
        assert_eq!(
            check_request("vision.review", &json!({})),
            Err("vision.review is not a capability FX ships".into())
        );
        assert_eq!(
            check_request("agent.turn", &json!([])),
            Err("a request for agent.turn is a JSON object".into())
        );
        // An undefined member is refused before anything else; then members in table order.
        assert_eq!(
            check_request("speech.generate", &json!({"text": 1, "model_id": "m"})),
            Err("speech.generate takes no member model_id".into())
        );
        assert_eq!(
            check_request("speech.generate", &json!({"text": 1})),
            Err("text is text".into())
        );
        assert_eq!(
            check_request("speech.generate", &json!({"text": "t"})),
            Err("speech.generate needs voice".into())
        );
        assert_eq!(
            check_request(
                "structured.generate",
                &json!({"prompt": "p", "schema": "{}", "matte": null})
            ),
            Err("schema is a JSON file or object".into())
        );
        assert_eq!(
            check_request(
                "structured.generate",
                &json!({"prompt": "p", "schema": {"type": "object"}, "system": null,
                        "matte": null, "max_tokens": null, "context": null})
            ),
            Ok(())
        );
        assert_eq!(
            check_request(
                "mesh.rig",
                &json!({"model": file, "allow_negative_check": 1})
            ),
            Err("allow_negative_check is true or false".into())
        );
    }
}
