//! The store's records (spec/store.md §3; spec/schemas/fx-result-record-v1, fx-call-record-v1,
//! fx-job-record-v1) and the call key (spec/identity.md §9).
//!
//! Each record has `to_value` (the JSON object the schema describes, members in any order; the
//! store writes `canon` of it) and `from_value` (a strict reader: the right `kind`, every required
//! member, no unknown member, digests as 64 lowercase hex, money through `Usd::from_value`; a
//! refusal is a sentence). Output values are encoded as store.md §3 says: `{"file": {…}}`,
//! `{"collection": [[key, output], …]}`, `{"list": [output, …]}`, `{"none": true}`.

use super::{ReadSet, Store, StoreError};
use grida_fx_core::money::Usd;
use grida_fx_core::val::{Collection, Val};
use grida_fx_core::value::{self, as_f64, is_digest};
use indexmap::IndexMap;
use serde_json::{Map, Value};

pub const RESULT_KIND: &str = "fx-result-record-v1";
pub const CALL_KIND: &str = "fx-call-record-v1";
pub const JOB_KIND: &str = "fx-job-record-v1";

/// The kind of the object `call_key` hashes (spec/identity.md §9).
const CALL_KEY_KIND: &str = "fx-call-v1";

/// A file as records name it: `{digest, kind, name, size, key?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub digest: String,
    pub kind: String,
    pub name: String,
    pub size: u64,
    /// Only for a keyed collection's item.
    pub key: Option<String>,
}

impl FileEntry {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("digest".into(), Value::from(self.digest.as_str()));
        map.insert("kind".into(), Value::from(self.kind.as_str()));
        map.insert("name".into(), Value::from(self.name.as_str()));
        map.insert("size".into(), Value::from(self.size));
        if let Some(key) = &self.key {
            map.insert("key".into(), Value::from(key.as_str()));
        }
        Value::Object(map)
    }

    pub fn from_value(value: &Value) -> Result<FileEntry, String> {
        FileEntry::read(value, true)
    }

    /// The entry without its `key`: the shape of a call record's file.
    fn unkeyed_value(&self) -> Value {
        FileEntry {
            key: None,
            ..self.clone()
        }
        .to_value()
    }

    /// Reads a file entry; a call record's files (`keyed: false`) have no `key` member.
    fn read(value: &Value, keyed: bool) -> Result<FileEntry, String> {
        let what = "a file entry";
        let map = object(value, what)?;
        let optional: &[&str] = if keyed { &["key"] } else { &[] };
        members(map, what, &["digest", "kind", "name", "size"], optional)?;
        let key = match map.get("key") {
            None => None,
            Some(key) => Some(string_of(key, "key", what)?),
        };
        Ok(FileEntry {
            digest: digest_member(map, "digest", what)?,
            kind: string_member(map, "kind", what)?,
            name: string_member(map, "name", what)?,
            size: whole(&map["size"]).ok_or_else(|| {
                format!(
                    "the size of {what} is {}, not a whole number of bytes",
                    shown(&map["size"])
                )
            })?,
            key,
        })
    }
}

/// An output port's value in a result record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutput {
    File(FileEntry),
    Collection(Vec<(String, RecordOutput)>),
    List(Vec<RecordOutput>),
    None,
}

impl RecordOutput {
    /// Encodes an output value: a file, a collection, a list, or missing/null as none. Anything
    /// else is refused (`a step output is a file, a list or a collection, not <kind word>`).
    pub fn from_val(value: &Val) -> Result<RecordOutput, String> {
        match value {
            Val::File(file) => Ok(RecordOutput::File(FileEntry {
                digest: file.digest.clone(),
                kind: file.kind.clone(),
                name: file.name.clone(),
                size: file.size,
                key: file.key.clone().filter(|key| !key.is_empty()),
            })),
            Val::Collection(collection) => collection
                .items
                .iter()
                .map(|(key, item)| Ok((key.clone(), RecordOutput::from_val(item)?)))
                .collect::<Result<_, String>>()
                .map(RecordOutput::Collection),
            Val::List(items) => items
                .iter()
                .map(RecordOutput::from_val)
                .collect::<Result<_, String>>()
                .map(RecordOutput::List),
            Val::Null | Val::Missing => Ok(RecordOutput::None),
            other => Err(format!(
                "a step output is a file, a list or a collection, not {}",
                other.kind_word()
            )),
        }
    }

