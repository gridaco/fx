//! GG 3D: `mesh.generate` from views ([`GgMesh`]) and `mesh.rig` ([`GgRig`]), long jobs on GG's
//! specialized endpoints (src/gg/README.md; spec/adapters.md §6), run by [`super::job`].
//!
//! A specialized endpoint is named by its id, not by a model: GG's endpoint ids are opaque, so
//! the route's contract names the one it submits to (`endpoint`, such as
//! `three-d.tripo.p2.multiview`), and a route without one is not served. The submit is one
//! `POST {base}/endpoints/<endpoint>` as `multipart/form-data`: the request JSON as the text part
//! `input`, then the files GG takes as parts of their own, with the attempt's `Idempotency-Key`.
//!
//! - [`GgMesh`]: `input` is `{texture?, pbr?, face_limit?, quad?}`, each as the request gives it
//!   (`quad` only when `true`: no GG model takes `false`), and each view is a part
//!   `images.<view>`, in the order front, left, back, right. The model is `result.glb`.
//! - [`GgRig`]: `input` is `{rig_type, spec}`, `biped` and `mixamo` when the request names none
//!   (`skeleton` is `spec`), and the model is the part `mesh`, named `unrigged.glb`. The rigged
//!   model is `result.glb`. When the contract names a `check_endpoint`, a free riggability check
//!   comes first: the free phase of spec/adapters.md §6. Its job is submitted with the model
//!   (`input` `{}`, the attempt's `Idempotency-Key` with `-check` appended) and waited for up to
//!   [`CHECK_DEADLINE`]; its result is `{riggable, rig_type}`. A doubted model (`riggable` not
//!   true, or `rig_type` not the requested one) is refused unless `allow_negative_check` is true.
//!   A check job that ends without a result is `Refused`; one still running at its deadline, or
//!   a `riggable` that is not a boolean, is `NotReceived`, and the engine sends the whole submit
//!   again, which GG answers with the same check job. The handle is then `{"job_id", "check":
//!   {"job_id", "riggable", "rig_type"}, "advisory_override"}`, the check's `rig_type` kept only
//!   when it is a short plain string (`^[A-Za-z0-9_.:-]{1,96}$`), else `null`.
//! - Each paid `input` ends with the provider options, which may set no member of [`MESH_OWNED`] or
//!   [`RIG_OWNED`] (spec/adapters.md §8); a rig's check job takes none.
//!
//! Each answer is file `model`, the GLB the download holds (`glTF` first), with
//! `data: {"facts": {"model_kind": "model/gltf-binary"}}` for a mesh and
//! `data: {"facts": {"riggable", "checked_rig_type", "advisory_override"}}` for a rig, from its
//! handle: `null`, `null` and `false` when no check ran.
//!
//! Submit refusals, in this order (spec/adapters.md §4): another route's contract, or one without
//! an endpoint; a request that does not fit its capability; no `GG_API_KEY`; the values (provider
//! options, [`wire::provider_options`], before the check job; a mesh's view names and count); then
//! the files (a mesh's views in the order front, left, back, right, each PNG or JPEG; a rig's
//! model, a GLB). The model's own limits (`face_limit`, rig types, skeletons) are GG's to refuse,
//! at no cost.

use super::Gateway;
use super::job::{Jobs, Kind, job_id};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Collected, Identity, LongJob, Submitted};
use crate::capabilities;
use crate::clock::Clock;
use crate::transport::{Body, HttpRequest, Lane, Method, Part};
use crate::wire;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// The contract `adapter` of [`GgMesh`].
pub const MESH_ADAPTER: &str = "gg-mesh";

/// The contract `adapter` of [`GgRig`].
pub const RIG_ADAPTER: &str = "gg-rig";

/// The contract [`GgMesh`] serves: [`MESH_ADAPTER`], behaviour the number 1.
pub const MESH_IDENTITY: Identity = Identity {
    adapter: MESH_ADAPTER,
    behavior: "1",
};

/// The contract [`GgRig`] serves: [`RIG_ADAPTER`], behaviour the number 1.
pub const RIG_IDENTITY: Identity = Identity {
    adapter: RIG_ADAPTER,
    behavior: "1",
};

/// The `input` members provider options may not set on a mesh (spec/adapters.md §8;
/// src/gg/README.md): the ones the adapter sends.
pub const MESH_OWNED: &[&str] = &["texture", "pbr", "face_limit", "quad"];

/// The `input` members provider options may not set on a rig: the ones the adapter sends.
pub const RIG_OWNED: &[&str] = &["rig_type", "spec"];

