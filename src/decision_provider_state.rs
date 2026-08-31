//! Read-only local evidence for Decision Report provider/model reliability.
//!
//! The provider transport owns submissions. This module only summarizes what
//! was already persisted, and deliberately does not make a provider request,
//! expose provider content/errors, or influence report submission.

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

use crate::{db::value_i64, models::AiProviderCapabilityPayload, state::json_text};

#[derive(Default)]
struct CapabilityAccumulator {
    strict_schema_request_count: i64,
    response_healing_request_count: i64,
    fusion_plugin_request_count: i64,
    attempt_count: i64,
    completed_count: i64,
    failed_count: i64,
    schema_failure_count: i64,
    timeout_failure_count: i64,
    parse_failure_count: i64,
    observed_prompt_token_count: i64,
    observed_completion_token_count: i64,
    observed_cost_report_count: i64,
    observed_cost_usd: f64,
}

/// Produces a stable model matrix from bounded local history. Request metadata
/// is treated as observed runtime behavior, not as a claim that another model
/// or provider supports a feature today.
pub(crate) fn ai_provider_capabilities_from_rows(
    rows: Vec<JsonValue>,
    configured_timeout_seconds: i64,
) -> Vec<AiProviderCapabilityPayload> {
    let mut grouped = BTreeMap::<(String, String), CapabilityAccumulator>::new();

    for row in rows {
        let request = embedded_json(&row, "request_json");
        let response = embedded_json(&row, "response_json");
        let provider = provider_label(request.as_ref());
        let model = json_text(&row, "model");
        let key = (
            if provider.is_empty() {
                "unrecorded".to_string()
            } else {
                provider
            },
            if model.is_empty() {
                "unrecorded".to_string()
            } else {
                model
            },
        );
        let stats = grouped.entry(key).or_default();
        stats.attempt_count += 1;
        if request_uses_strict_schema(request.as_ref()) {
            stats.strict_schema_request_count += 1;
        }
        if request_has_plugin(request.as_ref(), "response-healing") {
            stats.response_healing_request_count += 1;
        }
        if request_has_plugin(request.as_ref(), "fusion") {
            stats.fusion_plugin_request_count += 1;
        }

        let error_category = local_failure_category(&json_text(&row, "error_text"));
        if json_text(&row, "status") == "completed" {
            stats.completed_count += 1;
        } else if !error_category.is_empty() {
            stats.failed_count += 1;
            match error_category {
                "schema" => stats.schema_failure_count += 1,
                "timeout" => stats.timeout_failure_count += 1,
                "parse" => stats.parse_failure_count += 1,
                _ => {}
            }
        }

        let Some(usage) = response.as_ref().and_then(|response| response.get("usage")) else {
            continue;
        };
        stats.observed_prompt_token_count +=
            non_negative_i64(usage, "prompt_tokens").max(non_negative_i64(usage, "input_tokens"));
        stats.observed_completion_token_count += non_negative_i64(usage, "completion_tokens")
            .max(non_negative_i64(usage, "output_tokens"));
        if let Some(cost) = usage
            .get("cost")
            .and_then(JsonValue::as_f64)
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
        {
            stats.observed_cost_report_count += 1;
            stats.observed_cost_usd += cost;
        }
    }

    grouped
        .into_iter()
        .map(|((provider, model), stats)| {
            let terminal_count = stats.completed_count + stats.failed_count;
            AiProviderCapabilityPayload {
                provider,
                model,
                strict_schema_request_count: stats.strict_schema_request_count,
                response_healing_request_count: stats.response_healing_request_count,
                fusion_plugin_request_count: stats.fusion_plugin_request_count,
                configured_timeout_seconds: configured_timeout_seconds.max(0),
                attempt_count: stats.attempt_count,
                completed_count: stats.completed_count,
                failed_count: stats.failed_count,
                schema_failure_count: stats.schema_failure_count,
                timeout_failure_count: stats.timeout_failure_count,
                parse_failure_count: stats.parse_failure_count,
                completion_rate: (terminal_count > 0)
                    .then_some(stats.completed_count as f64 / terminal_count as f64),
                observed_prompt_token_count: stats.observed_prompt_token_count,
                observed_completion_token_count: stats.observed_completion_token_count,
                observed_cost_report_count: stats.observed_cost_report_count,
                observed_cost_usd: (stats.observed_cost_report_count > 0)
                    .then_some(stats.observed_cost_usd),
            }
        })
        .collect()
}

