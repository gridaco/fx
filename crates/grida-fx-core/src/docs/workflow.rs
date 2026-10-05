//! The workflow document, `fx: workflow/v1` (spec/schemas/fx-workflow-v1.schema.json).
//!
//! The typed form keeps every authored value the expander evaluates as raw JSON ([`Value`]):
//! `with`, `if`, `for_each`, `matrix` axes, `key`, `max`, `tables`, `let`, `outputs`, assertion
//! checks. Steps are shared ([`Rc`]) so frames can hold them cheaply. Maps keep authored order.
//! `independent_of` accepts one name or a list. Defaults: `as: item`, `on_reject: fail`,
//! `then: fail`, `view: false`, `feedback: false`.
//!
//! Reading a workflow runs three passes, and the first that refuses wins:
//! 1. the step-shape checks over the raw steps, recursively, with gnode's sentences in gnode's
//!    order (one per step: the first rule a step breaks; a group whose members break a rule is
//!    not checked itself), each as `<file>: steps.<name>[.steps.<name>]: <message>`;
//! 2. the `fx-workflow-v1` schema;
//! 3. the typed conversion, with the reserved-marker rule (identity.md §3) over `with`, `tables`,
//!    `let`, `outputs`, `for_each`, `matrix` values and input defaults, and money read as
//!    [`Usd`].

use super::{Schema, check_kind, truthy, validate};
use crate::error::{Error, Result};
use crate::money::Usd;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A workflow's steps, by name, in declaration order.
pub type Steps = Rc<IndexMap<String, Rc<Step>>>;

/// A `budget:` (`{max_usd}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_usd: Usd,
}

/// `on_fail` of an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnFail {
    #[default]
    Fail,
    Skip,
}

/// An `assert:` entry. `check` is a string expression or a boolean.
#[derive(Debug, Clone, PartialEq)]
pub struct Assertion {
    pub check: Value,
    pub message: String,
    pub on_fail: OnFail,
}

/// `keep_best: {by, order}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepBest {
    pub by: String,
    pub highest: bool,
}

/// `then:` of a regeneration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Then {
    #[default]
    Fail,
    Continue,
    Skip,
    KeepBest(KeepBest),
}

/// A regeneration's `max`: a number from 1 to 12, or an expression known while planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakesMax {
    Count(u32),
    Expr(String),
}

/// `regenerate: {max, then, until, feedback}` (on a group) or `on_reject: {regenerate: …}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regeneration {
    pub max: TakesMax,
    pub then: Then,
    pub until: Option<String>,
    pub feedback: bool,
}

/// A judge's `on_reject`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OnReject {
    #[default]
    Fail,
    Continue,
    Skip,
    Regenerate(Regeneration),
}

/// `pick:` among `takes:`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pick {
    #[default]
    Manual,
    FirstAccepted,
    Best(KeepBest),
}

/// A step's `view:` (recorded only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewSetting {
    Flag(bool),
    Template(String),
}

impl ViewSetting {
    pub fn to_value(&self) -> Value {
        match self {
            ViewSetting::Flag(b) => Value::Bool(*b),
            ViewSetting::Template(s) => Value::String(s.clone()),
        }
    }
}

/// One step (fx-workflow-v1 `$defs/step`).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub uses: Option<String>,
    /// A group's steps.
    pub steps: Option<Steps>,
    pub with: IndexMap<String, Value>,
    /// A string expression or a boolean.
    pub if_: Option<Value>,
    pub needs: Vec<String>,
    pub for_each: Option<Value>,
    pub as_: String,
    pub key: Option<String>,
    /// A number or an expression.
    pub max: Option<Value>,
    pub matrix: Option<IndexMap<String, Value>>,
    pub judges: Option<String>,
    pub on_reject: OnReject,
    pub regenerate: Option<Regeneration>,
    pub takes: Option<u32>,
    /// `None` when `pick:` is not written (manual applies).
    pub pick: Option<Pick>,
    pub asserts: Vec<Assertion>,
    pub at_plan: bool,
    pub budget: Option<Budget>,
    pub concurrency: Option<u32>,
    pub route: Option<String>,
    pub requires: Vec<String>,
    pub independent_of: Vec<String>,
    pub view: ViewSetting,
    pub timeout: Option<f64>,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// The typed workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowDoc {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    /// Input declarations in the shorthand (compiled by [`crate::inputs::compile_inputs`]).
    pub inputs: IndexMap<String, Value>,
    pub tables: IndexMap<String, Value>,
    pub let_: IndexMap<String, Value>,
    pub budget: Option<Budget>,
    pub asserts: Vec<Assertion>,
    pub steps: Steps,
    pub outputs: IndexMap<String, Value>,
    pub view: Option<String>,
}

