//! Codex: `image.generate` and `image.edit` through the user's own Codex, signed in with ChatGPT
//! (spec/providers.md §9.6).
//!
//! No key and no HTTP: the adapter draws through a [`CodexRuntime`], one `codex app-server` an
//! invocation starts on its first Codex call, keeps for the others and shuts down with the run.
//! Codex owns sign-in, the model and the usage; FX never reads its credential files, and its calls
//! bill the user's plan ([`Billing::Plan`]), never dollars. One send, in order:
//! 1. the route: capability `image.generate` or `image.edit` on provider `codex`, and a contract
//!    whose `adapter` (when present) is [`ADAPTER`] and whose `adapter_behavior` (when present)
//!    is [`ADAPTER_BEHAVIOR`];
//! 2. [`crate::capabilities::check_request`];
//! 3. values: a blank prompt, a `size` other than `auto` (Codex picks its own sizes: asked for
//!    1024x1024 it drew 1254x1254), an unknown `background`, a mask, more than [`MAX_PICTURES`]
//!    pictures (`image`, then `references`), and each picture's file: carried, not empty, and
//!    PNG, JPEG or WebP;
//! 4. the runtime's Codex, started if need be ([`runtime`]): no program ([`NOT_INSTALLED`],
//!    [`NETWORK_OFF`]), MCP servers that cannot be listed, or a sign-in other than ChatGPT is
//!    `Sent::Refused`, for this call and the invocation's others;
//! 5. a scratch folder of the call's own holding copies of its pictures (`picture-01.png`, ...),
//!    and a thread in it (read-only sandbox, no approvals, not persisted; a model provider other
//!    than OpenAI is refused), then `turn/start` with [`instructions`], which name the pictures
//!    by path as the image tool's `referenced_image_paths`, then the thread's notifications until
//!    its first good `imageGeneration` item. The rest of the turn is then interrupted, and its
//!    token usage read.
//!
//! Outcomes: a request Codex rejects as overloaded was not received (`Sent::NotReceived`); every
//! other failure is `Failed { cost: None, retryable: false }`, and nothing is resent: before
//! `turn/start` is written a failure is most likely to happen again; after, an image may already
//! have been made. A call that is dropped (its run was cancelled) interrupts its turn, through the
//! runtime's reader when Codex had not yet named it.
//! Codex reports no cost: an answer's cost is `None`, and its [`Usage`] says what Codex counted.

mod process;
mod protocol;
mod runtime;

pub use process::end_every_codex;
pub use runtime::CodexRuntime;

use crate::BoxFuture;
use crate::adapter::{
    Adapter, Answer, Billing, CallRequest, RequestAdapter, RouteRef, Sent, Usage,
};
use crate::capabilities;
use crate::registry::Adapters;
use crate::setup::Setup;
use crate::wire;
use runtime::{Connection, Note};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::mpsc;

/// The label of every reason.
pub const LABEL: &str = "Codex image generation";

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "fx-codex-image-v1";

/// The route contract's `adapter_behavior` this adapter serves. It names [`instructions`] and
/// the settings of a Codex conversation: a change to either is a new behaviour.
pub const ADAPTER_BEHAVIOR: &str = "2";

/// The most input pictures of one call (`image` and `references`): Codex's image tool draws
/// nothing for more.
pub const MAX_PICTURES: usize = 5;

/// The deadline of one send, from asking for Codex to the image.
pub const DEADLINE: Duration = Duration::from_secs(600);

/// The deadline for Codex to accept the turn (starting Codex too, on an invocation's first
/// call), within [`DEADLINE`].
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

/// The kinds of picture Codex reads, and the extension its copy takes.
const PICTURE_KINDS: [(&str, &str); 3] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
];