    /// Decodes into a runtime value with every file read through [`Store::file_value`].
    /// A collection comes back unjudged (no verdicts); none comes back as null.
    pub fn to_val(&self, store: &Store) -> Result<Val, StoreError> {
        Ok(match self {
            RecordOutput::File(entry) => Val::File(Box::new(store.file_value(entry)?)),
            RecordOutput::Collection(items) => Val::Collection(Box::new(Collection {
                items: items
                    .iter()
                    .map(|(key, item)| Ok((key.clone(), item.to_val(store)?)))
                    .collect::<Result<_, StoreError>>()?,
                verdicts: IndexMap::new(),
            })),
            RecordOutput::List(items) => Val::List(
                items
                    .iter()
                    .map(|item| item.to_val(store))
                    .collect::<Result<_, StoreError>>()?,
            ),
            RecordOutput::None => Val::Null,
        })
    }

    pub fn to_value(&self) -> Value {
        match self {
            RecordOutput::File(entry) => single("file", entry.to_value()),
            RecordOutput::Collection(items) => single(
                "collection",
                Value::Array(
                    items
                        .iter()
                        .map(|(key, item)| {
                            Value::Array(vec![Value::from(key.as_str()), item.to_value()])
                        })
                        .collect(),
                ),
            ),
            RecordOutput::List(items) => single(
                "list",
                Value::Array(items.iter().map(RecordOutput::to_value).collect()),
            ),
            RecordOutput::None => single("none", Value::Bool(true)),
        }
    }

    pub fn from_value(value: &Value) -> Result<RecordOutput, String> {
        let what = "an output";
        let map = object(value, what)?;
        let refused = || {
            format!(
                "an output is one of {{\"file\"}}, {{\"collection\"}}, {{\"list\"}} or \
                 {{\"none\": true}}, not {}",
                shown(value)
            )
        };
        if map.len() != 1 {
            return Err(refused());
        }
        let (name, inner) = map.iter().next().ok_or_else(refused)?;
        match name.as_str() {
            "file" => Ok(RecordOutput::File(FileEntry::from_value(inner)?)),
            "collection" => {
                let items = array(inner, "the items of a collection output")?;
                items
                    .iter()
                    .map(|pair| match pair.as_array().map(Vec::as_slice) {
                        Some([Value::String(key), item]) => {
                            Ok((key.clone(), RecordOutput::from_value(item)?))
                        }
                        _ => Err(format!(
                            "an item of a collection output is [key, output], not {}",
                            shown(pair)
                        )),
                    })
                    .collect::<Result<_, String>>()
                    .map(RecordOutput::Collection)
            }
            "list" => array(inner, "the items of a list output")?
                .iter()
                .map(RecordOutput::from_value)
                .collect::<Result<_, String>>()
                .map(RecordOutput::List),
            "none" if inner == &Value::Bool(true) => Ok(RecordOutput::None),
            _ => Err(refused()),
        }
    }

    /// Every file entry, in order.
    pub fn files(&self) -> Vec<&FileEntry> {
        let mut out = Vec::new();
        self.collect_files(&mut out);
        out
    }

    fn collect_files<'a>(&'a self, out: &mut Vec<&'a FileEntry>) {
        match self {
            RecordOutput::File(entry) => out.push(entry),
            RecordOutput::Collection(items) => {
                for (_, item) in items {
                    item.collect_files(out);
                }
            }
            RecordOutput::List(items) => {
                for item in items {
                    item.collect_files(out);
                }
            }
            RecordOutput::None => {}
        }
    }
}

