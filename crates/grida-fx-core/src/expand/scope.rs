//! Scopes and step views.
//!
//! [`StepScope`] resolves names in a frame, in this order: frame variables
//! (shadowing the roots), `inputs`, `tables` (raw, never evaluated), `let` (a [`ViewData::Lets`]
//! view; `let.<n>` resolves the let lazily in the *referencing* scope, unmemoized, refused when it
//! refers back to itself), `steps` (a [`ViewData::Steps`] view), else `unknown name 'x'`.
//! `steps.<name>` finds the step lexically ([`Expander::find`]), expands it, and returns
//! [`Expander::view_of`]: missing (absent), its pending token, or a node / repeat / group /
//! workflow / regenerating view, each as stage-gen's engine reads it; a node view's `outputs`,
//! `facts` and `take` are answered by `judge.rs` ([`Expander::node_result`],
//! [`Expander::node_take`]). `.*` results are [`ViewData::Every`]; finishing turns them
//! into collections (files keyed, verdicts kept for present keys) and refuses any other view. A
//! repeat indexed by a key only a run gives waits on that key and every instance of the repeat
//! ([`Expander::repeat_item_pending`]).
//!
//! [`TemplateScope`] is the prompt-file scope (identity.md §5): `vars`, `inputs`, then each key of
//! `vars` by its bare name; else `a prompt sees vars and inputs, not 'x'`. It has no views.

use super::Expander;
use super::frame::{ExpId, ExpKind, FrameId, Until};
use super::judge::What;
use crate::docs::workflow::Then;
use crate::expr::{self, ExprError, Scope};
use crate::text::py_repr_str;
use crate::val::{Collection, Pending, Val, Verdict, ViewId};
use indexmap::IndexMap;
use serde_json::json;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

/// A view handed to an expression.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViewData {
    /// `steps.x` of a node step; `pinned` is the judged take while a judge reads its subject.
    Node { exp: ExpId, pinned: Option<u32> },
    /// `steps.x` of a repeat.
    Repeat {
        exp: ExpId,
        judging: Option<(String, u32)>,
    },
    /// `steps.g` of a group (member lookup in `scope`).
    Group {
        exp: ExpId,
        scope: FrameId,
        judging: Option<(String, u32)>,
    },
    /// `steps.w` of a used workflow (`outputs` evaluated in `scope`).
    Workflow { exp: ExpId, scope: FrameId },
    /// `steps.g` of a regenerating group.
    Regenerating {
        exp: ExpId,
        judging: Option<(String, u32)>,
    },
    /// The result of `.*`: keyed elements with per-key verdicts.
    Every {
        items: Vec<(String, Val)>,
        verdicts: IndexMap<String, Option<Verdict>>,
    },
    /// `steps`.
    Steps { frame: FrameId },
    /// `let`.
    Lets { frame: FrameId },
    /// A native select's outputs: every field is the chosen value.
    AnyPort { value: Val },
}

/// The step scope of a frame.
pub(crate) struct StepScope<'x, 'a> {
    pub ex: &'x mut Expander<'a>,
    pub frame: FrameId,
}

impl Scope for StepScope<'_, '_> {
    fn root(&mut self, name: &str) -> Result<Val, ExprError> {
        self.ex.root(self.frame, name)
    }

    fn view_member(&mut self, view: ViewId, name: &str) -> Result<Val, ExprError> {
        self.ex.view_member(self.frame, view, name)
    }

    fn view_item(&mut self, view: ViewId, index: &Val) -> Result<Val, ExprError> {
        self.ex.view_item(self.frame, view, index)
    }

    fn view_item_pending(
        &mut self,
        view: ViewId,
        index: &Pending,
    ) -> Option<Result<Val, ExprError>> {
        self.ex.repeat_item_pending(view, index).map(Ok)
    }

    fn view_every(&mut self, view: ViewId) -> Result<Val, ExprError> {
        self.ex.view_every(self.frame, view)
    }

    fn view_len(&mut self, view: ViewId) -> Result<usize, ExprError> {
        self.ex.view_len(self.frame, view)
    }

    fn finish_view(&mut self, view: ViewId) -> Result<Val, ExprError> {
        self.ex.finish_view(self.frame, view)
    }

    fn facts(&mut self, value: &Val) -> Result<Val, ExprError> {
        match value {
            // A step that did not run, or failed, has no file to read facts from: its readers
            // see the same missing or failed value.
            Val::Missing | Val::Failed(_) => Ok(value.clone()),
            _ => file_facts(value),
        }
    }
}

/// The prompt-file scope.
pub(crate) struct TemplateScope {
    pub vars: Val,
    pub inputs: Rc<IndexMap<String, Val>>,
}

