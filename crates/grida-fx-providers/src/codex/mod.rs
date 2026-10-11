//! Codex: `image.generate` through the user's own Codex, signed in with ChatGPT
//! (spec/providers.md §9.6).
//!
//! No key and no HTTP: the adapter starts `codex app-server` and speaks its JSON-RPC
//! ([`session`]). Codex owns sign-in, the model and the usage; FX never reads its credential
//! files. One send, in order:
//! 1. the route: capability `image.generate` on provider `codex`, and a contract whose `adapter`
//!    (when present) is [`ADAPTER`] and whose `adapter_behavior` (when present) is
//!    [`ADAPTER_BEHAVIOR`];
//! 2. [`crate::capabilities::check_request`];
//! 3. values: a blank prompt, a `size` other than `auto` (Codex picks its own sizes: asked for
//!    1024x1024 it drew 1254x1254), an unknown `background`, references;
//! 4. the program the setup found ([`find_program`], which only [`crate::live::live_setup`]
//!    calls); none is `Sent::Refused` with [`NOT_INSTALLED`] (or [`NETWORK_OFF`]);
//! 5. the user's MCP servers, listed so each can be turned off ([`mcp_servers`]);
//! 6. one conversation, in an empty scratch folder, with Codex in a process group of its own
//!    ([`process`]): `initialize`, `account/read` (only a ChatGPT sign-in is admitted),
//!    `thread/start` (read-only sandbox, no approvals, not persisted), `turn/start` with
//!    [`instructions`], then this turn's notifications until its first good `imageGeneration`
//!    item.
//!
//! Outcomes: a refusal before the turn (no program, no ChatGPT sign-in, another model provider)
//! is `Sent::Refused`. Every other failure is `Failed { cost: None, retryable: false }`: before
//! the turn it is most likely to happen again; once `turn/start` is written an image may already
//! have been made. Nothing is ever resent. Codex reports no cost, so an answer's cost is `None`:
//! the route's hold, $0, is what is charged.

mod process;
mod session;

pub use process::{end_every_codex, kill_every_codex};

use crate::BoxFuture;
use crate::adapter::{Adapter, Answer, CallRequest, RequestAdapter, RouteRef, Sent};
use crate::capabilities;
use crate::registry::Adapters;
use crate::setup::Setup;
use crate::wire;
use serde_json::Value;
use session::Session;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;

/// The label of every reason.
pub const LABEL: &str = "Codex image generation";

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "fx-codex-image-v1";

/// The route contract's `adapter_behavior` this adapter serves. It names [`instructions`] and
/// the settings of [`session`]: a change to either is a new behaviour.
pub const ADAPTER_BEHAVIOR: &str = "1";

/// The whole-conversation deadline of one send.
pub const DEADLINE: Duration = Duration::from_secs(600);

/// The deadline for Codex to accept the turn, within [`DEADLINE`].
pub const START_DEADLINE: Duration = Duration::from_secs(60);

/// The variable that names the program, as for any tool (spec/protocol.md §5.6).
pub const PROGRAM_VARIABLE: &str = "GRIDA_FX_TOOL_CODEX";

/// Where the ChatGPT app for macOS keeps its own copy of the command.
pub const MACOS_APP_PROGRAM: &str =
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex";

/// The refusal when no program was found.
pub const NOT_INSTALLED: &str = "Codex is not installed: install it with npm install -g \
                                 @openai/codex and sign in with codex login, or set \
                                 GRIDA_FX_TOOL_CODEX";

/// The refusal of an exact size.
pub const NO_EXACT_SIZE: &str = "Codex image generation takes no exact size: use size auto, and \
                                 fx/image.resize@1 after it";

/// The refusal when `GRIDA_FX_NETWORK` is off: Codex generates over the network.
pub const NETWORK_OFF: &str = "Codex was not started: GRIDA_FX_NETWORK is off";

