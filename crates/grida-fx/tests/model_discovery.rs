//! Selected-model preparation through the command line. All HTTP traffic is synthetic and
//! confined to an explicitly configured loopback provider. The managed-call pilot returns
//! synthesized pixels and billing from the fixture; no real provider or account is contacted.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const MODEL: &str = "acme/img-v2";
const OPENROUTER_PATH: &str = "/v1/images/models/acme/img-v2/endpoints";
const PREPARED: &[&str] = &["plan", "workflow.yaml", "--json", "--prepared", "--check"];

struct Reply {
    method: String,
    path: String,
    status: u16,
    body: Value,
}

/// Blocking accept is woken by Drop. Every accepted stream has finite read/write deadlines;
/// unexpected requests receive a response too, so a failed expectation cannot hang the child.
struct MetadataServer {
    address: SocketAddr,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<String>>>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl MetadataServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let replies = Arc::new(Mutex::new(VecDeque::<Reply>::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let (pending, received, stop) = (
            Arc::clone(&replies),
            Arc::clone(&requests),
            Arc::clone(&stopping),
        );
        let worker = std::thread::spawn(move || {
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                if stop.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 || bytes.len() > 32 * 1024 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let header_end = bytes
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .expect("HTTP request headers")
                    + 4;
                let content_length = String::from_utf8_lossy(&bytes[..header_end])
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                assert!(content_length < 32 * 1024, "synthetic request is bounded");
                while bytes.len() < header_end + content_length {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(bytes).unwrap();
                let method = request.split_whitespace().next().unwrap_or("");
                let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                received.lock().unwrap().push(request.clone());
                let reply = pending.lock().unwrap().pop_front();
                let (status, body) = match reply {
                    Some(reply) if reply.method == method && reply.path == path => {
                        (reply.status, reply.body)
                    }
                    _ => (
                        500,
                        json!({"error":"unexpected synthetic metadata request"}),
                    ),
                };
                let body = serde_json::to_vec(&body).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Synthetic\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            address,
            replies,
            requests,
            stopping,
            worker: Some(worker),
        }
    }

    fn base(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn reply(&self, path: &str, status: u16, body: Value) {
        self.reply_method("GET", path, status, body);
    }

    fn reply_method(&self, method: &str, path: &str, status: u16, body: Value) {
        self.replies.lock().unwrap().push_back(Reply {
            method: method.into(),
            path: path.into(),
            status,
            body,
        });
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MetadataServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // Wake accept without a polling loop or a timing-dependent sleep.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(worker) = self.worker.take() {
            let finished = worker.join();
            if !std::thread::panicking() {
                finished.unwrap();
            }
        }
    }
}

fn configure(root: &Path, provider: &str) {
    let project = json!({"fx":"project/v1", "routes":{"image.generate":MODEL},
        "provider_preferences":{"image.generate":provider}});
    std::fs::write(root.join("fx.yaml"), serde_json::to_vec(&project).unwrap()).unwrap();
}

fn project(provider: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    configure(root.path(), provider);
    std::fs::write(root.path().join("workflow.yaml"), "fx: workflow/v1\nid: discovery\ntitle: Discovered image\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    with: {prompt: a small lantern}\noutputs:\n  image: '${{ steps.draw.outputs.image }}'\n").unwrap();
    root
}

fn command(
    root: &Path,
    provider: &str,
    server: &MetadataServer,
    online: bool,
    args: &[&str],
) -> Output {
    command_with_key(
        root,
        provider,
        server,
        online,
        args,
        Some("synthetic-discovery-key"),
    )
}

fn command_with_key(
    root: &Path,
    provider: &str,
    server: &MetadataServer,
    online: bool,
    args: &[&str],
    credential: Option<&str>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .current_dir(root)
        .env_clear()
        .args(args)
        .env("HOME", root)
        .env("NO_COLOR", "1")
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("GRIDA_FX_NETWORK", if online { "on" } else { "off" });
    let (key, base) = match provider {
        "gg" => ("GG_API_KEY", "GG_BASE_URL"),
        "openrouter" => ("OPENROUTER_API_KEY", "OPENROUTER_BASE_URL"),
        _ => unreachable!(),
    };
    if let Some(credential) = credential {
        command.env(key, credential);
    }
    command.env(base, server.base());
    command.output().unwrap()
}

fn document(output: &Output, expected: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn openrouter_metadata(priced: bool) -> Value {
    let endpoint = |price: &str| {
        json!({
            "supported_parameters": {
                "size":{"type":"enum","values":["1024x1024"]},
                "background":{"type":"enum","values":["auto","opaque","transparent"]}
            },
            "pricing": if priced { json!([{"billable":"output_image","unit":"image","cost_usd":price}]) } else { json!([]) }
        })
    };
    json!({"id":MODEL,"description":"untrusted metadata must not be cached verbatim",
        "endpoints":[endpoint("0.25"),endpoint("0.40")]})
}

fn serve_model(server: &MetadataServer, provider: &str, priced: bool) {
    if provider == "openrouter" {
        server.reply(OPENROUTER_PATH, 200, openrouter_metadata(priced));
        return;
    }
    let request = json!({"$ref":"#/$defs/ImageRequest"});
    let result = json!({"$ref":"#/$defs/ImageResult"});
    server.reply(
        "/v1/models",
        200,
        json!({"revision":"synthetic-v2","data":[{"id":MODEL,"gg":{
        "kind":"image","interfaces":["images.generations"],"capabilities":{"image_generation":true},
        "operation_ids":["acme-image-generate"],"request_schemas":{"images.generations":request},
        "result_schemas":{"images.generations":result}}}]}),
    );
    server.reply(
        "/v1/endpoints",
        200,
        json!({"revision":"synthetic-v2","data":[]}),
    );
    server.reply("/schemas/operations.json", 200, json!({"x-gg-operations":[{
        "model_id":MODEL,"kind":"image","path":"/v1/images/generations",
        "content_type":"application/json","request_view":"json","response_mode":"synchronous",
        "delivery":"transient_base64","operation_id":"acme-image-generate",
        "request_schema":request,"result_schema":result}],"$defs":{
            "ImageRequest":{"type":"object","required":["model","prompt"],"properties":{
                "model":{"const":MODEL},"prompt":{"type":"string"},
                "size":{"enum":["auto","1024x1024"]},"background":{"enum":["auto","opaque","transparent"]},
                "output_format":{"enum":["png"]}}},
            "ImageResult":{"type":"object","properties":{"data":{"type":"array","items":{
                "type":"object","properties":{"b64_json":{"type":"string"}}}}}}}}));
}

fn cached_files(root: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(root.join(".fx/models"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

fn expire_cache(root: &Path) {
    for file in cached_files(root) {
        let mut value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        value["fetched_at"] = json!(0);
        // Cache publications are read-only; replace our synthetic entry rather than editing it.
        std::fs::remove_file(&file).unwrap();
        std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
    }
}

#[test]
fn ordinary_prepared_plan_discovers_openrouter_and_prices_one_snapshot() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", true);
    let prepared = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    assert_eq!(prepared["kind"], "fx-prepared-plan-v1");
    assert_eq!(prepared["graph"]["problems"], json!([]));
    assert_eq!(prepared["graph"]["estimate"]["high_usd"], 0.4);
    assert_eq!(prepared["price"]["estimate"]["high_usd"], 0.4);
    let entry = &prepared["resolved_models"]["routes"][0];
    assert_eq!(entry["route"], "acme/img-v2@openrouter");
    assert_eq!(entry["price"], json!({"low_usd":0.25,"high_usd":0.4}));
    assert_eq!(
        entry["contract"]["adapter"],
        "openrouter-image-discovered-v1"
    );
    let fresh = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    assert_eq!(
        fresh, prepared,
        "a fresh cache keeps the same prepared facts"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with(&format!("GET {OPENROUTER_PATH} ")));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-discovery-key")
    );
    let caches = cached_files(root.path());
    assert_eq!(caches.len(), 1);
    let cache = std::fs::read_to_string(&caches[0]).unwrap();
    for private in [
        "synthetic-discovery-key",
        "127.0.0.1",
        "untrusted metadata",
        root.path().to_str().unwrap(),
    ] {
        assert!(
            !cache.contains(private),
            "metadata cache retained {private}"
        );
    }
    assert!(!root.path().join("runs").exists());
}

#[test]
fn gg_keyless_metadata_refuses_unpriced_binding_before_generation() {
    let root = project("gg");
    let server = MetadataServer::new();
    serve_model(&server, "gg", false);
    let refused = document(
        &command_with_key(root.path(), "gg", &server, true, PREPARED, None),
        1,
    );
    let problems = refused["graph"]["problems"].to_string();
    assert!(
        problems.contains("provider metadata does not provide a computable per-call price"),
        "{problems}"
    );
    assert!(
        problems.contains("supply an authored route with price or cap_usd"),
        "{problems}"
    );
    assert_eq!(refused["resolved_models"]["routes"], json!([]));
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request.starts_with("GET ")
        && !request.to_ascii_lowercase().contains("authorization")));
    assert!(!root.path().join(".fx/models").exists());
    assert!(!root.path().join("runs").exists());
}

#[test]
fn published_prices_remain_available_from_expired_cache_offline_without_rekeying() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", true);
    let initial = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    expire_cache(root.path());
    let cached = document(
        &command(root.path(), "openrouter", &server, false, PREPARED),
        0,
    );
    assert_eq!(server.requests().len(), 1);
    assert_eq!(cached["price"]["estimate"]["high_usd"], 0.4);
    assert_eq!(
        cached["resolved_models"]["routes"][0]["price"],
        json!({"low_usd":0.25,"high_usd":0.4})
    );
    assert!(
        initial["graph"]["instances"][0]["identity"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        initial["graph"]["instances"][0]["identity"],
        cached["graph"]["instances"][0]["identity"]
    );
    assert_eq!(
        initial["graph"]["instances"][0]["routes"],
        cached["graph"]["instances"][0]["routes"]
    );
}

#[test]
fn uncomputable_price_refuses_until_an_authored_route_supplies_a_cap() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", false);
    let refused = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        1,
    );
    let problems = refused["graph"]["problems"].to_string();
    assert!(
        problems.contains("provider metadata does not provide a computable per-call price"),
        "{problems}"
    );
    assert!(
        problems.contains("supply an authored route with price or cap_usd"),
        "{problems}"
    );
    assert_eq!(refused["resolved_models"]["routes"], json!([]));
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("fx.yaml")).unwrap()).unwrap();
    config["route_tables"] = json!(["authored.json"]);
    std::fs::write(
        root.path().join("fx.yaml"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("authored.json"),
        serde_json::to_vec(&json!({"fx":"routes/v1","routes":[{
            "capability":"image.generate","route":"acme/img-v2@openrouter","price":{"cap_usd":3}
        }]}))
        .unwrap(),
    )
    .unwrap();
    let prepared = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    assert_eq!(prepared["price"]["estimate"]["high_usd"], 3);
    assert_eq!(prepared["resolved_models"]["routes"], json!([]));
    assert_eq!(
        server.requests().len(),
        1,
        "an authored cap needs no additional discovery"
    );
}

