//! Server-owned, observational quality evidence for completed Decision Reports.
//!
//! The provider may suggest trades, but it cannot write this audit. The
//! completion boundary derives it after scope filtering and canonical strategy
//! plan construction. It is deliberately observational: Trading Manager and
//! Saxo continue to apply their existing fail-closed admission gates.

use serde_json::{Value as JsonValue, json};

pub(crate) fn completion_quality_audit(
    report: &JsonValue,
    requested_capital_plan: Option<&JsonValue>,
    decision_time_context: Option<&JsonValue>,
) -> JsonValue {
    let mut checks = Vec::new();
    let report_object = report.is_object();
    push_check(
        &mut checks,
        "normalized_report",
        report_object,
        "The completion payload is a normalized JSON object.",
        "The completion payload is missing or is not a JSON object.",
    );

    let required_sections = [
        "market_view",
        "capital_plan",
        "selected_assets",
        "symbol_sentiment",
        "suggested_trades",
    ];
    let missing_sections = required_sections
        .iter()
        .filter(|section| report.get(**section).is_none())
        .copied()
        .collect::<Vec<_>>();
    push_check(
        &mut checks,
        "required_sections",
        missing_sections.is_empty(),
        "All core Decision Report sections are present.",
        &format!(
            "Missing normalized section(s): {}.",
            missing_sections.join(", ")
        ),
    );

    let scope_status = report
        .get("market_scope_enforcement")
        .and_then(|scope| scope.get("status"))
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    push_check(
        &mut checks,
        "market_scope",
        matches!(scope_status, "not_required" | "enforced"),
        "Server-owned market-scope metadata is present.",
        "Market-scope enforcement metadata is missing or incomplete.",
    );

    let suggested_trades = report
        .get("suggested_trades")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let malformed_trade_count = suggested_trades
        .iter()
        .filter(|trade| !trade_shape_ok(trade))
        .count();
    push_check(
        &mut checks,
        "candidate_shape",
        malformed_trade_count == 0,
        &format!(
            "{} suggested trade candidate(s) have a complete basic order shape.",
            suggested_trades.len()
        ),
        &format!(
            "{malformed_trade_count} suggested trade candidate(s) have an incomplete order shape."
        ),
    );

    let missing_evidence_count = suggested_trades
        .iter()
        .filter(|trade| !candidate_has_required_evidence(trade))
        .count();
    push_check(
        &mut checks,
        "candidate_evidence",
        missing_evidence_count == 0,
        "Every suggested candidate has technical and Markov metadata.",
        &format!(
            "{missing_evidence_count} suggested candidate(s) are missing required technical or Markov metadata."
        ),
    );

    let daily_indicator_context = decision_time_context
        .and_then(|context| context.get("daily_indicators"))
        .filter(|context| context.is_object());
    let indicator_run_ok = daily_indicator_context
        .and_then(|context| context.get("latest_run"))
        .and_then(|run| run.get("status"))
        .and_then(JsonValue::as_str)
        == Some("ok");
    push_check(
        &mut checks,
        "daily_indicator_run",
        indicator_run_ok,
        "A completed daily-indicator run was available at decision time.",
        "No completed daily-indicator run was available in the persisted decision-time context.",
    );

    let indicator_signals = daily_indicator_context
        .and_then(|context| context.get("signals"))
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let missing_indicator_count = suggested_trades
        .iter()
        .filter(|trade| decision_time_indicator_signal(trade, &indicator_signals).is_none())
        .count();
    push_check(
        &mut checks,
        "candidate_daily_indicator_evidence",
        missing_indicator_count == 0,
        "Every suggested candidate has a matching daily-indicator snapshot from decision time.",
        &format!(
            "{missing_indicator_count} suggested candidate(s) have no matching daily-indicator snapshot from decision time."
        ),
    );

    let missing_instrument_count = suggested_trades
        .iter()
        .filter(|trade| {
            !decision_time_indicator_signal(trade, &indicator_signals)
                .is_some_and(indicator_signal_has_instrument_resolution)
        })
        .count();
    push_check(
        &mut checks,
        "candidate_instrument_resolution",
        missing_instrument_count == 0,
        "Every suggested candidate has a resolved Saxo instrument snapshot from decision time.",
        &format!(
            "{missing_instrument_count} suggested candidate(s) are missing resolved Saxo instrument evidence."
        ),
    );

    let missing_currency_count = suggested_trades
        .iter()
        .filter(|trade| {
            !decision_time_indicator_signal(trade, &indicator_signals)
                .is_some_and(indicator_signal_has_currency_context)
        })
        .count();
    push_check(
        &mut checks,
        "candidate_currency_context",
        missing_currency_count == 0,
        "Every suggested candidate has trading-currency, local-close, and DKK-close evidence from decision time.",
        &format!(
            "{missing_currency_count} suggested candidate(s) are missing trading-currency or DKK-close evidence."
        ),
    );

    let canonical_plan = report
        .get("strategy_plan")
        .and_then(|plan| plan.get("swing_orders"))
        .and_then(JsonValue::as_array)
        .is_some_and(|orders| orders == suggested_trades.as_slice());
    push_check(
        &mut checks,
        "canonical_manager_candidates",
        canonical_plan,
        "Trading Manager's stored strategy plan matches visible suggested trades.",
        "Trading Manager's stored strategy plan does not match visible suggested trades.",
    );

    let capital_consistent = requested_capital_plan
        .map(|requested| capital_plan_matches(requested, report.get("capital_plan")))
        .unwrap_or(true);
    push_check(
        &mut checks,
        "capital_context",
        capital_consistent,
        "Reported cash figures match the server-supplied capital context when it was available.",
        "Reported cash figures differ from the server-supplied capital context.",
    );

    let execution_safety = report.get("execution_safety");
    let safety_present = execution_safety.is_some_and(JsonValue::is_object)
        && execution_safety
            .and_then(|safety| safety.get("queue_eligible"))
            .and_then(JsonValue::as_bool)
            .is_some();
    push_check(
        &mut checks,
        "execution_authority",
        safety_present,
        "Server-owned execution-authority metadata is present.",
        "Server-owned execution-authority metadata is missing.",
    );

    let warning_count = checks
        .iter()
        .filter(|check| check["status"] != "pass")
        .count();
    let score = ((checks.len().saturating_sub(warning_count) * 100) / checks.len().max(1)) as i64;
    json!({
        "version": "v1",
        "status": if warning_count == 0 { "ready" } else { "review" },
        "score": score,
        "warning_count": warning_count,
        "candidate_count": suggested_trades.len(),
        "admission": "observational_only",
        "checks": checks,
        "safety": "This audit records completion evidence only. It cannot approve a report, override Trading Manager gates, create a queue entry, or reach Saxo."
    })
}