/// Settings for every Codex this adapter starts: nothing of the user's runs on a turn's end,
/// remembers it, hooks into it or offers it apps, plugins or (with [`mcp_servers`]) MCP tools;
/// and none of Codex's own tools but its image tool is offered (Codex ignores a feature it does
/// not know).
const SETTINGS: [&str; 17] = [
    "notify=[]",
    "features.memories=false",
    "features.hooks=false",
    "features.apps=false",
    "features.plugins=false",
    "features.shell_tool=false",
    "features.unified_exec=false",
    "features.multi_agent=false",
    "features.browser_use=false",
    "features.computer_use=false",
    "features.in_app_browser=false",
    "features.view_image=false",
    "features.goals=false",
    "features.sleep_tool=false",
    "features.tool_suggest=false",
    "features.skill_search=false",
    "web_search=\"disabled\"",
];

/// The deadline of listing the user's MCP servers.
const LIST_DEADLINE: Duration = Duration::from_secs(30);

/// How long an answered turn has to end after it is interrupted, for its token usage.
const SETTLE: Duration = Duration::from_secs(3);

/// Why a call ended without an image.
#[derive(Debug)]
enum Stop {
    /// Codex cannot draw on the user's plan: `Sent::Refused`.
    Refused(String),
    /// Codex rejected a request on arrival: `Sent::NotReceived`.
    NotReceived(String),
    Failed(String),
}

impl Stop {
    fn into_reason(self) -> String {
        match self {
            Stop::Refused(reason) | Stop::NotReceived(reason) | Stop::Failed(reason) => reason,
        }
    }

    #[cfg(test)]
    fn reason(self) -> String {
        self.into_reason()
    }
}

/// The adapter (module doc).
#[derive(Clone)]
pub struct CodexImages {
    runtime: Arc<CodexRuntime>,
    deadline: Duration,
    start_deadline: Duration,
}

impl std::fmt::Debug for CodexImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexImages")
            .field("deadline", &self.deadline)
            .field("start_deadline", &self.start_deadline)
            .finish_non_exhaustive()
    }
}

impl CodexImages {
    /// The adapter over `runtime`.
    pub fn new(runtime: Arc<CodexRuntime>) -> CodexImages {
        CodexImages {
            runtime,
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

    /// Steps 1 to 3 of the module doc: what to draw, or the refusal.
    fn prepare<'a>(&self, call: &'a CallRequest) -> Result<Drawing<'a>, String> {
        served(&call.route)?;
        capabilities::check_request(&call.route.capability, &call.request)?;
        let values = Values::read(&call.request)?;
        let pictures = values
            .pictures
            .iter()
            .map(|(member, value)| picture(call, value, member))
            .collect::<Result<_, _>>()?;
        Ok(Drawing { values, pictures })
    }
}

impl RequestAdapter for CodexImages {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let drawing = match self.prepare(call) {
                Ok(drawing) => drawing,
                Err(reason) => return Sent::Refused { reason },
            };
            draw(&self.runtime, &drawing, self.start_deadline, self.deadline).await
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer has no image".into());
        };
        crate::checks::image(call, image, None)
    }

    fn billing(&self) -> Billing {
        Billing::Plan
    }
}

/// Registers the adapter for `image.generate` and `image.edit` on `codex`, over one runtime of
/// the setup's program, which the registry keeps to shut it down with the invocation. Codex brings
/// its own sign-in and network: no key, no transport.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let runtime = Arc::new(CodexRuntime::new(setup.codex.clone()));
    let images = Arc::new(CodexImages::new(Arc::clone(&runtime)));
    for capability in ["image.generate", "image.edit"] {
        adapters.register(capability, "codex", Adapter::Request(images.clone()));
    }
    adapters.keep_runtime(runtime);
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