/// Variables Codex never inherits besides the allowlisted ones ([`crate::keys::ALLOWED`]): with
/// any of them it could bill, or send its requests, somewhere other than the user's plan.
const ALSO_WITHHELD: [&str; 5] = [
    "CODEX_API_KEY",
    "OPENAI_ORG_ID",
    "OPENAI_ORGANIZATION",
    "OPENAI_PROJECT",
    "OPENAI_PROJECT_ID",
];

/// Settings for every Codex this adapter starts: nothing of the user's runs on a turn's end,
/// remembers it, hooks into it or offers it apps, plugins or (with [`mcp_servers`]) MCP tools.
const SETTINGS: [&str; 5] = [
    "notify=[]",
    "features.memories=false",
    "features.hooks=false",
    "features.apps=false",
    "features.plugins=false",
];

/// The deadline of listing the user's MCP servers.
const LIST_DEADLINE: Duration = Duration::from_secs(30);

/// Why a conversation ended without an image.
#[derive(Debug)]
enum Stop {
    /// Codex cannot draw on the user's plan: `Sent::Refused`.
    Refused(String),
    Failed(String),
}

impl Stop {
    #[cfg(test)]
    fn reason(self) -> String {
        match self {
            Stop::Refused(reason) | Stop::Failed(reason) => reason,
        }
    }
}

/// The adapter (module doc).
#[derive(Debug, Clone)]
pub struct CodexImages {
    /// The program, or the refusal of every call.
    program: Result<PathBuf, String>,
    deadline: Duration,
    start_deadline: Duration,
}

impl CodexImages {
    /// The adapter over the setup's program.
    pub fn new(setup: &Setup) -> CodexImages {
        CodexImages::with_program(setup.codex.clone())
    }

    /// The adapter over `program` (tests: a fake Codex).
    pub fn with_program(program: Result<PathBuf, String>) -> CodexImages {
        CodexImages {
            program,
            deadline: DEADLINE,
            start_deadline: START_DEADLINE,
        }
    }

    /// The same adapter with other deadlines (tests).
    pub fn with_deadlines(mut self, start: Duration, whole: Duration) -> CodexImages {
        self.start_deadline = start;
        self.deadline = whole;
        self
    }

    /// Steps 1 to 4 of the module doc: the program and the turn's text, or the refusal.
    fn prepare(&self, call: &CallRequest) -> Result<(PathBuf, String), String> {
        served(&call.route)?;
        capabilities::check_request(&call.route.capability, &call.request)?;
        let values = Values::read(&call.request)?;
        let program = self.program.clone()?;
        Ok((program, instructions(&values)))
    }
}

impl RequestAdapter for CodexImages {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let (program, text) = match self.prepare(call) {
                Ok(prepared) => prepared,
                Err(reason) => return Sent::Refused { reason },
            };
            exchange(&program, &text, self.start_deadline, self.deadline).await
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer has no image".into());
        };
        crate::checks::image(call, image, None)
    }
}

/// Registers the adapter for `image.generate` on `codex`, over the setup's program. Codex brings
/// its own sign-in and network: no key, no transport.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    adapters.register(
        "image.generate",
        "codex",
        Adapter::Request(Arc::new(CodexImages::new(setup))),
    );
}

/// The program (step 4), as an absolute path to an executable file:
/// - `GRIDA_FX_TOOL_CODEX` when set and not blank (relative to the working directory), and
///   nothing else when it names no executable file;
/// - else the first `codex` in an absolute `PATH` entry (empty and relative entries are skipped,
///   so a project's own folder never supplies it);
/// - else `app` when it is executable.
pub fn find_program(env: &dyn Fn(&str) -> Option<String>, app: &Path) -> Option<PathBuf> {
    if let Some(named) = env(PROGRAM_VARIABLE).filter(|v| !v.trim().is_empty()) {
        let named = PathBuf::from(named);
        let named = if named.is_absolute() {
            named
        } else {
            std::env::current_dir().ok()?.join(named)
        };
        return executable(&named).then_some(named);
    }
    let on_path = env("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("codex"))
            .find(|candidate| executable(candidate))
    });
    on_path.or_else(|| executable(app).then(|| app.to_path_buf()))
}

