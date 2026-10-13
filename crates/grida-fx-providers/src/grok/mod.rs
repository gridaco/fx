//! Grok Build: `image.generate` and `image.edit` through the user's own Grok Build, signed in with
//! a Grok subscription (src/grok/README.md).
//!
//! No key and no HTTP: the adapter draws through a [`GrokRuntime`], one `grok agent --no-leader
//! stdio` an invocation starts on its first Grok call, keeps for the others and shuts down with the
//! run, spoken to over ACP ([`crate::acp`]). Grok Build owns the sign-in, the models and the usage;
//! FX never reads its credential files, and its calls bill the user's subscription
//! ([`Billing::Plan`]), never dollars. One send, in order:
//! 1. the route: capability `image.generate` or `image.edit` on provider `grok`, and a contract
//!    whose `adapter` (when present) is [`ADAPTER`] and whose `adapter_behavior` (when present)
//!    is [`ADAPTER_BEHAVIOR`];
//! 2. [`crate::capabilities::check_request`];
//! 3. values: a blank prompt, a `size` other than `auto`, a `transparent` background, a mask, more
//!    than [`MAX_PICTURES`] pictures (`image`, then `references`), and each picture's file:
//!    carried, not empty, and PNG or JPEG;
//! 4. the runtime's Grok Build, started if need be ([`runtime`]): no program ([`NOT_INSTALLED`],
//!    [`NETWORK_OFF`]), an older one, or a sign-in that is not a subscription is `Sent::Refused`,
//!    for this call and the invocation's others;
//! 5. a scratch folder of the call's own holding copies of its pictures (`picture-01.png`, ...),
//!    and a session in the runtime's folder whose agent offers Grok Build's two image tools and nothing else
//!    ([`profile`]), which asks before every tool call whatever the user's own permission mode
//!    (FX grants the image tools only, [`crate::acp`]), at low reasoning effort; then `session/prompt` with [`instructions`], and the
//!    session's updates until its first image tool call that completed with an image. The rest of
//!    the turn is then cancelled, its usage read, and the session deleted.
//!
//! Outcomes: a failure is `Failed { cost: None, retryable: false }`, and nothing is resent: before
//! `session/prompt` is written a failure is most likely to happen again; after, an image may
//! already have been made. The one exception is Grok Build's image tool saying it failed, so that
//! no image was saved: `retryable: true`, and the engine may make a new attempt. A call that is dropped (its run was cancelled) cancels its turn.
//! Grok Build reports no cost of an image: an answer's cost is `None`, and its [`Usage`] says what
//! Grok Build counted. Grok Build saves JPEG images; the answer is the same pixels as a PNG.

mod protocol;
mod runtime;

pub use runtime::GrokRuntime;

use crate::BoxFuture;
use crate::acp::{Failure, Notes};
use crate::adapter::{
    Adapter, Answer, Billing, CallRequest, RequestAdapter, RouteRef, Sent, Usage,
};
use crate::capabilities;
use crate::installed::{self, Picture, Program, failed};
use crate::registry::Adapters;
use crate::setup::Setup;
use crate::wire;
use runtime::Started;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::task::JoinHandle;

/// The label of every reason.
pub const LABEL: &str = "Grok Build image generation";

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "fx-grok-image-v1";

/// The route contract's `adapter_behavior` this adapter serves. It names [`instructions`], the
/// session's agent ([`profile`]) and the settings of [`environment`]: a change to any is a new
/// behaviour.
pub const ADAPTER_BEHAVIOR: &str = "1";

/// The oldest Grok Build whose behaviour this adapter was verified on.
pub const MIN_VERSION: &str = "1.0.52";

/// The most input pictures of one call (`image` and `references`): Grok Build's image edit model
/// takes three (it refuses more with HTTP 400).
pub const MAX_PICTURES: usize = 3;

/// The deadline of one send, from asking for Grok Build to the image.
pub const DEADLINE: Duration = Duration::from_secs(300);