/// `fx-result-record-v1`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultRecord {
    pub identity: String,
    /// The type identity.
    pub type_identity: String,
    pub outputs: IndexMap<String, RecordOutput>,
    /// The node facts the body reported (no `cost_usd`: the engine's, kept in `cost_usd`).
    pub facts: IndexMap<String, Value>,
    pub read: ReadSet,
    /// What the step's paid calls cost; `None` when it made none.
    pub cost_usd: Option<Usd>,
}

impl ResultRecord {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("kind".into(), Value::from(RESULT_KIND));
        map.insert("identity".into(), Value::from(self.identity.as_str()));
        map.insert("type".into(), Value::from(self.type_identity.as_str()));
        map.insert(
            "outputs".into(),
            Value::Object(
                self.outputs
                    .iter()
                    .map(|(port, output)| (port.clone(), output.to_value()))
                    .collect(),
            ),
        );
        map.insert(
            "facts".into(),
            Value::Object(
                self.facts
                    .iter()
                    .map(|(name, fact)| (name.clone(), fact.clone()))
                    .collect(),
            ),
        );
        map.insert(
            "read".into(),
            Value::Object(
                self.read
                    .iter()
                    .map(|(name, digest)| (name.clone(), Value::from(digest.as_str())))
                    .collect(),
            ),
        );
        map.insert("cost_usd".into(), money_value(self.cost_usd));
        Value::Object(map)
    }

    pub fn from_value(value: &Value) -> Result<ResultRecord, String> {
        let what = "the result record";
        let map = object(value, what)?;
        kind_of(map, RESULT_KIND, what)?;
        members(
            map,
            what,
            &[
                "kind", "identity", "type", "outputs", "facts", "read", "cost_usd",
            ],
            &[],
        )?;
        let outputs = object(&map["outputs"], "the outputs of the result record")?
            .iter()
            .map(|(port, output)| {
                RecordOutput::from_value(output)
                    .map(|output| (port.clone(), output))
                    .map_err(|e| format!("output {port}: {e}"))
            })
            .collect::<Result<IndexMap<_, _>, String>>()?;
        let facts = object(&map["facts"], "the facts of the result record")?
            .iter()
            .map(|(name, fact)| (name.clone(), fact.clone()))
            .collect();
        let mut read = ReadSet::new();
        for (name, digest) in object(&map["read"], "the read set of the result record")? {
            if !is_read_name(name) {
                return Err(format!(
                    "{} in the read set is not <with name>/<index>",
                    shown(&Value::from(name.as_str()))
                ));
            }
            match digest.as_str() {
                Some(digest) if is_digest(digest) => {
                    read.insert(name.clone(), digest.to_string());
                }
                _ => return Err(format!("read {name} is {}, not a digest", shown(digest))),
            }
        }
        Ok(ResultRecord {
            identity: digest_member(map, "identity", what)?,
            type_identity: string_member(map, "type", what)?,
            outputs,
            facts,
            read,
            cost_usd: money(&map["cost_usd"], what)?,
        })
    }
}

/// `{id, fingerprint}` of a route in call and job records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    pub id: String,
    pub fingerprint: String,
}

impl RouteEntry {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".into(), Value::from(self.id.as_str()));
        map.insert("fingerprint".into(), Value::from(self.fingerprint.as_str()));
        Value::Object(map)
    }

    fn from_value(value: &Value) -> Result<RouteEntry, String> {
        let what = "the route";
        let map = object(value, what)?;
        members(map, what, &["id", "fingerprint"], &[])?;
        let id = string_member(map, "id", what)?;
        if !is_route_id(&id) {
            return Err(format!(
                "the route {} is not <model>@<provider>",
                shown(&Value::from(id.as_str()))
            ));
        }
        Ok(RouteEntry {
            id,
            fingerprint: digest_member(map, "fingerprint", what)?,
        })
    }
}

/// A file a provider returned, in a call record: `{digest, kind, name, size}`.
pub type CallFile = FileEntry;

