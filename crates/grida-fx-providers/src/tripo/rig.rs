//! Tripo `mesh.rig`: a free riggability check, then the paid rig (spec/providers.md §9.4).
//!
//! `submit`: refuse a model that is not `model/gltf-binary` (or no key); upload it as
//! `unrigged.glb`; post `POST /animations/rig-check` `{"input": token}` and wait for that task up to
//! [`CHECK_DEADLINE`]; a doubted model (`riggable` not true, or `rig_type` not the requested one) is
//! refused unless `allow_negative_check` is true; then post `POST /animations/rig` once with
//! `{"input", "model", "rig_type", "spec": <skeleton>, "out_format": "glb"}`. The handle is
//! `{"task_id", "check": {"task_id", "riggable", "rig_type"}, "advisory_override"}`, where the
//! check's `rig_type` is kept only when it is a short plain string (`^[A-Za-z0-9_.:-]{1,96}$`),
//! else `null`, so a handle and an answer never carry a provider URL.
//! `collect`: wait up to [`COLLECT_DEADLINE`]; exactly one GLB among the downloads, as file
//! `model`; `data: {"facts": {"riggable", "checked_rig_type", "advisory_override"}}` from the
//! handle; cost from the rig task's credits.
//!
//! The whole check is the free phase (spec/providers.md §7): a check task that ends without
//! success is `Refused`; one still running at its deadline, an answer about another task, or a
//! `riggable` that is not a boolean is `NotReceived`, and the engine resends the whole submit.
//! Refusals before anything is sent come in the order of spec/providers.md §5: the route, the
//! request's shape (spec/capabilities.md §8), the key, then the model file (its bytes, its kind).

use super::{TripoApi, Waited};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Collected, LongJob, Submitted};
use serde_json::{Value, json};
use std::time::Duration;

/// How long the free check may take.
pub const CHECK_DEADLINE: Duration = Duration::from_secs(600);

/// How long one collect waits.
pub const COLLECT_DEADLINE: Duration = Duration::from_secs(600);

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "tripo-rig";

const CAPABILITY: &str = "mesh.rig";

/// The rig type when the request names none.
const DEFAULT_RIG_TYPE: &str = "biped";

/// The skeleton convention when the request names none.
const DEFAULT_SKELETON: &str = "mixamo";

/// Tripo's rig adapter (module doc).
#[derive(Debug, Clone)]
pub struct TripoRig {
    api: TripoApi,
}

impl TripoRig {
    pub fn new(api: TripoApi) -> TripoRig {
        TripoRig { api }
    }

    async fn submit_call(&self, call: &CallRequest) -> Submitted {
        let api = &self.api;
        if let Err(reason) = super::check_route(call, CAPABILITY, ADAPTER) {
            return api.refused(&reason);
        }
        if let Err(reason) = crate::capabilities::check_request(CAPABILITY, &call.request) {
            return api.refused(&reason);
        }
        if let Err(reason) = api.client.credential("authorization", "Bearer ") {
            return api.refused(&reason);
        }
        let glb = match model_bytes(call) {
            Ok(glb) => glb,
            Err(reason) => return api.refused(&reason),
        };
        let rig_type = text_or(call, "rig_type", DEFAULT_RIG_TYPE);
        let skeleton = text_or(call, "skeleton", DEFAULT_SKELETON);
        let allowed = call.request.get("allow_negative_check") == Some(&Value::Bool(true));

        // The free phase: the upload, then the riggability check and its wait.
        let token = match api.upload("unrigged.glb", "model/gltf-binary", glb).await {
            Ok(token) => token,
            Err(outcome) => return outcome,
        };
        let check_id = match api.post_check(&token).await {
            Ok(check_id) => check_id,
            Err(outcome) => return outcome,
        };
        let checked = match api.wait(&check_id, CHECK_DEADLINE).await {
            Waited::Done(task) => task,
            Waited::Ended(status) => {
                return api.refused(&format!("Tripo's riggability check ended as {status}"));
            }
            Waited::Unreachable(reason) => return api.not_received(&reason),
        };
        let riggable = match checked.output.get("riggable") {
            Some(Value::Bool(riggable)) => *riggable,
            _ => return api.not_received("Tripo's riggability check answered without a verdict"),
        };
        let returned_rig_type = checked.output.get("rig_type").unwrap_or(&Value::Null);
        // Only a short plain string goes into the handle (and later the answer's facts).
        let checked_rig_type = super::plain_or_null(returned_rig_type);
        let doubted = !riggable || checked_rig_type != Value::String(rig_type.clone());
        if doubted && !allowed {
            return api.refused(&format!(
                "Tripo's check doubts this model (riggable={riggable}, rig_type={})",
                super::shown(returned_rig_type)
            ));
        }

        // The paid POST, once.
        let body = json!({
            "input": token,
            "model": call.route.model,
            "rig_type": rig_type,
            "spec": skeleton,
            "out_format": "glb",
        });
        match api.post_task("/animations/rig", body).await {
            Ok(task_id) => Submitted::Accepted {
                handle: json!({
                    "task_id": task_id,
                    "check": {
                        "task_id": check_id,
                        "riggable": riggable,
                        "rig_type": checked_rig_type,
                    },
                    "advisory_override": doubted,
                }),
            },
            Err(outcome) => outcome,
        }
    }