/// The deadline for Grok Build to take the prompt (starting Grok Build too, on an invocation's
/// first call), within [`DEADLINE`].
pub const START_DEADLINE: Duration = Duration::from_secs(60);

/// The variable that names the program, as for any tool (spec/protocol.md §5.6).
pub const PROGRAM_VARIABLE: &str = "GRIDA_FX_TOOL_GROK";

/// The refusal when no program was found.
pub const NOT_INSTALLED: &str = "Grok Build is not installed: install it from https://x.ai/cli and \
                                 sign in with grok login, or set GRIDA_FX_TOOL_GROK";

/// The refusal when Grok Build has no cached sign-in.
pub const NOT_SIGNED_IN: &str = "Grok Build is not signed in: run grok login";

/// The refusal when the account's Grok Build cannot generate images.
pub const NO_SUBSCRIPTION: &str =
    "Grok Build's image tools need a SuperGrok subscription on the signed-in account";

/// The refusal of an exact size.
pub const NO_EXACT_SIZE: &str = "Grok Build image generation takes no exact size: use size auto, \
                                 and fx/image.resize@1 after it";

/// The refusal of a transparent background.
pub const NO_TRANSPARENCY: &str =
    "Grok Build draws opaque images: it takes no transparent background";

/// The refusal when `GRIDA_FX_NETWORK` is off: Grok Build generates over the network.
pub const NETWORK_OFF: &str = "Grok Build was not started: GRIDA_FX_NETWORK is off";

/// Grok Build's image tools: the only tools a session offers, and the only ones FX grants.
pub const TOOLS: [&str; 2] = ["image_gen", "image_edit"];

/// The agent of every session (`session/new`'s `_meta.agentProfile`): a curated agent whose tools
/// are the image tools only. Without `disallowedTools`, Grok Build adds its MCP tools
/// (`search_tool`, `use_tool`).
fn profile() -> Value {
    json!({
        "name": "grida-fx-image",
        "description": "Draws one image with Grok Build's image tools",
        "tools": TOOLS,
        "disallowedTools": ["search_tool", "use_tool"],
    })
}

/// The configuration overlay of every Grok Build FX starts (`GROK_CONFIG`): image generation on,
/// and telemetry and the features that act on their own off.
pub const OVERLAY: &str = r#"{"features":{"telemetry":false,"image_gen":true,"video_gen":false,"web_fetch":false,"lsp_tools":false,"write_file":false,"backend_tools":false,"codebase_indexing":false,"ask_user_question":false,"session_recap":false,"turn_summary":false}}"#;

/// Variables Grok Build never inherits besides the allowlisted ones ([`crate::keys::ALLOWED`]):
/// with `XAI_API_KEY` it could bill an API account; `GROK_CONFIG_PATH` would replace FX's overlay.
const ALSO_WITHHELD: [&str; 2] = ["XAI_API_KEY", "GROK_CONFIG_PATH"];

/// The kinds of picture Grok's image edits read, and the extension its copy takes.
const PICTURE_KINDS: [(&str, &str); 2] = [("image/png", "png"), ("image/jpeg", "jpg")];

/// How long an answered turn has to end after it is cancelled, for its usage.
const SETTLE: Duration = Duration::from_secs(3);

/// Why a call ended without an image.
#[derive(Debug)]
enum Stop {
    /// Grok Build cannot draw on the user's subscription: `Sent::Refused`.
    Refused(String),
    /// Grok Build's image tool reported that it failed, so no image was saved: the engine may make
    /// a new attempt (`retryable: true`).
    Retry(String),
    Failed(String),
}

impl Stop {
    #[cfg(test)]
    fn reason(self) -> String {
        match self {
            Stop::Refused(reason) | Stop::Retry(reason) | Stop::Failed(reason) => reason,
        }
    }
}

/// The adapter (module doc).
#[derive(Clone)]
pub struct GrokImages {
    runtime: Arc<GrokRuntime>,
    deadline: Duration,
    start_deadline: Duration,
}

