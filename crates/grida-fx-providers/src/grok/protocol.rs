//! What Grok Build says over ACP, read (spec/providers.md §9.8): its version and sign-in, its
//! tool calls, where it saved an image, its usage and its failures. Reasons are fixed sentences
//! with bounded tokens, never Grok Build's own text, which can name private paths
//! (spec/providers.md §8).

use super::Stop;
use crate::adapter::Usage;
use crate::installed::{self, Version};
pub(super) use crate::installed::{name, token};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// The most bytes an image Grok Build saved may have.
pub(super) const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

/// The name Grok Build gives a tool call: `_meta["x.ai/tool"].name`, else its `title` when that is
/// one of the image tools' own names (Grok Build's first `tool_call` update titles it so).
pub(super) fn tool_name(call: &Value) -> Option<String> {
    let named = call
        .get("_meta")
        .and_then(|meta| meta.get("x.ai/tool"))
        .and_then(|tool| tool.get("name"))
        .and_then(Value::as_str);
    named
        .or_else(|| {
            call.get("title")
                .and_then(Value::as_str)
                .filter(|title| super::TOOLS.contains(title))
        })
        .map(str::to_string)
}

/// Grok Build's version from `initialize` (`_meta.agentVersion`), when it is one.
pub(super) fn version_of(initialized: &Value) -> Option<String> {
    let version = initialized
        .get("_meta")
        .and_then(|meta| meta.get("agentVersion"))
        .and_then(Value::as_str)?;
    installed::version(version)
}

/// A Grok Build older than [`super::MIN_VERSION`], or one that names no version, is refused.
pub(super) fn check_version(version: Option<&str>) -> Result<(), Stop> {
    match Version::of(version, super::MIN_VERSION) {
        Version::Supported(_) => Ok(()),
        Version::Older { version, minimum } => Err(Stop::Refused(format!(
            "Grok Build {version} is older than {minimum}: run grok update"
        ))),
        Version::Unnamed => Err(Stop::Refused(format!(
            "Grok Build did not name its version: run grok update to {} or later",
            super::MIN_VERSION
        ))),
    }
}

