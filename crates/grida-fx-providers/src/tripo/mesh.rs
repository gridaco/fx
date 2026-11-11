//! Tripo `mesh.generate`: one image or named views (src/tripo/README.md).
//!
//! A single `image` uploads once and posts `input` to `/generation/image-to-model`, sharing
//! defaults and collection with multiview.
//! `submit`: refuse a bad multiview request (no `front`, a view other than front/back/left/right, `front`
//! alone, a view that is not PNG or JPEG, no key); upload each
//! view in the order front, back, left, right (`<view>.png` / `<view>.jpg`); then post `POST
//! /generation/multiview-to-model` once with `{"model", "quad", "texture", "pbr", "face_limit"?,
//! "inputs": [{"front": token}, …]}`, then the provider options, which may set no member of
//! [`OWNED`] (spec/adapters.md §8). The handle is `{"task_id"}`.
//! `collect`: wait up to [`COLLECT_DEADLINE`]; download every model URL; keep the first FBX, else
//! the first GLB, as file `model`; `data: {"facts": {"model_kind": <kind>}}`; cost from credits.
//!
//! Refusals come in the order of spec/adapters.md §4: the route, the request's shape
//! (spec/capabilities.md §7), the key, the values (provider options, then the view names, then
//! their count), then the view files in the order front, back, left, right
//! (each: its bytes, then its kind). A multiview task takes two views at least: Tripo refuses
//! `front` alone only at the paid POST (HTTP 400, after the uploads, a failed call at the whole
//! hold), so the adapter refuses it first, at $0, before anything is sent.

use super::TripoApi;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Collected, Identity, LongJob, Submitted};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// How long one collect waits.
pub const COLLECT_DEADLINE: Duration = Duration::from_secs(1200);

/// The view names Tripo takes, in the order it takes them.
pub const VIEWS: [&str; 4] = ["front", "back", "left", "right"];

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "tripo-multiview";

/// The paid task's path.
pub const PATH: &str = "/generation/multiview-to-model";

/// The single-image task uses the same upload and collection protocol.
pub const IMAGE_PATH: &str = "/generation/image-to-model";

/// The contract this adapter serves: [`ADAPTER`], and `adapter_behavior` the number 1.
pub const IDENTITY: Identity = Identity {
    adapter: ADAPTER,
    behavior: "1",
};

/// The members provider options may not set (spec/adapters.md §8; src/tripo/README.md): those of
/// the paid body.
pub const OWNED: &[&str] = &[
    "model",
    "quad",
    "texture",
    "pbr",
    "face_limit",
    "inputs",
    "input",
];

const CAPABILITY: &str = "mesh.generate";

/// Tripo's mesh adapter (module doc).
#[derive(Debug, Clone)]
pub struct TripoMesh {
    api: TripoApi,
}

/// A request's values, checked.
struct Values {
    /// The views by name.
    source: Source,
    /// The body's parameters, in wire order.
    params: Map<String, Value>,
}

enum Source {
    Image(Value),
    Views(Map<String, Value>),
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
        if let Err(reason) = super::check_route(call, CAPABILITY, &IDENTITY) {
            return api.refused(&reason);
        }
        if let Err(reason) = crate::capabilities::check_request(CAPABILITY, &call.request) {
            return api.refused(&reason);
        }
        if let Err(reason) = api.client.credential() {
            return api.invalid(call, &reason);
        }
        let options = match crate::wire::provider_options(call, OWNED) {
            Ok(options) => options,
            Err(reason) => return api.invalid(call, &reason),
        };
        let Values { source, params } = match request_values(call) {
            Ok(values) => values,
            Err(reason) => return api.invalid(call, &reason),
        };
        let single = matches!(&source, Source::Image(_));
        let views = match &source {
            Source::Image(image) => image_file(call, image).map(|image| vec![image]),
            Source::Views(views) => view_files(call, views),
        };
        let views = match views {
            Ok(views) => views,
            Err(reason) => return api.invalid(call, &reason),
        };

        // The free phase: every view, in Tripo's order.
        let mut inputs = Vec::with_capacity(views.len());
        for view in views {
            let suffix = if view.kind == "image/png" {
                "png"
            } else if view.kind == "image/webp" {
                "webp"
            } else {
                "jpg"
            };
            let filename = format!("{}.{suffix}", view.name);
            match api.upload(call, &filename, &view.kind, view.bytes).await {
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
        let path = if single {
            body.insert("input".into(), inputs[0]["image"].clone());
            IMAGE_PATH
        } else {
            body.insert("inputs".into(), Value::Array(inputs));
            PATH
        };
        body.extend(options);
        match api.post_task(call, path, Value::Object(body)).await {
            Ok(task_id) => Submitted::Accepted {
                handle: json!({ "task_id": task_id }),
            },
            Err(outcome) => outcome,
        }
    }

    async fn collect_call(&self, call: &CallRequest, handle: &Value) -> Collected {
        let api = &self.api;
        let Some(task_id) = super::handle_task_id(handle) else {
            return api.unreachable("the Tripo handle has no usable task id");
        };
        let (task, models) = match api.finished_models(call, task_id, COLLECT_DEADLINE).await {
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
            // The task succeeded, so Tripo charged it what the read reports.
            return Collected::Ended {
                reason: api.client.reason("the Tripo task made no model"),
                cost: task.cost(),
            };
        };
        let (kind, bytes) = models.into_iter().nth(index).unwrap_or_default();
        let data = json!({"facts": {"model_kind": kind}});
        Collected::Answered(Answer::new(data, task.cost()).with_file("model", &kind, bytes))
    }
}

/// The value checks (spec/adapters.md §4 step 4): the view names, their count (`front` and at
/// least one other), and the body's parameters in wire order (`quad`, `texture`, `pbr`, then
/// `face_limit` when given).
fn request_values(call: &CallRequest) -> Result<Values, String> {
    let source = if let Some(image) = call.request.get("image").filter(|v| !v.is_null()) {
        Source::Image(image.clone())
    } else {
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
            return Err(
                "a multiview task takes front and at least one of back, left, right".into(),
            );
        }
        Source::Views(views)
    };
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
        // The capability checks its integer type. Model/topology limits belong to Tripo;
        // do not narrow or cast a canonical integer using a stale provider snapshot.
        let integer = serde_json::from_str(&grida_fx_core::value::canon(limit))
            .expect("a checked canonical integer is JSON");
        params.insert("face_limit".into(), integer);
    }
    Ok(Values { source, params })
}

fn image_file(call: &CallRequest, image: &Value) -> Result<View, String> {
    let file = crate::wire::request_file(call, image, "the source image")?;
    if !matches!(
        file.kind.as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err(format!(
            "the source image is {}; Tripo takes PNG, JPEG or WebP",
            file.kind
        ));
    }
    Ok(View {
        name: "image",
        kind: file.kind.clone(),
        bytes: crate::wire::read_file(file, "the source image")?,
    })
}

/// The view files (spec/adapters.md §4 step 5), in Tripo's order: each must resolve to readable
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

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(self.collect_call(call, handle))
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }
}