impl std::fmt::Debug for GrokImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrokImages")
            .field("deadline", &self.deadline)
            .field("start_deadline", &self.start_deadline)
            .finish_non_exhaustive()
    }
}

impl GrokImages {
    /// The adapter over `runtime`.
    pub fn new(runtime: Arc<GrokRuntime>) -> GrokImages {
        GrokImages {
            runtime,
            deadline: DEADLINE,
            start_deadline: START_DEADLINE,
        }
    }

    /// The same adapter with other deadlines (tests).
    pub fn with_deadlines(mut self, start: Duration, whole: Duration) -> GrokImages {
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
        Ok(Drawing {
            prompt: values.prompt,
            pictures,
        })
    }
}

impl RequestAdapter for GrokImages {
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

/// Registers the adapter for `image.generate` and `image.edit` on `grok`, over one runtime of the
/// setup's program, which the registry keeps to shut it down with the invocation.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let runtime = Arc::new(GrokRuntime::new(setup.grok.clone()));
    let images = Arc::new(GrokImages::new(Arc::clone(&runtime)));
    for capability in ["image.generate", "image.edit"] {
        adapters.register(capability, "grok", Adapter::Request(images.clone()));
    }
    adapters.keep_runtime(runtime);
}

/// Where Grok Build's installer puts the command: `$HOME/.grok/bin/grok`.
fn installed_program(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let home = env("HOME").filter(|home| !home.trim().is_empty())?;
    let home = PathBuf::from(home);
    home.is_absolute()
        .then(|| home.join(".grok").join("bin").join("grok"))
}

/// The program (step 4), as an absolute path to an executable file:
/// - `GRIDA_FX_TOOL_GROK` when set and not blank (relative to the working directory), and nothing
///   else when it names no executable file;
/// - else the first `grok` in an absolute `PATH` entry (empty and relative entries are skipped, so
///   a project's own folder never supplies it). Never `agent`, which other programs also install;
/// - else [`installed_program`] when it is executable (origin `~/.grok/bin`).
pub fn find_program(env: &dyn Fn(&str) -> Option<String>) -> Option<Program> {
    let fallback = installed_program(env).map(|path| (path, "~/.grok/bin"));
    installed::find_program(env, PROGRAM_VARIABLE, "grok", fallback)
}

/// Every variable Grok Build never inherits.
pub fn withheld() -> impl Iterator<Item = &'static str> {
    installed::withheld(&ALSO_WITHHELD)
}

/// The variables every Grok Build FX starts gets: API-key sign-in off, no update during a run, no
/// managed MCP servers, nothing imported from Claude's, Cursor's or Codex's settings, and
/// [`OVERLAY`].
pub fn environment() -> Vec<(String, String)> {
    let mut set: Vec<(String, String)> = [
        ("GROK_DISABLE_API_KEY_AUTH", "1"),
        ("GROK_DISABLE_AUTOUPDATER", "1"),
        ("GROK_MANAGED_MCPS_ENABLED", "false"),
    ]
    .iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect();
    for source in ["CLAUDE", "CURSOR", "CODEX"] {
        for kind in ["SKILLS", "RULES", "AGENTS", "MCPS", "HOOKS", "SESSIONS"] {
            set.push((format!("GROK_{source}_{kind}_ENABLED"), "false".to_string()));
        }
    }
    set.push(("GROK_CONFIG".to_string(), OVERLAY.to_string()));
    set
}

/// A command that starts `grok agent --no-leader stdio` from `program` in `folder`: Grok Build's
/// own environment, less [`withheld`], plus [`environment`]; no stderr; in a process group of its
/// own; killed when dropped.
fn command(program: &Path, folder: &Path) -> Command {
    let args = ["agent", "--no-leader", "stdio"];
    let mut command = installed::command(program, args, folder, &ALSO_WITHHELD);
    command.envs(environment());
    command
}