/// A loaded workflow: the authored document (for the plan digest), its typed form, where it came
/// from, and its home project root.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedWorkflow {
    /// The JSON value the strict YAML loader produced, or the document a builder returned.
    pub document: Value,
    pub workflow: Rc<WorkflowDoc>,
    /// The plan digest's key (identity.md §10): the project-relative POSIX path, or
    /// `<path>:<function>` for a builder.
    pub source: String,
    /// The workflow file (or the builder file), absolute.
    pub path: PathBuf,
}

/// Parses and validates a workflow document. `file` labels errors.
pub fn parse_workflow(document: &Value, file: &str) -> Result<WorkflowDoc> {
    check_kind(Schema::Workflow, document, file)?;
    let mut shape = Vec::new();
    if let Some(steps) = document.get("steps").and_then(Value::as_object) {
        shape_of_steps(steps, "steps", &mut shape);
    }
    if !shape.is_empty() {
        return Err(Error::document(format!("{file}: {}", shape.join("; "))));
    }
    validate(Schema::Workflow, document, file)?;
    Typed { file }.workflow(document)
}

/// Reads a workflow file (strict YAML, schema, typed). `source` is its plan-digest key.
pub fn load_workflow(path: &Path, label: &str, source: &str) -> Result<LoadedWorkflow> {
    let document = crate::yaml::load_file(path, label)?;
    let workflow = parse_workflow(&document, label)?;
    Ok(LoadedWorkflow {
        document,
        workflow: Rc::new(workflow),
        source: source.to_string(),
        // Absolute with symbolic links resolved, like the project roots it is compared with.
        path: crate::inputs::bind::resolve(path),
    })
}

// ------------------------------------------------------------------------------ step shape

/// Present and not `null`, as a field gnode defaults to `None`.
fn set(step: &Map<String, Value>, key: &str) -> bool {
    step.get(key).is_some_and(|v| !v.is_null())
}

/// gnode's step-shape checks over every step of `steps` (`where_` = `steps` or
/// `<group>.steps`), in declaration order. A group's members are checked first; when one of them
/// is refused, the group itself is not checked (its own rules run only on valid members).
fn shape_of_steps(steps: &Map<String, Value>, where_: &str, out: &mut Vec<String>) {
    for (name, step) in steps {
        let Some(step) = step.as_object() else {
            continue;
        };
        let here = format!("{where_}.{name}");
        let before = out.len();
        if let Some(members) = step.get("steps").and_then(Value::as_object) {
            shape_of_steps(members, &format!("{here}.steps"), out);
        }
        if out.len() == before
            && let Some(message) = step_shape(step)
        {
            out.push(format!("{here}: {message}"));
        }
    }
}

