//! Project service lifecycle. Local descriptors are private coordination data, never run records.
//! The advisory lock owns a process lifetime; HTTP authentication owns stop authority. No PID
//! found in a file is ever signalled, and no service command executes workflow code.

use crate::cli::{InitArgs, LogsArgs, ServiceArgs, StartArgs};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_viewer::service::Catalog;
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 8787;
const WAIT: Duration = Duration::from_secs(10);
const DESCRIPTOR: &str = "instance.json";
const LOCK: &str = "process.lock";
const LOG: &str = "service.log";

pub fn init(args: &InitArgs) -> Result<u8, Error> {
    let root = Path::new(args.directory.as_deref().unwrap_or("."));
    std::fs::create_dir_all(root).map_err(|error| Error::io("project directory", &error))?;
    let path = root.join("fx.yaml");
    let created = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(
                b"fx: project/v1\nruns: runs\ncache: .fx/cache\nworkflows: [workflows]\n",
            )
            .map_err(|error| Error::io("fx.yaml", &error))?;
            true
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(Error::io("fx.yaml", &error)),
    };
    if !path.is_file() {
        return Err(Error::usage(
            "fx.yaml exists but is not a project file; nothing was overwritten",
        ));
    }
    // Validate an existing configuration without replacing or normalizing it.
    let project = Project::find(root)?;
    let value = json!({
        "kind": "fx-project-init-v1", "created": created,
        "project_root": project.root, "files_created": if created { vec!["fx.yaml"] } else { vec![] }
    });
    if args.json {
        println!("{value}");
    } else {
        println!(
            "{} fx.yaml",
            if created { "Created" } else { "Using existing" }
        );
        println!("Start the local service with grida-fx start --background.");
    }
    Ok(0)
}

fn project(where_: Option<&str>) -> Result<Project, Error> {
    let path = Path::new(where_.unwrap_or("."));
    if !path.is_dir() {
        return Err(Error::usage("--project must name an existing directory"));
    }
    Project::find(path)
}

fn catalog(project: &Project) -> Result<Catalog, Error> {
    Catalog::open(&project.root, &project.runs_dir())
        .map_err(|error| Error::io("project service catalog", &error))
}

