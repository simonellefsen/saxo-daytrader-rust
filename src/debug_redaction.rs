use serde_json::{Map, Value as JsonValue};

pub const DEBUG_PAYLOAD_MAX_CHARS: usize = 4_000;

pub fn compact_json_redacted(value: Option<&JsonValue>, max_len: usize) -> String {
    let Some(value) = value else {
        return "No payload available.".to_string();
    };
    let redacted = redact_debug_json(value);
    let rendered = serde_json::to_string_pretty(&redacted).unwrap_or_else(|_| redacted.to_string());
    compact_debug_text(&rendered, max_len)
}

pub fn compact_debug_text(value: &str, max_len: usize) -> String {
    let redacted = redact_debug_text(value);
    if redacted.chars().count() > max_len {
        format!("{}...", redacted.chars().take(max_len).collect::<String>())
    } else if redacted.trim().is_empty() {
        "No payload available.".to_string()
    } else {
        redacted
    }
}

pub fn redact_debug_json(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(object) => JsonValue::Object(
            object
                .iter()
                .map(|(key, value)| {
                    if is_sensitive_debug_key(key) {
                        (key.clone(), JsonValue::String("[redacted]".to_string()))
                    } else {
                        (key.clone(), redact_debug_json(value))
                    }
                })
                .collect::<Map<String, JsonValue>>(),
        ),
        JsonValue::Array(values) => {
            JsonValue::Array(values.iter().map(redact_debug_json).collect())
        }
        JsonValue::String(value) => JsonValue::String(redact_debug_text(value)),
        _ => value.clone(),
    }
}

fn is_sensitive_debug_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "apikey",
        "authorization",
        "bearer",
        "token",
        "refresh",
        "secret",
        "password",
        "accountkey",
        "clientkey",
        "databaseurl",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

pub fn redact_debug_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let trimmed = word.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']' | '{' | '}'
                )
            });
            if looks_like_secret_token(trimmed) {
                word.replace(trimmed, "[redacted]")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_secret_token(value: &str) -> bool {
    if value.starts_with("sk-") || value.starts_with("Bearer") {
        return true;
    }
    value.len() >= 32
        && value.chars().any(char::is_alphabetic)
        && value.chars().any(char::is_numeric)
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
}

/// A request error without the URL it was sent to.
///
/// reqwest prints the whole URL in a transport error ("error sending request
/// for url (...)"), query string included. Saxo URLs carry the account key in
/// the query and the client key in some paths, and a Slack webhook URL is the
/// credential itself. An unstripped error reaches the logs, stored error
/// columns and the run summaries the dashboard shows, so every request whose
/// URL holds a secret maps its send error through this.
pub(crate) fn request_error_without_url(err: reqwest::Error) -> reqwest::Error {
    err.without_url()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn redacts_sensitive_fields_and_token_like_text() {
        let payload = json!({
            "model": "openrouter/fusion",
            "api_key": "sk-test-123456789012345678901234567890",
            "nested": {
                "refresh_token": "refresh-123456789012345678901234567890",
                "content": "Use Bearer abcdef1234567890abcdef1234567890abcd"
            }
        });

        let rendered = compact_json_redacted(Some(&payload), 8_000);
        assert!(rendered.contains("openrouter/fusion"));
        assert!(rendered.contains("\"api_key\": \"[redacted]\""));
        assert!(rendered.contains("\"refresh_token\": \"[redacted]\""));
        assert!(!rendered.contains("sk-test-123456789012345678901234567890"));
        assert!(!rendered.contains("abcdef1234567890abcdef1234567890abcd"));
    }

    #[test]
    fn caps_after_redaction_without_returning_an_empty_value() {
        let rendered = compact_debug_text(&"x".repeat(80), 20);
        assert_eq!(rendered, format!("{}...", "x".repeat(20)));
        assert_eq!(compact_debug_text("  ", 20), "No payload available.");
        assert_eq!(compact_debug_text("ab😀cd", 3), "ab😀...");
    }

    #[test]
    fn redacts_hyphenated_credential_keys() {
        let payload = json!({"x-api-key": "secret-value", "access-token": "also-secret"});
        let rendered = compact_json_redacted(Some(&payload), 8_000);
        assert!(rendered.contains("\"x-api-key\": \"[redacted]\""));
        assert!(rendered.contains("\"access-token\": \"[redacted]\""));
    }

    /// The leak is real, and the helper removes it from every rendering an
    /// error reaches: the alternate chain logs and stored columns use, and the
    /// debug form.
    #[tokio::test]
    async fn a_request_error_loses_the_url_that_carries_the_keys() {
        let url = "http://127.0.0.1:9/trade/v1/infoprices?AccountKey=SECRET-ACCOUNT-KEY";
        let client = reqwest::Client::new();
        let raw = client
            .get(url)
            .send()
            .await
            .expect_err("nothing listens on port 9");
        assert!(
            format!("{raw:#}").contains("SECRET-ACCOUNT-KEY"),
            "an unstripped reqwest error prints the URL; this is the leak"
        );

        let stripped = request_error_without_url(
            client
                .get(url)
                .send()
                .await
                .expect_err("nothing listens on port 9"),
        );
        let wrapped = anyhow::Error::new(stripped).context("sending Saxo GET");
        for rendered in [
            format!("{wrapped:#}"),
            format!("{wrapped:?}"),
            format!("{wrapped}"),
        ] {
            assert!(!rendered.contains("SECRET-ACCOUNT-KEY"), "{rendered}");
            assert!(!rendered.contains("AccountKey="), "{rendered}");
        }
    }

    /// Every request in these files has a secret in its URL, so every send
    /// must strip the URL from its error. A new call that forgets fails here.
    #[test]
    fn every_request_with_a_secret_in_its_url_strips_it_from_errors() {
        let files = [
            ("markov_method.rs", include_str!("markov_method.rs")),
            ("saxo_order.rs", include_str!("saxo_order.rs")),
            ("saxo_portfolio.rs", include_str!("saxo_portfolio.rs")),
            ("state.rs", include_str!("state.rs")),
            ("notifications.rs", include_str!("notifications.rs")),
        ];
        let mut sends = 0;
        for (file, source) in files {
            let lines: Vec<&str> = source.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                if !line.contains(".send()") || line.trim_start().starts_with("//") {
                    continue;
                }
                sends += 1;
                let window = lines[index..(index + 3).min(lines.len())].join(" ");
                assert!(
                    window.contains("request_error_without_url"),
                    "{file}:{} sends a request without stripping the URL from its error",
                    index + 1
                );
            }
        }
        assert_eq!(sends, 8, "the guard must still see every known send");
    }
}
