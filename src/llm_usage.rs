//! Per-request token and cost ledger for the LLM calls this runtime makes.
//!
//! The provider capability matrix in `decision_provider_state.rs` answers "is
//! this model working" by folding history into one row per provider/model.
//! That hides the question this module exists for: what did each individual
//! request cost, and is the trend moving. A model swap changes token appetite
//! and price at the same time, and an aggregate cannot separate "we changed
//! models" from "we changed prompts" because both move the same average.
//!
//! Derived from `decision_reports` rather than written to a ledger table of
//! its own. The provider response is already persisted per request with a
//! timestamp, so deriving keeps one source of truth, makes the whole history
//! available the day the reader ships, and removes any way for a ledger to
//! disagree with what actually ran.
//!
//! Scope: this covers the Decision Report path, which is the only place this
//! codebase calls an LLM. Hermes runs its own inference inside its container
//! and bills through a gateway this process never sees, so its tokens are
//! absent here by construction rather than by omission.

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

use crate::models::{LlmRequestUsagePayload, LlmUsageDayPayload, LlmUsageLedgerPayload};

/// Where a request's cost figure came from.
///
/// OpenRouter reports `usage.cost` as the amount *it* billed, which is `0` for
/// a BYOK key because the inference was charged to the operator's own upstream
/// account instead. Reading only `cost` would therefore report a free fleet the
/// moment a key is swapped for a BYOK one -- a silent zero, not an error. The
/// upstream figure is the fallback, and the source is carried so a reader can
/// tell a genuinely free request from an unbilled one.
const COST_SOURCE_BILLED: &str = "billed";
const COST_SOURCE_UPSTREAM: &str = "upstream_byok";
const COST_SOURCE_NONE: &str = "not_reported";

pub(crate) fn llm_usage_ledger_from_rows(
    rows: Vec<JsonValue>,
    day_limit: usize,
) -> LlmUsageLedgerPayload {
    let requests: Vec<LlmRequestUsagePayload> = rows
        .into_iter()
        .filter_map(request_usage_from_row)
        .collect();
    let days = daily_rollup(&requests, day_limit);
    LlmUsageLedgerPayload {
        request_count: requests.len() as i64,
        prompt_token_count: requests.iter().map(|row| row.prompt_tokens).sum(),
        completion_token_count: requests.iter().map(|row| row.completion_tokens).sum(),
        reasoning_token_count: requests.iter().map(|row| row.reasoning_tokens).sum(),
        cost_usd: sum_cost(requests.iter().map(|row| row.cost_usd)),
        requests,
        days,
    }
}

fn request_usage_from_row(row: JsonValue) -> Option<LlmRequestUsagePayload> {
    let response = parse_embedded_json(row.get("response_json"))?;
    let usage = response.get("usage")?;
    let request = parse_embedded_json(row.get("request_json"));

    let prompt_tokens = token_count(usage, "prompt_tokens", "input_tokens");
    let completion_tokens = token_count(usage, "completion_tokens", "output_tokens");
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(JsonValue::as_i64)
        .filter(|value| *value >= 0)
        .unwrap_or(0);

    let (cost_usd, cost_source) = cost_from_usage(usage);

    // The requested ceiling is what makes a completion count legible: 5,015
    // tokens is comfortable against 16,384 and one bad day away from truncation
    // against 8,192. Reading it from the stored request keeps the pairing
    // honest across a config change rather than assuming today's value.
    let max_tokens_requested = request
        .as_ref()
        .and_then(|request| request.get("max_tokens"))
        .and_then(JsonValue::as_i64)
        .filter(|value| *value > 0);

    Some(LlmRequestUsagePayload {
        created_at: string_field(&row, "created_at"),
        model: response
            .get("model")
            .and_then(JsonValue::as_str)
            .map(str::to_string)
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| string_field(&row, "model")),
        status: string_field(&row, "status"),
        prompt_tokens,
        completion_tokens,
        reasoning_tokens,
        max_tokens_requested,
        completion_budget_used_pct: max_tokens_requested
            .filter(|ceiling| *ceiling > 0)
            .map(|ceiling| completion_tokens as f64 / ceiling as f64),
        // Never read anywhere else in this codebase, which is why a budget
        // overrun currently surfaces as a JSON parse failure instead of as
        // truncation. Carrying it here makes the distinction visible.
        finish_reason: response
            .get("choices")
            .and_then(JsonValue::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(JsonValue::as_str)
            .map(str::to_string),
        cost_usd,
        cost_source: cost_source.to_string(),
    })
}