#[test]
fn authored_override_and_explicit_route_tables_do_not_discover() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("fx.yaml")).unwrap()).unwrap();
    config["route_tables"] = json!(["authored.json"]);
    std::fs::write(
        root.path().join("fx.yaml"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("authored.json"),
        serde_json::to_vec(&json!({"fx":"routes/v1","routes":[{
            "capability":"image.generate","route":"acme/img-v2@openrouter","price":{"usd":0.07}
        }]}))
        .unwrap(),
    )
    .unwrap();
    let authored = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    assert_eq!(authored["price"]["estimate"]["high_usd"], 0.07);
    assert_eq!(authored["resolved_models"]["routes"], json!([]));
    configure(root.path(), "openrouter");
    std::fs::write(
        root.path().join("isolated.json"),
        "{\"fx\":\"routes/v1\",\"routes\":[]}",
    )
    .unwrap();
    let mut args = PREPARED.to_vec();
    args.extend(["--routes", "isolated.json"]);
    let isolated = document(&command(root.path(), "openrouter", &server, true, &args), 1);
    assert!(!isolated["graph"]["problems"].as_array().unwrap().is_empty());
    assert_eq!(isolated["resolved_models"]["routes"], json!([]));
    assert!(server.requests().is_empty());
    assert!(!root.path().join(".fx/models").exists());
}