/// `fx-call-record-v1`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    pub key: String,
    pub capability: String,
    pub route: RouteEntry,
    /// The canonical request, exactly as keyed.
    pub request: Value,
    pub take: Vec<u32>,
    /// By name, in the provider's order; `key` is never set here.
    pub files: IndexMap<String, CallFile>,
    pub data: Value,
    /// The cost the provider reported; `None` when it reported none.
    pub cost_usd: Option<Usd>,
}

impl CallRecord {
    /// The record's JSON object. A call record's files have no `key` member (the schema has
    /// none), so a key set on one is not written.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("kind".into(), Value::from(CALL_KIND));
        map.insert("key".into(), Value::from(self.key.as_str()));
        map.insert("capability".into(), Value::from(self.capability.as_str()));
        map.insert("route".into(), self.route.to_value());
        map.insert("request".into(), self.request.clone());
        map.insert("take".into(), take_value(&self.take));
        map.insert(
            "files".into(),
            Value::Object(
                self.files
                    .iter()
                    .map(|(name, file)| (name.clone(), file.unkeyed_value()))
                    .collect(),
            ),
        );
        map.insert("data".into(), self.data.clone());
        map.insert("cost_usd".into(), money_value(self.cost_usd));
        Value::Object(map)
    }

    pub fn from_value(value: &Value) -> Result<CallRecord, String> {
        let what = "the call record";
        let map = object(value, what)?;
        kind_of(map, CALL_KIND, what)?;
        members(
            map,
            what,
            &[
                "kind",
                "key",
                "capability",
                "route",
                "request",
                "take",
                "files",
                "data",
                "cost_usd",
            ],
            &[],
        )?;
        let files = object(&map["files"], "the files of the call record")?
            .iter()
            .map(|(name, file)| {
                FileEntry::read(file, false)
                    .map(|file| (name.clone(), file))
                    .map_err(|e| format!("file {name}: {e}"))
            })
            .collect::<Result<IndexMap<_, _>, String>>()?;
        Ok(CallRecord {
            key: digest_member(map, "key", what)?,
            capability: capability_member(map, what)?,
            route: RouteEntry::from_value(&map["route"])?,
            request: request_member(map, what)?,
            take: take_member(map, what)?,
            files,
            data: map["data"].clone(),
            cost_usd: money(&map["cost_usd"], what)?,
        })
    }

    /// The key recomputed from the record (store.md §3: it MUST equal `key`).
    pub fn computed_key(&self) -> String {
        call_key(
            &self.capability,
            &self.route.fingerprint,
            &self.request,
            &self.take,
        )
    }
}

/// A job's state (spec/store.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Submitting,
    Submitted,
    Settled,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Submitting => "submitting",
            JobState::Submitted => "submitted",
            JobState::Settled => "settled",
        }
    }

    fn parse(text: &str) -> Option<JobState> {
        match text {
            "submitting" => Some(JobState::Submitting),
            "submitted" => Some(JobState::Submitted),
            "settled" => Some(JobState::Settled),
            _ => None,
        }
    }
}

/// `fx-job-record-v1`.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub key: String,
    pub capability: String,
    pub route: RouteEntry,
    pub request: Value,
    pub take: Vec<u32>,
    pub state: JobState,
    /// `None` while `submitting`; an object once `submitted`.
    pub handle: Option<Value>,
}