/// A regular file this user may run (on Unix, any execute bit).
fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// Every variable Codex never inherits.
pub fn withheld() -> impl Iterator<Item = &'static str> {
    crate::keys::ALLOWED
        .iter()
        .copied()
        .chain(ALSO_WITHHELD.iter().copied())
}

/// The turn's text: fixed instructions, a sentence for a transparent or opaque background, then
/// the authored prompt verbatim (behaviour [`ADAPTER_BEHAVIOR`]).
pub fn instructions(values: &Values<'_>) -> String {
    let mut text = String::from(
        "Use $imagegen to generate exactly one image from the prompt between the <prompt> tags. \
         Give it the prompt as written.",
    );
    match values.background {
        "transparent" => text.push_str(" Give it a transparent background (an alpha channel)."),
        "opaque" => text.push_str(" Give it an opaque background, with no transparent pixels."),
        _ => {}
    }
    text.push_str(
        " Do not run commands, read or edit files, use any other tool or ask questions. When the \
         image is made, reply with one short sentence.",
    );
    format!("{text}\n\n<prompt>\n{}\n</prompt>", values.prompt)
}

/// Step 1: whether the route is one this adapter serves.
fn served(route: &RouteRef) -> Result<(), String> {
    let refused = || {
        format!(
            "{} is not a {} route this adapter serves",
            route.id(),
            route.capability
        )
    };
    if route.capability != "image.generate" || route.provider != "codex" {
        return Err(refused());
    }
    let contract = match &route.contract {
        Value::Null => return Ok(()),
        Value::Object(contract) => contract,
        _ => return Err(refused()),
    };
    let text_or_absent = |key: &str, want: &str| match contract.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::String(have)) => have == want,
        Some(_) => false,
    };
    if text_or_absent("adapter", ADAPTER) && text_or_absent("adapter_behavior", ADAPTER_BEHAVIOR) {
        Ok(())
    } else {
        Err(refused())
    }
}

/// A request's values after step 3.
#[derive(Debug, PartialEq)]
pub struct Values<'a> {
    /// Verbatim: the prompt is never trimmed or rewritten.
    pub prompt: &'a str,
    /// `auto`, `opaque` or `transparent`; `auto` when absent.
    pub background: &'a str,
}