/// The view names GG takes, in the order they are sent.
pub const VIEWS: [&str; 4] = ["front", "left", "back", "right"];

/// The most bytes of one model.
pub const MAX_MODEL_BYTES: u64 = 150_000_000;

/// The kind of every model GG answers with.
pub const GLB: &str = "model/gltf-binary";

/// How long a rig's free check may take.
pub const CHECK_DEADLINE: Duration = Duration::from_secs(600);

/// The rig type when the request names none.
pub const DEFAULT_RIG_TYPE: &str = "biped";

/// The skeleton convention when the request names none.
pub const DEFAULT_SKELETON: &str = "mixamo";

/// A 3D model job.
pub const MESH_KIND: Kind = Kind {
    noun: "3D model",
    submit_label: "GG 3D model submission",
    job_label: "GG 3D model job",
    download_label: "GG output model download",
    outstanding: "GG has not finished the 3D model job yet; it is collected on the next run",
    reference: |result| result.get("glb"),
    max_bytes: MAX_MODEL_BYTES,
    deliver: |bytes| {
        glb(&bytes)?;
        Ok(Answer::new(json!({"facts": {"model_kind": GLB}}), None).with_file("model", GLB, bytes))
    },
};

/// A rig job.
pub const RIG_KIND: Kind = Kind {
    noun: "rig",
    submit_label: "GG rig submission",
    job_label: "GG rig job",
    download_label: "GG output model download",
    outstanding: "GG has not finished the rig job yet; it is collected on the next run",
    reference: |result| result.get("glb"),
    max_bytes: MAX_MODEL_BYTES,
    deliver: |bytes| {
        glb(&bytes)?;
        let facts = json!({"riggable": null, "checked_rig_type": null, "advisory_override": false});
        Ok(Answer::new(json!({"facts": facts}), None).with_file("model", GLB, bytes))
    },
};

/// A rig's free check.
pub const CHECK_KIND: Kind = Kind {
    noun: "rig check",
    submit_label: "GG rig check submission",
    job_label: "GG rig check job",
    download_label: "GG rig check download",
    outstanding: "GG has not finished the rig check yet",
    reference: |_| None,
    max_bytes: 0,
    deliver: |_| Err("a rig check has no file".into()),
};

/// A short plain string a handle and an answer may keep.
static PLAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:-]{1,96}$").expect("a valid pattern"));

/// GG's endpoint ids: one path segment, never `.` or `..`.
static ENDPOINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,159}$").expect("a valid pattern"));

/// The bytes are a GLB, or the sentence that ends the job.
fn glb(bytes: &[u8]) -> Result<(), String> {
    if wire::sniff_model(bytes) == Some(GLB) {
        Ok(())
    } else {
        Err("GG output model is not a GLB file".into())
    }
}

/// The endpoints a 3D route contract names: the one it submits to and, for a rig, its check's.
struct Endpoints {
    endpoint: String,
    check: Option<String>,
}

/// Reads a 3D route contract (spec/adapters.md §4 step 1): `adapter`, behaviour `1`, the
/// `endpoint` it submits to and, among `members`, a rig's `check_endpoint`.
fn endpoints(
    call: &CallRequest,
    capability: &str,
    identity: &Identity,
    members: &[&str],
) -> Result<Endpoints, String> {
    let contract = super::read_contract(
        call,
        &[capability],
        identity.adapter,
        |v| identity.behaves(v),
        members,
    )?;
    let id = |name: &str| match contract.get(name) {
        Some(Value::String(id)) if ENDPOINT.is_match(id) => Ok(Some(id.clone())),
        Some(_) => Err(super::not_served(
            call,
            Some(&format!("{name} is one of GG's endpoint ids")),
        )),
        None => Ok(None),
    };
    let Some(endpoint) = id("endpoint")? else {
        return Err(super::not_served(
            call,
            Some("the contract names no GG endpoint"),
        ));
    };
    Ok(Endpoints {
        endpoint,
        check: id("check_endpoint")?,
    })
}

/// A submit request: `input` and `files` to `endpoint`, under the idempotency key `key`.
fn submit_request(
    jobs: &Jobs,
    endpoint: &str,
    key: &str,
    input: Map<String, Value>,
    files: Vec<Part>,
) -> Result<HttpRequest, String> {
    let gateway = &jobs.gateway;
    let credential = gateway.credential()?;
    let mut parts = vec![Part::text("input", &Value::Object(input).to_string())];
    parts.extend(files);
    Ok(HttpRequest::new(
        Method::Post,
        gateway.client.url(&format!("endpoints/{endpoint}")),
        Lane::Provider,
    )
    .credential(credential)
    .header("idempotency-key", key)
    .body(Body::Multipart(parts))
    .timeout(super::job::SUBMIT_DEADLINE)
    .max_response_bytes(super::job::JSON_RESPONSE_BYTES))
}

