//! Workflow-authored waits use the run's request store; no worker or paid reservation is held.

use super::{Message, Scheduler, node_started, place_file};
use crate::events::{Event, encode};
use crate::waiting::{self, RequestSpec, RequestState, TimeoutPolicy};
use grida_fx_core::Error;
use grida_fx_core::expand::{Instance, NodeResult, ResultStatus};
use grida_fx_core::val::Val;
use indexmap::IndexMap;
use serde_json::Value;
use std::time::Duration;

fn waiting_error(error: waiting::WaitingError) -> Error {
    Error::internal(error.to_string())
}

/// Freeze precisely the keyed candidates and review material that this occurrence saw. Files
/// are adopted by the scheduler before the request is published.
fn request_spec(instance: &Instance) -> Result<RequestSpec, Error> {
    let values = &instance.with;
    let text = |name: &str| match values.get(name) {
        Some(Val::Str(value)) => Some(value.clone()),
        _ => None,
    };
    let choices = match values.get("choices") {
        None => None,
        Some(Val::Object(map)) => Some(
            map.iter()
                .map(|(key, value)| (key.clone(), waiting::encode_reviewed(value)))
                .collect(),
        ),
        Some(Val::Collection(c)) => Some(
            c.items
                .iter()
                .map(|(key, value)| (key.clone(), waiting::encode_reviewed(value)))
                .collect(),
        ),
        Some(_) => return Err(Error::internal("waiting step needs keyed choices")),
    };
    Ok(RequestSpec {
        step: instance.id.clone(),
        message: text("message"),
        schema: values
            .get("schema")
            .and_then(Val::plain)
            .ok_or_else(|| Error::internal("waiting step needs an answer schema"))?,
        show: values
            .get("show")
            .map_or(Value::Null, waiting::encode_reviewed),
        choices,
        choice_field: text("choice_field"),
        timeout_seconds: values
            .get("timeout_seconds")
            .and_then(Val::plain)
            .and_then(|v| v.as_u64()),
        on_timeout: match text("on_timeout").as_deref() {
            Some("continue") => TimeoutPolicy::Continue,
            _ => TimeoutPolicy::Fail,
        },
    })
}

impl Scheduler<'_> {
    /// Wait for either an ordinary worker/control message or an independently written answer.
    /// Use the same receiver while waiting so a cancellation does not have to wait for a deadline.
    pub(super) fn wait_for_work(&mut self) -> Result<Vec<Message>, Error> {
        if self.awaiting.is_empty() {
            return Ok(self.wait());
        }
        loop {
            // Charging and dispatch use this runner. A write to the run folder can arrive from
            // a different process with no service or connection to the attached runner.
            let message = self.engine.handle.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), self.receiver.recv()).await
            });
            match message {
                Ok(Some(message)) => {
                    let mut messages = vec![message];
                    while let Ok(message) = self.receiver.try_recv() {
                        messages.push(message);
                    }
                    return Ok(messages);
                }
                Ok(None) => return Ok(Vec::new()),
                Err(_) => {
                    let pending = self.awaiting.values().map(String::as_str);
                    if waiting::any_resolved(&self.folder.path, pending).map_err(waiting_error)? {
                        return Ok(vec![Message::RequestResolved]);
                    }
                }
            }
        }
    }

    pub(super) fn waiting_step(&mut self, instance: &Instance) -> Result<bool, Error> {
        let opening = !self.awaiting.contains_key(&instance.id);
        let request = if let Some(id) = self.awaiting.get(&instance.id) {
            waiting::expire_due(&self.folder.path, id).map_err(waiting_error)?
        } else {
            for file in instance.with.values().flat_map(crate::store::files_in) {
                self.engine
                    .store
                    .adopt(file)
                    .map_err(|e| Error::internal(self.engine.scrub(&e.to_string())))?;
            }
            self.folder.capture_waiting_files(
                ["show", "choices"]
                    .into_iter()
                    .filter_map(|name| instance.with.get(name))
                    .flat_map(crate::store::files_in),
            );
            // Review material is exposed through the local request surface. The portable
            // execution event retains the node, its links and its identity.
            let mut started = node_started(instance);
            if let Event::NodeStarted { with, .. } = &mut started {
                with.clear();
            }
            self.emit(&started)?;
            let spec = request_spec(instance)?;
            let request =
                waiting::ensure_request(&self.folder.path, spec).map_err(waiting_error)?;
            // Even when just reattaching, the window is measured from the original opening.
            waiting::expire_due(&self.folder.path, &request.id).map_err(waiting_error)?
        };
        if opening || !matches!(request.state, RequestState::Pending) {
            self.emit(&Event::WaitingRequest {
                id: instance.id.clone(),
                request_id: request.id.clone(),
                status: request.state.status(),
                deadline: request.deadline.clone(),
            })?;
        }
        match &request.state {
            RequestState::Pending => {
                if super::schedule::group_of(instance).is_some() {
                    self.groups.open_requests.insert(instance.id.clone());
                }
                self.awaiting.insert(instance.id.clone(), request.id);
                Ok(true)
            }
            state => {
                let mut outputs = IndexMap::new();
                let mut facts = IndexMap::new();
                let expired = matches!(state, RequestState::Expired { .. });
                facts.insert(
                    "outcome".into(),
                    Value::from(if expired { "expired" } else { "answered" }),
                );
                let error = (expired && matches!(request.spec.on_timeout, TimeoutPolicy::Fail))
                    .then(|| "the response deadline expired".to_owned());
                if let RequestState::Answered { answer, .. } = state {
                    outputs.insert("answer".into(), Val::from_json(answer));
                    if let Some(value) = request.selected(answer) {
                        let selected = waiting::decode_reviewed(value, &self.engine.store)
                            .map_err(Error::internal)?;
                        outputs.insert("selected".into(), selected);
                    }
                }
                let status = if let Some(error) = &error {
                    self.emit(&Event::NodeFailed {
                        id: instance.id.clone(),
                        path: instance.path.clone(),
                        error: Some(error.clone()),
                        facts: Some(facts.clone()),
                        code: None,
                        diagnostic: None,
                        duration_ms: None,
                        display: super::terminal_display(instance),
                    })?;
                    self.failed.push((instance.id.clone(), Some(error.clone())));
                    ResultStatus::Failed
                } else {
                    for (relative, digest) in
                        crate::folder::step_files(&instance.path, &instance.takes, &outputs)
                    {
                        place_file(&self.engine.store, &self.folder, &relative, &digest)
                            .map_err(Error::internal)?;
                    }
                    self.emit(&Event::NodeFinished {
                        id: instance.id.clone(),
                        path: instance.path.clone(),
                        cache_hit: false,
                        outputs: outputs
                            .iter()
                            .map(|(key, val)| (key.clone(), encode(val)))
                            .collect(),
                        facts: facts.clone(),
                        duration_ms: 0,
                    })?;
                    ResultStatus::Succeeded
                };
                self.awaiting.shift_remove(&instance.id);
                self.groups.open_requests.remove(&instance.id);
                self.planner.results.insert(
                    instance.id.clone(),
                    NodeResult {
                        outputs,
                        facts,
                        status,
                        error,
                    },
                );
                Ok(false)
            }
        }
    }
}