impl Scope for TemplateScope {
    fn root(&mut self, name: &str) -> Result<Val, ExprError> {
        match name {
            "vars" => Ok(self.vars.clone()),
            "inputs" => Ok(Val::Object((*self.inputs).clone())),
            _ => match &self.vars {
                Val::Object(vars) if vars.contains_key(name) => Ok(vars[name].clone()),
                _ => Err(ExprError::new(format!(
                    "a prompt sees vars and inputs, not {}",
                    py_repr_str(name)
                ))),
            },
        }
    }

    fn view_member(&mut self, _view: ViewId, _name: &str) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_item(&mut self, _view: ViewId, _index: &Val) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_every(&mut self, _view: ViewId) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_len(&mut self, _view: ViewId) -> Result<usize, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn finish_view(&mut self, _view: ViewId) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn facts(&mut self, value: &Val) -> Result<Val, ExprError> {
        file_facts(value)
    }
}

/// `facts(f)` of a file (identity.md §4); anything else is refused.
fn file_facts(value: &Val) -> Result<Val, ExprError> {
    let Val::File(file) = value else {
        return Err(ExprError::new(format!(
            "facts() needs a file, not {}",
            value.kind_word()
        )));
    };
    let bytes = file
        .read_bytes()
        .map_err(|reason| ExprError::new(format!("{}: {reason}", file.name)))?;
    crate::facts::file_facts(&bytes, &file.kind)
        .map(|facts| Val::from_json(&facts))
        .map_err(|reason| ExprError::new(format!("{}: {reason}", file.name)))
}

/// `no step 'x'`.
#[inline(never)]
fn no_step(name: &str) -> ExprError {
    ExprError::new(format!("no step {}", py_repr_str(name)))
}

/// `.* applies to a repeated step`.
fn not_repeated() -> ExprError {
    ExprError::new(".* applies to a repeated step")
}

/// The word for a step view in type errors (the evaluator's `view_kind`).
const A_STEP: &str = "a step";