/// GG's `mesh.generate` adapter (module doc).
#[derive(Debug, Clone)]
pub struct GgMesh {
    jobs: Jobs,
}

impl GgMesh {
    pub fn new(gateway: Gateway, clock: Arc<dyn Clock>) -> GgMesh {
        GgMesh {
            jobs: Jobs::new(gateway, clock),
        }
    }

    /// Steps 1 to 5 of spec/adapters.md §4, then the submit request (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let endpoint = endpoints(call, "mesh.generate", &MESH_IDENTITY, &["endpoint"])?.endpoint;
        capabilities::check_request(&call.route.capability, &call.request)?;
        self.jobs.gateway.credential()?;
        let options = wire::provider_options(call, MESH_OWNED)?;
        let views = view_names(&call.request)?;
        let mut input = Map::new();
        for flag in ["texture", "pbr"] {
            if let Some(value) = call.request.get(flag).filter(|v| v.is_boolean()) {
                input.insert(flag.into(), value.clone());
            }
        }
        if let Some(limit) = call.request.get("face_limit").filter(|v| !v.is_null()) {
            input.insert("face_limit".into(), limit.clone());
        }
        if call.request.get("quad").and_then(Value::as_bool) == Some(true) {
            input.insert("quad".into(), json!(true));
        }
        input.extend(options);
        let mut files = Vec::with_capacity(views.len());
        for name in VIEWS {
            let Some(value) = views.get(name) else {
                continue;
            };
            let what = format!("the {name} view");
            let file = wire::request_file(call, value, &what)?;
            let extension = match file.kind.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                kind => return Err(format!("the {name} view is {kind}; GG takes PNG or JPEG")),
            };
            let bytes = wire::read_file(file, &what)?;
            files.push(Part::file(
                &format!("images.{name}"),
                &format!("{name}.{extension}"),
                &file.kind,
                bytes,
            ));
        }
        let key = self.jobs.gateway.idempotency_key(call);
        submit_request(&self.jobs, &endpoint, &key, input, files)
    }
}

/// The views by name (spec/adapters.md §4 step 4): `front` and at least one of the others, no
/// other name.
fn view_names(request: &Value) -> Result<&Map<String, Value>, String> {
    let views = match request.get("views") {
        Some(Value::Object(views)) if !views.is_empty() => views,
        _ => return Err("a mesh call needs its views, by name".into()),
    };
    let mut unknown: Vec<&str> = views
        .keys()
        .map(String::as_str)
        .filter(|name| !VIEWS.contains(name))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() || !views.contains_key("front") {
        let taken = "a multiview job takes front and any of left, back, right";
        return Err(if unknown.is_empty() {
            taken.to_string()
        } else {
            format!("{taken}; not {}", unknown.join(", "))
        });
    }
    if views.len() < 2 {
        return Err("a multiview job takes front and at least one of left, back, right".into());
    }
    Ok(views)
}

impl LongJob for GgMesh {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(async move { self.jobs.submit(call, &MESH_KIND, self.prepare(call)).await })
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(async move {
            let contract =
                endpoints(call, "mesh.generate", &MESH_IDENTITY, &["endpoint"]).map(|_| ());
            self.jobs
                .collect(job_id(handle), &MESH_KIND, contract)
                .await
        })
    }

    fn identity(&self) -> Option<Identity> {
        Some(MESH_IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        endpoints(call, "mesh.generate", &MESH_IDENTITY, &["endpoint"]).map(|_| ())
    }
}

/// GG's `mesh.rig` adapter (module doc).
#[derive(Debug, Clone)]
pub struct GgRig {
    jobs: Jobs,
}

/// A rig call, ready to send.
struct Rig {
    endpoints: Endpoints,
    rig_type: String,
    allowed: bool,
    input: Map<String, Value>,
    model: Vec<u8>,
}

/// The rig contract's members besides `adapter` and `adapter_behavior`.
const RIG_MEMBERS: [&str; 2] = ["endpoint", "check_endpoint"];

impl GgRig {
    pub fn new(gateway: Gateway, clock: Arc<dyn Clock>) -> GgRig {
        GgRig {
            jobs: Jobs::new(gateway, clock),
        }
    }