#[test]
fn canonical_metadata_on_an_authored_native_route_cannot_be_overwritten_by_discovery() {
    let root = project("gg");
    let server = MetadataServer::new();
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("fx.yaml")).unwrap()).unwrap();
    config["route_tables"] = json!(["authored.json"]);
    std::fs::write(
        root.path().join("fx.yaml"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    std::fs::write(root.path().join("authored.json"), serde_json::to_vec(&json!({"fx":"routes/v1","routes":[{
        "capability":"image.generate","route":"acme/img-v2@gg","model_id":"acme/canonical-v2","price":{"usd":0.07}
    }]})).unwrap()).unwrap();
    // The bare native ID no longer matches an entry with a distinct canonical model_id.
    // Discovery must not replace that authored entry to manufacture such a match.
    let refused = document(&command(root.path(), "gg", &server, true, PREPARED), 1);
    assert!(
        refused["graph"]["problems"]
            .to_string()
            .contains("no route for model acme/img-v2")
    );
    assert_eq!(refused["resolved_models"]["routes"], json!([]));
    assert!(server.requests().is_empty());
}

#[test]
fn local_facts_need_no_key_but_another_account_refreshes_its_metadata() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", true);
    let original = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    let offline = document(
        &command_with_key(root.path(), "openrouter", &server, false, PREPARED, None),
        0,
    );
    assert_eq!(offline, original);
    assert_eq!(server.requests().len(), 1);
    serve_model(&server, "openrouter", true);
    document(
        &command_with_key(
            root.path(),
            "openrouter",
            &server,
            true,
            PREPARED,
            Some("synthetic-other-account"),
        ),
        0,
    );
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        2,
        "a fresh entry for one key is not another account's metadata"
    );
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-other-account")
    );
}