fn owned_file(path: &Path, append: bool) -> io::Result<File> {
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && (!meta.is_file() || meta.file_type().is_symlink())
    {
        return Err(io::Error::other(
            "service state must be a regular local file",
        ));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn random_id() -> io::Result<String> {
    #[cfg(unix)]
    {
        let mut bytes = [0_u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    #[cfg(not(unix))]
    {
        Err(io::Error::other(
            "native project services currently support macOS and Linux",
        ))
    }
}

fn save(path: &Path, value: &Value) -> io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && (!meta.is_file() || meta.file_type().is_symlink())
    {
        return Err(io::Error::other(
            "service state must be a regular local file",
        ));
    }
    let temporary = path.with_extension(format!("{}.tmp", random_id()?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(value.to_string().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn read_json(path: &Path) -> io::Result<Value> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 64 * 1024 {
        return Err(io::Error::other("invalid service state file"));
    }
    serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)
}

fn status_value(catalog: &Catalog, state: &str) -> Value {
    json!({"kind": "fx-service-status-v1", "state": state, "protocol_version": 1,
        "project_id": catalog.project_id(), "instance_id": null, "pid": null, "url": null})
}

fn process_locked(catalog: &Catalog) -> io::Result<bool> {
    let file = owned_file(&catalog.state_dir().join(LOCK), false)?;
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// A bounded loopback-only HTTP exchange. An untrusted descriptor cannot select a remote host,
/// inject headers, cause redirects or cause an unbounded read. The service returns JSON with
/// Content-Length; HTTP trailers, compressed bodies and streaming/chunked replies are refused.
fn request(descriptor: &Value, method: &str, route: &str) -> io::Result<Value> {
    let port = descriptor["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or_else(|| io::Error::other("invalid service port"))?;
    let token = descriptor["token"]
        .as_str()
        .filter(|token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| io::Error::other("invalid service credential"))?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(
        stream,
        "{method} {route} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut chunk = [0_u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "service response timed out",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::other("service response exceeds limit"));
        }
    }
    let response = String::from_utf8(bytes).map_err(io::Error::other)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| io::Error::other("invalid service response"))?;
    if !head.starts_with("HTTP/1.1 200 ")
        || head.to_ascii_lowercase().contains("transfer-encoding:")
    {
        return Err(io::Error::other(
            "service did not accept the authenticated request",
        ));
    }
    serde_json::from_str(body).map_err(io::Error::other)
}

fn probe(catalog: &Catalog) -> Value {
    match process_locked(catalog) {
        Ok(false) => return status_value(catalog, "stopped"),
        Err(_) => return status_value(catalog, "unavailable"),
        Ok(true) => {}
    }
    let Ok(descriptor) = read_json(&catalog.state_dir().join(DESCRIPTOR)) else {
        return status_value(catalog, "unavailable");
    };
    let Ok(value) = request(&descriptor, "GET", "/api/service") else {
        return status_value(catalog, "unavailable");
    };
    let expected_url = format!("http://127.0.0.1:{}/", descriptor["port"]);
    if value["kind"] != "fx-service-status-v1"
        || value["protocol_version"] != 1
        || value["project_id"] != catalog.project_id()
        || value["instance_id"] != descriptor["instance_id"]
        || value["pid"] != descriptor["pid"]
        || value["url"] != expected_url
        || value["state"] != "running"
    {
        return status_value(catalog, "incompatible");
    }
    value
}

fn print_status(value: &Value, json: bool) -> Result<(), Error> {
    if json {
        println!("{value}");
    } else if value["state"] == "running" {
        println!("{}", value["url"].as_str().unwrap_or_default());
    } else {
        println!(
            "FX service: {}",
            value["state"].as_str().unwrap_or("unavailable")
        );
    }
    io::stdout()
        .flush()
        .map_err(|error| Error::io("service output", &error))
}

pub fn status(args: &ServiceArgs) -> Result<u8, Error> {
    let catalog = catalog(&project(args.project.as_deref())?)?;
    let value = probe(&catalog);
    print_status(&value, args.json)?;
    Ok(if value["state"] == "running" { 0 } else { 1 })
}

fn chosen_port(catalog: &Catalog, explicit: Option<u16>) -> Result<u16, Error> {
    if let Some(port) = explicit {
        if port == 0 {
            return Err(Error::usage(
                "start needs a fixed port from 1 to 65535; use --standalone for an available port",
            ));
        }
        return Ok(port);
    }
    match read_json(&catalog.state_dir().join("settings.json")) {
        Ok(value) => value["port"]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .ok_or_else(|| Error::usage("invalid remembered service port; supply --port")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DEFAULT_PORT),
        Err(error) => Err(Error::io("service settings", &error)),
    }
}

pub fn start(args: &StartArgs) -> Result<u8, Error> {
    let project = project(args.project.as_deref())?;
    let catalog = catalog(&project)?;
    let port = chosen_port(&catalog, args.port)?;
    let deadline = Instant::now() + WAIT;
    let mut existing = probe(&catalog);
    // A parallel caller may be between lock acquisition and readiness publication.
    // Probes themselves briefly hold this lock: re-read the entire status, rather than
    // combining an unavailable snapshot with a later unlocked observation.
    while existing["state"] == "unavailable" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        existing = probe(&catalog);
    }
    if existing["state"] == "running" {
        if existing["url"] != format!("http://127.0.0.1:{port}/") {
            return Err(Error::usage(
                "FX is already running on a different port; stop it before changing ports",
            ));
        }
        print_status(&existing, args.json)?;
        if args.open {
            super::view::launch_browser(existing["url"].as_str().unwrap_or_default().into());
        }
        return Ok(0);
    }
    if existing["state"] != "stopped" {
        return Err(Error::usage(
            "the project service is unavailable or incompatible; inspect grida-fx status and grida-fx logs before retrying",
        ));
    }
    if args.background {
        background(&project, &catalog, port, args)
    } else {
        foreground(catalog, port, args)
    }
}

fn foreground(catalog: Catalog, port: u16, args: &StartArgs) -> Result<u8, Error> {
    let lock = owned_file(&catalog.state_dir().join(LOCK), false)
        .map_err(|error| Error::io("service lock", &error))?;
    let deadline = Instant::now() + WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(Error::io("service lock", &error));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                // A status probe can briefly own the lock before a service exists. Retry
                // ownership rather than making one transient collision a startup failure.
                let existing = probe(&catalog);
                if existing["state"] == "running" {
                    if existing["url"] != format!("http://127.0.0.1:{port}/") {
                        return Err(Error::usage(
                            "FX is already running on a different port; stop it before changing ports",
                        ));
                    }
                    print_status(&existing, args.json)?;
                    if args.open {
                        super::view::launch_browser(
                            existing["url"].as_str().unwrap_or_default().into(),
                        );
                    }
                    return Ok(0);
                }
                if existing["state"] == "incompatible" || Instant::now() >= deadline {
                    return Err(Error::usage(
                        "another FX service owns this project but is not compatible or ready; inspect status and logs",
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let instance = random_id().map_err(|error| Error::io("service identity", &error))?;
    let token = random_id().map_err(|error| Error::io("service credential", &error))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::io("service runtime", &error))?;
    let result = runtime.block_on(async {
        let (server, stopped) = grida_fx_viewer::service::bind(catalog.clone(), port, instance.clone(), token.clone()).await
            .map_err(|error| Error::io("FX service port (choose --port if it is occupied)", &error))?;
        let descriptor = json!({"port": port, "project_id": catalog.project_id(), "instance_id": instance, "pid": std::process::id(), "token": token});
        save(&catalog.state_dir().join(DESCRIPTOR), &descriptor).map_err(|error| Error::io("service descriptor", &error))?;
        save(&catalog.state_dir().join("settings.json"), &json!({"port": port})).map_err(|error| Error::io("service settings", &error))?;
        let (shutdown_started, shutdown_notice) = tokio::sync::oneshot::channel();
        let mut serving = tokio::spawn(server.serve_until(async move {
            let _ = stopped.await;
            let _ = shutdown_started.send(());
        }));
        let value = probe(&catalog);
        if value["state"] != "running" {
            serving.abort();
            return Err(Error::usage("FX service failed its readiness check"));
        }
        print_status(&value, args.json)?;
        if args.open {
            super::view::launch_browser(value["url"].as_str().unwrap_or_default().into());
        }
        let outcome = tokio::select! {
            outcome = &mut serving => outcome,
            _ = shutdown_notice => {
                match tokio::time::timeout(Duration::from_secs(2), &mut serving).await {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        // A reader that stops consuming an artifact must not own service
                        // lifetime. Stop affects this server's I/O only, never run processes.
                        serving.abort();
                        let _ = serving.await;
                        Ok(Ok(()))
                    }
                }
            }
        }.map_err(|error| Error::usage(format!("FX service task: {error}")))?;
        let _ = std::fs::remove_file(catalog.state_dir().join(DESCRIPTOR));
        outcome.map_err(|error| Error::io("FX service", &error))?;
        Ok(0)
    });
    // Filesystem projection uses blocking workers. Bound their shutdown too; process exit
    // closes remaining local readers without touching the independently executing workflow.
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}

fn background(
    project: &Project,
    catalog: &Catalog,
    port: u16,
    args: &StartArgs,
) -> Result<u8, Error> {
    let executable = std::env::current_exe().map_err(|error| Error::io("FX executable", &error))?;
    background_with_executable(project, catalog, port, args, &executable)
}

fn background_with_executable(
    project: &Project,
    catalog: &Catalog,
    port: u16,
    args: &StartArgs,
    executable: &Path,
) -> Result<u8, Error> {
    let log = owned_file(&catalog.state_dir().join(LOG), true)
        .map_err(|error| Error::io("service log", &error))?;
    if log
        .metadata()
        .map_err(|error| Error::io("service log", &error))?
        .len()
        > 1024 * 1024
    {
        // Service logs contain lifecycle diagnostics only, never request bodies or run output.
        // Bound retained diagnostics across repeated starts without creating a log subsystem.
        log.set_len(0)
            .map_err(|error| Error::io("service log", &error))?;
    }
    let mut command = Command::new(executable);
    command
        .args(["start", "--project"])
        .arg(&project.root)
        .arg("--port")
        .arg(port.to_string())
        .current_dir(&project.root)
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .map_err(|error| Error::io("service log", &error))?,
        )
        .stderr(log);
    for name in [
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
        "OPENAI_BASE_URL",
        "OPENROUTER_BASE_URL",
        "FAL_BASE_URL",
        "ELEVENLABS_BASE_URL",
    ] {
        command.env_remove(name);
    }
    command.env("GRIDA_FX_DISABLE_DOTENV", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| Error::io("background FX service", &error))?;
    let deadline = Instant::now() + WAIT;
    loop {
        let value = probe(catalog);
        if value["state"] == "running" {
            if value["url"] != format!("http://127.0.0.1:{port}/") {
                // A parallel start may have won the lifetime lock using another requested port.
                // This child cannot be the differently bound winner. End only the launcher
                // owned by this invocation; never use the winning descriptor's PID.
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::usage(
                    "FX is already running on a different port; stop it before changing ports",
                ));
            }
            if value["pid"].as_u64() != Some(u64::from(child.id())) {
                // A different caller won. Our launcher may still be waiting to acquire the
                // lifetime lock; it must not survive this acknowledgment and restart FX after
                // the caller subsequently stops the winner. Reap only the child we spawned.
                if child
                    .try_wait()
                    .map_err(|error| Error::io("background FX launcher", &error))?
                    .is_none()
                {
                    child
                        .kill()
                        .map_err(|error| Error::io("background FX launcher", &error))?;
                }
                child
                    .wait()
                    .map_err(|error| Error::io("background FX launcher", &error))?;
                let current = probe(catalog);
                if current != value {
                    return Err(Error::usage(
                        "FX service changed before startup completed; retry status or start",
                    ));
                }
            }
            print_status(&value, args.json)?;
            if args.open {
                super::view::launch_browser(value["url"].as_str().unwrap_or_default().into());
            }
            return Ok(0);
        }
        if child
            .try_wait()
            .map_err(|error| Error::io("background FX service", &error))?
            .is_some()
            && !process_locked(catalog).unwrap_or(false)
        {
            return Err(Error::usage(
                "FX service could not start; run grida-fx logs to inspect the cause",
            ));
        }
        if Instant::now() >= deadline {
            // This handle belongs to the child this invocation spawned, never a descriptor PID.
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::usage(
                "FX service did not become ready within 10 seconds; inspect grida-fx logs",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn stop(args: &ServiceArgs) -> Result<u8, Error> {
    let catalog = catalog(&project(args.project.as_deref())?)?;
    let current = probe(&catalog);
    if current["state"] == "stopped" {
        print_status(&current, args.json)?;
        return Ok(0);
    }
    if current["state"] != "running" {
        print_status(&current, args.json)?;
        return Ok(1);
    }
    let descriptor = read_json(&catalog.state_dir().join(DESCRIPTOR))
        .map_err(|error| Error::io("service descriptor", &error))?;
    if descriptor["instance_id"] != current["instance_id"]
        || descriptor["project_id"] != current["project_id"]
    {
        return Err(Error::usage(
            "FX service changed while stopping; retry status",
        ));
    }
    request(&descriptor, "POST", "/api/service/stop")
        .map_err(|error| Error::io("service stop", &error))?;
    let deadline = Instant::now() + WAIT;
    while process_locked(&catalog).map_err(|error| Error::io("service lock", &error))? {
        if Instant::now() >= deadline {
            return Err(Error::usage(
                "FX accepted stop but has not finished shutting down; retry status",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    print_status(&status_value(&catalog, "stopped"), args.json)?;
    Ok(0)
}

pub fn logs(args: &LogsArgs) -> Result<u8, Error> {
    if !(1..=10000).contains(&args.lines) {
        return Err(Error::usage("--lines must be from 1 to 10000"));
    }
    let catalog = catalog(&project(args.project.as_deref())?)?;
    let path = catalog.state_dir().join(LOG);
    if !path.exists() {
        return Ok(0);
    }
    let meta =
        std::fs::symlink_metadata(&path).map_err(|error| Error::io("service log", &error))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(Error::usage("service log must be a regular local file"));
    }
    let mut file = File::open(path).map_err(|error| Error::io("service log", &error))?;
    let offset = meta.len().saturating_sub(1024 * 1024);
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| Error::io("service log", &error))?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::io("service log", &error))?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<_> = text.lines().skip(usize::from(offset > 0)).collect();
    for line in &lines[lines.len().saturating_sub(args.lines)..] {
        println!("{line}");
    }
    Ok(0)
}

/// Register before run-phase execution. Service absence never prevents recording the run.
pub(crate) fn register_run(
    project: &Project,
    folder: &Path,
    open: bool,
) -> Result<Option<String>, Error> {
    let catalog = catalog(project)?;
    let entry = catalog
        .register_run(folder)
        .map_err(|error| Error::io("service run registration", &error))?;
    registered_url(&catalog, &entry.url, open)
}

pub(crate) fn register_plan(
    project: &Project,
    graph: &Value,
    open: bool,
) -> Result<Option<String>, Error> {
    let catalog = catalog(project)?;
    let entry = catalog
        .register_plan(graph)
        .map_err(|error| Error::io("service plan registration", &error))?;
    registered_url(&catalog, &entry.url, open)
}

fn registered_url(catalog: &Catalog, path: &str, _open: bool) -> Result<Option<String>, Error> {
    let value = probe(catalog);
    let Some(base) = value["url"]
        .as_str()
        .filter(|_| value["state"] == "running")
    else {
        return Ok(None);
    };
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    Ok(Some(url))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The competing winner becomes ready while this caller's own child has not entered FX
    /// yet. Returning another winner's URL must also retire that outstanding child, otherwise
    /// it can acquire the lock and restart the service after a subsequent stop.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_reaps_its_delayed_launcher_before_acknowledging_another_winner() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fx.yaml"), "fx: project/v1\n").unwrap();
        let project = Project::find(root.path()).unwrap();
        let catalog = catalog(&project).unwrap();
        let lock = owned_file(&catalog.state_dir().join(LOCK), false).unwrap();
        lock.try_lock().unwrap();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let launcher = root.path().join("gated-launcher");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$3/launcher.pid\"\ni=0\nwhile [ ! -f \"$3/release-launcher\" ]; do\n i=$((i + 1))\n [ \"$i\" -lt 1000 ] || exit 2\n /bin/sleep 0.01\ndone\n: > \"$3/late-start\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let selected = catalog.clone();
        let parent = std::thread::spawn(move || {
            background_with_executable(
                &project,
                &selected,
                port,
                &StartArgs {
                    project: None,
                    port: Some(port),
                    background: true,
                    open: false,
                    json: true,
                },
                &launcher,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let child_pid = loop {
            if let Ok(value) = std::fs::read_to_string(root.path().join("launcher.pid"))
                && let Ok(pid) = value.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "the owned launcher did not start"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let instance = random_id().unwrap();
        let token = random_id().unwrap();
        let (server, stop) =
            grida_fx_viewer::service::bind(catalog.clone(), port, instance.clone(), token.clone())
                .await
                .unwrap();
        let descriptor = json!({"port":port,"project_id":catalog.project_id(),"instance_id":instance,"pid":std::process::id(),"token":token});
        save(&catalog.state_dir().join(DESCRIPTOR), &descriptor).unwrap();
        let serving = tokio::spawn(server.serve_until(async move {
            let _ = stop.await;
        }));
        assert_eq!(parent.join().unwrap().unwrap(), 0);
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &child_pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success(),
            "startup returned while its redundant launcher was still alive",
        );
        std::fs::write(root.path().join("release-launcher"), "").unwrap();
        assert!(!root.path().join("late-start").exists());
        request(&descriptor, "POST", "/api/service/stop").unwrap();
        serving.await.unwrap().unwrap();
    }
}