    /// Steps 1 to 5 of spec/adapters.md §4 (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<Rig, String> {
        let endpoints = endpoints(call, "mesh.rig", &RIG_IDENTITY, &RIG_MEMBERS)?;
        capabilities::check_request(&call.route.capability, &call.request)?;
        self.jobs.gateway.credential()?;
        let options = wire::provider_options(call, RIG_OWNED)?;
        let text = |name: &str, default: &str| match call.request.get(name) {
            Some(Value::String(text)) if !text.is_empty() => text.clone(),
            _ => default.to_string(),
        };
        let rig_type = text("rig_type", DEFAULT_RIG_TYPE);
        let mut input = Map::new();
        input.insert("rig_type".into(), json!(rig_type));
        input.insert("spec".into(), json!(text("skeleton", DEFAULT_SKELETON)));
        input.extend(options);
        let value = call.request.get("model").unwrap_or(&Value::Null);
        let file = wire::request_file(call, value, "model")?;
        if file.kind != GLB {
            return Err(format!("GG rigs a GLB, not {}", file.kind));
        }
        let model = wire::read_file(file, "model")?;
        Ok(Rig {
            endpoints,
            rig_type,
            allowed: call.request.get("allow_negative_check") == Some(&Value::Bool(true)),
            input,
            model,
        })
    }

    async fn submit_call(&self, call: &CallRequest) -> Submitted {
        let gateway = &self.jobs.gateway;
        let refused = |reason: &str| Submitted::Refused {
            reason: gateway.reason(reason),
        };
        let rig = match self.prepare(call) {
            Ok(rig) => rig,
            Err(reason) => return refused(&reason),
        };
        let mesh = || vec![Part::file("mesh", "unrigged.glb", GLB, rig.model.clone())];
        let key = gateway.idempotency_key(call);
        let check = match &rig.endpoints.check {
            None => None,
            Some(endpoint) => {
                let key = format!("{key}-check");
                let request = match submit_request(&self.jobs, endpoint, &key, Map::new(), mesh()) {
                    Ok(request) => request,
                    Err(reason) => return refused(&reason),
                };
                match self.check(call, request, &rig).await {
                    Ok(check) => Some(check),
                    Err(outcome) => return outcome,
                }
            }
        };
        let paid = submit_request(
            &self.jobs,
            &rig.endpoints.endpoint,
            &key,
            rig.input.clone(),
            mesh(),
        );
        match self.jobs.submit(call, &RIG_KIND, paid).await {
            Submitted::Accepted { handle } => Submitted::Accepted {
                handle: match check {
                    None => handle,
                    Some((check, doubted)) => json!({
                        "job_id": handle["job_id"],
                        "check": check,
                        "advisory_override": doubted,
                    }),
                },
            },
            other => other,
        }
    }

    /// The free check (module doc): its handle part `{"job_id", "riggable", "rig_type"}` and
    /// whether the model is doubted, or the submit's outcome.
    async fn check(
        &self,
        call: &CallRequest,
        request: HttpRequest,
        rig: &Rig,
    ) -> Result<(Value, bool), Submitted> {
        let gateway = &self.jobs.gateway;
        let not_received = |reason: &str| Submitted::not_received(gateway.reason(reason));
        let id = self.jobs.submit_free(call, &CHECK_KIND, request).await?;
        let credential = gateway
            .credential()
            .map_err(|reason| Submitted::Refused { reason })?;
        let deadline = self.jobs.now() + CHECK_DEADLINE;
        let job = match self
            .jobs
            .await_result(&credential, &id, &CHECK_KIND, deadline)
            .await
        {
            Ok(job) => job,
            Err(Collected::Ended { reason, .. }) => return Err(Submitted::Refused { reason }),
            Err(Collected::Unreachable { reason }) => {
                return Err(Submitted::NotReceived {
                    reason,
                    retry_after: None,
                });
            }
            Err(Collected::Answered(_)) => {
                return Err(not_received("GG's rig check answered with a file"));
            }
        };
        let result = job.get("result");
        let Some(riggable) = result
            .and_then(|result| result.get("riggable"))
            .and_then(Value::as_bool)
        else {
            return Err(not_received("GG's rig check answered without a verdict"));
        };
        let returned = result
            .and_then(|result| result.get("rig_type"))
            .unwrap_or(&Value::Null);
        let checked = plain_or_null(returned);
        let doubted = !riggable || checked != json!(rig.rig_type);
        if doubted && !rig.allowed {
            let shown = match &checked {
                Value::String(text) => text.clone(),
                _ => "unknown".into(),
            };
            return Err(Submitted::Refused {
                reason: gateway.reason(&format!(
                    "GG's rig check doubts this model (riggable={riggable}, rig_type={shown})"
                )),
            });
        }
        Ok((
            json!({"job_id": id, "riggable": riggable, "rig_type": checked}),
            doubted,
        ))
    }

    async fn collect_call(&self, call: &CallRequest, handle: &Value) -> Collected {
        let contract = endpoints(call, "mesh.rig", &RIG_IDENTITY, &RIG_MEMBERS).map(|_| ());
        let collected = self
            .jobs
            .collect(rig_job_id(handle), &RIG_KIND, contract)
            .await;
        let Collected::Answered(mut answer) = collected else {
            return collected;
        };
        let check = handle.get("check");
        let riggable = check
            .and_then(|check| check.get("riggable"))
            .filter(|value| value.is_boolean())
            .cloned()
            .unwrap_or(Value::Null);
        let checked_rig_type = check
            .and_then(|check| check.get("rig_type"))
            .map_or(Value::Null, plain_or_null);
        let advisory_override = handle
            .get("advisory_override")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        answer.data = json!({"facts": {
            "riggable": riggable,
            "checked_rig_type": checked_rig_type,
            "advisory_override": advisory_override,
        }});
        Collected::Answered(answer)
    }
}