/// Shared with the provider capability matrix so the two panels can never
/// disagree about what a request cost -- the aggregate read only `usage.cost`
/// and would have reported a free fleet under the BYOK key now in use.
pub(crate) fn cost_from_usage(usage: &JsonValue) -> (Option<f64>, &'static str) {
    let billed = finite_non_negative(usage.get("cost"));
    if let Some(billed) = billed.filter(|cost| *cost > 0.0) {
        return (Some(billed), COST_SOURCE_BILLED);
    }
    let upstream = finite_non_negative(
        usage
            .get("cost_details")
            .and_then(|details| details.get("upstream_inference_cost")),
    );
    if let Some(upstream) = upstream.filter(|cost| *cost > 0.0) {
        return (Some(upstream), COST_SOURCE_UPSTREAM);
    }
    // A reported zero is a real observation and stays one; a missing field is
    // not the same claim and must not be summed as if it were free.
    match billed {
        Some(zero) => (Some(zero), COST_SOURCE_BILLED),
        None => (None, COST_SOURCE_NONE),
    }
}

fn daily_rollup(requests: &[LlmRequestUsagePayload], day_limit: usize) -> Vec<LlmUsageDayPayload> {
    let mut grouped: BTreeMap<String, LlmUsageDayPayload> = BTreeMap::new();
    for request in requests {
        let day = request.created_at.get(..10).unwrap_or_default().to_string();
        if day.is_empty() {
            continue;
        }
        let entry = grouped
            .entry(day.clone())
            .or_insert_with(|| LlmUsageDayPayload {
                day,
                request_count: 0,
                prompt_token_count: 0,
                completion_token_count: 0,
                reasoning_token_count: 0,
                cost_usd: None,
                models: Vec::new(),
            });
        entry.request_count += 1;
        entry.prompt_token_count += request.prompt_tokens;
        entry.completion_token_count += request.completion_tokens;
        entry.reasoning_token_count += request.reasoning_tokens;
        if let Some(cost) = request.cost_usd {
            entry.cost_usd = Some(entry.cost_usd.unwrap_or(0.0) + cost);
        }
        if !request.model.is_empty() && !entry.models.contains(&request.model) {
            entry.models.push(request.model.clone());
        }
    }
    let mut days: Vec<LlmUsageDayPayload> = grouped.into_values().collect();
    days.sort_by(|left, right| right.day.cmp(&left.day));
    days.truncate(day_limit);
    days
}

fn sum_cost(costs: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let mut total = None;
    for cost in costs.flatten() {
        total = Some(total.unwrap_or(0.0) + cost);
    }
    total
}

fn token_count(usage: &JsonValue, primary: &str, alternate: &str) -> i64 {
    let read = |key: &str| {
        usage
            .get(key)
            .and_then(JsonValue::as_i64)
            .filter(|value| *value >= 0)
            .unwrap_or(0)
    };
    read(primary).max(read(alternate))
}