#[test]
fn explicit_not_found_does_not_fall_back_to_expired_metadata() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", true);
    document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    expire_cache(root.path());
    server.reply(
        OPENROUTER_PATH,
        404,
        json!({"error":"private rejected body"}),
    );
    let refused = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        1,
    );
    let problems = refused["graph"]["problems"].to_string();
    assert!(problems.contains("was not found"), "{problems}");
    assert!(!problems.contains("private rejected body"));
    assert_eq!(refused["resolved_models"]["routes"], json!([]));
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn prepared_dry_run_and_resume_keep_model_facts_after_metadata_cache_is_removed() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", true);
    let prepared = document(
        &command(root.path(), "openrouter", &server, true, PREPARED),
        0,
    );
    std::fs::write(
        root.path().join("prepared.json"),
        serde_json::to_vec(&prepared["resolved_models"]).unwrap(),
    )
    .unwrap();
    std::fs::remove_dir_all(root.path().join(".fx/models")).unwrap();
    let run_args = [
        "run",
        "workflow.yaml",
        "--run",
        "runs/prepared",
        "--no-view",
        "--max-usd",
        "1",
    ];
    let mut initial_args = run_args.to_vec();
    initial_args.extend(["--resolved-models", "prepared.json"]);
    let first = command(root.path(), "openrouter", &server, false, &initial_args);
    assert_eq!(
        first.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let folder = root.path().join("runs/prepared");
    let recorded: Value =
        serde_json::from_slice(&std::fs::read(folder.join("resolved-models.json")).unwrap())
            .unwrap();
    assert_eq!(recorded, prepared["resolved_models"]);
    let first_plan = std::fs::read(folder.join("plan.json")).unwrap();
    let resumed = command(root.path(), "openrouter", &server, false, &run_args);
    assert_eq!(
        resumed.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert!(String::from_utf8_lossy(&resumed.stdout).contains("run       runs/prepared"));
    assert_eq!(std::fs::read(folder.join("plan.json")).unwrap(), first_plan);
    let events: Vec<Value> = std::fs::read_to_string(folder.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "run_started")
            .count(),
        2
    );
    assert_eq!(server.requests().len(), 1);
    assert!(!root.path().join(".fx/models").exists());
}