impl<'a> Values<'a> {
    /// Reads a request that passed its capability's check (step 3).
    pub fn read(request: &'a Value) -> Result<Values<'a>, String> {
        let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
        if prompt.trim().is_empty() {
            return Err("an image call needs its prompt as text".into());
        }
        match request.get("size") {
            None | Some(Value::Null) => {}
            Some(Value::String(size)) if size == "auto" => {}
            Some(_) => return Err(NO_EXACT_SIZE.into()),
        }
        let background = match request.get("background") {
            None | Some(Value::Null) => "auto",
            Some(Value::String(b)) if matches!(b.as_str(), "auto" | "opaque" | "transparent") => {
                b.as_str()
            }
            Some(_) => return Err("background must be auto, opaque or transparent".into()),
        };
        let references = request.get("references").and_then(Value::as_array);
        if references.is_some_and(|list| !list.is_empty()) {
            return Err("Codex image generation takes no references".into());
        }
        Ok(Values { prompt, background })
    }
}

/// A command that starts `program` with `args` in `folder`: Codex's own environment, less
/// [`withheld`]; no stderr; in a process group of its own; killed when dropped.
fn command(program: &Path, args: &[String], folder: &Path) -> Command {
    let mut command = Command::new(program);
    command.args(args).current_dir(folder);
    for variable in withheld() {
        command.env_remove(variable);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// `-c` settings, `features.apps` and `features.plugins` off first.
fn settings(extra: impl IntoIterator<Item = String>) -> Vec<String> {
    SETTINGS
        .iter()
        .map(|setting| setting.to_string())
        .chain(extra)
        .flat_map(|setting| ["-c".to_string(), setting])
        .collect()
}

/// Step 5: a setting that turns off each MCP server the user's Codex would start, as `codex mcp
/// list --json` names them with apps and plugins off (a server only a plugin defines cannot be
/// named: Codex refuses its setting). A list Codex will not give is a refusal: those servers' tools
/// would otherwise be offered to the turn.
async fn mcp_servers(program: &Path, folder: &Path) -> Result<Vec<String>, String> {
    let refused =
        || "Codex's MCP servers could not be listed, so they cannot be turned off".to_string();
    let mut args = settings([]);
    args.extend(["mcp", "list", "--json"].map(String::from));
    let mut command = command(program, &args, folder);
    command.stdin(Stdio::null());
    let output = match tokio::time::timeout(LIST_DEADLINE, command.output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return Err(refused()),
    };
    let listed: Value = serde_json::from_slice(&output.stdout).map_err(|_| refused())?;
    let servers = listed.as_array().ok_or_else(refused)?;
    servers
        .iter()
        .filter(|server| server.get("enabled").and_then(Value::as_bool) != Some(false))
        .map(|server| {
            let name = server
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(refused)?;
            Ok(format!("mcp_servers.{}.enabled=false", toml_key(name)))
        })
        .collect()
}

/// A TOML key: bare when it may be, else a basic string.
fn toml_key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        name.to_string()
    } else {
        Value::from(name).to_string()
    }
}

/// Steps 5 and 6: one Codex, one thread, one turn (module doc).
async fn exchange(
    program: &Path,
    text: &str,
    start_deadline: Duration,
    deadline: Duration,
) -> Sent {
    let began = Instant::now();
    // Declared first, so it is removed last: after Codex and its group have ended.
    let folder = match tempfile::Builder::new().prefix("grida-fx-codex-").tempdir() {
        Ok(folder) => folder,
        Err(error) => {
            let reason = grida_fx_core::error::io_reason(&error);
            return failed(format!("{LABEL} was not started: {reason}"));
        }
    };
    let servers = match mcp_servers(program, folder.path()).await {
        Ok(servers) => servers,
        Err(reason) => return Sent::Refused { reason },
    };
    let mut args = vec!["app-server".to_string()];
    args.extend(settings(servers));
    let mut child = match command(program, &args, folder.path()).spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Sent::Refused {
                reason: NOT_INSTALLED.into(),
            };
        }
        Err(error) => {
            return Sent::Refused {
                reason: format!(
                    "{LABEL} cannot start codex: {}",
                    grida_fx_core::error::io_reason(&error)
                ),
            };
        }
    };
    let Some(group) = process::Group::enlist(&mut child) else {
        return failed(format!("{LABEL} was not started: the command is stopping"));
    };
    let Some(mut session) = Session::new(&mut child) else {
        return failed(format!("{LABEL} was not started: no pipes"));
    };
    let outcome = conversation(
        &mut session,
        folder.path(),
        text,
        start_deadline,
        deadline,
        began,
    )
    .await;
    session.close();
    drop(session);
    group.finish(&mut child).await;
    match outcome {
        Ok(bytes) => {
            let kind = wire::sniff_image(&bytes).unwrap_or("application/octet-stream");
            Sent::Answered(Answer::new(Value::Null, None).with_file("image", kind, bytes))
        }
        Err(Stop::Refused(reason)) => Sent::Refused { reason },
        Err(Stop::Failed(reason)) => failed(reason),
    }
}