fn push_check(checks: &mut Vec<JsonValue>, key: &str, pass: bool, success: &str, failure: &str) {
    checks.push(json!({
        "key": key,
        "status": if pass { "pass" } else { "review" },
        "message": if pass { success } else { failure }
    }));
}

fn trade_shape_ok(trade: &JsonValue) -> bool {
    let order_type = text(trade, "order_type");
    let action = text(trade, "action");
    let basic = !text(trade, "symbol").is_empty()
        && matches!(action.as_str(), "BUY" | "SELL")
        && number(trade, "quantity").is_some_and(|value| value > 0.0)
        && matches!(order_type.as_str(), "Market" | "Limit")
        && number(trade, "estimated_value_dkk").is_some_and(|value| value > 0.0)
        && !text(trade, "strategy_key").is_empty();
    basic
        && (order_type != "Limit"
            || number(trade, "limit_price_local").is_some_and(|value| value > 0.0))
}

fn candidate_has_required_evidence(trade: &JsonValue) -> bool {
    let technical = trade
        .get("strategy_metadata")
        .and_then(|metadata| metadata.get("technical"));
    let markov = trade
        .get("strategy_metadata")
        .and_then(|metadata| metadata.get("markov"));
    technical.is_some_and(JsonValue::is_object)
        && !technical
            .map(|value| text(value, "status"))
            .unwrap_or_default()
            .is_empty()
        && markov.is_some_and(JsonValue::is_object)
        && markov
            .map(|value| text(value, "run_date"))
            .is_some_and(|value| !value.is_empty())
        && markov
            .and_then(|value| number(value, "signed_signal"))
            .is_some()
}

fn decision_time_indicator_signal<'a>(
    trade: &JsonValue,
    signals: &'a [JsonValue],
) -> Option<&'a JsonValue> {
    let candidate_symbol = canonical_symbol_key(&text(trade, "symbol"));
    (!candidate_symbol.is_empty())
        .then_some(candidate_symbol)
        .and_then(|symbol| {
            signals
                .iter()
                .find(|signal| canonical_symbol_key(&text(signal, "symbol")) == symbol)
        })
}

