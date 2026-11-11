//! Sanitize supplementary failure evidence before it crosses the managed-call boundary.

use crate::CallRequest;
use crate::redact::{MAX_REASON_CHARS, Redactor};
use grida_fx_protocol::{AdapterDiagnostic, DiagnosticDetail};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)https?://[^\s\"'<>]+"#).unwrap());
static PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:[A-Za-z]:[\\/]|/)[^\s\"'<>]+"#).unwrap());

/// Redact secrets and input echoes, remove local paths/URLs and bound every field.
/// Attribution to the actual provider and route remains outside this adapter-supplied data.
pub fn sanitize(
    mut diagnostic: AdapterDiagnostic,
    call: &CallRequest,
    redactor: &Redactor,
) -> AdapterDiagnostic {
    let mut redactor = redactor.clone();
    fn echoes(value: &Value, redactor: &mut Redactor) {
        match value {
            Value::String(text) if text.len() >= 4 => *redactor = redactor.with(text),
            Value::Array(items) => items.iter().for_each(|v| echoes(v, redactor)),
            Value::Object(items) => items.values().for_each(|v| echoes(v, redactor)),
            _ => {}
        }
    }
    echoes(&call.request, &mut redactor);
    for file in call.files.values() {
        redactor = redactor.with(&file.path.to_string_lossy());
        if file.size <= 65_536
            && let Ok(bytes) = std::fs::read(&file.path)
        {
            if let Ok(text) = std::str::from_utf8(&bytes)
                && text.len() >= 4
            {
                redactor = redactor.with(text);
            }
            if bytes.len() >= 4 {
                use base64::Engine;
                redactor = redactor.with(&base64::engine::general_purpose::STANDARD.encode(&bytes));
            }
        }
    }

    let mut changed = false;
    let mut truncated = false;
    for field in [
        &mut diagnostic.code,
        &mut diagnostic.field,
        &mut diagnostic.request_id,
        &mut diagnostic.task_id,
        &mut diagnostic.operation,
    ] {
        if let Some(text) = field {
            let safe = !text.is_empty()
                && text.len() <= 128
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:@-".contains(&b))
                && !text.contains("://")
                && redactor.redact(text) == *text;
            if !safe {
                *field = None;
                changed = true;
            }
        }
    }
    if let Some(message) = &diagnostic.message {
        let text = redactor.redact(message);
        let text = URL.replace_all(&text, "[redacted URL]");
        let text = PATH.replace_all(&text, "[redacted path]");
        let text: String = text
            .chars()
            .filter(|c| {
                !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
            .collect();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        changed |= text != *message;
        truncated = text.chars().count() > MAX_REASON_CHARS;
        diagnostic.message = Some(redactor.reason(&text));
    }
    if diagnostic
        .http_status
        .is_some_and(|status| !(100..=599).contains(&status))
    {
        diagnostic.http_status = None;
        changed = true;
    }
    diagnostic.redacted |= changed;
    diagnostic.truncated |= truncated;
    if diagnostic.message.is_none() && changed {
        diagnostic.detail = DiagnosticDetail::Withheld;
    }
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CallBuilder;
    use grida_fx_protocol::{DiagnosticPhase, DiagnosticSource};
    use serde_json::json;

    #[test]
    fn evidence_is_bounded_after_secrets_files_urls_and_paths_are_redacted() {
        let mut builder = CallBuilder::new("acme.native", "model-a@acme");
        let file = builder.file("text/plain", b"private source contents");
        let call = builder
            .request(json!({"source":file,"prompt":"private prompt"}))
            .build();
        let mut diagnostic =
            AdapterDiagnostic::new(DiagnosticSource::Provider, DiagnosticPhase::Submit);
        diagnostic.operation = Some("future_operation".into());
        diagnostic.code = Some("bad_input".into());
        diagnostic.message = Some(format!(
            "private prompt private source contents made-up-secret https://assets.acme.test/a?sig=private /private/native/file.png \u{1b}[31m{}",
            "long ".repeat(150)
        ));
        let result = sanitize(
            diagnostic,
            &call,
            &Redactor::default().with("made-up-secret"),
        );
        let text = serde_json::to_string(&result).unwrap();
        for secret in [
            "private prompt",
            "private source contents",
            "made-up-secret",
            "https://",
            "/private/native",
            "\\u001b",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
        assert!(result.redacted && result.truncated);
        assert!(result.message.unwrap().chars().count() <= MAX_REASON_CHARS);
        assert_eq!(result.operation.as_deref(), Some("future_operation"));
        assert_eq!(result.code.as_deref(), Some("bad_input"));
    }

    #[test]
    fn input_and_file_echoes_are_withheld_in_every_metadata_field() {
        let mut builder = CallBuilder::new("acme.native", "model-a@acme");
        let file = builder.file("text/plain", b"private_file_contents");
        let call = builder
            .request(json!({"source":file,"prompt":"private_request_value"}))
            .build();
        for echo in [
            "private_file_contents",
            "private_request_value",
            "cHJpdmF0ZV9maWxlX2NvbnRlbnRz",
        ] {
            let mut diagnostic =
                AdapterDiagnostic::new(DiagnosticSource::Provider, DiagnosticPhase::Submit);
            diagnostic.code = Some(echo.into());
            diagnostic.field = Some(echo.into());
            diagnostic.request_id = Some(echo.into());
            diagnostic.task_id = Some(echo.into());
            diagnostic.operation = Some(echo.into());
            let result = sanitize(diagnostic, &call, &Redactor::default());
            assert_eq!(result.code, None);
            assert_eq!(result.field, None);
            assert_eq!(result.request_id, None);
            assert_eq!(result.task_id, None);
            assert_eq!(result.operation, None);
            assert!(result.redacted);
            assert_eq!(result.detail, DiagnosticDetail::Withheld);
        }
    }

    #[test]
    fn unsafe_identifiers_are_withheld_instead_of_becoming_new_record_fields() {
        let call = CallBuilder::new("acme.native", "model-a@acme").build();
        let mut diagnostic =
            AdapterDiagnostic::new(DiagnosticSource::Provider, DiagnosticPhase::Submit);
        diagnostic.request_id = Some("https://assets.acme.test/secret".into());
        diagnostic.field = Some("/private/native/file".into());
        diagnostic.http_status = Some(999);
        let result = sanitize(diagnostic, &call, &Redactor::default());
        assert_eq!(result.request_id, None);
        assert_eq!(result.field, None);
        assert_eq!(result.http_status, None);
        assert_eq!(result.detail, DiagnosticDetail::Withheld);
        assert!(result.redacted);
    }
}