fn embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

fn provider_label(request: Option<&JsonValue>) -> String {
    let Some(request) = request else {
        return String::new();
    };
    if request_has_plugin(Some(request), "response-healing") {
        "openrouter".to_string()
    } else if request
        .get("deferred")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        "xai".to_string()
    } else {
        "unrecorded".to_string()
    }
}

fn request_uses_strict_schema(request: Option<&JsonValue>) -> bool {
    request
        .and_then(|request| request.get("response_format"))
        .and_then(|format| format.get("json_schema"))
        .and_then(|schema| schema.get("strict"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn request_has_plugin(request: Option<&JsonValue>, expected_id: &str) -> bool {
    request
        .and_then(|request| request.get("plugins"))
        .and_then(JsonValue::as_array)
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|plugin| plugin.get("id").and_then(JsonValue::as_str) == Some(expected_id))
        })
}

fn local_failure_category(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.is_empty() {
        ""
    } else if error.contains("invalid_json_schema") || error.contains("invalid schema") {
        "schema"
    } else if error.contains("timed out") || error.contains("timeout") {
        "timeout"
    } else if error.contains("could not be normalized") || error.contains("invalid json") {
        "parse"
    } else {
        "other"
    }
}

fn non_negative_i64(value: &JsonValue, key: &str) -> i64 {
    value_i64(value, key).max(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn capability_matrix_is_aggregate_and_never_serializes_provider_documents() {
        let rows = vec![
            json!({
                "model": "openai/gpt-5.5",
                "status": "completed",
                "request_json": {
                    "response_format": {"json_schema": {"strict": true}},
                    "plugins": [{"id": "response-healing"}]
                },
                "response_json": {
                    "usage": {"prompt_tokens": 120, "completion_tokens": 30, "cost": 0.045},
                    "choices": [{"message": {"content": "must-not-reach-matrix"}}]
                },
                "error_text": null
            }),
            json!({
                "model": "openrouter/fusion",
                "status": "error",
                "request_json": {
                    "response_format": {"json_schema": {"strict": true}},
                    "plugins": [{"id": "fusion"}, {"id": "response-healing"}]
                },
                "response_json": null,
                "error_text": "OpenRouter timed out: must-not-reach-matrix"
            }),
        ];

        let matrix = ai_provider_capabilities_from_rows(rows, 600);

        assert_eq!(matrix.len(), 2);
        assert_eq!(matrix[0].model, "openai/gpt-5.5");
        assert_eq!(matrix[0].provider, "openrouter");
        assert_eq!(matrix[0].strict_schema_request_count, 1);
        assert_eq!(matrix[0].response_healing_request_count, 1);
        assert_eq!(matrix[0].completed_count, 1);
        assert_eq!(matrix[0].completion_rate, Some(1.0));
        assert_eq!(matrix[0].observed_prompt_token_count, 120);
        assert_eq!(matrix[0].observed_completion_token_count, 30);
        assert_eq!(matrix[0].observed_cost_usd, Some(0.045));
        assert_eq!(matrix[1].model, "openrouter/fusion");
        assert_eq!(matrix[1].fusion_plugin_request_count, 1);
        assert_eq!(matrix[1].timeout_failure_count, 1);
        assert_eq!(matrix[1].completion_rate, Some(0.0));
        let serialized = serde_json::to_string(&matrix).expect("matrix serializes");
        assert!(!serialized.contains("must-not-reach-matrix"));
        assert!(!serialized.contains("error_text"));
        assert!(!serialized.contains("response_json"));
    }

    #[test]
    fn in_flight_reports_do_not_reduce_observed_completion_rate() {
        let matrix = ai_provider_capabilities_from_rows(
            vec![json!({
                "model": "openai/gpt-5.5",
                "status": "submitted",
                "request_json": {"plugins": [{"id": "response-healing"}]},
                "response_json": null,
                "error_text": null
            })],
            600,
        );

        assert_eq!(matrix[0].attempt_count, 1);
        assert_eq!(matrix[0].completion_rate, None);
    }
}