/// The prompt's text (behaviour [`ADAPTER_BEHAVIOR`]): which tool to call once and with what, the
/// pictures' paths in order, fixed instructions, then the authored prompt verbatim.
pub fn instructions(prompt: &str, pictures: &[PathBuf]) -> String {
    let mut text = if pictures.is_empty() {
        "Call the image_gen tool exactly once.".to_string()
    } else {
        let paths: Vec<String> = pictures
            .iter()
            .map(|path| format!("`{}`", path.display()))
            .collect();
        format!(
            "Call the image_edit tool exactly once. Set image to these input images, in this \
             order: {}.",
            paths.join(", ")
        )
    };
    text.push_str(
        " Pass the text between the <prompt> tags as its prompt, exactly as written, and leave \
         aspect_ratio unset. Do not use any other tool or ask questions. When the image is made, \
         reply with one short sentence.",
    );
    format!("{text}\n\n<prompt>\n{prompt}\n</prompt>")
}

/// Step 1: whether the route is one this adapter serves.
fn served(route: &RouteRef) -> Result<(), String> {
    installed::served(route, "grok", ADAPTER, ADAPTER_BEHAVIOR)
}

/// A request's values after step 3.
#[derive(Debug, PartialEq)]
pub struct Values<'a> {
    /// Verbatim: the prompt is never trimmed or rewritten.
    pub prompt: &'a str,
    /// The input pictures in order, each with its member (`image`, `references[<i>]`); at most
    /// [`MAX_PICTURES`].
    pub pictures: Vec<(String, &'a Value)>,
}

impl<'a> Values<'a> {
    /// Reads a request that passed its capability's check (step 3).
    pub fn read(request: &'a Value) -> Result<Values<'a>, String> {
        let prompt = installed::prompt(request)?;
        installed::auto_size(request, NO_EXACT_SIZE)?;
        match request.get("background") {
            None | Some(Value::Null) => {}
            Some(Value::String(b)) if matches!(b.as_str(), "auto" | "opaque") => {}
            Some(Value::String(b)) if b == "transparent" => return Err(NO_TRANSPARENCY.into()),
            Some(_) => return Err("background must be auto, opaque or transparent".into()),
        }
        if request.get("mask").is_some_and(|mask| !mask.is_null()) {
            return Err("Grok Build image edits take no mask".into());
        }
        let pictures = installed::input_pictures(request, MAX_PICTURES, "Grok Build")?;
        Ok(Values { prompt, pictures })
    }
}

/// What one call draws: its prompt and its pictures' files.
struct Drawing<'a> {
    prompt: &'a str,
    pictures: Vec<Picture<'a>>,
}

/// Step 3 for one picture: a file the call carries, not empty, of a kind Grok Build reads.
fn picture<'a>(call: &'a CallRequest, value: &Value, member: &str) -> Result<Picture<'a>, String> {
    installed::picture(
        call,
        value,
        member,
        &PICTURE_KINDS,
        "Grok Build takes PNG or JPEG",
    )
}