fn indicator_signal_has_instrument_resolution(signal: &JsonValue) -> bool {
    number(signal, "uic").is_some_and(|uic| uic > 0.0) && !text(signal, "asset_type").is_empty()
}

fn indicator_signal_has_currency_context(signal: &JsonValue) -> bool {
    !text(signal, "currency").is_empty()
        && number(signal, "close").is_some_and(|value| value > 0.0)
        && number(signal, "close_dkk").is_some_and(|value| value > 0.0)
}

fn canonical_symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

fn capital_plan_matches(requested: &JsonValue, reported: Option<&JsonValue>) -> bool {
    let Some(reported) = reported else {
        return false;
    };
    ["cash_balance_dkk", "available_buy_budget_dkk"]
        .into_iter()
        .all(|key| match number(requested, key) {
            Some(requested_value) => number(reported, key)
                .is_some_and(|reported_value| (requested_value - reported_value).abs() < 0.01),
            None => true,
        })
}

fn number(value: &JsonValue, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite())
}

fn text(value: &JsonValue, key: &str) -> String {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::completion_quality_audit;

    fn complete_trade() -> serde_json::Value {
        json!({
            "symbol": "AMD:xnas",
            "action": "BUY",
            "quantity": 1.0,
            "order_type": "Market",
            "limit_price_local": null,
            "estimated_value_dkk": 1200.0,
            "strategy_key": "us-open-amd",
            "strategy_metadata": {
                "technical": {"status": "pass"},
                "markov": {"run_date": "2026-08-30", "signed_signal": 0.42}
            }
        })
    }

    fn decision_time_context() -> serde_json::Value {
        json!({
            "daily_indicators": {
                "latest_run": {"status": "ok"},
                "signals": [{
                    "symbol": "AMD:xnas",
                    "uic": 211,
                    "asset_type": "Stock",
                    "currency": "USD",
                    "close": 170.0,
                    "close_dkk": 1100.0
                }]
            }
        })
    }

    #[test]
    fn records_ready_evidence_for_canonical_report() {
        let suggested = vec![complete_trade()];
        let capital = json!({"cash_balance_dkk": 5000.0, "available_buy_budget_dkk": 1200.0});
        let report = json!({
            "market_view": {},
            "capital_plan": capital,
            "selected_assets": [],
            "symbol_sentiment": [],
            "suggested_trades": suggested,
            "strategy_plan": {"swing_orders": suggested},
            "market_scope_enforcement": {"status": "not_required"},
            "execution_safety": {"queue_eligible": true}
        });

        let decision_time_context = decision_time_context();
        let audit = completion_quality_audit(&report, Some(&capital), Some(&decision_time_context));

        assert_eq!(audit["status"], "ready");
        assert_eq!(audit["score"], 100);
        assert_eq!(audit["candidate_count"], 1);
        assert_eq!(audit["admission"], "observational_only");
    }

    #[test]
    fn records_review_without_changing_candidates() {
        let suggested = vec![json!({"symbol": "AMD:xnas", "action": "BUY"})];
        let report = json!({
            "suggested_trades": suggested,
            "strategy_plan": {"swing_orders": []},
            "market_scope_enforcement": {"status": "not_required"},
            "execution_safety": {"queue_eligible": true}
        });

        let audit = completion_quality_audit(&report, None, None);

        assert_eq!(audit["status"], "review");
        assert_eq!(audit["candidate_count"], 1);
        assert_eq!(audit["admission"], "observational_only");
        assert_eq!(report["suggested_trades"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn records_missing_decision_time_currency_and_instrument_evidence() {
        let suggested = vec![complete_trade()];
        let report = json!({
            "market_view": {},
            "capital_plan": {},
            "selected_assets": [],
            "symbol_sentiment": [],
            "suggested_trades": suggested,
            "strategy_plan": {"swing_orders": suggested},
            "market_scope_enforcement": {"status": "not_required"},
            "execution_safety": {"queue_eligible": true}
        });
        let context = json!({
            "daily_indicators": {
                "latest_run": {"status": "failed"},
                "signals": [{"symbol": "AMD:xnas"}]
            }
        });

        let audit = completion_quality_audit(&report, None, Some(&context));
        let checks = audit["checks"].as_array().expect("audit checks");
        for key in [
            "daily_indicator_run",
            "candidate_instrument_resolution",
            "candidate_currency_context",
        ] {
            assert!(
                checks
                    .iter()
                    .any(|check| { check["key"] == key && check["status"] == "review" })
            );
        }
        assert_eq!(audit["admission"], "observational_only");
    }
}
