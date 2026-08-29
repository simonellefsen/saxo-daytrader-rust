//! Provider HTTP boundary for Decision Reports.
//!
//! This module owns endpoint construction, timeout-bound HTTP requests, and
//! the small response envelope shared by the synchronous OpenRouter path and
//! xAI's deferred-completion path. It deliberately knows nothing about
//! scheduling, report persistence, Trading Manager gates, queues, or Saxo.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde_json::{Value as JsonValue, json};

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

    pub async fn submit_chat_completion(
        &self,
        api_key: &str,
        request: &JsonValue,
    ) -> Result<ProviderHttpResponse> {
        let response = self
            .http_client()?
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(api_key)
            .json(request)
            .send()
            .await
            .context("submitting AI decision report")?;
        Ok(Self::read_response(response, "failed to read AI provider response body").await)
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
        ProviderHttpResponse { status, body }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ChatCompletionRequest, DecisionProvider};

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
}