impl Expander<'_> {
    /// Hands a view to an expression.
    pub(crate) fn new_view(&mut self, data: ViewData) -> Val {
        self.views.push(data);
        Val::View(ViewId(self.views.len() - 1))
    }

    /// A step scope's root names (module doc).
    pub(crate) fn root(&mut self, frame: FrameId, name: &str) -> Result<Val, ExprError> {
        let data = &self.frames[frame.0];
        // Variables shadow the roots.
        if let Some(value) = data.variables.get(name) {
            return Ok(value.clone());
        }
        match name {
            "inputs" => Ok(Val::Object((*data.inputs).clone())),
            // Raw: expressions inside tables are never evaluated.
            "tables" => Ok(Val::Object(
                data.workflow
                    .tables
                    .iter()
                    .map(|(key, value)| (key.clone(), Val::from_json(value)))
                    .collect(),
            )),
            "let" => Ok(self.new_view(ViewData::Lets { frame })),
            "steps" => Ok(self.new_view(ViewData::Steps { frame })),
            _ => Err(ExprError::new(format!(
                "unknown name {}",
                py_repr_str(name)
            ))),
        }
    }

    /// The value a reference to an expanded step reads: missing for an absent step, an error
    /// for one still expanding, a pending repeat's token, else a view of its kind.
    ///
    /// `judging` is the evaluating frame's judging pin; a node view is pinned to its take only
    /// when the node is the judged step (callers check that, see `view_at`). A node whose
    /// instances are being created right now refers back to itself (FX refuses the cycle gnode
    /// read as missing).
    pub(crate) fn view_of(
        &mut self,
        exp: ExpId,
        judging: Option<(String, u32)>,
    ) -> Result<Val, ExprError> {
        let expansion = &self.exps[exp.0];
        match expansion.kind {
            ExpKind::Absent => Ok(Val::Missing),
            ExpKind::Expanding => Err(ExprError::new("a step refers back to itself")),
            ExpKind::Pending => Ok(match &expansion.token {
                Some(token) => Val::Pending(Box::new(token.clone())),
                None => Val::Missing,
            }),
            ExpKind::Node => {
                if self.is_instantiating(exp) {
                    return Err(self.refers_back(exp));
                }
                let pinned =
                    judging.and_then(|(name, take)| (name == expansion.name).then_some(take));
                Ok(self.new_view(ViewData::Node { exp, pinned }))
            }
            ExpKind::Repeat => Ok(self.new_view(ViewData::Repeat { exp, judging })),
            ExpKind::Group => match expansion.scope {
                Some(scope) => Ok(self.new_view(ViewData::Group {
                    exp,
                    scope,
                    judging,
                })),
                None => Ok(Val::Missing),
            },
            ExpKind::Workflow => match expansion.scope {
                Some(scope) => Ok(self.new_view(ViewData::Workflow { exp, scope })),
                None => Ok(Val::Missing),
            },
            ExpKind::Regenerating => Ok(self.new_view(ViewData::Regenerating { exp, judging })),
        }
    }

    /// [`Expander::view_of`] seen from `frame`: a node is pinned only when it is the very step
    /// the frame's judge judges (gnode compares expansions, not names).
    fn view_at(
        &mut self,
        frame: FrameId,
        exp: ExpId,
        judging: Option<(String, u32)>,
    ) -> Result<Val, ExprError> {
        let judging = match judging {
            Some((name, take)) if self.exps[exp.0].kind == ExpKind::Node => {
                let scope = self.scope_of(frame);
                let judged = if self.frames[scope.0].steps.contains_key(&name) {
                    Some(self.step(scope, &name)?)
                } else {
                    None
                };
                (judged == Some(exp)).then_some((name, take))
            }
            other => other,
        };
        self.view_of(exp, judging)
    }

    /// The node expansion and pin of a node view.
    fn node_view(&self, value: &Val) -> Option<(ExpId, Option<u32>)> {
        match value {
            Val::View(id) => match &self.views[id.0] {
                ViewData::Node { exp, pinned } => Some((*exp, *pinned)),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn view_member(
        &mut self,
        frame: FrameId,
        view: ViewId,
        name: &str,
    ) -> Result<Val, ExprError> {
        // `steps.x` expands `x` inside the evaluation that reads it: kept small (see
        // `expr::evaluate`); every other view is answered by `view_member_of`.
        match self.views[view.0] {
            ViewData::Steps { frame: steps_frame } => self.steps_member(steps_frame, name),
            ViewData::Group { .. } | ViewData::Workflow { .. } => {
                self.scope_member(frame, view, name)
            }
            _ => self.view_member_of(frame, view, name),
        }
    }

    /// A member step of a group or a used workflow (a used workflow's `outputs` too), expanded.
    #[inline(never)]
    fn scope_member(&mut self, frame: FrameId, view: ViewId, name: &str) -> Result<Val, ExprError> {
        let (exp, scope, judging, workflow) = match &self.views[view.0] {
            ViewData::Group {
                exp,
                scope,
                judging,
            } => (*exp, *scope, judging.clone(), false),
            ViewData::Workflow { exp, scope } => (*exp, *scope, None, true),
            _ => return self.view_member_of(frame, view, name),
        };
        if workflow && name == "outputs" {
            return Ok(self.workflow_outputs(exp, scope));
        }
        let member = self.member_of(scope, name)?;
        self.view_at(frame, member, judging)
    }

    /// `steps.<name>`: the step found lexically, expanded, and its view.
    #[inline(never)]
    fn steps_member(&mut self, steps_frame: FrameId, name: &str) -> Result<Val, ExprError> {
        let Some(owner) = self.find(steps_frame, name) else {
            return Err(no_step(name));
        };
        let exp = self.step(owner, name)?;
        self.steps_view(steps_frame, exp)
    }

    /// The view of a step reached through `steps`: while a judge's take is expanded, the step
    /// it judges reads as that take.
    #[inline(never)]
    fn steps_view(&mut self, steps_frame: FrameId, exp: ExpId) -> Result<Val, ExprError> {
        let scope = self.scope_of(steps_frame);
        let judging = self.frames[steps_frame.0]
            .judging
            .clone()
            .filter(|(judged, _)| self.frames[scope.0].steps.contains_key(judged));
        self.view_at(steps_frame, exp, judging)
    }

    #[inline(never)]
    fn view_member_of(
        &mut self,
        frame: FrameId,
        view: ViewId,
        name: &str,
    ) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            ViewData::Node { exp, pinned } => match name {
                "outputs" => self.node_result(exp, pinned, What::Outputs),
                "facts" => self.node_result(exp, pinned, What::Facts),
                "take" => self.node_take(exp, pinned),
                _ => Err(ExprError::new(format!(
                    "a step has outputs, facts and take, not {}",
                    py_repr_str(name)
                ))),
            },
            ViewData::Repeat { .. } => match name {
                // `steps.x.outputs` is `steps.x.*.outputs`.
                "outputs" | "facts" => {
                    let every = self.view_every(frame, view)?;
                    let mut scope = StepScope { ex: self, frame };
                    expr::member(&mut scope, every, name)
                }
                _ => Err(ExprError::new(
                    "pick one instance with [key], or every one with .*",
                )),
            },
            ViewData::Every { items, verdicts } => {
                let mut verdicts = verdicts;
                let mut mapped = Vec::with_capacity(items.len());
                for (key, value) in items {
                    if matches!(value, Val::Missing) {
                        continue;
                    }
                    if let Some((exp, pinned)) = self.node_view(&value) {
                        let verdict = self.verdict_of_chosen(exp, pinned);
                        verdicts.insert(key.clone(), verdict);
                    }
                    let member = {
                        let mut scope = StepScope { ex: self, frame };
                        expr::member(&mut scope, value, name)?
                    };
                    // Left-out items, skipped results and optional outputs that are missing.
                    if matches!(member, Val::Missing) {
                        continue;
                    }
                    if let Some((exp, pinned)) = self.node_view(&member) {
                        let verdict = self.verdict_of_chosen(exp, pinned);
                        verdicts.insert(key.clone(), verdict);
                    }
                    mapped.push((key, member));
                }
                Ok(self.new_view(ViewData::Every {
                    items: mapped,
                    verdicts,
                }))
            }
            ViewData::Group { .. } | ViewData::Workflow { .. } => {
                self.scope_member(frame, view, name)
            }
            ViewData::Regenerating { exp, .. } => self.regenerating_member(exp, name),
            ViewData::Steps { frame: steps_frame } => self.steps_member(steps_frame, name),
            ViewData::Lets { frame: lets_frame } => self.let_value(lets_frame, name),
            ViewData::AnyPort { value } => Ok(value),
        }
    }

    pub(crate) fn view_item(
        &mut self,
        frame: FrameId,
        view: ViewId,
        index: &Val,
    ) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            ViewData::Repeat { exp, .. } => {
                // `[0]` looks up the key `'0'`, not the first position.
                let key = index.key_text()?;
                let child = self.exps[exp.0]
                    .children
                    .iter()
                    .find(|(child_key, _)| *child_key == key)
                    .map(|(_, child)| *child);
                match child {
                    Some(child) => self.view_of(child, None),
                    None => Err(ExprError::new(format!(
                        "no instance [{}]",
                        py_repr_str(&key)
                    ))),
                }
            }
            ViewData::Every { items, verdicts } => {
                let mut indexed = Vec::with_capacity(items.len());
                for (key, value) in items {
                    let mut scope = StepScope { ex: self, frame };
                    indexed.push((key, expr::item(&mut scope, value, index.clone())?));
                }
                Ok(self.new_view(ViewData::Every {
                    items: indexed,
                    verdicts,
                }))
            }
            _ => Err(ExprError::new(format!("{A_STEP} cannot be indexed"))),
        }
    }

    /// `steps.rep[k]` of a repeat whose key `k` only a run gives: any instance may be the one
    /// picked, so the value waits on `k` and on every instance of the repeat (gnode's
    /// `_RepeatView.expression_item`; token `{"index": k's token}`). `None` for any other view.
    pub(crate) fn repeat_item_pending(&mut self, view: ViewId, index: &Pending) -> Option<Val> {
        let ViewData::Repeat { exp, .. } = self.views[view.0] else {
            return None;
        };
        let mut refs = index.refs.clone();
        refs.extend(self.instance_ids(exp));
        let token = json!({"index": index.token});
        Some(Val::Pending(Box::new(Pending::new(refs, &token))))
    }

    pub(crate) fn view_every(&mut self, frame: FrameId, view: ViewId) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            ViewData::Repeat { exp, .. } => {
                let children = self.exps[exp.0].children.clone();
                let mut items = Vec::with_capacity(children.len());
                for (key, child) in children {
                    items.push((key, self.view_of(child, None)?));
                }
                Ok(self.new_view(ViewData::Every {
                    items,
                    verdicts: IndexMap::new(),
                }))
            }
            ViewData::Every { items, .. } => {
                // A repeat inside a repeat: every inner instance, keyed `outer.inner`.
                let mut flat = Vec::new();
                for (key, value) in items {
                    if matches!(value, Val::Missing) {
                        continue;
                    }
                    let inner = {
                        let mut scope = StepScope { ex: self, frame };
                        expr::every(&mut scope, value)?
                    };
                    let nested = match &inner {
                        Val::View(id) => match &self.views[id.0] {
                            ViewData::Every { items, .. } => Some(items.clone()),
                            _ => None,
                        },
                        _ => None,
                    };
                    match nested {
                        Some(nested) => flat.extend(
                            nested
                                .into_iter()
                                .map(|(inner_key, item)| (format!("{key}.{inner_key}"), item)),
                        ),
                        None => flat.push((key, inner)),
                    }
                }
                Ok(self.new_view(ViewData::Every {
                    items: flat,
                    verdicts: IndexMap::new(),
                }))
            }
            _ => Err(not_repeated()),
        }
    }

    pub(crate) fn view_len(&mut self, _frame: FrameId, view: ViewId) -> Result<usize, ExprError> {
        match &self.views[view.0] {
            // Items left out by `if:` count; items with a duplicate key were never added.
            ViewData::Repeat { exp, .. } => Ok(self.exps[exp.0].children.len()),
            ViewData::Every { items, .. } => Ok(items.len()),
            _ => Err(ExprError::new(format!("len() of {A_STEP}"))),
        }
    }

    /// What an expression that ends on a view gives: a `.*` result becomes a collection (files
    /// keyed, select ports unwrapped, verdicts kept for the keys present), a select's port its
    /// value; any other view is refused.
    pub(crate) fn finish_view(&mut self, frame: FrameId, view: ViewId) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            ViewData::Every { items, verdicts } => {
                let mut values = Vec::with_capacity(items.len());
                for (key, value) in items {
                    let value = match value {
                        Val::View(id) => match &self.views[id.0] {
                            ViewData::AnyPort { value } => value.clone(),
                            _ => {
                                return Err(ExprError::new(
                                    "name what to take from each instance, e.g. .outputs.image",
                                ));
                            }
                        },
                        other => other,
                    };
                    let value = match value {
                        Val::File(file) => Val::File(Box::new(file.with_key(&key))),
                        other => other,
                    };
                    values.push((key, value));
                }
                let present: HashSet<&str> = values.iter().map(|(key, _)| key.as_str()).collect();
                let verdicts = verdicts
                    .into_iter()
                    .filter(|(key, _)| present.contains(key.as_str()))
                    .collect();
                Ok(Val::Collection(Box::new(Collection {
                    items: values,
                    verdicts,
                })))
            }
            ViewData::AnyPort { value } => {
                let mut scope = StepScope { ex: self, frame };
                expr::finish(&mut scope, value)
            }
            _ => Err(ExprError::names_a_step()),
        }
    }

    /// `let.<name>` resolved in the referencing frame.
    pub(crate) fn let_value(&mut self, frame: FrameId, name: &str) -> Result<Val, ExprError> {
        let workflow = Rc::clone(&self.frames[frame.0].workflow);
        let Some(source) = workflow.let_.get(name) else {
            return Err(ExprError::new(format!("no let.{name}")));
        };
        // Evaluated lazily and every time, so a let that reaches itself would never end.
        let active = format!("{}:{name}", frame.0);
        if self.lets_active.contains(&active) {
            return Err(ExprError::new(format!("let.{name} refers back to itself")));
        }
        self.lets_active.push(active);
        let value = {
            let mut scope = StepScope { ex: self, frame };
            expr::resolve(source, &mut scope)
        };
        self.lets_active.pop();
        value
    }

    /// The member step `name` of a group or used workflow scope.
    fn member_of(&mut self, scope: FrameId, name: &str) -> Result<ExpId, ExprError> {
        if !self.frames[scope.0].steps.contains_key(name) {
            return Err(ExprError::new(format!(
                "the group has no step {}",
                py_repr_str(name)
            )));
        }
        self.step(scope, name)
    }

    /// A used workflow's outputs, evaluated in its scope each time they are read; problems are
    /// keyed `outputs.<name>`.
    fn workflow_outputs(&mut self, exp: ExpId, scope: FrameId) -> Val {
        let Some(document) = self.exps[exp.0].document.clone() else {
            return Val::Missing;
        };
        let mut outputs = IndexMap::new();
        for (output, value) in &document.workflow.outputs {
            let value = self.evaluate(scope, value, &format!("outputs.{output}"));
            outputs.insert(output.clone(), value);
        }
        Val::Object(outputs)
    }

    /// A member of a regenerating group: the last take's once decided.
    fn regenerating_member(&mut self, exp: ExpId, name: &str) -> Result<Val, ExprError> {
        let Some(&last_scope) = self.exps[exp.0].take_scopes.last() else {
            return Ok(Val::Missing);
        };
        let last = self.exps[exp.0]
            .until
            .last()
            .cloned()
            .unwrap_or(Until::Known(false));
        let then = self.exps[exp.0].then.clone();
        match last {
            Until::Pending(_) => {
                let refs: BTreeSet<String> = self.instance_ids(exp).into_iter().collect();
                let ids: Vec<&String> = refs.iter().collect();
                let token = json!({"regenerating": ids, "member": name});
                Ok(Val::Pending(Box::new(Pending::new(refs.clone(), &token))))
            }
            Until::Known(false) if then == Then::Fail => {
                let ids = self.instance_ids(exp);
                Ok(Val::Failed(
                    ids.last().cloned().unwrap_or_else(|| "group".to_string()),
                ))
            }
            Until::Known(false) if then == Then::Skip => Ok(Val::Missing),
            _ => {
                let member = self.member_of(last_scope, name)?;
                self.view_of(member, None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame::FrameChanges;
    use super::super::testing::*;
    use super::*;
    use crate::val::FileValue;
    use serde_json::Value;

    fn view_id(value: &Val) -> ViewId {
        match value {
            Val::View(id) => *id,
            other => panic!("not a view: {other:?}"),
        }
    }

    fn file(name: &str) -> FileValue {
        FileValue {
            digest: "0".repeat(64),
            kind: "image/png".into(),
            name: name.into(),
            size: 1,
            key: None,
            content: None,
            location: None,
        }
    }

    #[test]
    fn what_each_kind_of_step_reads_as() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let exp = ex.new_exp(root, "x");
        ex.exp_mut(exp).kind = ExpKind::Absent;
        assert_eq!(ex.view_of(exp, None), Ok(Val::Missing));
        ex.exp_mut(exp).kind = ExpKind::Expanding;
        assert_eq!(
            ex.view_of(exp, None).unwrap_err().0,
            "a step refers back to itself"
        );
        let token = Pending {
            refs: BTreeSet::from(["split#1".to_string()]),
            token: "t".into(),
        };
        ex.exp_mut(exp).kind = ExpKind::Pending;
        ex.exp_mut(exp).token = Some(token.clone());
        assert_eq!(ex.view_of(exp, None), Ok(Val::Pending(Box::new(token))));
        ex.exp_mut(exp).kind = ExpKind::Node;
        let view = ex.view_of(exp, None).unwrap();
        assert_eq!(
            ex.views[view_id(&view).0],
            ViewData::Node { exp, pinned: None }
        );
    }

    #[test]
    fn a_node_read_while_its_instances_are_made_refers_back_to_itself() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let group = ex.nest(
            root,
            FrameChanges {
                decl_prefix: Some("g.".into()),
                ..Default::default()
            },
        );
        let exp = ex.new_exp(group, "e");
        ex.exp_mut(exp).kind = ExpKind::Node;
        ex.instantiating.push(exp);
        assert_eq!(
            ex.view_of(exp, None).unwrap_err().0,
            "g.e refers back to itself"
        );
        // Once its instances exist (its judges read it then), it is an ordinary node.
        ex.exp_mut(exp).instances.push("g.e#1".into());
        assert!(ex.view_of(exp, None).is_ok());
        // With three takes, the window lasts until the third exists.
        ex.exp_mut(exp).takes = vec![1, 2, 3];
        assert!(ex.view_of(exp, None).is_err());
        ex.instantiating.pop();
        assert!(ex.view_of(exp, None).is_ok());
    }

    #[test]
    fn a_judge_reads_the_take_it_judges_and_nothing_else() {
        let mut fixture = Fixture::new(workflow_doc(vec![
            ("draw", node_step("nope")),
            ("check", node_step("nope")),
            ("g", group_step(vec![("draw", node_step("nope"))])),
        ]));
        let (mut ex, root) = fixture.expander();
        let draw = ex.new_exp(root, "draw");
        ex.exp_mut(draw).kind = ExpKind::Node;
        ex.memo.insert((root, "draw".to_string()), draw);
        let inner = ex.nest(
            root,
            FrameChanges {
                steps: Some(steps_of(vec![("draw", node_step("nope"))])),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        let inner_draw = ex.new_exp(inner, "draw");
        ex.exp_mut(inner_draw).kind = ExpKind::Node;
        let judge_take = ex.derive(
            root,
            FrameChanges {
                judging: Some(Some(("draw".into(), 2))),
                ..Default::default()
            },
        );
        let steps = ex.root(judge_take, "steps").unwrap();
        let read = ex.view_member(judge_take, view_id(&steps), "draw").unwrap();
        assert_eq!(
            ex.views[view_id(&read).0],
            ViewData::Node {
                exp: draw,
                pinned: Some(2)
            }
        );
        // A step of the same name in another scope is not the judged one.
        let other = ex
            .view_at(judge_take, inner_draw, Some(("draw".into(), 2)))
            .unwrap();
        assert_eq!(
            ex.views[view_id(&other).0],
            ViewData::Node {
                exp: inner_draw,
                pinned: None
            }
        );
        // Outside a judge's take nothing is pinned.
        let steps = ex.root(root, "steps").unwrap();
        let plain = ex.view_member(root, view_id(&steps), "draw").unwrap();
        assert_eq!(
            ex.views[view_id(&plain).0],
            ViewData::Node {
                exp: draw,
                pinned: None
            }
        );
    }

    #[test]
    fn views_refuse_what_they_do_not_have() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let node = ex.new_exp(root, "x");
        ex.exp_mut(node).kind = ExpKind::Node;
        let view = view_id(&ex.view_of(node, None).unwrap());
        assert_eq!(
            ex.view_member(root, view, "result").unwrap_err().0,
            "a step has outputs, facts and take, not 'result'"
        );
        assert_eq!(
            ex.view_every(root, view).unwrap_err().0,
            ".* applies to a repeated step"
        );
        assert_eq!(
            ex.view_item(root, view, &Val::Number(0.0)).unwrap_err().0,
            "a step cannot be indexed"
        );
        assert_eq!(ex.view_len(root, view).unwrap_err().0, "len() of a step");
        assert_eq!(
            ex.finish_view(root, view).unwrap_err(),
            ExprError::names_a_step()
        );
        let repeat = ex.new_exp(root, "x");
        ex.exp_mut(repeat).kind = ExpKind::Repeat;
        let view = view_id(&ex.view_of(repeat, None).unwrap());
        assert_eq!(
            ex.view_member(root, view, "take").unwrap_err().0,
            "pick one instance with [key], or every one with .*"
        );
        let steps = view_id(&ex.root(root, "steps").unwrap());
        assert_eq!(
            ex.view_member(root, steps, "zz").unwrap_err().0,
            "no step 'zz'"
        );
        assert_eq!(
            ex.view_item(root, steps, &Val::Str("x".into()))
                .unwrap_err()
                .0,
            "a step cannot be indexed"
        );
    }

    #[test]
    fn a_repeat_counts_every_item_it_listed() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let repeat = ex.new_exp(root, "x");
        ex.exp_mut(repeat).kind = ExpKind::Repeat;
        for (key, kind) in [
            ("a", ExpKind::Node),
            ("b", ExpKind::Absent),
            ("c", ExpKind::Node),
        ] {
            let child = ex.new_exp(root, "x");
            ex.exp_mut(child).kind = kind;
            ex.exp_mut(repeat).children.push((key.into(), child));
        }
        let view = view_id(&ex.view_of(repeat, None).unwrap());
        // Items left out by `if:` count.
        assert_eq!(ex.view_len(root, view), Ok(3));
        let every = view_id(&ex.view_every(root, view).unwrap());
        assert_eq!(ex.view_len(root, every), Ok(3));
        match &ex.views[every.0] {
            ViewData::Every { items, verdicts } => {
                let keys: Vec<&str> = items.iter().map(|(k, _)| k.as_str()).collect();
                assert_eq!(keys, ["a", "b", "c"]);
                assert_eq!(items[1].1, Val::Missing);
                assert!(verdicts.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn finishing_every_makes_a_keyed_collection() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let port = ex.new_view(ViewData::AnyPort {
            value: Val::Str("chosen".into()),
        });
        let mut verdicts = IndexMap::new();
        verdicts.insert("a".to_string(), Some(Verdict::Accept));
        verdicts.insert("gone".to_string(), Some(Verdict::Reject));
        verdicts.insert("b".to_string(), None);
        let every = ex.new_view(ViewData::Every {
            items: vec![
                ("a".into(), Val::File(Box::new(file("pic.png")))),
                ("b".into(), port),
            ],
            verdicts,
        });
        let finished = ex.finish_view(root, view_id(&every)).unwrap();
        let Val::Collection(collection) = finished else {
            panic!("not a collection");
        };
        assert_eq!(
            collection.items[0],
            (
                "a".to_string(),
                Val::File(Box::new(file("pic.png").with_key("a")))
            )
        );
        assert_eq!(
            collection.items[1],
            ("b".to_string(), Val::Str("chosen".into()))
        );
        // Verdicts are kept only for keys still present.
        assert_eq!(collection.verdicts.keys().collect::<Vec<_>>(), ["a", "b"]);

        let node = ex.new_exp(root, "x");
        ex.exp_mut(node).kind = ExpKind::Node;
        let node_view = ex.view_of(node, None).unwrap();
        let every = ex.new_view(ViewData::Every {
            items: vec![("a".into(), node_view)],
            verdicts: IndexMap::new(),
        });
        assert_eq!(
            ex.finish_view(root, view_id(&every)).unwrap_err().0,
            "name what to take from each instance, e.g. .outputs.image"
        );
    }

    #[test]
    fn a_regenerating_group_reads_its_last_take_once_decided() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let take = ex.nest(
            root,
            FrameChanges {
                steps: Some(steps_of(vec![("m", node_step("nope"))])),
                prefix: Some("build.".into()),
                decl_prefix: Some("build.".into()),
                takes: Some(vec![1]),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        let group = ex.new_exp(root, "build");
        ex.exp_mut(group).kind = ExpKind::Regenerating;
        let view = view_id(&ex.view_of(group, None).unwrap());
        // No takes at all.
        assert_eq!(ex.view_member(root, view, "m"), Ok(Val::Missing));
        ex.exp_mut(group).take_scopes.push(take);
        ex.exp_mut(group).until.push(Until::Known(false));
        ex.exp_mut(group).then = Then::Fail;
        assert_eq!(
            ex.view_member(root, view, "m"),
            Ok(Val::Failed("group".into()))
        );
        ex.exp_mut(group).then = Then::Skip;
        assert_eq!(ex.view_member(root, view, "m"), Ok(Val::Missing));
        ex.exp_mut(group).until = vec![Until::Pending(Pending {
            refs: BTreeSet::new(),
            token: "t".into(),
        })];
        assert!(matches!(
            ex.view_member(root, view, "m"),
            Ok(Val::Pending(_))
        ));
        ex.exp_mut(group).until = vec![Until::Known(true)];
        // The member `m` was refused (its `uses`), so it reads as missing.
        assert_eq!(ex.view_member(root, view, "m"), Ok(Val::Missing));
        assert_eq!(
            ex.view_member(root, view, "zz").unwrap_err().0,
            "the group has no step 'zz'"
        );
    }

    #[test]
    fn groups_and_used_workflows_name_their_members() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let scope = ex.nest(
            root,
            FrameChanges {
                steps: Some(steps_of(vec![("m", node_step("nope"))])),
                decl_prefix: Some("g.".into()),
                parent: Some(Some(root)),
                ..Default::default()
            },
        );
        let group = ex.new_exp(root, "g");
        ex.exp_mut(group).kind = ExpKind::Group;
        ex.exp_mut(group).scope = Some(scope);
        let view = view_id(&ex.view_of(group, None).unwrap());
        assert_eq!(ex.view_member(root, view, "m"), Ok(Val::Missing));
        assert_eq!(
            ex.view_member(root, view, "facts").unwrap_err().0,
            "the group has no step 'facts'"
        );
        let used = ex.new_exp(root, "w");
        ex.exp_mut(used).kind = ExpKind::Workflow;
        ex.exp_mut(used).scope = Some(scope);
        let view = view_id(&ex.view_of(used, None).unwrap());
        // No document recorded: its outputs are missing.
        assert_eq!(ex.view_member(root, view, "outputs"), Ok(Val::Missing));
        assert_eq!(
            ex.view_member(root, view, "facts").unwrap_err().0,
            "the group has no step 'facts'"
        );
    }

    #[test]
    fn names_of_the_step_scope() {
        let mut doc = workflow_doc(vec![("x", node_step("nope"))]);
        doc.tables
            .insert("sizes".into(), serde_json::json!({"s": "${{ raw }}"}));
        let mut fixture = Fixture::new(doc);
        fixture.inputs.insert("name".into(), Val::Str("ada".into()));
        let (mut ex, root) = fixture.expander();
        let mut inputs = IndexMap::new();
        inputs.insert("name".to_string(), Val::Str("ada".into()));
        assert_eq!(ex.root(root, "inputs"), Ok(Val::Object(inputs)));
        match ex.root(root, "tables").unwrap() {
            Val::Object(tables) => assert!(tables.contains_key("sizes")),
            other => panic!("{other:?}"),
        }
        let lets = ex.root(root, "let").unwrap();
        assert_eq!(ex.views[view_id(&lets).0], ViewData::Lets { frame: root });
        assert_eq!(ex.root(root, "zz").unwrap_err().0, "unknown name 'zz'");
        // Variables shadow the roots.
        let mut variables = IndexMap::new();
        variables.insert("inputs".to_string(), Val::Number(1.0));
        let item = ex.derive(
            root,
            FrameChanges {
                variables: Some(variables),
                ..Default::default()
            },
        );
        assert_eq!(ex.root(item, "inputs"), Ok(Val::Number(1.0)));
        assert_eq!(ex.let_value(item, "zz").unwrap_err().0, "no let.zz");
    }

    #[test]
    fn a_let_reaching_itself_is_refused() {
        let mut doc = workflow_doc(vec![("x", node_step("nope"))]);
        doc.let_
            .insert("x".into(), Value::String("${{ let.x }}".into()));
        let mut fixture = Fixture::new(doc);
        let (mut ex, root) = fixture.expander();
        // As if `let.x` were being evaluated in this frame already.
        ex.lets_active.push(format!("{}:x", root.0));
        assert_eq!(
            ex.let_value(root, "x").unwrap_err().0,
            "let.x refers back to itself"
        );
    }

    #[test]
    fn the_prompt_scope_sees_vars_and_inputs() {
        let mut vars = IndexMap::new();
        vars.insert("n".to_string(), Val::Number(2.0));
        let mut inputs = IndexMap::new();
        inputs.insert("name".to_string(), Val::Str("ada".into()));
        let mut scope = TemplateScope {
            vars: Val::Object(vars.clone()),
            inputs: Rc::new(inputs.clone()),
        };
        assert_eq!(scope.root("vars"), Ok(Val::Object(vars)));
        assert_eq!(scope.root("inputs"), Ok(Val::Object(inputs)));
        assert_eq!(scope.root("n"), Ok(Val::Number(2.0)));
        assert_eq!(
            scope.root("steps").unwrap_err().0,
            "a prompt sees vars and inputs, not 'steps'"
        );
        assert_eq!(
            scope.view_member(ViewId(0), "x").unwrap_err(),
            ExprError::names_a_step()
        );
    }

    #[test]
    fn facts_of_a_step_that_did_not_run_are_that_value() {
        let mut fixture = Fixture::new(workflow_doc(vec![("x", node_step("nope"))]));
        let (mut ex, root) = fixture.expander();
        let mut scope = StepScope {
            ex: &mut ex,
            frame: root,
        };
        assert_eq!(scope.facts(&Val::Missing), Ok(Val::Missing));
        assert_eq!(
            scope.facts(&Val::Failed("a#1".into())),
            Ok(Val::Failed("a#1".into()))
        );
        let refused = scope.facts(&Val::Null).unwrap_err().0;
        assert!(
            refused.starts_with("facts() needs a file, not"),
            "{refused}"
        );
        let mut prompt = TemplateScope {
            vars: Val::Null,
            inputs: Rc::new(IndexMap::new()),
        };
        let refused = prompt.facts(&Val::Missing).unwrap_err().0;
        assert!(
            refused.starts_with("facts() needs a file, not"),
            "{refused}"
        );
    }
}