/// The conversation under its deadlines, every stop as a reason that says whether the turn had
/// been asked for.
async fn conversation(
    session: &mut Session,
    folder: &Path,
    text: &str,
    start_deadline: Duration,
    deadline: Duration,
    began: Instant,
) -> Result<Vec<u8>, Stop> {
    let label = |session: &Session, reason: String| {
        if session.turn_requested {
            format!("{LABEL} failed: {reason}")
        } else {
            format!("{LABEL} was not started: {reason}")
        }
    };
    let started = tokio::time::timeout(start_deadline, session.start(folder, text)).await;
    let started = match started {
        Ok(started) => started,
        Err(_) => {
            session.interrupt().await;
            let reason = format!(
                "Codex did not start the turn within {} s",
                start_deadline.as_secs()
            );
            return Err(Stop::Failed(label(session, reason)));
        }
    };
    if let Err(stop) = started {
        return Err(match stop {
            Stop::Refused(reason) => Stop::Refused(reason),
            Stop::Failed(reason) => Stop::Failed(label(session, reason)),
        });
    }
    let left = deadline.saturating_sub(began.elapsed());
    match tokio::time::timeout(left, session.image()).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(stop)) => Err(Stop::Failed(label(session, stop_reason(stop)))),
        Err(_) => {
            session.interrupt().await;
            let reason = format!("Codex did not finish within {} s", deadline.as_secs());
            Err(Stop::Failed(label(session, reason)))
        }
    }
}

fn stop_reason(stop: Stop) -> String {
    match stop {
        Stop::Refused(reason) | Stop::Failed(reason) => reason,
    }
}