#[test]
fn discovered_model_runs_settles_and_replays_its_paid_call_without_a_key() {
    let root = project("openrouter");
    let server = MetadataServer::new();
    serve_model(&server, "openrouter", true);
    // Pixels are synthesized by the committed testing helper, never generated artwork.
    let png = grida_fx_providers::testing::media::png(2, 2, None);
    server.reply_method(
        "POST",
        "/v1/images",
        200,
        json!({"data":[{"b64_json":STANDARD.encode(&png)}],"usage":{"cost":0.02}}),
    );
    let live_args = [
        "run",
        "workflow.yaml",
        "--run",
        "runs/live",
        "--live",
        "--max-usd",
        "1",
        "--no-view",
    ];
    let live = command(root.path(), "openrouter", &server, true, &live_args);
    assert_eq!(
        live.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&live.stdout),
        String::from_utf8_lossy(&live.stderr)
    );
    let inspected = document(
        &command(
            root.path(),
            "openrouter",
            &server,
            false,
            &["inspect", "runs/live", "--json", "--verify"],
        ),
        0,
    );
    assert_eq!(inspected["run"]["state"], "succeeded");
    assert_eq!(inspected["run"]["charged_usd"], 0.02);
    assert_eq!(inspected["verification"]["problems"], json!([]));
    let snapshot: Value = serde_json::from_slice(
        &std::fs::read(root.path().join("runs/live/resolved-models.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["routes"][0]["route"], "acme/img-v2@openrouter");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with(&format!("GET {OPENROUTER_PATH} ")));
    assert!(requests[1].starts_with("POST /v1/images "));
    let body: Value = serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["prompt"], "a small lantern");
    let events = |folder: &str| -> Vec<Value> {
        std::fs::read_to_string(root.path().join(folder).join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let live_events = events("runs/live");
    assert!(
        live_events
            .iter()
            .any(|event| event["event"] == "budget_reserved" && event["amount_usd"] == 0.4)
    );
    assert!(live_events.iter().any(|event| event["event"] == "call"
        && event["cached"] == false
        && event["cost_usd"] == 0.02));
    // Force the next body through the paid-call cache, rather than the step-result cache.
    std::fs::remove_dir_all(root.path().join(".fx/cache/results")).unwrap();
    let replay = command_with_key(
        root.path(),
        "openrouter",
        &server,
        false,
        &[
            "run",
            "workflow.yaml",
            "--run",
            "runs/replay",
            "--max-usd",
            "1",
            "--no-view",
        ],
        None,
    );
    assert_eq!(
        replay.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay_events = events("runs/replay");
    assert!(replay_events.iter().any(|event| event["event"] == "call"
        && event["cached"] == true
        && event["cost_usd"] == 0));
    assert!(
        replay_events
            .iter()
            .any(|event| event["event"] == "run_finished" && event["charged_usd"] == 0)
    );
    // An existing run carries facts independently of the optional metadata cache.
    std::fs::remove_dir_all(root.path().join(".fx/models")).unwrap();
    let resumed = command_with_key(
        root.path(),
        "openrouter",
        &server,
        false,
        &[
            "run",
            "workflow.yaml",
            "--run",
            "runs/live",
            "--max-usd",
            "1",
            "--no-view",
        ],
        None,
    );
    assert_eq!(
        resumed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(
        server.requests().len(),
        2,
        "replay and resume send no lookup or generation requests"
    );
    assert_eq!(
        std::fs::read(root.path().join("runs/replay/outputs/image.png")).unwrap(),
        png
    );
}