fn finite_non_negative(value: Option<&JsonValue>) -> Option<f64> {
    value
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn string_field(row: &JsonValue, key: &str) -> String {
    row.get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Stored provider documents are TEXT columns holding JSON, so a row arrives
/// as a string that still has to be parsed. Tolerates an already-decoded
/// object so the same reader works against either shape.
fn parse_embedded_json(value: Option<&JsonValue>) -> Option<JsonValue> {
    match value {
        Some(JsonValue::String(text)) => serde_json::from_str(text).ok(),
        Some(JsonValue::Object(_)) => value.cloned(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(created_at: &str, model: &str, usage: JsonValue, max_tokens: i64) -> JsonValue {
        json!({
            "created_at": created_at,
            "model": model,
            "status": "completed",
            "request_json": json!({"model": model, "max_tokens": max_tokens}).to_string(),
            "response_json": json!({
                "model": model,
                "choices": [{"finish_reason": "stop"}],
                "usage": usage
            })
            .to_string(),
        })
    }

    /// A BYOK key makes OpenRouter report `usage.cost` as 0 because it billed
    /// nothing -- the inference was charged to the operator's own upstream
    /// account. Reading only `cost` would show a free fleet the moment a key is
    /// swapped, which is a silent zero rather than an error, so the upstream
    /// figure is the fallback and the source travels with the number.
    #[test]
    fn a_byok_request_reports_its_upstream_cost_rather_than_a_free_one() {
        let ledger = llm_usage_ledger_from_rows(
            vec![row(
                "2026-09-04T09:00:00Z",
                "~google/gemini-flash-latest",
                json!({
                    "prompt_tokens": 184_918,
                    "completion_tokens": 5_015,
                    "completion_tokens_details": {"reasoning_tokens": 2_926},
                    "cost": 0,
                    "is_byok": true,
                    "cost_details": {"upstream_inference_cost": 0.3149895}
                }),
                16_384,
            )],
            30,
        );

        let request = &ledger.requests[0];
        assert_eq!(request.cost_usd, Some(0.3149895));
        assert_eq!(request.cost_source, "upstream_byok");
        assert_eq!(request.reasoning_tokens, 2_926);
        assert_eq!(request.max_tokens_requested, Some(16_384));
    }

    /// A cost the provider never reported is not a cost of zero. Summing the
    /// two together would quietly understate the bill, so an unreported cost
    /// leaves the total absent rather than contributing nothing to it.
    #[test]
    fn an_unreported_cost_does_not_read_as_free() {
        let ledger = llm_usage_ledger_from_rows(
            vec![
                row(
                    "2026-09-04T09:00:00Z",
                    "model-a",
                    json!({"prompt_tokens": 10, "completion_tokens": 5}),
                    8_192,
                ),
                row(
                    "2026-09-04T10:00:00Z",
                    "model-b",
                    json!({"prompt_tokens": 20, "completion_tokens": 6, "cost": 0.25}),
                    8_192,
                ),
            ],
            30,
        );

        assert_eq!(ledger.requests[0].cost_usd, None);
        assert_eq!(ledger.requests[0].cost_source, "not_reported");
        assert_eq!(
            ledger.cost_usd,
            Some(0.25),
            "only the observed cost is summed"
        );
        assert_eq!(ledger.prompt_token_count, 30);
    }

    /// The budget share is the column that makes a completion count legible:
    /// 5,015 tokens is comfortable against 16,384 and one busy day from
    /// truncation against 8,192. The same request must therefore read
    /// differently depending on the ceiling it actually ran under.
    #[test]
    fn the_output_budget_share_is_measured_against_the_ceiling_that_ran() {
        let usage = json!({"prompt_tokens": 100, "completion_tokens": 5_015});
        let tight = llm_usage_ledger_from_rows(
            vec![row("2026-09-04T09:00:00Z", "m", usage.clone(), 8_192)],
            30,
        );
        let roomy =
            llm_usage_ledger_from_rows(vec![row("2026-09-04T09:00:00Z", "m", usage, 16_384)], 30);

        let tight_share = tight.requests[0].completion_budget_used_pct.expect("share");
        let roomy_share = roomy.requests[0].completion_budget_used_pct.expect("share");
        assert!((tight_share - 0.612).abs() < 0.001, "{tight_share}");
        assert!((roomy_share - 0.306).abs() < 0.001, "{roomy_share}");
    }

    /// The rollup is what turns a list of requests into a trend, so days group
    /// newest first and a day carries every model that ran in it -- a model
    /// swap mid-day must stay visible rather than being flattened to one name.
    #[test]
    fn days_roll_up_newest_first_and_keep_every_model_that_ran() {
        let ledger = llm_usage_ledger_from_rows(
            vec![
                row(
                    "2026-09-03T18:19:29Z",
                    "openai/gpt-5.6-terra",
                    json!({"prompt_tokens": 126_004, "completion_tokens": 3_674, "cost": 0.359}),
                    8_192,
                ),
                row(
                    "2026-09-04T09:00:00Z",
                    "~google/gemini-flash-latest",
                    json!({"prompt_tokens": 184_918, "completion_tokens": 5_015, "cost": 0.3}),
                    16_384,
                ),
                row(
                    "2026-09-04T14:00:00Z",
                    "openai/gpt-5.6-terra",
                    json!({"prompt_tokens": 1_000, "completion_tokens": 100, "cost": 0.01}),
                    16_384,
                ),
            ],
            30,
        );

        assert_eq!(ledger.days.len(), 2);
        assert_eq!(ledger.days[0].day, "2026-09-04");
        assert_eq!(ledger.days[0].request_count, 2);
        assert_eq!(ledger.days[0].prompt_token_count, 185_918);
        assert_eq!(
            ledger.days[0].models,
            vec![
                "~google/gemini-flash-latest".to_string(),
                "openai/gpt-5.6-terra".to_string()
            ],
            "a mid-day model swap stays visible in the rollup"
        );
        assert_eq!(ledger.days[1].day, "2026-09-03");
    }

    /// A response that stopped because it ran out of output budget is
    /// truncated, not finished. Nothing else in this codebase reads
    /// `finish_reason`, so an overrun would otherwise reach the operator as an
    /// unexplained JSON parse failure.
    #[test]
    fn a_truncated_response_carries_the_finish_reason_that_says_so() {
        let mut truncated = row(
            "2026-09-04T09:00:00Z",
            "m",
            json!({"prompt_tokens": 10, "completion_tokens": 8_192}),
            8_192,
        );
        truncated["response_json"] = JsonValue::from(
            json!({
                "model": "m",
                "choices": [{"finish_reason": "length"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 8_192}
            })
            .to_string(),
        );

        let ledger = llm_usage_ledger_from_rows(vec![truncated], 30);
        assert_eq!(ledger.requests[0].finish_reason.as_deref(), Some("length"));
    }
}
