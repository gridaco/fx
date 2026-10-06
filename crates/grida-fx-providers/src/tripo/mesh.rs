//! Tripo `mesh.generate`: multiview to model (spec/providers.md §9.4).
//!
//! `submit`: refuse a bad request (no `front`, a view other than front/back/left/right, `front`
//! alone, a view that is not PNG or JPEG, `face_limit` outside 48..25 000, no key); upload each
//! view in the order front, back, left, right (`<view>.png` / `<view>.jpg`); then post
//! `POST /generation/multiview-to-model` once with `{"model", "quad", "texture", "pbr",
//! "face_limit"?, "inputs": [{"front": token}, …]}`. The handle is `{"task_id"}`.
//! `collect`: wait up to [`COLLECT_DEADLINE`]; download every model URL; keep the first FBX, else
//! the first GLB, as file `model`; `data: {"facts": {"model_kind": <kind>}}`; cost from credits.
//!
//! Refusals come in the order of spec/providers.md §5: the route, the request's shape
//! (spec/capabilities.md §7), the key, the values (the view names, then their count, then
//! `face_limit`), then the view files in the order front, back, left, right (each: its bytes, then
//! its kind). A multiview task takes two views at least: Tripo refuses `front` alone only at the
//! paid POST (HTTP 400, a failed call at the whole hold), so the adapter refuses it first, at $0.

use super::TripoApi;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Collected, LongJob, Submitted};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// How long one collect waits.
pub const COLLECT_DEADLINE: Duration = Duration::from_secs(1200);

/// The view names Tripo takes, in the order it takes them.
pub const VIEWS: [&str; 4] = ["front", "back", "left", "right"];

/// The `face_limit` range.
pub const FACE_LIMIT: std::ops::RangeInclusive<i64> = 48..=25_000;

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "tripo-multiview";

const CAPABILITY: &str = "mesh.generate";

/// Tripo's mesh adapter (module doc).
#[derive(Debug, Clone)]
pub struct TripoMesh {
    api: TripoApi,
}

/// A request's values, checked.
struct Values {
    /// The views by name.
    views: Map<String, Value>,
    /// The body's parameters, in wire order.
    params: Map<String, Value>,
}

/// One view ready to upload.
struct View {
    name: &'static str,
    kind: String,
    bytes: Vec<u8>,
}

impl TripoMesh {
    pub fn new(api: TripoApi) -> TripoMesh {
        TripoMesh { api }
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
        let Values { views, params } = match request_values(call) {
            Ok(values) => values,
            Err(reason) => return api.refused(&reason),
        };
        let views = match view_files(call, &views) {
            Ok(views) => views,
            Err(reason) => return api.refused(&reason),
        };

        // The free phase: every view, in Tripo's order.
        let mut inputs = Vec::with_capacity(views.len());
        for view in views {
            let suffix = if view.kind == "image/png" {
                "png"
            } else {
                "jpg"
            };
            let filename = format!("{}.{suffix}", view.name);
            match api.upload(&filename, &view.kind, view.bytes).await {
                Ok(token) => {
                    let mut input = Map::new();
                    input.insert(view.name.to_string(), Value::String(token));
                    inputs.push(Value::Object(input));
                }
                Err(outcome) => return outcome,
            }
        }

        // The paid POST, once.
        let mut body = Map::new();
        body.insert("model".into(), Value::String(call.route.model.clone()));
        body.extend(params);
        body.insert("inputs".into(), Value::Array(inputs));
        match api
            .post_task("/generation/multiview-to-model", Value::Object(body))
            .await
        {
            Ok(task_id) => Submitted::Accepted {
                handle: json!({ "task_id": task_id }),
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
        let chosen = models
            .iter()
            .position(|(kind, _)| kind == "model/fbx")
            .or_else(|| {
                models
                    .iter()
                    .position(|(kind, _)| kind == "model/gltf-binary")
            });
        let Some(index) = chosen else {
            return Collected::Ended {
                reason: api.client.reason("the Tripo task made no model"),
            };
        };
        let (kind, bytes) = models.into_iter().nth(index).unwrap_or_default();
        let data = json!({"facts": {"model_kind": kind}});
        Collected::Answered(
            Answer::new(data, super::credits_cost(task.credits_consumed.as_ref()))
                .with_file("model", &kind, bytes),
        )
    }
}

/// The value checks (spec/providers.md §5 step 4): the view names, their count (`front` and at
/// least one other), and the body's parameters in wire order (`quad`, `texture`, `pbr`, then
/// `face_limit` when given).
fn request_values(call: &CallRequest) -> Result<Values, String> {
    let views = match call.request.get("views") {
        Some(Value::Object(views)) if !views.is_empty() => views.clone(),
        _ => return Err("a mesh call needs its views, by name".into()),
    };
    let mut unknown: Vec<&str> = views
        .keys()
        .map(String::as_str)
        .filter(|name| !VIEWS.contains(name))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() || !views.contains_key("front") {
        let taken = "a multiview task takes front and any of back, left, right";
        return Err(if unknown.is_empty() {
            taken.to_string()
        } else {
            format!("{taken}; not {}", unknown.join(", "))
        });
    }
    // Every name is known and one is `front`, so a single name is `front` alone.
    if views.len() < 2 {
        return Err("a multiview task takes front and at least one of back, left, right".into());
    }
    let flag = |name: &str, default: bool| {
        call.request
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    let mut params = Map::new();
    params.insert("quad".into(), Value::Bool(flag("quad", false)));
    params.insert("texture".into(), Value::Bool(flag("texture", true)));
    params.insert("pbr".into(), Value::Bool(flag("pbr", false)));
    if let Some(limit) = call.request.get("face_limit").filter(|v| !v.is_null()) {
        let limit = limit
            .as_f64()
            .filter(|x| x.fract() == 0.0 && x.is_finite())
            .map(|x| x as i64)
            .filter(|x| FACE_LIMIT.contains(x))
            .ok_or_else(|| {
                format!(
                    "face_limit is {} to {}",
                    FACE_LIMIT.start(),
                    FACE_LIMIT.end()
                )
            })?;
        params.insert("face_limit".into(), json!(limit));
    }
    Ok(Values { views, params })
}

/// The view files (spec/providers.md §5 step 5), in Tripo's order: each must resolve to readable
/// bytes, of kind PNG or JPEG.
fn view_files(call: &CallRequest, views: &Map<String, Value>) -> Result<Vec<View>, String> {
    let mut found = Vec::with_capacity(views.len());
    for name in VIEWS {
        let Some(value) = views.get(name) else {
            continue;
        };
        let what = format!("the {name} view");
        let file = crate::wire::request_file(call, value, &what)?;
        if file.kind != "image/png" && file.kind != "image/jpeg" {
            return Err(format!(
                "the {name} view is {}; Tripo takes PNG or JPEG",
                file.kind
            ));
        }
        let bytes = crate::wire::read_file(file, &what)?;
        found.push(View {
            name,
            kind: file.kind.clone(),
            bytes,
        });
    }
    Ok(found)
}

impl LongJob for TripoMesh {
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
