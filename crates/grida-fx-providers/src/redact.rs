//! Redaction of every sentence an adapter reports (spec/providers.md §8).
//!
//! A reason (`Sent::Failed`, `Collected::Ended`, …) reaches events, errors and the terminal, so it
//! never holds a key, an authorization header, a signed URL, an upload token, a data URL or a
//! response body. Adapters build reasons from fixed sentences plus allowlisted fields, and pass
//! every one through [`Redactor::reason`] as a last line of defence:
//!
//! 1. each configured secret (every key of the invocation, plus whatever an adapter adds with
//!    [`Redactor::with`], such as a signed result URL) is replaced by `[redacted]`, longest first;
//! 2. then the patterns, in this order, case-insensitive except the key shapes:
//!    - a data URL's payload: `data:<type>[;…];base64,<payload>` → `data:<type>;base64,[redacted]`;
//!    - the string value of a `b64_json`, `base64`, `audio_data` or `image_data` member (JSON
//!      `"name": "…"` or `name='…'`) → `[redacted]`;
//!    - an `authorization` (or `proxy-authorization`) value, with or without its `Bearer`, `Key`,
//!      `Basic` or `Token` scheme (`authorization: Bearer x`, `"Authorization": "Key x"`) →
//!      `[redacted]`; so is an `xi-api-key` or `x-api-key` header value;
//!    - the quoted value of an `api_key` (`api-key`, `apikey`), `token`, `secret`, `credential`
//!      or `password` member (a plural too, and a longer name ending in one, such as
//!      `access_token`) → `[redacted]`;
//!    - `sk-…` and `sk-or-…` keys (eight or more key characters) → `[redacted]`;
//!    - an `http(s)` URL's userinfo (`https://user:pw@` → `https://[redacted]@`) and query
//!      (`?…`, to the next space or quote, the fragment included → `?[redacted]`);
//!    - any run of 80 or more base64 or base64url characters, with its `=` padding →
//!      `[redacted base64]`;
//! 3. whitespace collapsed to single spaces, trimmed, and the result cut to
//!    [`MAX_REASON_CHARS`] characters, ending in `…` when cut.

use crate::keys::Secret;
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

/// The longest reason, in characters.
pub const MAX_REASON_CHARS: usize = 500;

/// What a redacted secret becomes.
pub const REDACTED: &str = "[redacted]";

/// Redacts reasons (module doc).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Redactor {
    secrets: Vec<Secret>,
}

impl Redactor {
    pub fn new(secrets: Vec<Secret>) -> Redactor {
        let mut secrets: Vec<Secret> = secrets
            .into_iter()
            .filter(|s| !s.expose().is_empty())
            .collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.expose().len()));
        Redactor { secrets }
    }

    /// The same redactor with one more secret (a signed URL, an upload token).
    pub fn with(&self, extra: &str) -> Redactor {
        let mut secrets = self.secrets.clone();
        secrets.push(Secret::new(extra));
        Redactor::new(secrets)
    }

    /// Steps 1 and 2 of the module doc.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            out = out.replace(secret.expose(), REDACTED);
        }
        for (pattern, replacement) in PATTERNS.iter() {
            let replaced = match pattern.replace_all(&out, *replacement) {
                Cow::Borrowed(_) => None,
                Cow::Owned(replaced) => Some(replaced),
            };
            if let Some(replaced) = replaced {
                out = replaced;
            }
        }
        out
    }

    /// Steps 1 to 3: the sentence an adapter may report.
    pub fn reason(&self, text: &str) -> String {
        bounded(&self.redact(text), MAX_REASON_CHARS)
    }
}