impl JobRecord {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("kind".into(), Value::from(JOB_KIND));
        map.insert("key".into(), Value::from(self.key.as_str()));
        map.insert("capability".into(), Value::from(self.capability.as_str()));
        map.insert("route".into(), self.route.to_value());
        map.insert("request".into(), self.request.clone());
        map.insert("take".into(), take_value(&self.take));
        map.insert("state".into(), Value::from(self.state.as_str()));
        map.insert("handle".into(), self.handle.clone().unwrap_or(Value::Null));
        Value::Object(map)
    }

    pub fn from_value(value: &Value) -> Result<JobRecord, String> {
        let what = "the job record";
        let map = object(value, what)?;
        kind_of(map, JOB_KIND, what)?;
        members(
            map,
            what,
            &[
                "kind",
                "key",
                "capability",
                "route",
                "request",
                "take",
                "state",
                "handle",
            ],
            &[],
        )?;
        let state = map["state"]
            .as_str()
            .and_then(JobState::parse)
            .ok_or_else(|| {
                format!(
                    "the state of {what} is {}, not submitting, submitted or settled",
                    shown(&map["state"])
                )
            })?;
        let handle = match (&map["handle"], state) {
            (Value::Null, JobState::Submitted) => {
                return Err(format!("{what} is submitted and has no handle"));
            }
            (Value::Null, _) => None,
            (Value::Object(_), JobState::Submitting) => {
                return Err(format!("{what} is submitting and already has a handle"));
            }
            (handle @ Value::Object(_), _) => Some(handle.clone()),
            (other, _) => {
                return Err(format!(
                    "the handle of {what} is {}, not an object or null",
                    shown(other)
                ));
            }
        };
        Ok(JobRecord {
            key: digest_member(map, "key", what)?,
            capability: capability_member(map, what)?,
            route: RouteEntry::from_value(&map["route"])?,
            request: request_member(map, what)?,
            take: take_member(map, what)?,
            state,
            handle,
        })
    }
}

/// `call_key` (spec/identity.md §9): `digest({"kind": "fx-call-v1", capability, route:
/// fingerprint, request, take})`. `request` is already plain (files as `{"file": digest}`).
pub fn call_key(capability: &str, fingerprint: &str, request: &Value, take: &[u32]) -> String {
    value::digest(&value::object([
        ("kind", Value::from(CALL_KEY_KIND)),
        ("capability", Value::from(capability)),
        ("route", Value::from(fingerprint)),
        ("request", request.clone()),
        ("take", take_value(take)),
    ]))
}

// ------------------------------------------------------------------ reading helpers

/// `{name: value}`.
fn single(name: &str, value: Value) -> Value {
    value::object([(name, value)])
}

fn take_value(take: &[u32]) -> Value {
    Value::Array(take.iter().map(|t| Value::from(*t)).collect())
}

fn money_value(amount: Option<Usd>) -> Value {
    amount.map_or(Value::Null, Usd::to_value)
}

/// A JSON value in a message: its canonical text, cut short when long.
fn shown(value: &Value) -> String {
    let text = value::canon(value);
    if text.chars().count() > 60 {
        let cut: String = text.chars().take(57).collect();
        format!("{cut}...")
    } else {
        text
    }
}

fn word(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} is {}, not an object", word(value)))
}

fn array<'a>(value: &'a Value, what: &str) -> Result<&'a Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{what} is {}, not a list", word(value)))
}

