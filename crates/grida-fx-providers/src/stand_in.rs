//! The providers' checks of a stand-in's answer (spec/protocol.md §5.7, §6.1 *Stand-in answers*;
//! spec/providers.md §1, §4.4).
//!
//! A stand-in run answers each paid call the call cache cannot answer with a function the caller
//! supplies, instead of a provider. The engine holds such an answer to what a provider's answer is
//! held to. [`StandInChecks`] makes the checks that are the providers':
//! - [`StandInChecks::request`], sub-step 1: the request against its capability, before the
//!   stand-in is asked (spec/capabilities.md §1; spec/providers.md §5 step 2). The route's
//!   refusals of values (spec/providers.md §5 steps 1, 3, 4 and 5) live inside each adapter's
//!   send, and are not made;
//! - [`StandInChecks::answer`], sub-steps 3.1 and 3.2: the answer's shape, since no adapter made
//!   it, and then the route's check. The round trip of `data` through canonical JSON and the
//!   engine's check of `agent.turn` (3.3 and 3.4) are the engine's.
//!
//! **The shape** (spec/capabilities.md §1, *Answers*), the first refusal winning, for a capability
//! FX ships ([`capabilities::CAPABILITIES`]):
//! 1. a file the capability names is missing: `the answer holds no <name>`;
//! 2. a file it does not name: `<capability> returns no file named <name>`;
//! 3. each file's kind. A file without one takes the kind the capability names for it
//!    (`image/png`, `audio/mpeg`, `video/mp4`, `model/gltf-binary`); a file of another kind is
//!    refused, `<name> is <kind>, not <kind>`. `mesh.generate`'s `model` takes the kind its bytes
//!    show instead: `model is <kind>, not model/fbx or model/gltf-binary` for a kind given that
//!    is neither, `model is neither binary glTF nor binary FBX` for bytes that show neither, and
//!    `model is <kind>, not <kind its bytes show>` for a kind given that the bytes contradict;
//! 4. `data`:
//!    - where the capability names none, it is `null`: `<capability> returns no data`;
//!    - `structured.generate`'s is an object holding only `json`: `structured.generate returns
//!      its data as {"json": <value>}`;
//!    - `mesh.rig`'s is `{"facts": {"riggable", "checked_rig_type", "advisory_override"}}` and
//!      nothing else, `riggable` true, false or `null`, `checked_rig_type` text or `null`,
//!      `advisory_override` true or false: `mesh.rig returns its data as {"facts": {"riggable",
//!      "checked_rig_type", "advisory_override"}}`;
//!    - `agent.turn`'s is left to the engine's check (spec/protocol.md §6.2);
//!    - `video.generate`'s and `mesh.generate`'s is taken from the answer's file, so a stand-in
//!      answers `null` (`<capability>'s data is taken from its file: answer null`), and it is
//!      written here: `{"facts": {"width", "height", "duration_seconds", "fps"}}` from the clip
//!      ([`ClipFacts`]; `the answer's video is not a clip FX can read: <reason>` when it has none),
//!      and `{"facts": {"model_kind"}}` from the model's kind.
//!
//! For a capability FX does not ship, each file's kind is required (`<name> has no kind, and
//! <capability> names none for it`) and only checked to be a kind (spec/identity.md §4; `<name> is
//! <kind>, not a kind`), and `data` is kept as it is.
//!
//! **The route's check** is the check of the adapter FX registers for the route's capability and
//! provider, from a registry built with no keys over a transport that refuses every exchange
//! ([`live::offline_setup`]): only its `check` is ever called. A route no adapter serves gets the
//! check spec/capabilities.md gives its capability for every route ([`checks::every_route`]), and
//! a capability with none gets none.

use crate::adapter::{Adapter, Answer, AnsweredFile, CallRequest};
use crate::capabilities;
use crate::checks::{self, ClipFacts, MAX_KIND_CHARS};
use crate::keys::Keys;
use crate::live;
use crate::redact::bounded;
use crate::registry::Adapters;
use crate::wire;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

