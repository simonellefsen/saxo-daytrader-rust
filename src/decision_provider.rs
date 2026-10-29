//! Provider HTTP boundary for Decision Reports.
//!
//! This module owns endpoint construction, timeout-bound HTTP requests, and
//! the small response envelope shared by the synchronous OpenRouter path and
//! xAI's deferred-completion path. It deliberately knows nothing about
//! scheduling, report persistence, Trading Manager gates, queues, or Saxo.

use std::{collections::HashSet, time::Duration};

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde_json::{Value as JsonValue, json};
use tracing::warn;

use crate::decision_schema;

/// A provider refuses a request it has not started with 429 -- OpenRouter's
/// admission control could not confirm the account's credits in time, or the
/// account is over its rate -- and says how long to wait. Nothing was
/// generated or billed, so the same request is sent again after that wait. A
/// failed report uses up its pulse for the day, so without this a ten-second
/// wait cost a whole session's decisions (report 455, 2026-10-29).
const SUBMIT_MAX_ATTEMPTS: u32 = 3;
/// The wait when a 429 names none.
const SUBMIT_DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(10);
/// A longer wait is not a hiccup: the 429 stands and the report fails.
const SUBMIT_MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Provider-neutral request inputs. The provider owns only the transport
/// shape; report prompting, schema selection, and persistence remain outside
/// this module.
pub(crate) struct ChatCompletionRequest<'a> {
    pub model: &'a str,
    pub system_content: &'a str,
    pub user_content: &'a str,
    pub response_format: JsonValue,
    pub max_tokens: i64,
    pub reasoning_effort: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecisionProvider {
    name: String,
    base_url: String,
    timeout: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderHttpResponse {
    pub status: StatusCode,
    pub body: String,
    /// Requests sent to get this response, counting retried 429s.
    pub attempts: u32,
}

impl ProviderHttpResponse {
    pub fn is_accepted(&self) -> bool {
        self.status == StatusCode::ACCEPTED
    }
}

impl DecisionProvider {
    pub fn new(name: &str, base_url: &str, timeout_seconds: u64) -> Self {
        Self {
            name: name.trim().to_ascii_lowercase(),
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(timeout_seconds),
        }
    }

    #[cfg(test)]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_xai(&self) -> bool {
        self.name == "xai"
    }

    pub fn is_openrouter(&self) -> bool {
        self.name == "openrouter"
    }

    pub fn api_key_env_name(&self) -> &'static str {
        if self.is_openrouter() {
            "OPENROUTER_API_KEY"
        } else {
            "XAI_API_KEY"
        }
    }

    pub fn deferred_completion_url(&self, request_id: &str) -> String {
        format!("{}/chat/deferred-completion/{request_id}", self.base_url)
    }

    pub fn build_chat_completion_request(&self, request: ChatCompletionRequest<'_>) -> JsonValue {
        let mut body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": request.system_content},
                {"role": "user", "content": request.user_content}
            ],
            "response_format": request.response_format,
            "max_tokens": request.max_tokens,
        });
        if self.is_openrouter() {
            let mut plugins = vec![json!({"id": "response-healing"})];
            if request.model == "openrouter/fusion" {
                plugins.insert(0, json!({"id": "fusion", "preset": "general-high"}));
            }
            if let Some(object) = body.as_object_mut() {
                object.insert("plugins".to_string(), JsonValue::from(plugins));
            }
        }
        if let Some(reasoning_effort) = request.reasoning_effort {
            if let Some(object) = body.as_object_mut() {
                object.insert(
                    "reasoning_effort".to_string(),
                    JsonValue::from(reasoning_effort),
                );
            }
        }
        body
    }

    /// Submits one report, sending it again after a 429 the provider asks to
    /// wait out briefly. Any other status, a transport error, or a 429 asking
    /// for more than `SUBMIT_MAX_RETRY_AFTER` is returned at once: those
    /// requests may have started, and retrying them could bill twice.
    pub async fn submit_chat_completion(
        &self,
        api_key: &str,
        request: &JsonValue,
    ) -> Result<ProviderHttpResponse> {
        let client = self.http_client()?;
        let mut attempt = 1;
        loop {
            let response = client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(api_key)
                .json(request)
                .send()
                .await
                .context("submitting AI decision report")?;
            let header_wait = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| crate::jev::retry_after_seconds(Some(value), chrono::Utc::now()))
                .map(Duration::from_secs);
            let mut reply =
                Self::read_response(response, "failed to read AI provider response body").await;
            reply.attempts = attempt;
            if reply.status != StatusCode::TOO_MANY_REQUESTS || attempt >= SUBMIT_MAX_ATTEMPTS {
                return Ok(reply);
            }
            let wait = header_wait
                .or_else(|| body_retry_after(&reply.body))
                .unwrap_or(SUBMIT_DEFAULT_RETRY_AFTER);
            if wait > SUBMIT_MAX_RETRY_AFTER {
                return Ok(reply);
            }
            warn!(
                provider = %self.name,
                attempt,
                wait_seconds = wait.as_secs(),
                "AI provider refused the decision report with HTTP 429; sending it again"
            );
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    pub async fn poll_deferred_completion(
        &self,
        api_key: &str,
        request_id: &str,
    ) -> Result<ProviderHttpResponse> {
        let response = self
            .http_client()?
            .get(self.deferred_completion_url(request_id))
            .bearer_auth(api_key)
            .send()
            .await
            .context("polling xAI deferred decision report")?;
        Ok(Self::read_response(response, "failed to read xAI deferred response body").await)
    }

    fn http_client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .context("building AI provider HTTP client")
    }

    async fn read_response(
        response: reqwest::Response,
        body_error_context: &str,
    ) -> ProviderHttpResponse {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|err| format!("{body_error_context}: {err}"));
        ProviderHttpResponse {
            status,
            body,
            attempts: 1,
        }
    }
}