    async fn collect_call(&self, handle: &Value) -> Collected {
        let api = &self.api;
        let Some(task_id) = super::handle_task_id(handle) else {
            return api.unreachable("the Tripo handle has no usable task id");
        };
        let (task, models) = match api.finished_models(task_id, COLLECT_DEADLINE).await {
            Ok(finished) => finished,
            Err(outcome) => return outcome,
        };
        let mut glbs = models
            .into_iter()
            .filter(|(kind, _)| kind == "model/gltf-binary");
        let (Some((_, bytes)), None) = (glbs.next(), glbs.next()) else {
            return Collected::Ended {
                reason: api.client.reason("the Tripo rig task made no single GLB"),
            };
        };
        let check = handle.get("check");
        let riggable = check
            .and_then(|c| c.get("riggable"))
            .filter(|v| v.is_boolean())
            .cloned()
            .unwrap_or(Value::Null);
        let checked_rig_type = check
            .and_then(|c| c.get("rig_type"))
            .map_or(Value::Null, super::plain_or_null);
        let advisory_override = handle
            .get("advisory_override")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let data = json!({"facts": {
            "riggable": riggable,
            "checked_rig_type": checked_rig_type,
            "advisory_override": advisory_override,
        }});
        Collected::Answered(
            Answer::new(data, super::credits_cost(task.credits_consumed.as_ref())).with_file(
                "model",
                "model/gltf-binary",
                bytes,
            ),
        )
    }
}

/// The model's bytes (spec/providers.md §5 step 5): a file the call carries, of kind
/// `model/gltf-binary`, readable and not empty.
fn model_bytes(call: &CallRequest) -> Result<Vec<u8>, String> {
    let missing = || "a rig call needs its model".to_string();
    let value = call.request.get("model").ok_or_else(missing)?;
    let file = crate::wire::request_file(call, value, "model").map_err(|_| missing())?;
    if file.kind != "model/gltf-binary" {
        return Err(format!("Tripo rigs a GLB, not {}", file.kind));
    }
    crate::wire::read_file(file, "model").map_err(|_| missing())
}

/// A text member, or `default` when it is absent, `null` or empty.
fn text_or(call: &CallRequest, name: &str, default: &str) -> String {
    match call.request.get(name) {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        _ => default.to_string(),
    }
}

impl LongJob for TripoRig {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(self.submit_call(call))
    }

    fn collect<'a>(
        &'a self,
        _call: &'a CallRequest,
        handle: &'a Value,
    ) -> BoxFuture<'a, Collected> {
        Box::pin(self.collect_call(handle))
    }
}