/// A file of a stand-in's answer as received: kind optional (spec/protocol.md §5.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandInFile {
    /// The file kind (spec/identity.md §4); `None` takes the kind the capability names.
    pub kind: Option<String>,
    pub bytes: Vec<u8>,
}

/// The kinds `mesh.generate`'s `model` may have, as its sentence lists them.
const MODEL_KINDS: [&str; 2] = ["model/fbx", "model/gltf-binary"];

/// A file kind or a family, as the protocol's schemas write one.
static KIND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:/[a-z0-9.+-]+)?$").expect("a valid pattern"));

/// The checks of spec/protocol.md §6.1 "Stand-in answers" sub-steps 1 and 3.1–3.2 (module doc).
#[derive(Debug, Clone)]
pub struct StandInChecks {
    /// The adapters whose checks a route gets: `live::adapters(&live::offline_setup(Keys::none()))`.
    adapters: Adapters,
}

impl Default for StandInChecks {
    fn default() -> StandInChecks {
        StandInChecks::new()
    }
}

impl StandInChecks {
    /// The checks with every adapter FX ships, built keyless over a transport that refuses every
    /// exchange (module doc).
    pub fn new() -> StandInChecks {
        StandInChecks::with_adapters(live::adapters(&live::offline_setup(Keys::none())))
    }

    /// The checks with `adapters` in place of FX's own: a route served by one of them gets its
    /// check, any other the capability's every-route check. Nothing but `check` is called.
    pub fn with_adapters(adapters: Adapters) -> StandInChecks {
        StandInChecks { adapters }
    }

    /// The request check of spec/capabilities.md §1 ([`capabilities::check_request`]) for a
    /// capability FX ships; `Ok(())` for any other. A refusal is `capability_refused`, and the
    /// stand-in is not asked.
    pub fn request(&self, capability: &str, request: &Value) -> Result<(), String> {
        match capabilities::capability(capability) {
            Some(_) => capabilities::check_request(capability, request),
            None => Ok(()),
        }
    }

    /// The answer's shape (module doc), with the kinds of files that gave none and the `data` of
    /// `video.generate` and `mesh.generate` written, and then the route's check: the registered
    /// adapter's when one serves the route's capability and provider, else the capability's
    /// every-route check. Returns the answer to store, with no cost. A refusal is `call_failed`.
    pub fn answer(
        &self,
        call: &CallRequest,
        files: IndexMap<String, StandInFile>,
        data: Value,
    ) -> Result<Answer, String> {
        let answer = shape(&call.route.capability, files, data)?;
        match self.adapters.serving(&call.route) {
            Some(Adapter::Request(adapter)) => adapter.check(call, &answer)?,
            Some(Adapter::Job(adapter)) => adapter.check(call, &answer)?,
            None => checks::every_route(call, &answer)?,
        }
        Ok(answer)
    }
}

/// A kind as a refusal shows it: as it is when it is a kind, else as a JSON string, so blanks
/// and control characters show; cut to [`MAX_KIND_CHARS`] either way.
fn shown(kind: &str) -> String {
    if KIND.is_match(kind) {
        bounded(kind, MAX_KIND_CHARS)
    } else {
        bounded(&Value::from(kind).to_string(), MAX_KIND_CHARS)
    }
}

/// The shape of the module doc, as the answer to check and store.
fn shape(
    capability: &str,
    files: IndexMap<String, StandInFile>,
    data: Value,
) -> Result<Answer, String> {
    let Some(shipped) = capabilities::capability(capability) else {
        return unshipped(capability, files, data);
    };
    for (name, _) in shipped.files {
        if !files.contains_key(*name) {
            return Err(format!("the answer holds no {name}"));
        }
    }
    if let Some(name) = files
        .keys()
        .find(|name| !shipped.files.iter().any(|(named, _)| named == name))
    {
        return Err(format!("{capability} returns no file named {name}"));
    }
    let mut answer = Answer::new(Value::Null, None);
    for (name, file) in files {
        let named = shipped
            .files
            .iter()
            .find(|(named, _)| *named == name)
            .map_or("", |(_, kind)| *kind);
        let kind = if named.contains('/') {
            match file.kind {
                Some(kind) if kind != named => {
                    return Err(format!("{name} is {}, not {named}", shown(&kind)));
                }
                _ => named.to_string(),
            }
        } else {
            model_kind(&name, file.kind.as_deref(), &file.bytes)?
        };
        answer.files.insert(
            name,
            AnsweredFile {
                kind,
                bytes: file.bytes,
            },
        );
    }
    answer.data = shaped_data(capability, &answer, data)?;
    Ok(answer)
}