/// Steps 4 and 5 (module doc) under their deadlines.
async fn draw(
    runtime: &GrokRuntime,
    wanted: &Drawing<'_>,
    start_deadline: Duration,
    deadline: Duration,
) -> Sent {
    let began = Instant::now();
    let mut turn: Option<Turn> = None;
    let begun = tokio::time::timeout(start_deadline, begin(runtime, wanted, &mut turn))
        .await
        .unwrap_or_else(|_| {
            Err(Stop::Failed(format!(
                "Grok Build did not take the prompt within {} s",
                start_deadline.as_secs()
            )))
        });
    // A stop before `session/prompt` is written.
    let stopped = match begun {
        Ok(()) => None,
        Err(Stop::Refused(reason)) => Some(Sent::Refused { reason }),
        Err(Stop::Failed(reason) | Stop::Retry(reason)) => {
            Some(failed(format!("{LABEL} was not started: {reason}")))
        }
    };
    if let Some(sent) = stopped {
        if let Some(turn) = turn {
            turn.close().await;
        }
        return sent;
    }
    let Some(mut drawing) = turn else {
        return failed(format!("{LABEL} was not started: no session"));
    };
    let left = deadline.saturating_sub(began.elapsed());
    let drawn = match tokio::time::timeout(left, drawing.image()).await {
        Ok(Ok(bytes)) => {
            let png = tokio::task::spawn_blocking(move || to_png(bytes));
            drawing.settle().await;
            png.await
                .unwrap_or_else(|_| Err(NOT_ENCODED.into()))
                .map_err(Stop::Failed)
        }
        Ok(Err(stop)) => Err(stop),
        Err(_) => Err(Stop::Failed(format!(
            "Grok Build did not finish within {} s",
            deadline.as_secs()
        ))),
    };
    let sent = match drawn {
        Ok(png) => Sent::Answered(
            Answer::new(Value::Null, None)
                .with_file("image", "image/png", png)
                .with_usage(std::mem::take(&mut drawing.usage)),
        ),
        Err(Stop::Refused(reason)) => {
            runtime.refuse_the_rest(reason.clone());
            failed(format!("{LABEL} failed: {reason}"))
        }
        Err(Stop::Failed(reason)) => failed(format!("{LABEL} failed: {reason}")),
        Err(Stop::Retry(reason)) => Sent::Failed {
            reason: format!("{LABEL} failed: {reason}"),
            cost: None,
            retryable: true,
        },
    };
    drawing.close().await;
    sent
}

/// The runtime's Grok Build, a session, and the prompt: on its way into `slot`, so a stop at any
/// point still ends the session.
async fn begin(
    runtime: &GrokRuntime,
    wanted: &Drawing<'_>,
    slot: &mut Option<Turn>,
) -> Result<(), Stop> {
    let started = runtime.started().await?;
    let folder = tempfile::Builder::new()
        .prefix("call-")
        .tempdir_in(started.folder())
        .map_err(|error| Stop::Failed(grida_fx_core::error::io_reason(&error)))?;
    let pictures = installed::place(&wanted.pictures, folder.path()).map_err(Stop::Failed)?;
    let text = instructions(wanted.prompt, &pictures);
    let (session, notes) = started
        .connection
        .new_session(json!({
            "cwd": started.folder().to_string_lossy(),
            "mcpServers": [],
            "_meta": {"agentProfile": profile(), "yoloMode": false},
        }))
        .await
        .map_err(|failure| runtime::stop("session/new", failure))?;
    started.opened(&session);
    let turn = slot.insert(Turn {
        started: Arc::clone(&started),
        session,
        notes,
        prompt: None,
        usage: Usage {
            runtime: started.version.as_ref().map(|v| format!("grok {v}")),
            ..Usage::default()
        },
        closed: false,
        _folder: folder,
    });
    // A Grok Build that refuses the effort draws at its own: the image is the same tool's.
    let _ = started
        .connection
        .set_config_option(&turn.session, "reasoning_effort", "low")
        .await;
    let (asked, session) = (Arc::clone(&started), turn.session.clone());
    turn.prompt = Some(tokio::spawn(async move {
        asked.connection.prompt(&session, &text).await
    }));
    Ok(())
}

/// What a turn waits on next.
enum Event {
    Note(Option<Value>),
    Answered(Result<Value, Failure>),
}

/// A call's session on the runtime's Grok Build, ended ([`end`]) by [`Turn::close`]; dropped
/// before (its call was cancelled), it ends in a task of its own, which the runtime's shutdown
/// waits for. Dropped at all, it stops routing its notifications.
struct Turn {
    started: Arc<Started>,
    session: String,
    notes: Notes,
    /// The `session/prompt` request, from when it is written until the turn ends.
    prompt: Option<JoinHandle<Result<Value, Failure>>>,
    usage: Usage,
    /// The session's ending was taken ([`Turn::ending`]).
    closed: bool,
    /// The folder of the call's pictures, removed after it.
    _folder: tempfile::TempDir,
}