/// The first shape rule a step breaks, the rules checked in this order.
fn step_shape(step: &Map<String, Value>) -> Option<String> {
    let group = set(step, "steps");
    if group == set(step, "uses") {
        return Some("a step has either uses: (a node type) or steps: (a group)".into());
    }
    if set(step, "for_each") && set(step, "matrix") {
        return Some("a step repeats over for_each: or matrix:, not both".into());
    }
    if group {
        for name in ["judges", "takes", "pick", "route", "at"] {
            if set(step, name) {
                return Some(format!("{name}: applies to a node step, not a group"));
            }
        }
        if step.get("with").is_some_and(truthy) {
            return Some("with: applies to a node step, not a group".into());
        }
        if let Some(regenerate) = step.get("regenerate").and_then(Value::as_object)
            && !set(regenerate, "until")
        {
            return Some("a group's regenerate: needs until:".into());
        }
    } else if set(step, "regenerate") {
        return Some(
            "regenerate: on a step is written on its judge, as on_reject: { regenerate: ... }; \
             a group takes regenerate: { max, until }"
                .into(),
        );
    }
    let on_reject = step.get("on_reject").filter(|v| !v.is_null());
    if on_reject.is_some_and(|v| v.as_str() != Some("fail")) && !set(step, "judges") {
        return Some("on_reject: belongs on a judge (a step with judges:)".into());
    }
    if set(step, "pick") && !set(step, "takes") {
        return Some("pick: chooses among takes:, so it needs takes:".into());
    }
    let at_plan = step.get("at").and_then(Value::as_str) == Some("plan");
    let free = |key: &str| step.get(key).is_some_and(truthy);
    if at_plan && (free("takes") || free("judges") || free("route")) {
        return Some("an at: plan step is free, local and deterministic".into());
    }
    let until = on_reject
        .and_then(|v| v.get("regenerate"))
        .and_then(|v| v.get("until"));
    if until.is_some_and(truthy) {
        return Some("a judge's regenerate: ends on its own verdict; until: is for groups".into());
    }
    None
}

// ------------------------------------------------------------------------------ typed form

/// The typed conversion of a document that passed the schema. Unexpected shapes are still
/// refused (never a panic), naming the location.
struct Typed<'a> {
    file: &'a str,
}

