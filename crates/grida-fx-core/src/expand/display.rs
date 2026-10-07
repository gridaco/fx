//! Engine-owned display boundaries. None of this metadata participates in execution.

use super::frame::FrameId;
use super::wiring::Binding;
use super::{Expander, instance_id};
use crate::docs::workflow::{LoadedWorkflow, Step};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scope {
    pub id: String,
    pub parent: Option<String>,
    pub kind: &'static str,
    pub path: String,
    pub step: String,
    pub take: Vec<u32>,
    pub title: String,
    pub source: Option<String>,
    pub ports: ScopePorts,
    pub input_bindings: Vec<Binding>,
    pub output_bindings: Vec<Binding>,
    pub nodes: Vec<String>,
    pub pending: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ScopePorts {
    pub inputs: BTreeMap<String, Value>,
    pub outputs: Vec<String>,
}

impl Expander<'_> {
    pub(crate) fn display_scope(
        &mut self,
        frame: FrameId,
        declared: &Step,
        step: &str,
        loaded: Option<&LoadedWorkflow>,
        input_bindings: Vec<Binding>,
    ) {
        let context = &mut self.frames[frame.0];
        let path = context.prefix.trim_end_matches('.').to_string();
        let id = format!("scope:{}", instance_id(&path, &context.takes));
        let parent = context.display_scope.replace(id.clone());
        let (kind, source, ports) = match loaded {
            Some(loaded) => {
                context.workflow_scope = Some(id.clone());
                (
                    "workflow",
                    Some(loaded.source.clone()),
                    ScopePorts {
                        inputs: loaded
                            .workflow
                            .inputs
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        outputs: loaded.workflow.outputs.keys().cloned().collect(),
                    },
                )
            }
            None => ("group", None, ScopePorts::default()),
        };
        let title = declared
            .title
            .clone()
            .or_else(|| loaded.map(|loaded| loaded.workflow.title.clone()))
            .unwrap_or_else(|| step.rsplit('.').next().unwrap_or(step).to_string());
        self.display_scopes.insert(
            id.clone(),
            Scope {
                id,
                parent,
                kind,
                path,
                step: step.to_string(),
                take: context.takes.clone(),
                title,
                source,
                ports,
                input_bindings,
                output_bindings: Vec::new(),
                nodes: Vec::new(),
                pending: Vec::new(),
            },
        );
    }

    /// Only real, retained instances enter membership; trials and shadow pricing restore
    /// their display records alongside the expansion they roll back.
    pub(crate) fn finish_display_scopes(&mut self) {
        for scope in self.display_scopes.values_mut() {
            scope.nodes.clear();
            scope.pending.clear();
        }
        for instance in self.instances.values() {
            if let Some(id) = &instance.display_scope
                && let Some(scope) = self.display_scopes.get_mut(id)
            {
                scope.nodes.push(instance.id.clone());
            }
        }
        for pending in &self.pending {
            if let Some(id) = &pending.display_scope
                && let Some(scope) = self.display_scopes.get_mut(id)
                && !scope.pending.contains(&pending.path)
            {
                scope.pending.push(pending.path.clone());
            }
        }
    }
}