/// The step-2 patterns of the module doc, in order, with their replacements.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // A data URL's payload, its media type kept.
        (
            r#"(?i)data:([^\s;,"')]+)(?:;[^,\s]*)?;base64,[A-Za-z0-9+/_=\\-]+"#,
            "data:${1};base64,[redacted]",
        ),
        // The string value of a member that carries media as base64.
        (
            r#"(?i)((?:b64_json|base64|audio_data|image_data)["']?\s*[:=]\s*["'])[^"']+(["'])"#,
            "${1}[redacted]${2}",
        ),
        // An authorization value, with or without its scheme.
        (
            r#"(?i)((?:proxy-)?authorization["']?\s*[:=]\s*["']?)(?:(?:bearer|key|basic|token)\s+)?[^\s,"'}]+"#,
            "${1}[redacted]",
        ),
        // An API-key header value.
        (
            r#"(?i)((?:xi|x)-api-key["']?\s*[:=]\s*["']?)[^\s,"'}]+"#,
            "${1}[redacted]",
        ),
        // The quoted value of a credential-like member.
        (
            r#"(?i)(["']?[A-Za-z0-9_-]*(?:api[_-]?keys?|tokens?|secrets?|credentials?|passwords?)["']?\s*[:=]\s*["'])[^"']+(["'])"#,
            "${1}[redacted]${2}",
        ),
        // OpenAI and OpenRouter key shapes.
        (r"\bsk-(?:or-)?[A-Za-z0-9_-]{8,}", "[redacted]"),
        // A URL's userinfo.
        (r#"(?i)(\bhttps?://)[^\s/?#@"'<>]+@"#, "${1}[redacted]@"),
        // A URL's query (and fragment).
        (
            r#"(?i)(\bhttps?://[^\s?#"'<>]*)\?[^\s"'<>]*"#,
            "${1}?[redacted]",
        ),
        // A long base64 or base64url run.
        (r"[A-Za-z0-9+/_-]{80,}={0,2}", "[redacted base64]"),
    ]
    .into_iter()
    .map(|(pattern, replacement)| (Regex::new(pattern).expect("a valid pattern"), replacement))
    .collect()
});