/// The rig job a rig handle names: `{"job_id"}`, or that with the check's `check` and
/// `advisory_override`.
fn rig_job_id(handle: &Value) -> Option<&str> {
    let members = handle.as_object()?;
    if members
        .keys()
        .any(|key| !matches!(key.as_str(), "job_id" | "check" | "advisory_override"))
    {
        return None;
    }
    job_id(&json!({"job_id": members.get("job_id")?}))?;
    members.get("job_id").and_then(Value::as_str)
}

/// `value` when it is a short plain string, else `null`.
fn plain_or_null(value: &Value) -> Value {
    match value {
        Value::String(text) if PLAIN.is_match(text) => value.clone(),
        _ => Value::Null,
    }
}

impl LongJob for GgRig {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(self.submit_call(call))
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(self.collect_call(call, handle))
    }

    fn identity(&self) -> Option<Identity> {
        Some(RIG_IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        endpoints(call, "mesh.rig", &RIG_IDENTITY, &RIG_MEMBERS).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_are_front_and_another() {
        let views = |value: Value| view_names(&json!({"views": value})).map(|views| views.len());
        assert_eq!(views(json!({"front": 1, "back": 2})), Ok(2));
        assert_eq!(
            views(json!({})).unwrap_err(),
            "a mesh call needs its views, by name"
        );
        assert_eq!(
            views(json!({"back": 1, "left": 2})).unwrap_err(),
            "a multiview job takes front and any of left, back, right"
        );
        assert_eq!(
            views(json!({"front": 1, "top": 2, "side": 3})).unwrap_err(),
            "a multiview job takes front and any of left, back, right; not side, top"
        );
        assert_eq!(
            views(json!({"front": 1})).unwrap_err(),
            "a multiview job takes front and at least one of left, back, right"
        );
    }

    #[test]
    fn answers_are_glbs() {
        let glb_bytes = b"glTF\x02\0\0\0rest".to_vec();
        let answer = (MESH_KIND.deliver)(glb_bytes.clone()).unwrap();
        assert_eq!(answer.files["model"].kind, GLB);
        assert_eq!(answer.data, json!({"facts": {"model_kind": GLB}}));
        let answer = (RIG_KIND.deliver)(glb_bytes).unwrap();
        assert_eq!(
            answer.data,
            json!({"facts": {"riggable": null, "checked_rig_type": null, "advisory_override": false}})
        );
        assert_eq!(rig_job_id(&json!({"job_id": "j1"})), Some("j1"));
        assert_eq!(
            rig_job_id(&json!({"job_id": "j1", "check": {}, "advisory_override": true})),
            Some("j1")
        );
        assert_eq!(rig_job_id(&json!({"job_id": "j1", "url": "x"})), None);
        assert_eq!(rig_job_id(&json!({"job_id": ".."})), None);
        assert_eq!(plain_or_null(&json!("biped")), json!("biped"));
        assert_eq!(plain_or_null(&json!("a b")), Value::Null);
        assert_eq!(
            (MESH_KIND.deliver)(b"Kaydara FBX Binary  \0".to_vec()).unwrap_err(),
            "GG output model is not a GLB file"
        );
        assert_eq!(
            (MESH_KIND.reference)(&json!({"glb": {"url": "https://h.example/m.glb"}})),
            Some(&json!({"url": "https://h.example/m.glb"}))
        );
    }
}