impl Typed<'_> {
    fn refuse(&self, where_: &str, message: impl std::fmt::Display) -> Error {
        Error::document(format!("{}: {where_}: {message}", self.file))
    }

    fn markers(&self, value: &Value, where_: &str) -> Result<()> {
        crate::value::check_markers(value, where_)
            .map_err(|refused| Error::document(format!("{}: {}", self.file, refused.message)))
    }

    fn workflow(&self, document: &Value) -> Result<WorkflowDoc> {
        let map = self.object(document, "(document)")?;
        let inputs = self.mapping(map.get("inputs"), "inputs")?;
        for (name, declared) in &inputs {
            crate::inputs::check_default_markers(declared, &format!("inputs.{name}")).map_err(
                |refused| Error::document(format!("{}: {}", self.file, refused.message)),
            )?;
        }
        let tables = self.mapping(map.get("tables"), "tables")?;
        for (name, value) in &tables {
            self.markers(value, &format!("tables.{name}"))?;
        }
        let let_ = self.mapping(map.get("let"), "let")?;
        for (name, value) in &let_ {
            self.markers(value, &format!("let.{name}"))?;
        }
        let outputs = self.mapping(map.get("outputs"), "outputs")?;
        for (name, value) in &outputs {
            self.markers(value, &format!("outputs.{name}"))?;
        }
        let steps = match map.get("steps") {
            Some(Value::Object(steps)) => self.steps(steps, "steps")?,
            _ => return Err(self.refuse("steps", "a workflow has at least one step")),
        };
        Ok(WorkflowDoc {
            id: self.string(map.get("id"), "id")?,
            title: self.string(map.get("title"), "title")?,
            description: self.optional_string(map.get("description"), "description")?,
            inputs,
            tables,
            let_,
            budget: self.budget(map.get("budget"), "budget")?,
            asserts: self.asserts(map.get("assert"), "assert")?,
            steps,
            outputs,
            view: self.optional_string(map.get("view"), "view")?,
        })
    }

    fn steps(&self, steps: &Map<String, Value>, where_: &str) -> Result<Steps> {
        let mut out = IndexMap::with_capacity(steps.len());
        for (name, step) in steps {
            let here = format!("{where_}.{name}");
            out.insert(name.clone(), Rc::new(self.step(step, &here)?));
        }
        Ok(Rc::new(out))
    }

    fn step(&self, value: &Value, where_: &str) -> Result<Step> {
        let map = self.object(value, where_)?;
        let at = |key: &str| format!("{where_}.{key}");
        let with = self.mapping(map.get("with"), &at("with"))?;
        for (name, value) in &with {
            self.markers(value, &format!("{where_}.with.{name}"))?;
        }
        let for_each = map.get("for_each").filter(|v| !v.is_null()).cloned();
        if let Some(items) = &for_each {
            self.markers(items, &at("for_each"))?;
        }
        let matrix = match map.get("matrix") {
            None | Some(Value::Null) => None,
            some => {
                let axes = self.mapping(some, &at("matrix"))?;
                for (name, value) in &axes {
                    self.markers(value, &format!("{where_}.matrix.{name}"))?;
                }
                Some(axes)
            }
        };
        let steps = match map.get("steps") {
            None | Some(Value::Null) => None,
            Some(Value::Object(members)) => Some(self.steps(members, &at("steps"))?),
            Some(_) => return Err(self.refuse(&at("steps"), "a group's steps are a mapping")),
        };
        let on_reject = match map.get("on_reject") {
            None | Some(Value::Null) => OnReject::Fail,
            Some(Value::String(word)) => match word.as_str() {
                "fail" => OnReject::Fail,
                "continue" => OnReject::Continue,
                "skip" => OnReject::Skip,
                other => return Err(self.refuse(&at("on_reject"), format!("unknown {other}"))),
            },
            Some(value) => {
                let here = at("on_reject.regenerate");
                OnReject::Regenerate(self.regeneration(value.get("regenerate"), &here)?)
            }
        };
        let regenerate = match map.get("regenerate") {
            None | Some(Value::Null) => None,
            some => Some(self.regeneration(some, &at("regenerate"))?),
        };
        let pick = match map.get("pick") {
            None | Some(Value::Null) => None,
            Some(Value::String(word)) => Some(match word.as_str() {
                "manual" => Pick::Manual,
                "first_accepted" => Pick::FirstAccepted,
                other => return Err(self.refuse(&at("pick"), format!("unknown {other}"))),
            }),
            Some(value) => Some(Pick::Best(
                self.keep_best(value.get("best"), &at("pick.best"))?,
            )),
        };
        let independent_of = match map.get("independent_of") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(name)) => vec![name.clone()],
            some => self.strings(some, &at("independent_of"))?,
        };
        let view = match map.get("view") {
            None | Some(Value::Null) => ViewSetting::Flag(false),
            Some(Value::Bool(flag)) => ViewSetting::Flag(*flag),
            Some(Value::String(template)) => ViewSetting::Template(template.clone()),
            Some(_) => return Err(self.refuse(&at("view"), "a view is a boolean or a template")),
        };
        let timeout = match map.get("timeout") {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => Some(crate::value::as_f64(n)),
            Some(_) => return Err(self.refuse(&at("timeout"), "a timeout is a number")),
        };
        Ok(Step {
            uses: self.optional_string(map.get("uses"), &at("uses"))?,
            steps,
            with,
            if_: map.get("if").filter(|v| !v.is_null()).cloned(),
            needs: self.strings(map.get("needs"), &at("needs"))?,
            for_each,
            as_: self
                .optional_string(map.get("as"), &at("as"))?
                .unwrap_or_else(|| "item".into()),
            key: self.optional_string(map.get("key"), &at("key"))?,
            max: map.get("max").filter(|v| !v.is_null()).cloned(),
            matrix,
            judges: self.optional_string(map.get("judges"), &at("judges"))?,
            on_reject,
            regenerate,
            takes: self.count(map.get("takes"), &at("takes"))?,
            pick,
            asserts: self.asserts(map.get("assert"), &at("assert"))?,
            at_plan: map.get("at").and_then(Value::as_str) == Some("plan"),
            budget: self.budget(map.get("budget"), &at("budget"))?,
            concurrency: self.count(map.get("concurrency"), &at("concurrency"))?,
            route: self.optional_string(map.get("route"), &at("route"))?,
            requires: self.strings(map.get("requires"), &at("requires"))?,
            independent_of,
            view,
            timeout,
            title: self.optional_string(map.get("title"), &at("title"))?,
            description: self.optional_string(map.get("description"), &at("description"))?,
        })
    }

    fn regeneration(&self, value: Option<&Value>, where_: &str) -> Result<Regeneration> {
        let map = match value {
            Some(value) => self.object(value, where_)?,
            None => return Err(self.refuse(where_, "a regeneration is a mapping")),
        };
        let max = match map.get("max") {
            Some(Value::String(expression)) => TakesMax::Expr(expression.clone()),
            some => TakesMax::Count(
                self.count(some, &format!("{where_}.max"))?
                    .ok_or_else(|| self.refuse(&format!("{where_}.max"), "max is required"))?,
            ),
        };
        let then = match map.get("then") {
            None | Some(Value::Null) => Then::Fail,
            Some(Value::String(word)) => match word.as_str() {
                "fail" => Then::Fail,
                "continue" => Then::Continue,
                "skip" => Then::Skip,
                other => {
                    return Err(self.refuse(&format!("{where_}.then"), format!("unknown {other}")));
                }
            },
            Some(value) => Then::KeepBest(
                self.keep_best(value.get("keep_best"), &format!("{where_}.then.keep_best"))?,
            ),
        };
        let feedback = match map.get("feedback") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err(self.refuse(&format!("{where_}.feedback"), "not a boolean")),
        };
        Ok(Regeneration {
            max,
            then,
            until: self.optional_string(map.get("until"), &format!("{where_}.until"))?,
            feedback,
        })
    }

    fn keep_best(&self, value: Option<&Value>, where_: &str) -> Result<KeepBest> {
        let map = match value {
            Some(value) => self.object(value, where_)?,
            None => return Err(self.refuse(where_, "keep_best is a mapping")),
        };
        Ok(KeepBest {
            by: self.string(map.get("by"), &format!("{where_}.by"))?,
            highest: map.get("order").and_then(Value::as_str) == Some("highest"),
        })
    }

    fn asserts(&self, value: Option<&Value>, where_: &str) -> Result<Vec<Assertion>> {
        let items = match value {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(items)) => items,
            Some(_) => return Err(self.refuse(where_, "assertions are a list")),
        };
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let here = format!("{where_}.{i}");
            let map = self.object(item, &here)?;
            out.push(Assertion {
                check: map.get("check").cloned().unwrap_or(Value::Null),
                message: self.string(map.get("message"), &format!("{here}.message"))?,
                on_fail: match map.get("on_fail").and_then(Value::as_str) {
                    Some("skip") => OnFail::Skip,
                    _ => OnFail::Fail,
                },
            });
        }
        Ok(out)
    }

    fn budget(&self, value: Option<&Value>, where_: &str) -> Result<Option<Budget>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(value) => {
                let here = format!("{where_}.max_usd");
                let amount = value
                    .get("max_usd")
                    .ok_or_else(|| self.refuse(&here, "a budget names max_usd"))?;
                let max_usd =
                    Usd::from_value(amount).map_err(|message| self.refuse(&here, message))?;
                Ok(Some(Budget { max_usd }))
            }
        }
    }

    fn object<'v>(&self, value: &'v Value, where_: &str) -> Result<&'v Map<String, Value>> {
        value
            .as_object()
            .ok_or_else(|| self.refuse(where_, "expected a mapping"))
    }

    fn mapping(&self, value: Option<&Value>, where_: &str) -> Result<IndexMap<String, Value>> {
        match value {
            None | Some(Value::Null) => Ok(IndexMap::new()),
            Some(Value::Object(map)) => {
                Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            }
            Some(_) => Err(self.refuse(where_, "expected a mapping")),
        }
    }

    fn string(&self, value: Option<&Value>, where_: &str) -> Result<String> {
        self.optional_string(value, where_)?
            .ok_or_else(|| self.refuse(where_, "expected text"))
    }

    fn optional_string(&self, value: Option<&Value>, where_: &str) -> Result<Option<String>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(self.refuse(where_, "expected text")),
        }
    }

    fn strings(&self, value: Option<&Value>, where_: &str) -> Result<Vec<String>> {
        match value {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, item)| self.string(Some(item), &format!("{where_}.{i}")))
                .collect(),
            Some(_) => Err(self.refuse(where_, "expected a list")),
        }
    }

    /// A whole, non-negative count (`takes`, `concurrency`, a regeneration's `max`).
    fn count(&self, value: Option<&Value>, where_: &str) -> Result<Option<u32>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => {
                let x = crate::value::as_f64(n);
                if x.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&x) {
                    Ok(Some(x as u32))
                } else {
                    Err(self.refuse(where_, format!("{x} is not a count")))
                }
            }
            Some(_) => Err(self.refuse(where_, "expected a number")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shape(steps: Value) -> String {
        let document = json!({"fx": "workflow/v1", "id": "w", "title": "W", "steps": steps});
        parse_workflow(&document, "w.yaml").unwrap_err().message
    }

    #[test]
    fn step_shape_messages_in_gnode_order() {
        let cases = [
            (
                json!({}),
                "a step has either uses: (a node type) or steps: (a group)",
            ),
            (
                json!({"uses": "fx/x@1", "steps": {"a": {"uses": "fx/x@1"}}}),
                "a step has either uses: (a node type) or steps: (a group)",
            ),
            (
                json!({"uses": "fx/x@1", "for_each": [1], "matrix": {"a": [1]}}),
                "a step repeats over for_each: or matrix:, not both",
            ),
            (
                json!({"steps": {"a": {"uses": "fx/x@1"}}, "takes": 2, "judges": "x"}),
                "judges: applies to a node step, not a group",
            ),
            (
                json!({"steps": {"a": {"uses": "fx/x@1"}}, "takes": 2, "route": "a@b"}),
                "takes: applies to a node step, not a group",
            ),
            (
                json!({"steps": {"a": {"uses": "fx/x@1"}}, "at": "plan"}),
                "at: applies to a node step, not a group",
            ),
            (
                json!({"steps": {"a": {"uses": "fx/x@1"}}, "with": {"x": 1}}),
                "with: applies to a node step, not a group",
            ),
            (
                json!({"steps": {"a": {"uses": "fx/x@1"}}, "regenerate": {"max": 2}}),
                "a group's regenerate: needs until:",
            ),
            (
                json!({"uses": "fx/x@1", "regenerate": {"max": 2, "until": "x"}}),
                "regenerate: on a step is written on its judge, as on_reject: { regenerate: ... }; \
                 a group takes regenerate: { max, until }",
            ),
            (
                json!({"uses": "fx/x@1", "on_reject": "continue"}),
                "on_reject: belongs on a judge (a step with judges:)",
            ),
            (
                json!({"uses": "fx/x@1", "pick": "manual"}),
                "pick: chooses among takes:, so it needs takes:",
            ),
            (
                json!({"uses": "fx/x@1", "at": "plan", "route": "a@b"}),
                "an at: plan step is free, local and deterministic",
            ),
            (
                json!({"uses": "fx/x@1", "judges": "a",
                       "on_reject": {"regenerate": {"max": 2, "until": "x"}}}),
                "a judge's regenerate: ends on its own verdict; until: is for groups",
            ),
        ];
        for (step, message) in cases {
            assert_eq!(
                shape(json!({"s": step.clone()})),
                format!("w.yaml: steps.s: {message}"),
                "{step}"
            );
        }
    }

    #[test]
    fn step_shapes_are_collected_and_nested() {
        assert_eq!(
            shape(json!({
                "a": {"uses": "fx/x@1", "pick": "manual"},
                "ok": {"uses": "fx/x@1"},
                "g": {"steps": {"inner": {}}, "takes": 2},
            })),
            "w.yaml: steps.a: pick: chooses among takes:, so it needs takes:; \
             steps.g.steps.inner: a step has either uses: (a node type) or steps: (a group)"
        );
    }

    #[test]
    fn falsy_values_do_not_trip_the_at_plan_rule() {
        let document = json!({"fx": "workflow/v1", "id": "w", "title": "W",
            "steps": {"s": {"uses": "fx/x@1", "at": "plan", "route": null, "judges": null}}});
        let workflow = parse_workflow(&document, "w.yaml").unwrap();
        assert!(workflow.steps["s"].at_plan);
    }

    #[test]
    fn the_schema_runs_after_the_shape_checks() {
        let document = json!({"fx": "workflow/v1", "id": "Bad", "title": "W",
            "steps": {"s": {"uses": "fx/x@1"}}});
        let message = parse_workflow(&document, "w.yaml").unwrap_err().message;
        assert!(message.starts_with("w.yaml: id: "), "{message}");
        let document = json!({"fx": "workflow/v1", "id": "w", "title": "W", "steps": {}});
        let message = parse_workflow(&document, "w.yaml").unwrap_err().message;
        assert!(message.starts_with("w.yaml: steps: "), "{message}");
        let document = json!({"fx": "workflow", "id": "w"});
        assert_eq!(
            parse_workflow(&document, "w.yaml").unwrap_err().message,
            "w.yaml: a workflow file starts with fx: workflow/v1"
        );
    }

    #[test]
    fn typed_steps_keep_their_order_and_defaults() {
        let document = json!({
            "fx": "workflow/v1", "id": "w", "title": "W",
            "inputs": {"n": {"type": "integer", "default": 2}},
            "steps": {
                "z": {"uses": "fx/x@1", "with": {"a": "${{ inputs.n }}"}, "takes": 3,
                      "pick": {"best": {"by": "score", "order": "highest"}}},
                "a": {"uses": "./nodes/j.py#judge", "judges": "z", "independent_of": "z",
                      "on_reject": {"regenerate": {"max": 3, "then": {"keep_best": {"by": "s", "order": "lowest"}}}}},
                "g": {"steps": {"m": {"uses": "fx/x@1", "at": "plan"}},
                      "regenerate": {"max": "${{ 2 }}", "until": "x", "feedback": true}},
                "r": {"uses": "fx/x@1", "for_each": [1, 2], "as": "it", "max": 2,
                      "view": "t.html", "timeout": 2.5, "if": false,
                      "assert": [{"check": true, "message": "m", "on_fail": "skip"}]},
            },
        });
        let w = parse_workflow(&document, "w.yaml").unwrap();
        assert_eq!(w.steps.keys().collect::<Vec<_>>(), ["z", "a", "g", "r"]);
        let z = &w.steps["z"];
        assert_eq!(z.as_, "item");
        assert_eq!(z.takes, Some(3));
        assert_eq!(
            z.pick,
            Some(Pick::Best(KeepBest {
                by: "score".into(),
                highest: true
            }))
        );
        assert_eq!(z.on_reject, OnReject::Fail);
        assert_eq!(z.view, ViewSetting::Flag(false));
        let a = &w.steps["a"];
        assert_eq!(a.independent_of, ["z"]);
        assert_eq!(a.pick, None);
        assert_eq!(
            a.on_reject,
            OnReject::Regenerate(Regeneration {
                max: TakesMax::Count(3),
                then: Then::KeepBest(KeepBest {
                    by: "s".into(),
                    highest: false
                }),
                until: None,
                feedback: false,
            })
        );
        let g = &w.steps["g"];
        assert!(g.steps.as_ref().unwrap()["m"].at_plan);
        assert_eq!(
            g.regenerate.as_ref().unwrap().max,
            TakesMax::Expr("${{ 2 }}".into())
        );
        assert!(g.regenerate.as_ref().unwrap().feedback);
        let r = &w.steps["r"];
        assert_eq!(r.as_, "it");
        assert_eq!(r.for_each, Some(json!([1, 2])));
        assert_eq!(r.max, Some(json!(2)));
        assert_eq!(r.view, ViewSetting::Template("t.html".into()));
        assert_eq!(r.timeout, Some(2.5));
        assert_eq!(r.if_, Some(json!(false)));
        assert_eq!(r.asserts[0].on_fail, OnFail::Skip);
        assert_eq!(w.inputs["n"], json!({"type": "integer", "default": 2}));
    }

    /// Budgets are read as money (they need [`crate::money`]).
    mod with_money {
        use super::*;

        #[test]
        fn budgets_are_micro_dollars() {
            let document = json!({"fx": "workflow/v1", "id": "w", "title": "W",
                "budget": {"max_usd": 1.5},
                "steps": {"s": {"uses": "fx/x@1", "budget": {"max_usd": 0.000001}}}});
            let w = parse_workflow(&document, "w.yaml").unwrap();
            assert_eq!(
                w.budget,
                Some(Budget {
                    max_usd: Usd(1_500_000)
                })
            );
            assert_eq!(w.steps["s"].budget, Some(Budget { max_usd: Usd(1) }));
            let document = json!({"fx": "workflow/v1", "id": "w", "title": "W",
                "budget": {"max_usd": 0.0000001}, "steps": {"s": {"uses": "fx/x@1"}}});
            let message = parse_workflow(&document, "w.yaml").unwrap_err().message;
            assert!(message.starts_with("w.yaml: budget.max_usd: "), "{message}");
        }
    }

    #[test]
    fn reserved_markers_are_refused_where_they_are_read() {
        let digest = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";
        let base = |extra: Value| {
            let mut document = json!({"fx": "workflow/v1", "id": "w", "title": "W",
                "steps": {"s": {"uses": "fx/x@1"}}});
            for (k, v) in extra.as_object().unwrap() {
                document[k] = v.clone();
            }
            parse_workflow(&document, "w.yaml")
        };
        let message =
            base(json!({"steps": {"s": {"uses": "fx/x@1", "with": {"a": {"missing": true}}}}}))
                .unwrap_err()
                .message;
        assert!(message.starts_with("w.yaml: steps.s.with.a: "), "{message}");
        let message = base(json!({"tables": {"t": [{"file": digest}]}}))
            .unwrap_err()
            .message;
        assert!(message.starts_with("w.yaml: tables.t[0]: "), "{message}");
        let message = base(json!({"let": {"x": {"pending": 1}}}))
            .unwrap_err()
            .message;
        assert!(message.starts_with("w.yaml: let.x: "), "{message}");
        let message = base(json!({"outputs": {"o": {"failed": "a#1"}}}))
            .unwrap_err()
            .message;
        assert!(message.starts_with("w.yaml: outputs.o: "), "{message}");
        let message =
            base(json!({"steps": {"s": {"uses": "fx/x@1", "for_each": [{"collection": []}]}}}))
                .unwrap_err()
                .message;
        assert!(
            message.starts_with("w.yaml: steps.s.for_each[0]: "),
            "{message}"
        );
        let message =
            base(json!({"steps": {"s": {"uses": "fx/x@1", "matrix": {"a": [{"missing": true}]}}}}))
                .unwrap_err()
                .message;
        assert!(
            message.starts_with("w.yaml: steps.s.matrix.a[0]: "),
            "{message}"
        );
        let message =
            base(json!({"inputs": {"g": {"x": {"type": "string", "default": {"pending": 0}}}}}))
                .unwrap_err()
                .message;
        assert!(
            message.starts_with("w.yaml: inputs.g.x.default: "),
            "{message}"
        );
        // Ordinary data that only looks similar is fine, and so is an input named like a marker.
        assert!(base(json!({"tables": {"t": {"file": "a.png", "missing": false}}})).is_ok());
        assert!(base(json!({"inputs": {"pending": {"type": "string"}}})).is_ok());
    }
}