/// Whether `initialize` offers the sign-in `grok login` caches.
pub(super) fn offers_cached_token(initialized: &Value) -> bool {
    initialized["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|method| method.get("id").and_then(Value::as_str) == Some("cached_token"))
}

/// An account that Grok Build bills through the xAI API (`backend_billed`) does not draw on a
/// subscription. Nothing else of the account is read or kept.
pub(super) fn check_account(authenticated: &Value) -> Result<(), Stop> {
    let billed = authenticated
        .get("_meta")
        .and_then(|meta| meta.get("backend_billed"))
        .and_then(Value::as_bool);
    match billed {
        Some(true) => Err(Stop::Refused(
            "Grok Build bills this account through the xAI API, not a Grok subscription: sign in \
             with grok login"
                .into(),
        )),
        _ => Ok(()),
    }
}

/// The image a completed image tool call saved: a regular file (not a link) at most
/// [`MAX_IMAGE_BYTES`] long, named `<n>.<jpg|jpeg|png|webp>` in a folder `images` of a folder named
/// after this session; anything else is refused.
pub(super) fn saved_image(raw_output: &Value, session: &str) -> Result<PathBuf, String> {
    let refused = || "Grok Build reported an image FX does not read".to_string();
    let kind = raw_output.get("type").and_then(Value::as_str);
    if !matches!(kind, Some("ImageGen" | "ImageEdit")) {
        return Err(refused());
    }
    let path = raw_output
        .get("path")
        .and_then(Value::as_str)
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or_else(refused)?;
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(refused());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(refused)?;
    let (stem, extension) = name.split_once('.').ok_or_else(refused)?;
    let named = !stem.is_empty()
        && stem.len() <= 6
        && stem.bytes().all(|b| b.is_ascii_digit())
        && matches!(extension, "jpg" | "jpeg" | "png" | "webp");
    let images = path.parent().ok_or_else(refused)?;
    let folder = images.parent().ok_or_else(refused)?;
    let placed = images.file_name().and_then(|n| n.to_str()) == Some("images")
        && folder.file_name().and_then(|n| n.to_str()) == Some(session);
    if !named || !placed {
        return Err(refused());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| refused())?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_IMAGE_BYTES {
        return Err(refused());
    }
    Ok(path.to_path_buf())
}

/// A tool call update's status.
pub(super) fn status(update: &Value) -> Option<&str> {
    update.get("status").and_then(Value::as_str)
}

/// Why an image tool call failed, read from Grok Build's words for it (never shown):
/// - the subscription its image tools need, when they name SuperGrok;
/// - a request xAI refused (`HTTP 4xx`, but 408 and 429): final, since it would be refused again;
/// - else, such as a network error, 408, 429 or `HTTP 5xx`: a fixed sentence the engine may try
///   again for, since no image was saved.
pub(super) fn tool_failure(update: &Value) -> Stop {
    let words = update.to_string();
    if words.contains("SuperGrok") {
        return Stop::Refused(super::NO_SUBSCRIPTION.into());
    }
    match http_status(&words) {
        Some(status) if (400..500).contains(&status) && !matches!(status, 408 | 429) => {
            Stop::Failed(format!(
                "Grok Build's image tool was refused (HTTP {status})"
            ))
        }
        _ => Stop::Retry("Grok Build's image tool failed".into()),
    }
}

/// The first status in `HTTP <nnn>` in `words`.
fn http_status(words: &str) -> Option<u16> {
    words.match_indices("HTTP ").find_map(|(at, _)| {
        let digits = words.get(at + 5..at + 8)?;
        let next = words[at + 8..].chars().next();
        (digits.bytes().all(|b| b.is_ascii_digit()) && !next.is_some_and(|c| c.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
    })
}

/// Why a prompt ended without an image: its `stopReason`.
pub(super) fn ended_without_image(result: &Value) -> String {
    match result.get("stopReason").and_then(Value::as_str) {
        Some("end_turn") | None => "Grok Build answered without an image".to_string(),
        Some(reason) => format!("Grok Build's turn ended {} without an image", token(reason)),
    }
}

/// A prompt result's usage (`_meta.usage`, and `_meta.modelId`) onto `usage`.
pub(super) fn add_usage(usage: &mut Usage, result: &Value) {
    let meta = &result["_meta"];
    let total = &meta["usage"];
    let count = |name: &str| total.get(name).and_then(Value::as_u64);
    if let Some(input) = count("inputTokens") {
        usage.input_tokens = Some(input);
        usage.cached_input_tokens = count("cachedReadTokens");
        usage.output_tokens = count("outputTokens");
    }
    if let Some(model) = meta.get("modelId").and_then(Value::as_str).and_then(name) {
        usage.agent_model = Some(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An `initialize` result.
    fn handshake_result(version: &str, methods: &[&str]) -> Value {
        json!({
            "protocolVersion": 1,
            "authMethods": methods.iter().map(|m| json!({"id": m, "name": m})).collect::<Vec<_>>(),
            "_meta": {"agentVersion": version},
        })
    }

    #[test]
    fn tools_are_named_by_grok_s_own_meta_then_their_title() {
        let meta =
            json!({"title": "imagine: a kite", "_meta": {"x.ai/tool": {"name": "image_gen"}}});
        assert_eq!(tool_name(&meta).as_deref(), Some("image_gen"));
        assert_eq!(
            tool_name(&json!({"title": "image_edit"})).as_deref(),
            Some("image_edit")
        );
        assert_eq!(tool_name(&json!({"title": "run_terminal_command"})), None);
        let shell = json!({"_meta": {"x.ai/tool": {"name": "run_terminal_command"}}});
        assert_eq!(tool_name(&shell).as_deref(), Some("run_terminal_command"));
    }

    #[test]
    fn only_a_recent_signed_in_subscription_is_admitted() {
        let initialized = handshake_result("1.0.52", &["cached_token", "grok.com"]);
        assert_eq!(version_of(&initialized).as_deref(), Some("1.0.52"));
        assert!(offers_cached_token(&initialized));
        assert!(check_version(Some("1.0.52")).is_ok());
        assert!(check_version(Some("1.1.0")).is_ok());
        assert!(check_version(Some("2.0.0-beta.1")).is_ok());
        let reason = |v: Option<&str>| check_version(v).err().map(|s| s.reason());
        assert_eq!(
            reason(Some("1.0.30")).as_deref(),
            Some("Grok Build 1.0.30 is older than 1.0.52: run grok update")
        );
        assert!(
            reason(None)
                .unwrap()
                .starts_with("Grok Build did not name its version")
        );
        assert_eq!(
            version_of(&json!({"_meta": {"agentVersion": "../../x"}})),
            None
        );
        let keyed = handshake_result("1.0.52", &["xai.api_key"]);
        assert!(!offers_cached_token(&keyed));
        assert!(
            check_account(&json!({"_meta": {"backend_billed": false, "email": "a@b"}})).is_ok()
        );
        assert!(check_account(&json!({})).is_ok());
        assert!(
            check_account(&json!({"_meta": {"backend_billed": true}}))
                .err()
                .unwrap()
                .reason()
                .contains("through the xAI API")
        );
    }

    #[test]
    fn an_image_is_read_only_from_this_session_s_images_folder() {
        let home = tempfile::tempdir().unwrap();
        let images = home
            .path()
            .join("sessions")
            .join("%2Fwork")
            .join("s-1")
            .join("images");
        std::fs::create_dir_all(&images).unwrap();
        let saved = images.join("1.jpg");
        std::fs::write(&saved, b"\xff\xd8\xff jpeg").unwrap();
        let raw = |path: &Path| json!({"type": "ImageGen", "path": path.to_string_lossy(), "filename": "1.jpg"});
        assert_eq!(saved_image(&raw(&saved), "s-1"), Ok(saved.clone()));
        let refused = Err("Grok Build reported an image FX does not read".to_string());
        assert_eq!(saved_image(&raw(&saved), "s-2"), refused);
        assert_eq!(
            saved_image(
                &json!({"type": "Bash", "path": saved.to_string_lossy()}),
                "s-1"
            ),
            refused
        );
        let elsewhere = home.path().join("1.jpg");
        std::fs::write(&elsewhere, b"x").unwrap();
        assert_eq!(saved_image(&raw(&elsewhere), "s-1"), refused);
        let odd = images.join("secret.txt");
        std::fs::write(&odd, b"x").unwrap();
        assert_eq!(saved_image(&raw(&odd), "s-1"), refused);
        let empty = images.join("2.jpg");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(saved_image(&raw(&empty), "s-1"), refused);
        let climbing = images.join("..").join("images").join("1.jpg");
        assert_eq!(saved_image(&raw(&climbing), "s-1"), refused);
        #[cfg(unix)]
        {
            let link = images.join("3.jpg");
            std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
            assert_eq!(saved_image(&raw(&link), "s-1"), refused);
        }
        assert_eq!(
            saved_image(&json!({"type": "ImageGen", "path": "images/1.jpg"}), "s-1"),
            refused
        );
    }

    #[test]
    fn failures_and_usage_use_bounded_words_only() {
        assert_eq!(
            tool_failure(
                &json!({"content": [{"type": "content", "content": {"type": "text",
                "text": "Image generation is a SuperGrok feature and isn't available"}}]})
            )
            .reason(),
            super::super::NO_SUBSCRIPTION
        );
        assert_eq!(
            tool_failure(&json!({"content": "see /Users/a/.grok/x"})).reason(),
            "Grok Build's image tool failed"
        );
        let said = |message: &str| json!({"status": "failed", "rawOutput": {"error": "tool_execution_failed", "message": message}});
        assert!(matches!(
            tool_failure(&said("Image edit failed with HTTP 400 Bad Request: {\"error\":\"at most 3\"}")),
            Stop::Failed(reason) if reason == "Grok Build's image tool was refused (HTTP 400)"
        ));
        for retried in [
            "Image generation API request failed: error sending request for url",
            "Image generation failed with HTTP 429 Too Many Requests",
            "Image generation failed with HTTP 503 Service Unavailable",
            "Image generation failed with HTTP 4000",
        ] {
            assert!(
                matches!(tool_failure(&said(retried)), Stop::Retry(_)),
                "{retried}"
            );
        }
        assert_eq!(
            ended_without_image(&json!({"stopReason": "end_turn"})),
            "Grok Build answered without an image"
        );
        assert_eq!(
            ended_without_image(&json!({"stopReason": "max_turn_requests"})),
            "Grok Build's turn ended max_turn_requests without an image"
        );
        assert_eq!(
            ended_without_image(&json!({"stopReason": "/Users/a"})),
            "Grok Build's turn ended unknown without an image"
        );
        let mut usage = Usage::default();
        add_usage(
            &mut usage,
            &json!({"stopReason": "end_turn", "_meta": {"modelId": "grok-4.7", "usage": {
                "inputTokens": 11081, "outputTokens": 182, "cachedReadTokens": 6784,
                "costUsdTicks": 44465200}}}),
        );
        assert_eq!(
            (
                usage.agent_model.as_deref(),
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.output_tokens
            ),
            (Some("grok-4.7"), Some(11081), Some(6784), Some(182))
        );
    }
}