/// Every required member present, and nothing but the required and optional ones.
fn members(
    map: &Map<String, Value>,
    what: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<(), String> {
    if let Some(missing) = required.iter().find(|name| !map.contains_key(**name)) {
        return Err(format!("{what} has no {missing}"));
    }
    if let Some(unknown) = map
        .keys()
        .find(|name| !required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
    {
        return Err(format!(
            "{what} has an unknown member {}",
            shown(&Value::from(unknown.as_str()))
        ));
    }
    Ok(())
}

/// The record's `kind` is `expected` (checked first, so another record's kind says so).
fn kind_of(map: &Map<String, Value>, expected: &str, what: &str) -> Result<(), String> {
    match map.get("kind") {
        Some(Value::String(kind)) if kind == expected => Ok(()),
        Some(other) => Err(format!(
            "the kind of {what} is {}, not {expected}",
            shown(other)
        )),
        None => Err(format!("{what} has no kind")),
    }
}

fn string_of(value: &Value, name: &str, what: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("the {name} of {what} is {}, not text", word(value)))
}

fn string_member(map: &Map<String, Value>, name: &str, what: &str) -> Result<String, String> {
    string_of(&map[name], name, what)
}

fn digest_member(map: &Map<String, Value>, name: &str, what: &str) -> Result<String, String> {
    match map[name].as_str() {
        Some(text) if is_digest(text) => Ok(text.to_string()),
        _ => Err(format!(
            "the {name} of {what} is {}, not a digest",
            shown(&map[name])
        )),
    }
}

fn capability_member(map: &Map<String, Value>, what: &str) -> Result<String, String> {
    let capability = string_member(map, "capability", what)?;
    if !is_capability(&capability) {
        return Err(format!(
            "{} is not a capability name",
            shown(&Value::from(capability.as_str()))
        ));
    }
    Ok(capability)
}

fn request_member(map: &Map<String, Value>, what: &str) -> Result<Value, String> {
    match &map["request"] {
        request @ Value::Object(_) => Ok(request.clone()),
        other => Err(format!(
            "the request of {what} is {}, not an object",
            word(other)
        )),
    }
}

/// `take`: a non-empty list of whole numbers from 1.
fn take_member(map: &Map<String, Value>, what: &str) -> Result<Vec<u32>, String> {
    let items = array(&map["take"], &format!("the take of {what}"))?;
    if items.is_empty() {
        return Err(format!("the take of {what} is empty"));
    }
    items
        .iter()
        .map(|item| {
            whole(item)
                .filter(|t| *t >= 1)
                .and_then(|t| u32::try_from(t).ok())
                .ok_or_else(|| {
                    format!(
                        "the take of {what} holds {}, not a whole number from 1",
                        shown(item)
                    )
                })
        })
        .collect()
}

/// `cost_usd`: an amount, or null.
fn money(value: &Value, what: &str) -> Result<Option<Usd>, String> {
    match value {
        Value::Null => Ok(None),
        amount => Usd::from_value(amount)
            .map(Some)
            .map_err(|e| format!("the cost_usd of {what}: {e}")),
    }
}

/// A non-negative whole number (`70` or `70.0`), exact within 2^53.
fn whole(value: &Value) -> Option<u64> {
    let Value::Number(n) = value else {
        return None;
    };
    if let Some(u) = n.as_u64() {
        return Some(u);
    }
    let x = as_f64(n);
    (x.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&x)).then_some(x as u64)
}

/// `^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$` (the capability names of fx-routes-v1).
fn is_capability(text: &str) -> bool {
    text.split('.').all(|segment| {
        let mut chars = segment.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    })
}