/// OpenRouter repeats the wait inside the error body
/// (`error.metadata.headers.Retry-After`), which survives a proxy that drops
/// the header.
fn body_retry_after(body: &str) -> Option<Duration> {
    let payload: JsonValue = serde_json::from_str(body).ok()?;
    let value = payload.pointer("/error/metadata/headers/Retry-After")?;
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .map(Duration::from_secs)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SchemaValidationIssue {
    pub path: String,
    pub message: String,
}

pub(crate) fn decision_report_response_format(provider: &str) -> JsonValue {
    if provider != "openrouter" {
        return json!({"type": "json_object"});
    }
    let schema = openrouter_strict_schema(decision_schema::decision_report_json_schema());
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "daytrader_decision_report",
            "strict": true,
            "schema": schema
        }
    })
}

pub(crate) fn openrouter_strict_schema(mut schema: JsonValue) -> JsonValue {
    enforce_openrouter_strict_schema(&mut schema);
    schema
}

fn enforce_openrouter_strict_schema(schema: &mut JsonValue) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };

    let is_object_schema = object.get("type").is_some_and(schema_type_includes_object);
    if is_object_schema {
        object.insert("additionalProperties".to_string(), JsonValue::from(false));

        if let Some(properties) = object.get("properties").and_then(JsonValue::as_object) {
            let mut required = object
                .get("required")
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default();
            for property in properties.keys() {
                if !required
                    .iter()
                    .any(|value| value.as_str() == Some(property.as_str()))
                {
                    required.push(JsonValue::from(property.clone()));
                }
            }
            object.insert("required".to_string(), JsonValue::from(required));
        }
    }

    if let Some(properties) = object
        .get_mut("properties")
        .and_then(JsonValue::as_object_mut)
    {
        for child in properties.values_mut() {
            enforce_openrouter_strict_schema(child);
        }
    }

    if let Some(items) = object.get_mut("items") {
        enforce_openrouter_strict_schema(items);
    }

    for branch_key in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = object.get_mut(branch_key).and_then(JsonValue::as_array_mut) {
            for child in branches {
                enforce_openrouter_strict_schema(child);
            }
        }
    }

    for definitions_key in ["$defs", "definitions"] {
        if let Some(definitions) = object
            .get_mut(definitions_key)
            .and_then(JsonValue::as_object_mut)
        {
            for child in definitions.values_mut() {
                enforce_openrouter_strict_schema(child);
            }
        }
    }
}

fn schema_type_includes_object(value: &JsonValue) -> bool {
    match value {
        JsonValue::String(text) => text == "object",
        JsonValue::Array(values) => values.iter().any(|item| item.as_str() == Some("object")),
        _ => false,
    }
}

pub(crate) fn validate_openrouter_strict_schema(schema: &JsonValue) -> Vec<SchemaValidationIssue> {
    let mut issues = Vec::new();
    validate_openrouter_strict_schema_at(schema, "schema", &mut issues);
    issues
}