impl Turn {
    async fn next(&mut self) -> Event {
        let notes = &mut self.notes;
        match self.prompt.as_mut() {
            // Updates first: the reader routes a turn's updates before its answer, so none that
            // came before the answer is lost to it.
            Some(prompt) => tokio::select! {
                biased;
                note = notes.recv() => Event::Note(note),
                answered = prompt => {
                    self.prompt = None;
                    Event::Answered(answered.unwrap_or_else(|_| {
                        Err(Failure::Ended("the prompt was dropped".into()))
                    }))
                }
            },
            None => Event::Note(notes.recv().await),
        }
    }

    /// The first image of this turn's image tool calls; a turn that ends without one is a
    /// failure, and so is a call of any other tool.
    async fn image(&mut self) -> Result<Vec<u8>, Stop> {
        let mut failure: Option<Stop> = None;
        loop {
            match self.next().await {
                Event::Note(None) => return Err(Stop::Failed(self.started.connection.ended())),
                Event::Note(Some(note)) => {
                    let update = &note["update"];
                    let kind = update.get("sessionUpdate").and_then(Value::as_str);
                    if !matches!(kind, Some("tool_call" | "tool_call_update")) {
                        continue;
                    }
                    if let Some(tool) = protocol::tool_name(update)
                        && !TOOLS.contains(&tool.as_str())
                    {
                        return Err(Stop::Failed(format!(
                            "Grok Build called {}, a tool FX does not allow",
                            protocol::token(&tool)
                        )));
                    }
                    match protocol::status(update) {
                        Some("completed") => {
                            let read = protocol::saved_image(&update["rawOutput"], &self.session)
                                .and_then(|path| {
                                    std::fs::read(path).map_err(|_| {
                                        "Grok Build's image could not be read".to_string()
                                    })
                                });
                            match read {
                                Ok(bytes) => return Ok(bytes),
                                Err(reason) => {
                                    failure.get_or_insert(Stop::Failed(reason));
                                }
                            }
                        }
                        Some("failed") => {
                            failure.get_or_insert(protocol::tool_failure(update));
                        }
                        _ => {}
                    }
                }
                Event::Answered(answered) => {
                    let result = answered.map_err(|f| runtime::stop("session/prompt", f))?;
                    protocol::add_usage(&mut self.usage, &result);
                    return Err(failure
                        .unwrap_or_else(|| Stop::Failed(protocol::ended_without_image(&result))));
                }
            }
        }
    }

    /// After the image: cancels the rest of the turn, and reads its usage until it ends or
    /// [`SETTLE`] passes.
    async fn settle(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        self.started.connection.cancel(&self.session);
        if let Ok(Ok(Ok(result))) = tokio::time::timeout(SETTLE, prompt).await {
            protocol::add_usage(&mut self.usage, &result);
        }
    }

    /// The session's ending ([`end`]), the first time it is asked for.
    fn ending(&mut self) -> Option<impl Future<Output = ()> + use<>> {
        if std::mem::replace(&mut self.closed, true) {
            return None;
        }
        let prompt = self.prompt.take();
        Some(end(Arc::clone(&self.started), self.session.clone(), prompt))
    }