/// The kind of `mesh.generate`'s model: the kind its bytes show (module doc, step 3).
fn model_kind(name: &str, given: Option<&str>, bytes: &[u8]) -> Result<String, String> {
    if let Some(given) = given
        && !MODEL_KINDS.contains(&given)
    {
        return Err(format!(
            "{name} is {}, not {}",
            shown(given),
            MODEL_KINDS.join(" or ")
        ));
    }
    let Some(shown_by_bytes) = wire::sniff_model(bytes) else {
        return Err(format!("{name} is neither binary glTF nor binary FBX"));
    };
    match given {
        Some(given) if given != shown_by_bytes => {
            Err(format!("{name} is {given}, not {shown_by_bytes}"))
        }
        _ => Ok(shown_by_bytes.to_string()),
    }
}

/// The answer's `data` once its shape passes (module doc, step 4); `answer` holds its files.
fn shaped_data(capability: &str, answer: &Answer, data: Value) -> Result<Value, String> {
    match capability {
        "agent.turn" => Ok(data),
        "structured.generate" => match &data {
            Value::Object(members) if members.len() == 1 && members.contains_key("json") => {
                Ok(data)
            }
            _ => Err(r#"structured.generate returns its data as {"json": <value>}"#.into()),
        },
        "mesh.rig" if rig_facts(&data) => Ok(data),
        "mesh.rig" => Err(concat!(
            r#"mesh.rig returns its data as {"facts": {"riggable", "checked_rig_type", "#,
            r#""advisory_override"}}"#
        )
        .into()),
        "video.generate" | "mesh.generate" if !data.is_null() => Err(format!(
            "{capability}'s data is taken from its file: answer null"
        )),
        "video.generate" => {
            let clip = &answer.files["video"].bytes;
            let facts = ClipFacts::of_mp4(clip).map_err(|reason| {
                format!("the answer's video is not a clip FX can read: {reason}")
            })?;
            Ok(json!({"facts": facts.to_json()}))
        }
        "mesh.generate" => Ok(json!({"facts": {"model_kind": answer.files["model"].kind}})),
        _ if !data.is_null() => Err(format!("{capability} returns no data")),
        _ => Ok(Value::Null),
    }
}

/// Whether `data` is `mesh.rig`'s (spec/capabilities.md §8): `{"facts": {"riggable",
/// "checked_rig_type", "advisory_override"}}` with their types, and nothing else.
fn rig_facts(data: &Value) -> bool {
    let Some(outer) = data.as_object().filter(|outer| outer.len() == 1) else {
        return false;
    };
    let Some(Value::Object(facts)) = outer.get("facts") else {
        return false;
    };
    facts.len() == 3
        && matches!(facts.get("riggable"), Some(Value::Bool(_) | Value::Null))
        && matches!(
            facts.get("checked_rig_type"),
            Some(Value::String(_) | Value::Null)
        )
        && matches!(facts.get("advisory_override"), Some(Value::Bool(_)))
}

/// The shape of an answer for a capability FX does not ship (module doc).
fn unshipped(
    capability: &str,
    files: IndexMap<String, StandInFile>,
    data: Value,
) -> Result<Answer, String> {
    let mut answer = Answer::new(data, None);
    for (name, file) in files {
        let kind = match file.kind {
            Some(kind) if KIND.is_match(&kind) => kind,
            Some(kind) => return Err(format!("{name} is {}, not a kind", shown(&kind))),
            None => {
                return Err(format!(
                    "{name} has no kind, and {capability} names none for it"
                ));
            }
        };
        answer.files.insert(
            name,
            AnsweredFile {
                kind,
                bytes: file.bytes,
            },
        );
    }
    Ok(answer)
}