/// A failure: never retried, and charged the route's hold ($0) since Codex reports no cost.
fn failed(reason: String) -> Sent {
    Sent::Failed {
        reason,
        cost: None,
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn route(provider: &str, contract: Value) -> RouteRef {
        RouteRef {
            capability: "image.generate".into(),
            model: "imagegen".into(),
            provider: provider.into(),
            contract,
        }
    }

    #[test]
    fn routes_are_served_by_provider_and_contract() {
        assert_eq!(
            served(&route(
                "codex",
                json!({"adapter": ADAPTER, "adapter_behavior": "1"})
            )),
            Ok(())
        );
        assert_eq!(served(&route("codex", Value::Null)), Ok(()));
        for contract in [
            json!({"adapter": "fx-openai-image-v1"}),
            json!({"adapter_behavior": 1}),
            json!([]),
        ] {
            assert!(served(&route("codex", contract)).is_err());
        }
        assert_eq!(
            served(&route("openai", json!({}))),
            Err("imagegen@openai is not a image.generate route this adapter serves".into())
        );
    }

    #[test]
    fn values_refuse_what_codex_does_not_honour() {
        let read = |request: Value| {
            Values::read(&request).map(|v| (v.prompt.to_string(), v.background.to_string()))
        };
        assert_eq!(
            read(json!({"prompt": " a kite "})),
            Ok((" a kite ".into(), "auto".into()))
        );
        assert_eq!(
            read(json!({"prompt": "p", "size": "auto", "background": "transparent"})),
            Ok(("p".into(), "transparent".into()))
        );
        assert_eq!(
            read(json!({"prompt": "p", "size": null, "references": []})),
            Ok(("p".into(), "auto".into()))
        );
        for (request, reason) in [
            (
                json!({"prompt": " "}),
                "an image call needs its prompt as text",
            ),
            (json!({"prompt": "p", "size": "1024x1024"}), NO_EXACT_SIZE),
            (
                json!({"prompt": "p", "background": "clear"}),
                "background must be auto, opaque or transparent",
            ),
            (
                json!({"prompt": "p", "references": [{"file": "a"}]}),
                "Codex image generation takes no references",
            ),
        ] {
            assert_eq!(read(request.clone()), Err(reason.to_string()), "{request}");
        }
    }

    #[test]
    fn the_instructions_name_the_background_then_the_prompt_verbatim() {
        let plain = Values {
            prompt: "a red\n kite ",
            background: "auto",
        };
        assert_eq!(
            instructions(&plain),
            "Use $imagegen to generate exactly one image from the prompt between the <prompt> \
             tags. Give it the prompt as written. Do not run commands, read or edit files, use \
             any other tool or ask questions. When the image is made, reply with one short \
             sentence.\n\n<prompt>\na red\n kite \n</prompt>"
        );
        let icon = Values {
            prompt: "p",
            background: "transparent",
        };
        assert!(instructions(&icon).contains(
            "as written. Give it a transparent background (an alpha channel). Do not run"
        ));
        let opaque = Values {
            prompt: "p",
            background: "opaque",
        };
        assert!(
            instructions(&opaque)
                .contains(" Give it an opaque background, with no transparent pixels. ")
        );
    }

    #[test]
    fn the_program_is_the_variable_then_path_then_the_app() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = |name: &str, mode: u32| {
            let path = dir.path().join(name);
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let on_path = bin.join("codex");
        std::fs::write(&on_path, "").unwrap();
        std::fs::set_permissions(&on_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let app = program("app-codex", 0o755);
        let plain = program("plain-codex", 0o644);
        let env = |pairs: Vec<(&'static str, String)>| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.clone())
            }
        };
        let path = bin.to_string_lossy().to_string();
        let shown = |p: &Path| p.to_string_lossy().to_string();
        // A named program is the only one, and must be executable.
        let named = env(vec![
            (PROGRAM_VARIABLE, shown(&app)),
            ("PATH", path.clone()),
        ]);
        assert_eq!(
            find_program(&named, Path::new("/missing")),
            Some(app.clone())
        );
        let missing = env(vec![
            (PROGRAM_VARIABLE, "/nonexistent/codex".into()),
            ("PATH", path.clone()),
        ]);
        assert_eq!(find_program(&missing, &app), None);
        let not_runnable = env(vec![(PROGRAM_VARIABLE, shown(&plain))]);
        assert_eq!(find_program(&not_runnable, &app), None);
        // A relative name becomes absolute.
        let relative = env(vec![(PROGRAM_VARIABLE, "some/codex".into())]);
        let found = find_program(&relative, &app);
        assert!(found.is_none_or(|p| p.is_absolute()));
        // Then PATH, absolute entries only; then the app.
        let blank = env(vec![(PROGRAM_VARIABLE, " ".into()), ("PATH", path.clone())]);
        assert_eq!(find_program(&blank, &app), Some(on_path));
        let relative_path = env(vec![(
            "PATH",
            format!("bin{}", if cfg!(windows) { ";" } else { ":" }),
        )]);
        assert_eq!(find_program(&relative_path, &app), Some(app.clone()));
        assert_eq!(find_program(&env(vec![]), &plain), None);
    }

    #[test]
    fn codex_inherits_no_key_and_no_account_setting() {
        let withheld: Vec<&str> = withheld().collect();
        for variable in crate::keys::ALLOWED.iter().chain(&ALSO_WITHHELD) {
            assert!(withheld.contains(variable), "{variable}");
        }
        assert!(withheld.contains(&"OPENAI_API_KEY") && withheld.contains(&"OPENAI_BASE_URL"));
    }

    #[test]
    fn settings_turn_off_apps_and_plugins_before_named_servers() {
        assert_eq!(
            settings(["mcp_servers.acme.enabled=false".to_string()]),
            [
                "-c",
                "notify=[]",
                "-c",
                "features.memories=false",
                "-c",
                "features.hooks=false",
                "-c",
                "features.apps=false",
                "-c",
                "features.plugins=false",
                "-c",
                "mcp_servers.acme.enabled=false",
            ]
        );
        assert_eq!(toml_key("node_repl"), "node_repl");
        assert_eq!(toml_key("acme-docs"), "acme-docs");
        assert_eq!(toml_key("acme docs"), "\"acme docs\"");
        assert_eq!(toml_key("a\"b"), "\"a\\\"b\"");
    }
}