/// `^.+@[a-z0-9._-]+$` (a route id: `<model>@<provider>`, split at the last `@`).
fn is_route_id(text: &str) -> bool {
    let Some((model, provider)) = text.rsplit_once('@') else {
        return false;
    };
    !model.is_empty()
        && !model.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        && !provider.is_empty()
        && provider
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// `^[A-Za-z_][A-Za-z0-9_]*/(?:0|[1-9][0-9]*)$` (a read set's names).
fn is_read_name(text: &str) -> bool {
    let Some((name, index)) = text.split_once('/') else {
        return false;
    };
    let mut chars = name.chars();
    let name_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    let index_ok = !index.is_empty()
        && index.bytes().all(|b| b.is_ascii_digit())
        && (index == "0" || !index.starts_with('0'));
    name_ok && index_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const D1: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";
    const FP: &str = "4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4";

    #[test]
    fn patterns_follow_the_schemas() {
        assert!(is_capability("image.generate"));
        assert!(is_capability("text_2.chat"));
        assert!(!is_capability("Image.generate"));
        assert!(!is_capability("image..generate"));
        assert!(!is_capability(""));
        assert!(!is_capability("2d.draw"));
        assert!(is_route_id("img-a@acme"));
        assert!(is_route_id("a@b@acme.io"));
        assert!(!is_route_id("@acme"));
        assert!(!is_route_id("img-a@"));
        assert!(!is_route_id("img-a@Acme"));
        assert!(!is_route_id("img-a"));
        assert!(is_read_name("image/0"));
        assert!(is_read_name("_x9/10"));
        assert!(!is_read_name("image/01"));
        assert!(!is_read_name("image/"));
        assert!(!is_read_name("9x/0"));
        assert!(!is_read_name("a-b/0"));
        assert!(!is_read_name("a/0/1"));
    }

    #[test]
    fn whole_numbers_accept_integral_doubles_only() {
        assert_eq!(whole(&json!(70)), Some(70));
        assert_eq!(whole(&json!(70.0)), Some(70));
        assert_eq!(whole(&json!(1.5)), None);
        assert_eq!(whole(&json!(-1)), None);
        assert_eq!(whole(&json!("70")), None);
    }

    #[test]
    fn the_call_key_of_identity_md_14() {
        let key = call_key(
            "image.generate",
            FP,
            &json!({"background": "auto", "prompt": "A picture of 2 lines"}),
            &[1],
        );
        assert_eq!(
            key,
            "9f66a9f2d23c7cf21331a3a45778bf9a660451a79cd4df3aae5f576e0b807d38"
        );
    }

    #[test]
    fn file_entries_keep_a_key_only_when_present() {
        let entry = FileEntry {
            digest: D1.into(),
            kind: "text/plain".into(),
            name: "a".into(),
            size: 8,
            key: None,
        };
        assert_eq!(
            entry.to_value(),
            json!({"digest": D1, "kind": "text/plain", "name": "a", "size": 8})
        );
        let keyed = FileEntry {
            key: Some("ada".into()),
            ..entry.clone()
        };
        assert_eq!(FileEntry::from_value(&keyed.to_value()), Ok(keyed.clone()));
        assert_eq!(keyed.unkeyed_value(), entry.to_value());
        assert!(FileEntry::read(&keyed.to_value(), false).is_err());
    }

    #[test]
    fn outputs_need_exactly_one_member() {
        assert!(RecordOutput::from_value(&json!({})).is_err());
        assert!(RecordOutput::from_value(&json!({"none": true, "list": []})).is_err());
        assert!(RecordOutput::from_value(&json!({"none": false})).is_err());
        assert!(RecordOutput::from_value(&json!({"other": 1})).is_err());
        assert!(RecordOutput::from_value(&json!({"collection": [["a"]]})).is_err());
        assert!(RecordOutput::from_value(&json!({"collection": [[1, {"none": true}]]})).is_err());
        assert_eq!(
            RecordOutput::from_value(&json!({"list": [{"none": true}]})),
            Ok(RecordOutput::List(vec![RecordOutput::None]))
        );
    }

    #[test]
    fn job_handles_follow_the_state() {
        let base = json!({
            "kind": JOB_KIND, "key": D1, "capability": "video.generate",
            "route": {"id": "vid-a@acme", "fingerprint": FP},
            "request": {}, "take": [1], "state": "submitting", "handle": null,
        });
        assert!(JobRecord::from_value(&base).is_ok());
        let mut with_handle = base.clone();
        with_handle["handle"] = json!({"id": "R1"});
        assert!(JobRecord::from_value(&with_handle).is_err());
        with_handle["state"] = json!("submitted");
        assert!(JobRecord::from_value(&with_handle).is_ok());
        let mut submitted_null = base.clone();
        submitted_null["state"] = json!("submitted");
        assert!(JobRecord::from_value(&submitted_null).is_err());
        let mut settled = base.clone();
        settled["state"] = json!("settled");
        assert!(JobRecord::from_value(&settled).is_ok());
        settled["handle"] = json!({"id": "R1"});
        assert!(JobRecord::from_value(&settled).is_ok());
        settled["handle"] = json!("R1");
        assert!(JobRecord::from_value(&settled).is_err());
        let mut unknown = base.clone();
        unknown["state"] = json!("done");
        assert!(JobRecord::from_value(&unknown).is_err());
    }
}