fn validate_openrouter_strict_schema_at(
    schema: &JsonValue,
    path: &str,
    issues: &mut Vec<SchemaValidationIssue>,
) {
    if schema.get("type").is_some_and(schema_type_includes_object) {
        if schema.get("additionalProperties") != Some(&JsonValue::from(false)) {
            issues.push(SchemaValidationIssue {
                path: path.to_string(),
                message: "object schemas must set additionalProperties=false".to_string(),
            });
        }

        let Some(properties) = schema.get("properties").and_then(JsonValue::as_object) else {
            issues.push(SchemaValidationIssue {
                path: path.to_string(),
                message: "object schemas must define properties".to_string(),
            });
            return;
        };

        let Some(required) = schema.get("required").and_then(JsonValue::as_array) else {
            issues.push(SchemaValidationIssue {
                path: path.to_string(),
                message: "object schemas must list required properties".to_string(),
            });
            return;
        };

        let mut seen_required = HashSet::new();
        for value in required {
            let Some(required_name) = value.as_str() else {
                issues.push(SchemaValidationIssue {
                    path: path.to_string(),
                    message: "required entries must be strings".to_string(),
                });
                continue;
            };
            if !seen_required.insert(required_name.to_string()) {
                issues.push(SchemaValidationIssue {
                    path: path.to_string(),
                    message: format!("required property {required_name:?} is duplicated"),
                });
            }
            if !properties.contains_key(required_name) {
                issues.push(SchemaValidationIssue {
                    path: path.to_string(),
                    message: format!(
                        "required property {required_name:?} is not declared in properties"
                    ),
                });
            }
        }

        for property in properties.keys() {
            if !required
                .iter()
                .any(|value| value.as_str() == Some(property.as_str()))
            {
                issues.push(SchemaValidationIssue {
                    path: format!("{path}.{property}"),
                    message: "property must be listed in required for strict structured outputs"
                        .to_string(),
                });
            }
        }
    }

    if let Some(properties) = schema.get("properties").and_then(JsonValue::as_object) {
        for (name, child) in properties {
            validate_openrouter_strict_schema_at(child, &format!("{path}.{name}"), issues);
        }
    }

    if let Some(items) = schema.get("items") {
        validate_openrouter_strict_schema_at(items, &format!("{path}[]"), issues);
    }

    for branch_key in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = schema.get(branch_key).and_then(JsonValue::as_array) {
            for (index, child) in branches.iter().enumerate() {
                validate_openrouter_strict_schema_at(
                    child,
                    &format!("{path}.{branch_key}[{index}]"),
                    issues,
                );
            }
        }
    }

    for definitions_key in ["$defs", "definitions"] {
        if let Some(definitions) = schema.get(definitions_key).and_then(JsonValue::as_object) {
            for (name, child) in definitions {
                validate_openrouter_strict_schema_at(
                    child,
                    &format!("{path}.{definitions_key}.{name}"),
                    issues,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use reqwest::StatusCode;
    use serde_json::json;

    use super::{
        ChatCompletionRequest, DecisionProvider, body_retry_after, decision_report_response_format,
        validate_openrouter_strict_schema,
    };

    /// The 429 that cost the US open report on 2026-10-29 (report 455).
    const ADMISSION_CONTROL_429: &str = r#"{"error":{"message":"OpenRouter could not verify available credits for this request in time. Retry shortly.","code":429,"metadata":{"limit_source":"openrouter_admission_control","remedy_hint":"Slow down the request rate for this account, then retry after the reset (see X-RateLimit-Reset).","headers":{"Retry-After":"10"},"provider_name":null}}}"#;

    type Reply = (u16, Option<&'static str>, String);

    /// Serves `replies` on `/chat/completions` in order, then 500, on loopback.
    async fn scripted_provider(replies: Vec<Reply>) -> (DecisionProvider, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let base_url = format!("http://{}", listener.local_addr().expect("an address"));
        let hits = Arc::new(AtomicUsize::new(0));
        let script = Arc::new(Mutex::new(std::collections::VecDeque::from(replies)));
        let counter = hits.clone();
        let app = axum::Router::new().route(
            "/chat/completions",
            axum::routing::post(move || {
                let counter = counter.clone();
                let script = script.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let (status, retry_after, body) = script
                        .lock()
                        .expect("the script")
                        .pop_front()
                        .unwrap_or((500, None, "unscripted".to_string()));
                    let mut headers = axum::http::HeaderMap::new();
                    if let Some(value) = retry_after {
                        headers.insert(
                            axum::http::header::RETRY_AFTER,
                            axum::http::HeaderValue::from_static(value),
                        );
                    }
                    (
                        axum::http::StatusCode::from_u16(status).expect("a status"),
                        headers,
                        body,
                    )
                }
            }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (DecisionProvider::new("openrouter", &base_url, 5), hits)
    }

    async fn submit(provider: &DecisionProvider) -> super::ProviderHttpResponse {
        provider
            .submit_chat_completion("test-key", &json!({"model": "test"}))
            .await
            .expect("a response")
    }

    #[test]
    fn the_wait_is_read_from_openrouters_error_body() {
        assert_eq!(
            body_retry_after(ADMISSION_CONTROL_429),
            Some(Duration::from_secs(10))
        );
        assert_eq!(body_retry_after(r#"{"error":{"code":429}}"#), None);
        assert_eq!(body_retry_after("not json"), None);
    }

    #[tokio::test]
    async fn a_429_is_sent_again_after_the_wait_it_asks_for() {
        let (provider, hits) = scripted_provider(vec![
            (429, Some("0"), ADMISSION_CONTROL_429.to_string()),
            (200, None, r#"{"id":"gen-1"}"#.to_string()),
        ])
        .await;
        let reply = submit(&provider).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.attempts, 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// Without the header, the body's wait is used. A zero keeps the test fast.
    #[tokio::test]
    async fn a_429_without_the_header_waits_as_its_body_says() {
        let body = ADMISSION_CONTROL_429.replace(r#""Retry-After":"10""#, r#""Retry-After":"0""#);
        let (provider, hits) = scripted_provider(vec![
            (429, None, body),
            (200, None, r#"{"id":"gen-1"}"#.to_string()),
        ])
        .await;
        assert_eq!(submit(&provider).await.attempts, 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retries_stop_at_three_requests() {
        let (provider, hits) = scripted_provider(vec![
            (429, Some("0"), ADMISSION_CONTROL_429.to_string()),
            (429, Some("0"), ADMISSION_CONTROL_429.to_string()),
            (429, Some("0"), ADMISSION_CONTROL_429.to_string()),
            (200, None, r#"{"id":"gen-1"}"#.to_string()),
        ])
        .await;
        let reply = submit(&provider).await;
        assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(reply.attempts, 3);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    /// A long wait is an outage, not a hiccup, and any other error may come
    /// from a request that started and was billed: neither is sent again.
    #[tokio::test]
    async fn a_long_wait_or_another_error_is_not_sent_again() {
        let (provider, hits) =
            scripted_provider(vec![(429, Some("120"), ADMISSION_CONTROL_429.to_string())]).await;
        let reply = submit(&provider).await;
        assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(reply.attempts, 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        for status in [500, 502, 402] {
            let (provider, hits) =
                scripted_provider(vec![(status, Some("0"), "{}".to_string())]).await;
            let reply = submit(&provider).await;
            assert_eq!(reply.status.as_u16(), status);
            assert_eq!(reply.attempts, 1);
            assert_eq!(hits.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn provider_preserves_sync_and_deferred_contracts() {
        let openrouter = DecisionProvider::new(" OpenRouter ", "https://openrouter.ai/api/v1/", 30);
        assert_eq!(openrouter.name(), "openrouter");
        assert!(openrouter.is_openrouter());
        assert!(!openrouter.is_xai());
        assert_eq!(openrouter.api_key_env_name(), "OPENROUTER_API_KEY");

        let xai = DecisionProvider::new("xai", "https://api.x.ai/v1/", 30);
        assert!(xai.is_xai());
        assert_eq!(xai.api_key_env_name(), "XAI_API_KEY");
        assert_eq!(
            xai.deferred_completion_url("request-123"),
            "https://api.x.ai/v1/chat/deferred-completion/request-123"
        );
    }

    #[test]
    fn openrouter_request_keeps_plugins_and_shared_fields() {
        let provider = DecisionProvider::new("openrouter", "https://openrouter.ai/api/v1", 30);
        let request = provider.build_chat_completion_request(ChatCompletionRequest {
            model: "openrouter/fusion",
            system_content: "strict JSON",
            user_content: "{\"portfolio\":[]}",
            response_format: json!({"type": "json_schema"}),
            max_tokens: 8192,
            reasoning_effort: Some("high"),
        });

        assert_eq!(request["messages"][0]["content"], "strict JSON");
        assert_eq!(request["messages"][1]["content"], "{\"portfolio\":[]}");
        assert_eq!(request["response_format"]["type"], "json_schema");
        assert_eq!(request["plugins"][0]["id"], "fusion");
        assert_eq!(request["plugins"][1]["id"], "response-healing");
        assert_eq!(request["reasoning_effort"], "high");
    }

    #[test]
    fn xai_request_omits_openrouter_only_plugins() {
        let provider = DecisionProvider::new("xai", "https://api.x.ai/v1", 30);
        let request = provider.build_chat_completion_request(ChatCompletionRequest {
            model: "grok-4",
            system_content: "strict JSON",
            user_content: "{}",
            response_format: json!({"type": "json_object"}),
            max_tokens: 4096,
            reasoning_effort: None,
        });

        assert_eq!(request["model"], "grok-4");
        assert_eq!(request["max_tokens"], 4096);
        assert!(request.get("plugins").is_none());
        assert!(request.get("reasoning_effort").is_none());
    }

    #[test]
    fn openrouter_response_format_uses_a_valid_strict_schema() {
        let response_format = decision_report_response_format("openrouter");
        let schema = &response_format["json_schema"]["schema"];

        assert_eq!(response_format["type"], "json_schema");
        assert_eq!(response_format["json_schema"]["strict"], true);
        assert!(validate_openrouter_strict_schema(schema).is_empty());
    }
}