/// The turn's text (behaviour [`ADAPTER_BEHAVIOR`]): the image tool's arguments, the pictures'
/// paths in order, fixed instructions, then the authored prompt verbatim. Codex draws an opaque
/// image unless `transparent_background` is true, so `auto` is opaque.
pub fn instructions(values: &Values<'_>, pictures: &[PathBuf]) -> String {
    let transparent = values.background == "transparent";
    let mut text = format!(
        "Call the image_gen.imagegen tool exactly once. Pass the text between the <prompt> tags \
         as its prompt, exactly as written. Set transparent_background to {transparent}."
    );
    if !pictures.is_empty() {
        let paths: Vec<String> = pictures
            .iter()
            .map(|path| format!("`{}`", path.display()))
            .collect();
        text.push_str(&format!(
            " Set referenced_image_paths to these input images, in this order: {}.",
            paths.join(", ")
        ));
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
    let capability = route.capability.as_str();
    if !matches!(capability, "image.generate" | "image.edit") || route.provider != "codex" {
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
    /// The input pictures in order, each with its member (`image`, `references[<i>]`); at most
    /// [`MAX_PICTURES`].
    pub pictures: Vec<(String, &'a Value)>,
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
        if request.get("mask").is_some_and(|mask| !mask.is_null()) {
            return Err("Codex image edits take no mask".into());
        }
        let mut pictures: Vec<(String, &Value)> = request
            .get("image")
            .filter(|image| !image.is_null())
            .map(|image| ("image".to_string(), image))
            .into_iter()
            .collect();
        let references = request.get("references").and_then(Value::as_array);
        pictures.extend(
            references
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, value)| (format!("references[{i}]"), value)),
        );
        if pictures.len() > MAX_PICTURES {
            return Err(format!("Codex takes at most {MAX_PICTURES} input pictures"));
        }
        Ok(Values {
            prompt,
            background,
            pictures,
        })
    }
}

/// What one call draws: its values and its pictures' files.
struct Drawing<'a> {
    values: Values<'a>,
    pictures: Vec<Picture<'a>>,
}

/// An input picture: its store copy, and the extension its copy for Codex takes.
struct Picture<'a> {
    source: &'a Path,
    extension: &'static str,
}

/// Step 3 for one picture: a file the call carries, not empty, of a kind Codex reads.
fn picture<'a>(call: &'a CallRequest, value: &Value, member: &str) -> Result<Picture<'a>, String> {
    let file = wire::request_file(call, value, member)?;
    let readable = std::fs::metadata(&file.path).is_ok_and(|m| m.is_file() && m.len() > 0);
    if !readable {
        return Err(format!("{member} has no bytes to send"));
    }
    let Some((_, extension)) = PICTURE_KINDS.iter().find(|(kind, _)| *kind == file.kind) else {
        return Err(format!(
            "{member} is {}; Codex takes PNG, JPEG or WebP",
            file.kind
        ));
    };
    Ok(Picture {
        source: &file.path,
        extension,
    })
}