    /// Ends the session, waiting [`SETTLE`] twice at most.
    async fn close(mut self) {
        if let Some(ending) = self.ending() {
            ending.await;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let unfinished = self.prompt.is_some();
        if let Some(ending) = self.ending() {
            match tokio::runtime::Handle::try_current() {
                Ok(tokio) => self.started.ending(tokio.spawn(ending)),
                // Without a tokio runtime, it asks without waiting.
                Err(_) => {
                    let connection = &self.started.connection;
                    if unfinished {
                        connection.cancel(&self.session);
                    }
                    connection.fire("_x.ai/session/delete", json!({"sessionId": self.session}));
                }
            }
        }
        self.started.connection.forget(&self.session);
    }
}

/// Ends a call's session: cancels a turn that has not ended (`prompt`) and waits for its answer,
/// [`SETTLE`] at most (a session deleted while its turn winds down is written again), then deletes
/// the session, [`SETTLE`] at most.
async fn end(
    started: Arc<Started>,
    session: String,
    prompt: Option<JoinHandle<Result<Value, Failure>>>,
) {
    let connection = &started.connection;
    if let Some(prompt) = prompt {
        connection.cancel(&session);
        let _ = tokio::time::timeout(SETTLE, prompt).await;
    }
    let deleted = connection.request("_x.ai/session/delete", json!({"sessionId": session}));
    if let Ok(Ok(_)) = tokio::time::timeout(SETTLE, deleted).await {
        started.closed(&session);
    }
}

/// The reason of an image that could not be encoded as PNG.
const NOT_ENCODED: &str = "Grok Build's image could not be encoded as PNG";

/// The answer's PNG: a PNG as it is, any other image decoded and encoded as PNG, the same pixels.
fn to_png(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if wire::sniff_image(&bytes) == Some("image/png") {
        return Ok(bytes);
    }
    let picture = image::load_from_memory(&bytes)
        .map_err(|_| "Grok Build's image is not decodable".to_string())?;
    let mut png = Vec::new();
    picture
        .write_with_encoder(image::codecs::png::PngEncoder::new(&mut png))
        .map_err(|_| NOT_ENCODED.to_string())?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn route(provider: &str, contract: Value) -> RouteRef {
        RouteRef {
            capability: "image.generate".into(),
            model: "imagine".into(),
            provider: provider.into(),
            contract,
        }
    }

    #[test]
    fn routes_are_served_by_provider_and_contract() {
        assert_eq!(
            served(&route(
                "grok",
                json!({"adapter": ADAPTER, "adapter_behavior": "1"})
            )),
            Ok(())
        );
        assert_eq!(served(&route("grok", Value::Null)), Ok(()));
        let mut edit = route("grok", json!({}));
        edit.capability = "image.edit".into();
        assert_eq!(served(&edit), Ok(()));
        for contract in [
            json!({"adapter": "fx-codex-image-v1"}),
            json!({"adapter_behavior": 1}),
            json!({"adapter_behavior": "2"}),
            json!([]),
        ] {
            assert!(served(&route("grok", contract)).is_err());
        }
        assert_eq!(
            served(&route("codex", json!({}))),
            Err("imagine@codex is not a image.generate route this adapter serves".into())
        );
    }

    #[test]
    fn values_refuse_what_grok_does_not_honour() {
        let read = |request: Value| Values::read(&request).map(|v| v.prompt.to_string());
        let file = |name: &str| json!({"file": name});
        assert_eq!(read(json!({"prompt": " a kite "})), Ok(" a kite ".into()));
        assert_eq!(
            read(json!({"prompt": "p", "size": "auto", "background": "opaque"})),
            Ok("p".into())
        );
        for (request, reason) in [
            (
                json!({"prompt": " "}),
                "an image call needs its prompt as text",
            ),
            (json!({"prompt": "p", "size": "1024x1024"}), NO_EXACT_SIZE),
            (
                json!({"prompt": "p", "background": "transparent"}),
                NO_TRANSPARENCY,
            ),
            (
                json!({"prompt": "p", "background": "clear"}),
                "background must be auto, opaque or transparent",
            ),
            (
                json!({"prompt": "p", "image": file("a"), "mask": file("m")}),
                "Grok Build image edits take no mask",
            ),
            (
                json!({"prompt": "p", "image": file("a"), "references": vec![file("r"); 3]}),
                "Grok Build takes at most 3 input pictures",
            ),
        ] {
            assert_eq!(read(request.clone()), Err(reason.to_string()), "{request}");
        }
    }

    #[test]
    fn the_instructions_name_one_tool_then_the_prompt_verbatim() {
        let prompt = "a red\n kite ";
        assert_eq!(
            instructions(prompt, &[]),
            "Call the image_gen tool exactly once. Pass the text between the <prompt> tags as its \
             prompt, exactly as written, and leave aspect_ratio unset. Do not use any other tool \
             or ask questions. When the image is made, reply with one short sentence.\n\n<prompt>\n\
             a red\n kite \n</prompt>"
        );
        let pictures = [
            PathBuf::from("/tmp/x/picture-01.png"),
            PathBuf::from("/tmp/x/picture-02.jpg"),
        ];
        assert!(instructions(prompt, &pictures).starts_with(
            "Call the image_edit tool exactly once. Set image to these input images, in this \
             order: `/tmp/x/picture-01.png`, `/tmp/x/picture-02.jpg`. Pass the text"
        ));
    }

    #[test]
    fn the_program_is_the_variable_then_path_then_the_installer_s() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let runnable = |path: &Path| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let bin = dir.path().join("bin");
        let on_path = bin.join("grok");
        runnable(&on_path);
        runnable(&bin.join("agent"));
        let home = dir.path().join("home");
        let installed = home.join(".grok").join("bin").join("grok");
        runnable(&installed);
        let named = dir.path().join("named-grok");
        runnable(&named);
        let shown = |p: &Path| p.to_string_lossy().to_string();
        let env = |pairs: Vec<(&'static str, String)>| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.clone())
            }
        };
        let find_program = |env: &dyn Fn(&str) -> Option<String>| {
            find_program(env).map(|program| (program.path, program.origin))
        };
        assert_eq!(
            find_program(&env(vec![
                (PROGRAM_VARIABLE, shown(&named)),
                ("PATH", shown(&bin))
            ])),
            Some((named.clone(), PROGRAM_VARIABLE))
        );
        assert_eq!(
            find_program(&env(vec![(PROGRAM_VARIABLE, "/nonexistent/grok".into())])),
            None
        );
        assert_eq!(
            find_program(&env(vec![("PATH", shown(&bin)), ("HOME", shown(&home))])),
            Some((on_path, "PATH"))
        );
        // `agent` alone is never taken for Grok Build.
        let only_agent = dir.path().join("only-agent");
        runnable(&only_agent.join("agent"));
        assert_eq!(
            find_program(&env(vec![
                ("PATH", shown(&only_agent)),
                ("HOME", shown(&home))
            ])),
            Some((installed, "~/.grok/bin"))
        );
        assert_eq!(find_program(&env(vec![("PATH", shown(&only_agent))])), None);
        assert_eq!(find_program(&env(vec![("HOME", "relative".into())])), None);
    }

    #[test]
    fn grok_inherits_no_key_and_starts_locked_down() {
        let withheld: Vec<&str> = withheld().collect();
        assert!(withheld.contains(&"XAI_API_KEY") && withheld.contains(&"OPENAI_API_KEY"));
        let set = environment();
        let value = |name: &str| set.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
        assert_eq!(value("GROK_DISABLE_API_KEY_AUTH"), Some("1"));
        assert_eq!(value("GROK_CURSOR_MCPS_ENABLED"), Some("false"));
        assert_eq!(value("GROK_CLAUDE_HOOKS_ENABLED"), Some("false"));
        let overlay: Value = serde_json::from_str(value("GROK_CONFIG").unwrap()).unwrap();
        assert_eq!(overlay["features"]["image_gen"], true);
        assert_eq!(overlay["features"]["write_file"], false);
        assert_eq!(
            profile()["disallowedTools"],
            json!(["search_tool", "use_tool"])
        );
    }

    #[test]
    fn a_jpeg_becomes_a_png_of_the_same_pixels() {
        let png = to_png(crate::testing::media::jpeg(8, 4)).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 4));
        assert!(!decoded.color().has_alpha());
        let already = crate::testing::media::png(2, 2, None);
        assert_eq!(to_png(already.clone()).unwrap(), already);
        assert_eq!(
            to_png(b"not an image".to_vec()),
            Err("Grok Build's image is not decodable".into())
        );
    }
}