/// Whitespace collapsed and trimmed, cut to `max` characters with a trailing `…` when cut.
pub fn bounded(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_replaced_longest_first() {
        let redactor = Redactor::new(vec![Secret::new("abc"), Secret::new("abcdef")]);
        assert_eq!(
            redactor.redact("key abcdef and abc"),
            "key [redacted] and [redacted]"
        );
        let with_url = redactor.with("https://cdn.example.test/x?sig=1");
        assert_eq!(
            with_url.reason("fetching https://cdn.example.test/x?sig=1 failed"),
            "fetching [redacted] failed"
        );
    }

    #[test]
    fn reasons_are_collapsed_and_bounded() {
        assert_eq!(bounded("  a \n b  ", 10), "a b");
        let long = "x".repeat(600);
        let cut = bounded(&long, MAX_REASON_CHARS);
        assert_eq!(cut.chars().count(), MAX_REASON_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn empty_secrets_are_ignored() {
        let redactor = Redactor::new(vec![Secret::new("")]);
        assert_eq!(redactor.redact("text"), "text");
    }

    fn redact(text: &str) -> String {
        Redactor::default().redact(text)
    }

    #[test]
    fn data_url_payloads_are_redacted_keeping_their_type() {
        assert_eq!(
            redact("bad picture data:image/png;base64,iVBORw0KGgo= here"),
            "bad picture data:image/png;base64,[redacted] here"
        );
        assert_eq!(
            redact("DATA:Audio/MPEG;charset=x;base64,SUQzBAA+/_- end"),
            "data:Audio/MPEG;base64,[redacted] end"
        );
        assert_eq!(
            redact(r#"{"url":"data:image/webp;base64,UklGR\/abc"}"#),
            r#"{"url":"data:image/webp;base64,[redacted]"}"#
        );
    }

    #[test]
    fn base64_members_are_redacted() {
        assert_eq!(
            redact(r#"{"b64_json": "iVBORw0KGgo=", "n": 1}"#),
            r#"{"b64_json": "[redacted]", "n": 1}"#
        );
        assert_eq!(
            redact("audio_data='SUQz' image_data=\"abc\" BASE64:'Zm9v'"),
            "audio_data='[redacted]' image_data=\"[redacted]\" BASE64:'[redacted]'"
        );
    }

    #[test]
    fn authorization_values_are_redacted() {
        assert_eq!(
            redact("authorization: Bearer abc.def-123, next"),
            "authorization: [redacted], next"
        );
        assert_eq!(
            redact("Authorization=Key fal:1234"),
            "Authorization=[redacted]"
        );
        assert_eq!(
            redact(r#"{"Authorization": "Bearer abc"}"#),
            r#"{"Authorization": "[redacted]"}"#
        );
        assert_eq!(redact("xi-api-key: 0123abcd"), "xi-api-key: [redacted]");
        assert_eq!(
            redact("missing authorization header"),
            "missing authorization header"
        );
    }

    #[test]
    fn credential_members_are_redacted() {
        assert_eq!(
            redact(r#"{"api_key": "k1", "token": "t1", "secret": "s1", "credential": "c1"}"#),
            r#"{"api_key": "[redacted]", "token": "[redacted]", "secret": "[redacted]", "credential": "[redacted]"}"#
        );
        assert_eq!(
            redact("apiKey='abc' upload_token=\"u-1\" API-KEY: 'x'"),
            "apiKey='[redacted]' upload_token=\"[redacted]\" API-KEY: '[redacted]'"
        );
        // Allowlisted error fields are not members with quoted values.
        assert_eq!(
            redact("type=invalid_request_error; code=invalid_api_key; param=api_key"),
            "type=invalid_request_error; code=invalid_api_key; param=api_key"
        );
    }

    #[test]
    fn key_shapes_are_redacted() {
        assert_eq!(
            redact("used sk-proj-AbCdEf123456 and sk-or-v1-0123456789abcdef."),
            "used [redacted] and [redacted]."
        );
        assert_eq!(redact("sk-short ok"), "sk-short ok");
        assert_eq!(redact("task-abcdefghijk"), "task-abcdefghijk");
    }

    #[test]
    fn url_queries_and_userinfo_are_redacted() {
        assert_eq!(
            redact("fetching https://cdn.example.test/out/a.png?X-Amz-Signature=abc&e=1 failed"),
            "fetching https://cdn.example.test/out/a.png?[redacted] failed"
        );
        assert_eq!(
            redact(r#"{"url":"HTTP://h.test/a?sig=1#frag"}"#),
            r#"{"url":"HTTP://h.test/a?[redacted]"}"#
        );
        assert_eq!(
            redact("https://user:pw@h.test/a"),
            "https://[redacted]@h.test/a"
        );
        assert_eq!(
            redact("https://api.example.test/v1/images/generations"),
            "https://api.example.test/v1/images/generations"
        );
    }

    #[test]
    fn long_base64_runs_are_redacted() {
        let run = "QUJD".repeat(20);
        assert_eq!(
            redact(&format!("body {run}== tail")),
            "body [redacted base64] tail"
        );
        let url_safe = "a-b_".repeat(20);
        assert_eq!(redact(&format!("x {url_safe}")), "x [redacted base64]");
        let short = "QUJD".repeat(19);
        assert_eq!(redact(&format!("ok {short}")), format!("ok {short}"));
        let digest = "0123456789abcdef".repeat(4);
        assert_eq!(redact(&digest), digest, "a 64-character digest stays");
    }

    #[test]
    fn redaction_is_stable_and_reasons_hold_none_of_it() {
        let redactor = Redactor::new(vec![Secret::new("fal-key-private-value")]);
        let payload = "A".repeat(80);
        let text = format!(
            "authorization=Key fal-key-private-value api_key='fal-key-private-value' \
             data:image/png;base64,{payload} b64_json='{payload}'"
        );
        let once = redactor.reason(&text);
        assert!(!once.contains("fal-key-private-value"), "{once}");
        assert!(!once.contains(&payload), "{once}");
        assert_eq!(
            redactor.reason(&once),
            once,
            "redacting twice changes nothing"
        );
    }
}