/// Step 5: copies of the pictures in `folder`, in order, as `picture-<nn>.<extension>`.
fn place(pictures: &[Picture<'_>], folder: &Path) -> Result<Vec<PathBuf>, Stop> {
    pictures
        .iter()
        .enumerate()
        .map(|(i, picture)| {
            let copy = folder.join(format!("picture-{:02}.{}", i + 1, picture.extension));
            std::fs::copy(picture.source, &copy)
                .map(|_| copy)
                .map_err(|error| Stop::Failed(grida_fx_core::error::io_reason(&error)))
        })
        .collect()
}

/// A command that starts `program` with `args` in `folder`: Codex's own environment, less
/// [`withheld`]; no stderr; in a process group of its own; killed when dropped.
pub(super) fn command(program: &Path, args: &[String], folder: &Path) -> Command {
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

/// `-c` settings: [`SETTINGS`] first.
pub(super) fn settings(extra: impl IntoIterator<Item = String>) -> Vec<String> {
    SETTINGS
        .iter()
        .map(|setting| setting.to_string())
        .chain(extra)
        .flat_map(|setting| ["-c".to_string(), setting])
        .collect()
}

/// Step 4: a setting that turns off each MCP server the user's Codex would start, as `codex mcp
/// list --json` names them with apps and plugins off (a server only a plugin defines cannot be
/// named: Codex refuses its setting). A list Codex will not give is a refusal: those servers' tools
/// would otherwise be offered to the turn.
pub(super) async fn mcp_servers(program: &Path, folder: &Path) -> Result<Vec<String>, String> {
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

/// Steps 4 and 5 (module doc) under their deadlines.
async fn draw(
    runtime: &CodexRuntime,
    wanted: &Drawing<'_>,
    start_deadline: Duration,
    deadline: Duration,
) -> Sent {
    let began = Instant::now();
    if let Some(reason) = runtime.limited(unix_now()) {
        return Sent::Refused { reason };
    }
    let mut turn: Option<Turn> = None;
    let label = |turn: &Option<Turn>, reason: String| {
        if turn.as_ref().is_some_and(|turn| turn.requested) {
            format!("{LABEL} failed: {reason}")
        } else {
            format!("{LABEL} was not started: {reason}")
        }
    };
    match tokio::time::timeout(start_deadline, begin(runtime, wanted, &mut turn)).await {
        Ok(Ok(())) => {}
        Ok(Err(Stop::Refused(reason))) => return Sent::Refused { reason },
        Ok(Err(Stop::NotReceived(reason))) => {
            return Sent::NotReceived {
                reason: format!("{LABEL} was not started: {reason}"),
                retry_after: None,
            };
        }
        Ok(Err(Stop::Failed(reason))) => return failed(label(&turn, reason)),
        Err(_) => {
            let reason = format!(
                "Codex did not start the turn within {} s",
                start_deadline.as_secs()
            );
            return failed(label(&turn, reason));
        }
    }
    let Some(mut drawing) = turn else {
        return failed(format!("{LABEL} was not started: no turn"));
    };
    let left = deadline.saturating_sub(began.elapsed());
    match tokio::time::timeout(left, drawing.image()).await {
        Ok(Ok(bytes)) => {
            drawing.settle().await;
            let kind = wire::sniff_image(&bytes).unwrap_or("application/octet-stream");
            Sent::Answered(
                Answer::new(Value::Null, None)
                    .with_file("image", kind, bytes)
                    .with_usage(drawing.usage()),
            )
        }
        Ok(Err(stop)) => {
            if let Some(resets_at) = drawing.limit {
                runtime.reached(resets_at);
            }
            failed(label(&Some(drawing), stop.into_reason()))
        }
        Err(_) => {
            let reason = format!("Codex did not finish within {} s", deadline.as_secs());
            failed(label(&Some(drawing), reason))
        }
    }
}

/// The runtime's Codex, a thread, and the turn: on its way into `slot`, so a stop at any point
/// still knows whether `turn/start` was written.
async fn begin(
    runtime: &CodexRuntime,
    wanted: &Drawing<'_>,
    slot: &mut Option<Turn>,
) -> Result<(), Stop> {
    let connection = runtime.connection().await?;
    let folder = tempfile::Builder::new()
        .prefix("grida-fx-codex-")
        .tempdir()
        .map_err(|error| Stop::Failed(grida_fx_core::error::io_reason(&error)))?;
    let pictures = place(&wanted.pictures, folder.path())?;
    let text = instructions(&wanted.values, &pictures);
    let (started, notes) = connection
        .start_thread(json!({
            "cwd": folder.path().to_string_lossy(),
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "ephemeral": true,
        }))
        .await?;
    let thread = protocol::text_at(&started, &["thread", "id"]);
    let turn = slot.insert(Turn {
        connection: Arc::clone(&connection),
        thread: thread.clone(),
        turn: None,
        requested: false,
        finished: false,
        limit: None,
        notes,
        usage: Usage {
            runtime: connection.version.as_ref().map(|v| format!("codex {v}")),
            agent_model: started
                .get("model")
                .and_then(Value::as_str)
                .and_then(protocol::name),
            ..Usage::default()
        },
        _folder: folder,
    });
    match started.get("modelProvider").and_then(Value::as_str) {
        None | Some("openai") => {}
        Some(other) => {
            return Err(Stop::Refused(format!(
                "Codex uses the model provider {}, not OpenAI: image generation needs Codex \
                 signed in with ChatGPT",
                protocol::token(other)
            )));
        }
    }
    let Some(thread) = thread else {
        return Err(Stop::Failed("Codex started no thread".into()));
    };
    turn.requested = true;
    let accepted = connection
        .start_turn(
            &thread,
            json!({
                "threadId": thread,
                "input": [{"type": "text", "text": text}],
                "effort": protocol::EFFORT,
                "serviceTierForTurn": protocol::SERVICE_TIER,
            }),
        )
        .await?;
    turn.turn = protocol::text_at(&accepted, &["turn", "id"]);
    Ok(())
}

/// A call's thread and turn on the runtime's Codex. Dropped before the turn ended (the call was
/// cancelled, or ran out of time), it asks Codex to stop the turn; dropped at all, it stops
/// routing the thread's notifications.
struct Turn {
    connection: Arc<Connection>,
    thread: Option<String>,
    turn: Option<String>,
    /// Set just before `turn/start` is written: from then on a generation may have started.
    requested: bool,
    /// The turn ended (`turn/completed`).
    finished: bool,
    /// The turn said the usage limit is reached ([`protocol::usage_limit`]).
    limit: Option<Option<i64>>,
    notes: mpsc::UnboundedReceiver<Note>,
    usage: Usage,
    /// The thread's working folder, removed after it.
    _folder: tempfile::TempDir,
}

impl Turn {
    /// The first good image of this turn; a turn that completes without one is a failure.
    async fn image(&mut self) -> Result<Vec<u8>, Stop> {
        let mut failure: Option<String> = None;
        loop {
            let Some(note) = self.notes.recv().await else {
                return Err(Stop::Failed(self.connection.ended()));
            };
            let params = &note.params;
            match note.method.as_str() {
                "item/completed" if self.ours(params) => {
                    let item = &params["item"];
                    if item["type"] != "imageGeneration" {
                        continue;
                    }
                    match protocol::image_of(item) {
                        Ok(bytes) => return Ok(bytes),
                        Err(reason) => {
                            self.limit = self.limit.or(protocol::usage_limit(Some(item), None));
                            failure.get_or_insert(reason);
                        }
                    }
                }
                "thread/tokenUsage/updated" => protocol::add_tokens(&mut self.usage, params),
                "turn/completed" if self.our_turn(&params["turn"]) => {
                    self.finished = true;
                    let turn = &params["turn"];
                    self.limit = self.limit.or(protocol::usage_limit(None, Some(turn)));
                    return Err(Stop::Failed(
                        failure.unwrap_or_else(|| protocol::turn_failure(turn)),
                    ));
                }
                _ => {}
            }
        }
    }

    /// After the image: interrupts the rest of the turn, and reads its token usage until it ends
    /// or [`SETTLE`] passes.
    async fn settle(&mut self) {
        self.interrupt();
        let ending = async {
            while let Some(note) = self.notes.recv().await {
                match note.method.as_str() {
                    "thread/tokenUsage/updated" => {
                        protocol::add_tokens(&mut self.usage, &note.params)
                    }
                    "turn/completed" if self.our_turn(&note.params["turn"]) => return true,
                    _ => {}
                }
            }
            false
        };
        if let Ok(true) = tokio::time::timeout(SETTLE, ending).await {
            self.finished = true;
        }
        self.finished = true;
    }

    fn usage(&self) -> Usage {
        self.usage.clone()
    }

    /// An item of this turn (its thread is this call's: the runtime routes by thread).
    fn ours(&self, params: &Value) -> bool {
        let turn = params.get("turnId").and_then(Value::as_str);
        turn.is_none_or(|t| self.turn.as_deref().is_none_or(|ours| t == ours))
    }

    fn our_turn(&self, turn: &Value) -> bool {
        let id = turn.get("id").and_then(Value::as_str);
        match (&self.turn, id) {
            (Some(ours), Some(id)) => id == ours,
            _ => true,
        }
    }

    /// Asks Codex to stop the turn, without waiting. Before Codex named the turn, the runtime's
    /// reader does it when the name arrives ([`Connection::start_turn`]).
    fn interrupt(&self) {
        if let (Some(thread), Some(turn)) = (&self.thread, &self.turn) {
            self.connection.notify(protocol::interrupt(thread, turn));
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if self.requested && !self.finished {
            self.interrupt();
        }
        if let Some(thread) = &self.thread {
            self.connection.forget(thread);
        }
    }
}

/// Seconds since the Unix epoch, as Codex's `resetsAt` counts them.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
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
                json!({"adapter": ADAPTER, "adapter_behavior": "2"})
            )),
            Ok(())
        );
        assert_eq!(served(&route("codex", Value::Null)), Ok(()));
        let mut edit = route("codex", json!({}));
        edit.capability = "image.edit".into();
        assert_eq!(served(&edit), Ok(()));
        edit.capability = "image.upscale".into();
        assert!(served(&edit).is_err());
        for contract in [
            json!({"adapter": "fx-openai-image-v1"}),
            json!({"adapter_behavior": 1}),
            // The skill-mention behaviour that took no pictures.
            json!({"adapter_behavior": "1"}),
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
        let file = |name: &str| json!({"file": name});
        let five = vec![file("r"); MAX_PICTURES - 1];
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
                json!({"prompt": "p", "image": file("a"), "mask": file("m")}),
                "Codex image edits take no mask",
            ),
            (
                json!({"prompt": "p", "image": file("a"), "references": vec![file("r"); 5]}),
                "Codex takes at most 5 input pictures",
            ),
        ] {
            assert_eq!(read(request.clone()), Err(reason.to_string()), "{request}");
        }
        // An edit's image comes first, then the references, in order.
        let request = json!({"prompt": "p", "image": file("a"), "references": five, "mask": null});
        let members: Vec<String> = Values::read(&request)
            .unwrap()
            .pictures
            .into_iter()
            .map(|(member, value)| format!("{member}={}", value["file"].as_str().unwrap()))
            .collect();
        assert_eq!(
            members,
            [
                "image=a",
                "references[0]=r",
                "references[1]=r",
                "references[2]=r",
                "references[3]=r"
            ]
        );
    }

    #[test]
    fn the_instructions_name_the_tool_s_arguments_then_the_prompt_verbatim() {
        let values = |background| Values {
            prompt: "a red\n kite ",
            background,
            pictures: Vec::new(),
        };
        assert_eq!(
            instructions(&values("auto"), &[]),
            "Call the image_gen.imagegen tool exactly once. Pass the text between the <prompt> \
             tags as its prompt, exactly as written. Set transparent_background to false. Do not \
             run commands, read or edit files, use any other tool or ask questions. When the image \
             is made, reply with one short sentence.\n\n<prompt>\na red\n kite \n</prompt>"
        );
        assert_eq!(
            instructions(&values("opaque"), &[]),
            instructions(&values("auto"), &[])
        );
        let pictures = [
            PathBuf::from("/tmp/x/picture-01.png"),
            PathBuf::from("/tmp/x/picture-02.webp"),
        ];
        assert!(instructions(&values("transparent"), &pictures).contains(
            "exactly as written. Set transparent_background to true. Set \
                 referenced_image_paths to these input images, in this order: \
                 `/tmp/x/picture-01.png`, `/tmp/x/picture-02.webp`. Do not run"
        ));
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
                "features.shell_tool=false",
                "-c",
                "features.unified_exec=false",
                "-c",
                "features.multi_agent=false",
                "-c",
                "features.browser_use=false",
                "-c",
                "features.computer_use=false",
                "-c",
                "features.in_app_browser=false",
                "-c",
                "features.view_image=false",
                "-c",
                "features.goals=false",
                "-c",
                "features.sleep_tool=false",
                "-c",
                "features.tool_suggest=false",
                "-c",
                "features.skill_search=false",
                "-c",
                "web_search=\"disabled\"",
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
