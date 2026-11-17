//! Captured display values and relationships. This ledger never participates in execution.

use super::frame::FrameId;
use super::wiring::SourceKind;
use super::{Expander, instance_id};
use crate::val::{FileValue, Val};
use crate::value::digest;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_VALUE_BYTES: usize = 64 * 1024;
pub const MAX_VALUE_VIEWS: usize = 4096;
pub const MAX_VALUE_LINKS: usize = 16384;
const MAX_DEPTH: usize = 64;
const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValueView {
    pub id: String,
    pub scope_id: Option<String>,
    pub kind: &'static str,
    pub name: String,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueEndpoint {
    Value {
        value_id: String,
    },
    Instance {
        instance_id: String,
        port: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_kind: Option<&'static str>,
    },
}

impl ValueEndpoint {
    pub fn value(id: &str) -> Self {
        Self::Value {
            value_id: id.into(),
        }
    }
    pub fn input(instance_id: &str, port: &str) -> Self {
        Self::Instance {
            instance_id: instance_id.into(),
            port: port.into(),
            source_kind: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ValueLink {
    pub source: ValueEndpoint,
    pub target: ValueEndpoint,
}

/// Engine-internal candidate for copying already-bound input bytes into a run. This type is
/// intentionally not serializable: locations are private and never enter graph/event data.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueFileSource {
    pub value_id: String,
    pub file: FileValue,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Ledger {
    pub views: IndexMap<String, ValueView>,
    pub links: BTreeSet<ValueLink>,
    pub files: IndexMap<(String, String), ValueFileSource>,
    bytes: usize,
}
impl Ledger {
    pub fn insert(&mut self, mut view: ValueView) {
        if !self.views.contains_key(&view.id) && self.views.len() >= MAX_VALUE_VIEWS {
            if let Some((_, last)) = self.views.last_mut() {
                last.diagnostic =
                    Some("Additional presentation evidence exceeds its recording limit.");
            }
            return;
        }
        let previous = self
            .views
            .get(&view.id)
            .and_then(|v| v.data.as_ref())
            .map_or(0, |v| serde_json::to_vec(v).map_or(0, |v| v.len()));
        self.bytes = self.bytes.saturating_sub(previous);
        let size = view
            .data
            .as_ref()
            .map_or(0, |v| serde_json::to_vec(v).map_or(0, |v| v.len()));
        if self.bytes.saturating_add(size) > MAX_TOTAL_BYTES {
            view.data = None;
            view.state = "unavailable";
            view.diagnostic = Some("Presentation evidence exceeds its recording limit.");
        } else {
            self.bytes += size;
        }
        self.views.insert(view.id.clone(), view);
    }
    fn file_sources(&mut self, value_id: &str, value: &Val) {
        if self.files.len() >= MAX_VALUE_LINKS {
            return;
        }
        match value {
            Val::File(file) => {
                let key = (value_id.to_string(), file.digest.clone());
                self.files.entry(key).or_insert_with(|| ValueFileSource {
                    value_id: value_id.into(),
                    file: FileValue {
                        digest: file.digest.clone(),
                        kind: file.kind.clone(),
                        name: portable_file_name(&file.name),
                        size: file.size,
                        key: file.key.clone(),
                        content: None,
                        location: file.location.clone(),
                    },
                });
            }
            Val::List(values) => {
                for value in values {
                    self.file_sources(value_id, value);
                }
            }
            Val::Object(values) => {
                for value in values.values() {
                    self.file_sources(value_id, value);
                }
            }
            Val::Collection(values) => {
                for (_, value) in &values.items {
                    self.file_sources(value_id, value);
                }
            }
            _ => {}
        }
    }

    pub fn link(&mut self, source: ValueEndpoint, target: ValueEndpoint) {
        // Existing bindings already record direct instance-to-instance data flow.
        if matches!(
            (&source, &target),
            (
                ValueEndpoint::Instance { .. },
                ValueEndpoint::Instance { .. }
            )
        ) {
            return;
        }
        if self.links.len() >= MAX_VALUE_LINKS {
            if let Some((_, last)) = self.views.last_mut() {
                last.diagnostic =
                    Some("Additional presentation connections exceed their recording limit.");
            }
            return;
        }
        if source != target {
            self.links.insert(ValueLink { source, target });
        }
    }
    pub fn sources(&mut self, sources: Vec<ValueEndpoint>, target: ValueEndpoint) {
        for source in sources {
            self.link(source, target.clone());
        }
    }
}

#[derive(Debug, Clone)]
enum Source {
    Value(String),
    Members(BTreeMap<String, String>),
    Result {
        id: String,
        kind: &'static str,
        ports: Vec<String>,
        any: bool,
    },
}
#[derive(Debug, Clone, Default)]
struct Access {
    capture: Option<usize>,
    sources: Vec<Source>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct Trace {
    pub referenced_values: BTreeSet<String>,
    captures: Vec<Option<BTreeSet<ValueEndpoint>>>,
    accesses: Vec<Access>,
}
impl Trace {
    fn current(&self) -> Option<usize> {
        self.captures
            .len()
            .checked_sub(1)
            .filter(|&i| self.captures[i].is_some())
    }
    pub fn begin(&mut self) {
        self.captures.push(Some(BTreeSet::new()));
    }
    pub fn end(&mut self) -> Vec<ValueEndpoint> {
        self.captures
            .pop()
            .flatten()
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }
    pub fn suspend(&mut self) {
        self.captures.push(None);
    }
    pub fn resume(&mut self) {
        self.captures.pop();
    }
    pub fn begin_access(&mut self) {
        self.accesses.push(Access {
            capture: self.current(),
            sources: Vec::new(),
        });
    }
    pub fn end_access(&mut self, ok: bool) {
        let Some(access) = self.accesses.pop() else {
            return;
        };
        if !ok {
            return;
        }
        if let Some(outer) = self.accesses.last_mut()
            && outer.capture == access.capture
        {
            outer.sources.extend(access.sources);
            return;
        }
        for source in access.sources {
            self.emit(access.capture, source, None);
        }
    }
    fn add(&mut self, source: Source) {
        let capture = self.current();
        if let Some(access) = self.accesses.last_mut()
            && access.capture == capture
        {
            access.sources.push(source);
        } else {
            self.emit(capture, source, None);
        }
    }
    pub fn value(&mut self, id: &str) {
        self.add(Source::Value(id.into()));
    }
    pub fn members(&mut self, ids: BTreeMap<String, String>) {
        self.add(Source::Members(ids));
    }
    pub fn result(&mut self, id: &str, kind: SourceKind, ports: Vec<String>, any: bool) {
        let kind = match kind {
            SourceKind::Output => "output",
            SourceKind::Fact => "fact",
            _ => return,
        };
        self.add(Source::Result {
            id: id.into(),
            kind,
            ports,
            any,
        });
    }
    pub fn references(&mut self, bindings: &[super::wiring::Binding]) {
        for b in bindings {
            self.result(&b.source, b.source_kind, vec![b.source_port.clone()], false);
        }
    }
    pub fn member(&mut self, name: &str) {
        let Some(access) = self.accesses.last_mut() else {
            return;
        };
        let capture = access.capture;
        let sources = std::mem::take(&mut access.sources);
        for source in sources {
            self.emit(capture, source, Some(name));
        }
    }
    pub fn unknown_member(&mut self) {
        if let Some(a) = self.accesses.last_mut() {
            a.sources.clear();
        }
    }
    fn emit(&mut self, index: Option<usize>, source: Source, member: Option<&str>) {
        match &source {
            Source::Value(id) => {
                if self.referenced_values.len() < MAX_VALUE_VIEWS {
                    self.referenced_values.insert(id.clone());
                }
            }
            Source::Members(ids) => {
                for (name, id) in ids {
                    if member.is_none_or(|m| m == name)
                        && self.referenced_values.len() < MAX_VALUE_VIEWS
                    {
                        self.referenced_values.insert(id.clone());
                    }
                }
            }
            Source::Result { .. } => {}
        }
        let Some(capture) = index
            .and_then(|i| self.captures.get_mut(i))
            .and_then(Option::as_mut)
        else {
            return;
        };
        match source {
            Source::Value(id) => {
                capture.insert(ValueEndpoint::value(&id));
            }
            Source::Members(ids) => {
                for (_, id) in ids
                    .into_iter()
                    .filter(|(name, _)| member.is_none_or(|m| m == name))
                {
                    capture.insert(ValueEndpoint::value(&id));
                }
            }
            Source::Result {
                id,
                kind,
                ports,
                any,
            } => {
                let selected = match member {
                    Some(_) if any => vec!["value".into()],
                    Some(name) if kind == "fact" || ports.iter().any(|p| p == name) => {
                        vec![name.into()]
                    }
                    Some(_) => Vec::new(),
                    None => ports,
                };
                for port in selected {
                    capture.insert(ValueEndpoint::Instance {
                        instance_id: id.clone(),
                        port,
                        source_kind: Some(kind),
                    });
                }
            }
        }
    }
}

fn fits(value: &Val, depth: usize, budget: &mut usize) -> bool {
    if depth > MAX_DEPTH || *budget == 0 {
        return false;
    }
    let cost = match value {
        Val::Str(text) => text.len() + 16,
        Val::File(file) => file.name.len() + file.kind.len() + 256,
        _ => 16,
    };
    if cost > *budget {
        return false;
    }
    *budget -= cost;
    match value {
        Val::List(values) => values.iter().all(|value| fits(value, depth + 1, budget)),
        Val::Object(values) => values.iter().all(|(key, value)| {
            if key.len() > *budget {
                return false;
            }
            *budget -= key.len();
            fits(value, depth + 1, budget)
        }),
        Val::Collection(values) => values.items.iter().all(|(key, value)| {
            if key.len() > *budget {
                return false;
            }
            *budget -= key.len();
            fits(value, depth + 1, budget)
        }),
        _ => true,
    }
}

fn fits_json(value: &Value, depth: usize, budget: &mut usize) -> bool {
    if depth > MAX_DEPTH || *budget == 0 {
        return false;
    }
    let cost = value.as_str().map_or(16, |text| text.len() + 16);
    if cost > *budget {
        return false;
    }
    *budget -= cost;
    match value {
        Value::Array(values) => values
            .iter()
            .all(|value| fits_json(value, depth + 1, budget)),
        Value::Object(values) => values.iter().all(|(key, value)| {
            if key.len() > *budget {
                return false;
            }
            *budget -= key.len();
            fits_json(value, depth + 1, budget)
        }),
        _ => true,
    }
}

fn contains_file(value: &Val) -> bool {
    match value {
        Val::File(_) => true,
        Val::Object(values) => values.values().any(contains_file),
        Val::List(values) => values.iter().any(contains_file),
        Val::Collection(values) => values.items.iter().any(|(_, value)| contains_file(value)),
        _ => false,
    }
}

fn bounded_json_depth(value: &Value, depth: usize) -> bool {
    depth <= MAX_DEPTH
        && match value {
            Value::Array(values) => values
                .iter()
                .all(|value| bounded_json_depth(value, depth + 1)),
            Value::Object(values) => values
                .values()
                .all(|value| bounded_json_depth(value, depth + 1)),
            _ => true,
        }
}

fn portable_file_name(name: &str) -> String {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let name: String = basename
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    if matches!(name.as_str(), "" | "." | "..") {
        "file".into()
    } else {
        name
    }
}

fn encode(value: &Val, depth: usize, budget: &mut usize) -> Option<Value> {
    if depth > MAX_DEPTH || *budget == 0 {
        return None;
    }
    *budget = budget.saturating_sub(1);
    let encoded = match value {
        Val::File(file) => {
            let mut data = json!({"digest":file.digest,"kind":file.kind,"name":portable_file_name(&file.name),"size":file.size});
            if let Some(key) = &file.key {
                data["key"] = json!(key)
            }
            json!({"file":data})
        }
        Val::List(items) => {
            json!({"list":items.iter().map(|v|encode(v,depth+1,budget)).collect::<Option<Vec<_>>>()?})
        }
        Val::Collection(items) => {
            json!({"collection":items.items.iter().map(|(k,v)|Some(json!([k,encode(v,depth+1,budget)?]))).collect::<Option<Vec<_>>>()?})
        }
        Val::Object(values) if values.values().any(contains_file) => {
            let fields: serde_json::Map<String, Value> = values
                .iter()
                .map(|(name, value)| Some((name.clone(), encode(value, depth + 1, budget)?)))
                .collect::<Option<_>>()?;
            json!({"object": fields})
        }
        Val::Missing => json!({"none":true}),
        Val::Pending(_) | Val::Failed(_) | Val::View(_) => return None,
        _ => json!({"value":value.plain()?}),
    };
    if serde_json::to_vec(&encoded).ok()?.len() > MAX_VALUE_BYTES
        || (depth == 0 && !bounded_json_depth(&encoded, 0))
    {
        None
    } else {
        Some(encoded)
    }
}

impl Expander<'_> {
    pub(crate) fn value_id(&self, frame: FrameId, kind: &str, name: &str) -> String {
        let data = &self.frames[frame.0];
        let owner = data.workflow_scope.as_deref();
        let contextual = kind == "let";
        let context = if contextual {
            json!({"prefix":data.prefix,"take":data.takes,"judging":data.judging,"variables":data.variables.keys().collect::<Vec<_>>(),"consumer":(!data.variables.is_empty() || data.judging.is_some()).then(|| self.value_consumers.last()).flatten()})
        } else {
            Value::Null
        };
        format!(
            "value:{}",
            digest(&json!({"scope":owner,"kind":kind,"name":name,"context":context}))
        )
    }
    pub(crate) fn value_view(
        &mut self,
        frame: FrameId,
        kind: &'static str,
        name: &str,
        value: &Val,
    ) -> String {
        let id = self.value_id(frame, kind, name);
        if self.value_ledger.views.len() >= MAX_VALUE_VIEWS
            && !self.value_ledger.views.contains_key(&id)
        {
            if let Some((_, last)) = self.value_ledger.views.last_mut() {
                last.diagnostic =
                    Some("Additional presentation evidence exceeds its recording limit.");
            }
            return id;
        }
        let scope_id = if kind == "let" {
            self.frames[frame.0].display_scope.clone()
        } else {
            self.frames[frame.0].workflow_scope.clone()
        };
        let context = (kind == "let").then(|| {
            let data = &self.frames[frame.0];
            let label = if !data.variables.is_empty() || data.judging.is_some() {
                self.value_consumers
                    .last()
                    .cloned()
                    .unwrap_or_else(|| instance_id(data.prefix.trim_end_matches('.'), &data.takes))
            } else {
                instance_id(data.prefix.trim_end_matches('.'), &data.takes)
            };
            let label = if label.starts_with('#') {
                format!("root{label}")
            } else {
                label
            };
            label
                .chars()
                .map(|c| {
                    if c.is_control() {
                        ' '
                    } else if c == '\\' {
                        '∖'
                    } else {
                        c
                    }
                })
                .take(256)
                .collect()
        });
        let mut state = if value.contains_pending() {
            "pending"
        } else if matches!(value, Val::Missing) {
            "absent"
        } else if value.contains_failed() || value.contains_view() {
            "unavailable"
        } else {
            "known"
        };
        let mut budget = MAX_VALUE_BYTES;
        let data = if state == "known" && fits(value, 0, &mut budget) {
            let mut encoded_budget = MAX_VALUE_BYTES;
            encode(value, 0, &mut encoded_budget)
        } else {
            None
        };
        let diagnostic = if state == "known" && data.is_none() {
            state = "unavailable";
            Some("Presentation evidence exceeds its recording limit.")
        } else {
            None
        };
        self.value_ledger.insert(ValueView {
            id: id.clone(),
            scope_id,
            kind,
            name: name.into(),
            state,
            data,
            context,
            diagnostic,
        });
        if kind == "input"
            && self
                .value_ledger
                .views
                .get(&id)
                .is_some_and(|view| view.state == "known" && view.data.is_some())
        {
            self.value_ledger.file_sources(&id, value);
        }
        id
    }
    pub(crate) fn unavailable_value_view(
        &mut self,
        frame: FrameId,
        kind: &'static str,
        name: &str,
    ) {
        let id = self.value_id(frame, kind, name);
        self.value_ledger.insert(ValueView {
            id,
            scope_id: self.frames[frame.0].workflow_scope.clone(),
            kind,
            name: name.into(),
            state: "unavailable",
            data: None,
            context: None,
            diagnostic: Some("Not evaluated in this recording."),
        });
    }

    pub(crate) fn input_value_views(&mut self, frame: FrameId) {
        let inputs = self.frames[frame.0].inputs.clone();
        let names: Vec<_> = self.frames[frame.0]
            .workflow
            .inputs
            .keys()
            .cloned()
            .collect();
        for name in names {
            if let Some(value) = inputs.get(&name) {
                self.value_view(frame, "input", &name, value);
            }
        }
    }
    pub(crate) fn value_members(&self, frame: FrameId, kind: &str) -> BTreeMap<String, String> {
        let data = &self.frames[frame.0];
        let names: Vec<_> = if kind == "input" {
            data.inputs.keys().collect()
        } else {
            data.workflow.tables.keys().collect()
        };
        names
            .into_iter()
            .take(MAX_VALUE_VIEWS)
            .map(|name| (name.clone(), self.value_id(frame, kind, name)))
            .collect()
    }
    pub(crate) fn finish_value_views(&mut self) {
        let mut referenced: BTreeSet<_> = self
            .value_ledger
            .links
            .iter()
            .flat_map(|l| [&l.source, &l.target])
            .filter_map(|e| match e {
                ValueEndpoint::Value { value_id } => Some(value_id.clone()),
                _ => None,
            })
            .collect();
        referenced.extend(self.wiring.values.referenced_values.iter().cloned());
        let mut tables = IndexMap::new();
        let mut seen = BTreeSet::new();
        for (index, frame) in self.frames.iter().enumerate() {
            if !seen.insert(frame.workflow_scope.clone()) {
                continue;
            }
            for name in frame.workflow.tables.keys().take(MAX_VALUE_VIEWS) {
                let id = self.value_id(FrameId(index), "table", name);
                if referenced.contains(&id) {
                    tables
                        .entry(id)
                        .or_insert_with(|| (FrameId(index), name.clone()));
                }
            }
        }
        for (_, (frame, name)) in tables {
            let workflow = std::rc::Rc::clone(&self.frames[frame.0].workflow);
            let raw = &workflow.tables[&name];
            let mut budget = MAX_VALUE_BYTES;
            if fits_json(raw, 0, &mut budget) {
                self.value_view(frame, "table", &name, &Val::from_json(raw));
            } else {
                let id = self.value_id(frame, "table", &name);
                self.value_ledger.insert(ValueView {
                    id,
                    scope_id: self.frames[frame.0].workflow_scope.clone(),
                    kind: "table",
                    name,
                    state: "unavailable",
                    data: None,
                    context: None,
                    diagnostic: Some("Presentation evidence exceeds its recording limit."),
                });
            }
        }
        let known: BTreeSet<_> = self.value_ledger.views.keys().cloned().collect();
        self.value_ledger.links.retain(|link| {
            [&link.source, &link.target]
                .into_iter()
                .all(|endpoint| match endpoint {
                    ValueEndpoint::Value { value_id } => known.contains(value_id),
                    ValueEndpoint::Instance { instance_id, .. } => {
                        self.instances.contains_key(instance_id)
                    }
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::testing::{Fixture, workflow_doc};
    use crate::val::FileValue;

    #[test]
    fn known_null_absent_pending_and_bounded_evidence_are_distinct() {
        let mut fixture = Fixture::new(workflow_doc(Vec::new()));
        let (mut ex, root) = fixture.expander();
        let null = ex.value_view(root, "input", "null", &Val::Null);
        let missing = ex.value_view(root, "input", "missing", &Val::Missing);
        let pending = ex.value_view(
            root,
            "input",
            "pending",
            &Val::Pending(Box::new(crate::val::Pending::of("future#1", None))),
        );
        let large = ex.value_view(
            root,
            "input",
            "large",
            &Val::Str("x".repeat(MAX_VALUE_BYTES + 1)),
        );
        assert_eq!(
            ex.value_ledger.views[&null].data,
            Some(json!({"value":null}))
        );
        assert_eq!(ex.value_ledger.views[&missing].state, "absent");
        assert_eq!(ex.value_ledger.views[&pending].state, "pending");
        assert_eq!(ex.value_ledger.views[&large].state, "unavailable");
        assert!(ex.value_ledger.views[&large].diagnostic.is_some());
    }

    #[test]
    fn encoded_depth_limit_accounts_for_explicit_list_wrappers() {
        let mut value = Val::Str("leaf".into());
        for _ in 0..31 {
            value = Val::List(vec![value]);
        }
        let data = encode(&value, 0, &mut MAX_VALUE_BYTES.clone()).unwrap();
        assert!(bounded_json_depth(&data, 0));
        value = Val::List(vec![value]);
        assert!(encode(&value, 0, &mut MAX_VALUE_BYTES.clone()).is_none());
    }

    #[test]
    fn typed_files_are_portable_while_file_shaped_json_stays_opaque() {
        let file = Val::File(Box::new(FileValue {
            digest: "a".repeat(64),
            kind: "image/png".into(),
            name: "/private/acme/source.png".into(),
            size: 12,
            key: None,
            content: None,
            location: Some("/private/acme/source.png".into()),
        }));
        let encoded = encode(&file, 0, &mut MAX_VALUE_BYTES.clone()).unwrap();
        assert_eq!(encoded["file"]["name"], "source.png");
        assert!(!encoded.to_string().contains("private"));
        let nested = Val::Object(IndexMap::from([(
            "layers".into(),
            Val::List(vec![Val::Object(IndexMap::from([("image".into(), file)]))]),
        )]));
        let nested = encode(&nested, 0, &mut MAX_VALUE_BYTES.clone()).unwrap();
        assert_eq!(
            nested["object"]["layers"]["list"][0]["object"]["image"]["file"]["name"],
            "source.png"
        );
        assert!(!nested.to_string().contains("private"));
        let opaque = Val::from_json(&json!({"file":{"digest":"a".repeat(64),"kind":"image/png"}}));
        let encoded = encode(&opaque, 0, &mut MAX_VALUE_BYTES.clone()).unwrap();
        assert!(encoded.get("file").is_none());
        assert!(encoded["value"].get("file").is_some());
        let mut fixture = Fixture::new(workflow_doc(Vec::new()));
        let (mut ex, root) = fixture.expander();
        ex.value_view(root, "input", "opaque", &opaque);
        assert!(ex.value_ledger.files.is_empty());
        let nested_value = Val::List(vec![Val::Object(IndexMap::from([(
            "image".into(),
            Val::File(Box::new(FileValue {
                digest: "a".repeat(64),
                kind: "image/png".into(),
                name: "source.png".into(),
                size: 12,
                key: None,
                content: None,
                location: Some("/private/acme/source.png".into()),
            })),
        )]))]);
        ex.value_view(root, "input", "nested", &nested_value);
        assert_eq!(ex.value_ledger.files.len(), 1);
        assert!(
            ex.value_ledger
                .files
                .values()
                .next()
                .unwrap()
                .file
                .content
                .is_none()
        );
    }
}
