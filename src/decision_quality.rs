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

    let provenance = metadata_provenance(&suggested_trades, decision_time_context);

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
        "metadata_provenance": provenance,
        "safety": "This audit records completion evidence only. It cannot approve a report, override Trading Manager gates, create a queue entry, or reach Saxo."
    })
}

/// Digits after the point at which a written figure identifies where it came
/// from. Below this a match with another symbol is as likely to be chance as
/// copying: a written `0.0` "matches" every signal under 0.05.
const IDENTIFYING_DECIMALS: usize = 6;

/// How far apart two full-precision figures may be and still be one number.
/// A signal is stored single-precision in one place and double-precision in
/// another, which moves the eighth significant digit: 0.5828422796683945
/// against 0.5828422904014587 is the same signal.
const SAME_NUMBER_RELATIVE: f64 = 1e-7;

/// Where each suggested trade's metadata came from, traced through the
/// decision-time evidence.
///
/// The provider writes `strategy_metadata` itself, and nothing checked that it
/// describes the trade's own symbol. Report #259 attached DTE:xetr's Markov
/// signal to its NESTE trade -- identical to 17 digits, where NESTE had no
/// signal -- and #260 then quoted it as NESTE's. Report #195 wrote JPM's and
/// JNJ's Quiver signals into their Markov metadata. The Trading Manager's gates
/// look up their own signal by the order's symbol, so none of these reached a
/// gate; but the metadata is what the report reasoned from.
///
/// Recorded beside the checks, not among them. The audit's status and checks
/// reach Hermes' preflight, and whether this should is a separate decision.
/// It is observational either way: it cannot block, approve or change a trade.
fn metadata_provenance(trades: &[JsonValue], context: Option<&JsonValue>) -> JsonValue {
    let Some(context) = context.filter(|context| context.is_object()) else {
        return json!({"version": "v2", "status": "not_available"});
    };
    let listed: std::collections::HashMap<String, &JsonValue> = context
        .pointer("/markov_method/signals")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| Some((canonical_symbol_key(row.get("symbol")?.as_str()?), row)))
        .collect();
    let embedded: std::collections::HashMap<String, JsonValue> =
        crate::markov_method::embedded_prompt_signal_rows(context)
            .into_iter()
            .map(|(symbol, row)| (canonical_symbol_key(&symbol), row))
            .collect();
    let indicators = context
        .pointer("/daily_indicators/signals")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut leaves = Vec::new();
    numeric_leaves(context, "", None, &mut leaves);
    let threshold = markov_gate_threshold(context);

    let mut findings: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut per_trade = Vec::new();
    for trade in trades {
        let symbol = text(trade, "symbol");
        let key = canonical_symbol_key(&symbol);
        let own_markov = listed.get(&key).copied().or_else(|| embedded.get(&key));
        let own_indicator = decision_time_indicator_signal(trade, &indicators);
        let markov = trade.pointer("/strategy_metadata/markov");
        let signal = markov
            .and_then(|markov| number(markov, "signed_signal"))
            .map(|written| markov_signal_finding(&key, written, own_markov, threshold, &leaves));
        if let Some(finding) = signal
            .as_ref()
            .and_then(|signal| signal["finding"].as_str())
        {
            *findings.entry(finding_label(finding)).or_default() += 1;
        }
        let disagreements = metadata_disagreements(trade, own_markov, own_indicator);
        per_trade.push(json!({
            "symbol": symbol,
            "markov_signed_signal": signal,
            "disagreements": disagreements,
        }));
    }
    let cross = per_trade
        .iter()
        .filter(|trade| {
            trade["markov_signed_signal"]["finding"]
                .as_str()
                .is_some_and(|finding| !SIGNAL_FINDINGS_THAT_PASS.contains(&finding))
        })
        .count();
    let disagreeing = per_trade
        .iter()
        .filter(|trade| {
            trade["disagreements"]
                .as_array()
                .is_some_and(|list| !list.is_empty())
        })
        .count();
    json!({
        // v2: a short figure matches by rounding alone, not truncation.
        "version": "v2",
        "status": if cross == 0 && disagreeing == 0 { "consistent" } else { "review" },
        "trades_with_a_signal_from_elsewhere": cross,
        "trades_with_disagreeing_metadata": disagreeing,
        "signal_findings": findings,
        "trades": per_trade,
        "reading": "A Markov signal written at full precision identifies its source; one written \
                    short can only be checked against the trade's own symbol. `own_symbol` is the \
                    only finding that passes, apart from a `0` written for a symbol with no signal \
                    at all, which the presence check forces the provider to write. Observational: \
                    recorded beside the audit's checks, not among them.",
    })
}

/// Findings for a trade's Markov signal that describe its own symbol, or
/// describe nothing because there is nothing to describe.
const SIGNAL_FINDINGS_THAT_PASS: &[&str] = &["own_symbol", "no_signal_placeholder"];

fn finding_label(finding: &str) -> &'static str {
    [
        "own_symbol",
        "no_signal_placeholder",
        "disagrees_with_own_symbol",
        "no_own_signal",
        "gate_threshold_as_signal",
        "own_symbol_other_markov_value",
        "own_symbol_other_source",
        "other_symbol_markov",
        "other_symbol_other_source",
        "carried_from_earlier_report",
        "no_source",
    ]
    .into_iter()
    .find(|label| *label == finding)
    .unwrap_or("unclassified")
}

fn written_decimals(value: f64) -> usize {
    let text = format!("{value}");
    text.split_once('.')
        .map_or(0, |(_, fraction)| fraction.len())
}

/// Whether a written figure is the stored one: the stored value rounded at
/// the precision written, or -- for a long figure -- the same number stored at
/// the other precision.
///
/// Both, always. Report #304 wrote EQNR's 0.4292262494564056 as 0.429226: six
/// decimals, rounded, and treating six decimals as a full-precision copy
/// called its own signal a figure from nowhere.
///
/// Rounding is the numeric checker's own test, `written_by_rounding`, so the
/// two cannot drift apart. Truncation is not accepted: Simon settled the
/// convention as rounding on 2026-09-26, for every figure a report writes. The
/// storage allowance stays, because it is not a way of writing a figure: one
/// signal held in single and double precision differs in the eighth digit.
fn same_figure(written: f64, actual: f64) -> bool {
    let decimals = written_decimals(written);
    let rounded = crate::jev_numeric::written_by_rounding(written, decimals, actual);
    let stored_elsewhere = decimals >= IDENTIFYING_DECIMALS
        && (written - actual).abs() <= SAME_NUMBER_RELATIVE * actual.abs().max(1.0);
    rounded || stored_elsewhere
}

/// Whether `written` is `actual` truncated at the precision written. Used only
/// to name where a figure came from, never to accept it: under the rounding
/// convention a truncation is written wrongly.
fn truncates_to(written: f64, actual: f64) -> bool {
    let scale = 10f64.powi(written_decimals(written).min(15) as i32);
    ((actual * scale).trunc() / scale - written).abs() <= 1e-12 * written.abs().max(1.0)
}

/// The Markov gate's threshold as the decision-time context recorded it.
fn markov_gate_threshold(context: &JsonValue) -> Option<f64> {
    context
        .pointer("/decision_time_gate_policy/markov_starter/min_signed_signal")
        .or_else(|| context.pointer("/decision_policy/markov_gate/min_signed_signal"))
        .and_then(JsonValue::as_f64)
}

/// Every number in the decision-time context, with its path and the symbol of
/// the nearest enclosing object that names one.
fn numeric_leaves<'a>(
    value: &'a JsonValue,
    path: &str,
    symbol: Option<&'a str>,
    out: &mut Vec<(String, Option<String>, f64)>,
) {
    match value {
        JsonValue::Object(map) => {
            let symbol = map.get("symbol").and_then(JsonValue::as_str).or(symbol);
            for (key, child) in map {
                numeric_leaves(child, &format!("{path}.{key}"), symbol, out);
            }
        }
        JsonValue::Array(items) => {
            for child in items {
                numeric_leaves(child, &format!("{path}[]"), symbol, out);
            }
        }
        JsonValue::Number(number) => {
            if let Some(value) = number.as_f64() {
                out.push((path.to_string(), symbol.map(canonical_symbol_key), value));
            }
        }
        _ => {}
    }
}

/// Where a trade's Markov signal came from.
fn markov_signal_finding(
    symbol: &str,
    written: f64,
    own: Option<&JsonValue>,
    threshold: Option<f64>,
    leaves: &[(String, Option<String>, f64)],
) -> JsonValue {
    let own_value = own.and_then(|row| number(row, "signed_signal"));
    if own_value.is_some_and(|actual| same_figure(written, actual)) {
        return json!({"written": written, "finding": "own_symbol"});
    }
    // Ten trades in reports #211-#253 wrote 0.15 for symbols the prompt gave
    // no signal: the gate's minimum, written as though it were the signal the
    // trade needed.
    if threshold.is_some_and(|threshold| written != 0.0 && same_figure(written, threshold)) {
        return json!({
            "written": written,
            "decision_time": own_value,
            "finding": "gate_threshold_as_signal",
            "threshold": threshold,
        });
    }
    if written_decimals(written) < IDENTIFYING_DECIMALS {
        let finding = match own_value {
            Some(_) => "disagrees_with_own_symbol",
            None if written == 0.0 => "no_signal_placeholder",
            None => "no_own_signal",
        };
        return json!({"written": written, "decision_time": own_value, "finding": finding});
    }
    // Written at full precision, and not the trade's own signal: find the
    // source. Runtime-supplied sources outrank the model's earlier output.
    let mut sources: Vec<(u8, &'static str, &str, Option<&str>)> = leaves
        .iter()
        .filter(|(_, _, value)| same_figure(written, *value))
        .map(|(path, leaf_symbol, _)| {
            let same = leaf_symbol.as_deref() == Some(symbol);
            let (rank, finding) = if path.starts_with(".earlier_same_scope_report") {
                (4, "carried_from_earlier_report")
            } else if same && path.contains("markov") {
                (0, "own_symbol_other_markov_value")
            } else if same {
                (1, "own_symbol_other_source")
            } else if path.contains("markov") {
                (2, "other_symbol_markov")
            } else {
                (3, "other_symbol_other_source")
            };
            (rank, finding, path.as_str(), leaf_symbol.as_deref())
        })
        .collect();
    sources.sort_by_key(|(rank, ..)| *rank);
    let Some(&(best, finding, ..)) = sources.first() else {
        // The trade's own signal cut short rather than rounded came from its
        // own symbol, and is written wrongly under the rounding convention.
        // Calling it a figure from nowhere would name an invention that is not
        // there.
        let finding = if own_value.is_some_and(|actual| truncates_to(written, actual)) {
            "disagrees_with_own_symbol"
        } else {
            "no_source"
        };
        return json!({"written": written, "decision_time": own_value, "finding": finding});
    };
    let found_at: Vec<JsonValue> = sources
        .iter()
        .filter(|(rank, ..)| *rank == best)
        .take(5)
        .map(|(_, _, path, symbol)| json!({"path": path, "symbol": symbol}))
        .collect();
    json!({
        "written": written,
        "decision_time": own_value,
        "finding": finding,
        "found_at": found_at,
    })
}

/// Metadata that disagrees with the trade's own symbol at decision time. Only
/// the own symbol is compared: a state, a direction or a count of five is
/// shared by too many symbols to say where it came from.
fn metadata_disagreements(
    trade: &JsonValue,
    own_markov: Option<&JsonValue>,
    own_indicator: Option<&JsonValue>,
) -> Vec<JsonValue> {
    let mut found = Vec::new();
    let same_text = |left: &str, right: &str| left.eq_ignore_ascii_case(right);
    if let (Some(markov), Some(own)) = (trade.pointer("/strategy_metadata/markov"), own_markov) {
        for field in ["state", "direction", "run_date"] {
            let (written, actual) = (text(markov, field), text(own, field));
            if !written.is_empty() && !actual.is_empty() && !same_text(&written, &actual) {
                found.push(json!({"field": format!("markov.{field}"), "written": written, "decision_time": actual}));
            }
        }
    }
    let technical = trade.pointer("/strategy_metadata/technical");
    let reported_ok = technical.is_some_and(|technical| text(technical, "status") == "ok");
    if let (Some(technical), Some(own), true) = (technical, own_indicator, reported_ok) {
        for field in ["confluence_count", "min_confluences"] {
            if let (Some(written), Some(actual)) = (number(technical, field), number(own, field))
                && written != actual
            {
                found.push(json!({"field": format!("technical.{field}"), "written": written, "decision_time": actual}));
            }
        }
        for field in ["sentiment", "trend_bias"] {
            let (written, actual) = (text(technical, field), text(own, field));
            if !written.is_empty() && !actual.is_empty() && !same_text(&written, &actual) {
                found.push(json!({"field": format!("technical.{field}"), "written": written, "decision_time": actual}));
            }
        }
    }
    found
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

/// Whether each trade's metadata describes its own symbol, from its own source.
#[cfg(test)]
mod metadata_provenance_tests {
    use serde_json::{Value as JsonValue, json};

    use super::{completion_quality_audit, metadata_provenance};

    fn trade(symbol: &str, signed_signal: f64) -> JsonValue {
        json!({
            "symbol": symbol,
            "strategy_metadata": {
                "markov": {"signed_signal": signed_signal, "state": "Sideways", "direction": "long", "run_date": "2026-09-01"},
                "technical": {"status": "ok", "confluence_count": 5, "min_confluences": 3, "sentiment": "BUY", "trend_bias": "bullish"},
            },
        })
    }

    fn markov(symbol: &str, signed_signal: f64) -> JsonValue {
        json!({"symbol": symbol, "signed_signal": signed_signal, "conviction": signed_signal.abs(),
               "state": "Sideways", "direction": "long", "run_date": "2026-09-01"})
    }

    fn indicator(symbol: &str, sentiment: &str) -> JsonValue {
        json!({"symbol": symbol, "confluence_count": 5, "min_confluences": 3,
               "sentiment": sentiment, "trend_bias": "bullish"})
    }

    fn context(markov_rows: Vec<JsonValue>, extra: JsonValue) -> JsonValue {
        let mut context = json!({
            "markov_method": {"signals": markov_rows},
            "daily_indicators": {"signals": [indicator("NESTE:xhel", "BUY"), indicator("DTE:xetr", "BUY"), indicator("JPM:xnys", "BUY")]},
        });
        if let (Some(target), Some(fields)) = (context.as_object_mut(), extra.as_object()) {
            for (key, value) in fields {
                target.insert(key.clone(), value.clone());
            }
        }
        context
    }

    fn finding(provenance: &JsonValue, index: usize) -> &str {
        provenance["trades"][index]["markov_signed_signal"]["finding"]
            .as_str()
            .unwrap_or_default()
    }

    /// Report #259: DTE:xetr's signal on the NESTE trade, where NESTE had none.
    #[test]
    fn another_symbols_signal_is_named_as_its_source() {
        let dte = 0.23313425481319427;
        let context = context(vec![markov("DTE:xetr", dte)], json!({}));
        let provenance = metadata_provenance(&[trade("NESTE:xhel", dte)], Some(&context));
        assert_eq!(finding(&provenance, 0), "other_symbol_markov");
        assert_eq!(
            provenance["trades"][0]["markov_signed_signal"]["found_at"][0]["symbol"],
            "DTE:XETR"
        );
        assert_eq!(provenance["trades_with_a_signal_from_elsewhere"], 1);
        assert_eq!(provenance["status"], "review");
    }

    /// Report #195: JPM's Quiver signal written as its Markov signal.
    #[test]
    fn the_right_symbol_from_the_wrong_source_is_named() {
        let quiver = 0.24634137749671936;
        let context = context(
            vec![],
            json!({"quiver_signals": {"signals": [{"symbol": "JPM:xnys", "signal": quiver}]}}),
        );
        let provenance = metadata_provenance(&[trade("JPM:xnys", quiver)], Some(&context));
        assert_eq!(finding(&provenance, 0), "own_symbol_other_source");
        assert_eq!(
            provenance["trades"][0]["markov_signed_signal"]["found_at"][0]["path"],
            ".quiver_signals.signals[].signal"
        );
    }

    /// The same signal stored single-precision in one place and double in
    /// another is one number, not a disagreement.
    #[test]
    fn single_and_double_precision_copies_are_the_same_signal() {
        let context = context(vec![markov("NESTE:xhel", 0.5828422904014587)], json!({}));
        let provenance =
            metadata_provenance(&[trade("NESTE:xhel", 0.5828422796683945)], Some(&context));
        assert_eq!(finding(&provenance, 0), "own_symbol");
        assert_eq!(provenance["status"], "consistent");
    }

    /// A figure written short is checked against its own symbol only; a `0`
    /// for a symbol with no signal is the placeholder the presence check
    /// forces, not a source error.
    #[test]
    fn a_short_figure_is_never_attributed_to_another_symbol() {
        let context = context(vec![markov("DTE:xetr", 0.0312)], json!({}));
        let provenance = metadata_provenance(
            &[trade("NESTE:xhel", 0.0), trade("JPM:xnys", 0.03)],
            Some(&context),
        );
        assert_eq!(finding(&provenance, 0), "no_signal_placeholder");
        assert_eq!(
            finding(&provenance, 1),
            "no_own_signal",
            "not DTE's, though 0.03 would round from it"
        );
        assert_eq!(provenance["trades_with_a_signal_from_elsewhere"], 1);
    }

    /// A figure the model carried from its own earlier report ranks below any
    /// runtime source, and is named as such.
    #[test]
    fn a_figure_found_only_in_an_earlier_report_is_named_as_carried() {
        let carried = 0.23313425481319427;
        let context = context(
            vec![],
            json!({"earlier_same_scope_report": {"report": {"suggested_trades": [
                {"symbol": "NESTE:xhel", "strategy_metadata": {"markov": {"signed_signal": carried}}}
            ]}}}),
        );
        let provenance = metadata_provenance(&[trade("NESTE:xhel", carried)], Some(&context));
        assert_eq!(finding(&provenance, 0), "carried_from_earlier_report");
    }

    /// Report #304 wrote EQNR's 0.4292262494564056 as 0.429226 -- six
    /// decimals, rounded. That is its own signal, not a figure from nowhere.
    #[test]
    fn a_rounded_long_figure_is_still_its_own_signal() {
        let context = context(vec![markov("NESTE:xhel", 0.4292262494564056)], json!({}));
        let provenance = metadata_provenance(&[trade("NESTE:xhel", 0.429226)], Some(&context));
        assert_eq!(finding(&provenance, 0), "own_symbol");
    }

    /// The convention is rounding, as for every figure a report writes. A
    /// short figure truncated where rounding gives another is not the trade's
    /// own signal as written.
    #[test]
    fn a_short_figure_matches_its_own_signal_only_by_rounding() {
        let short = context(vec![markov("NESTE:xhel", 0.4199841320514679)], json!({}));
        let rounded = metadata_provenance(&[trade("NESTE:xhel", 0.42)], Some(&short));
        assert_eq!(finding(&rounded, 0), "own_symbol");
        let truncated = metadata_provenance(&[trade("NESTE:xhel", 0.4199)], Some(&short));
        assert_eq!(finding(&truncated, 0), "disagrees_with_own_symbol");
        assert_eq!(truncated["version"], "v2");

        // Long enough to name a source, and cut short: still its own signal,
        // written wrongly, not a figure from nowhere.
        let long = context(vec![markov("NESTE:xhel", 0.4292266494564056)], json!({}));
        let cut = metadata_provenance(&[trade("NESTE:xhel", 0.429226)], Some(&long));
        assert_eq!(finding(&cut, 0), "disagrees_with_own_symbol");
        let invented = metadata_provenance(&[trade("NESTE:xhel", 0.311111)], Some(&long));
        assert_eq!(finding(&invented, 0), "no_source");
    }

    /// The gate's minimum written as the signal, for a symbol the prompt gave
    /// none.
    #[test]
    fn the_gate_threshold_written_as_a_signal_is_named() {
        let context = context(
            vec![markov("DTE:xetr", 0.4)],
            json!({"decision_time_gate_policy": {"markov_starter": {"min_signed_signal": 0.15}}}),
        );
        let provenance = metadata_provenance(&[trade("NESTE:xhel", 0.15)], Some(&context));
        assert_eq!(finding(&provenance, 0), "gate_threshold_as_signal");
        assert_eq!(
            provenance["trades"][0]["markov_signed_signal"]["threshold"],
            0.15
        );
        assert_eq!(provenance["trades_with_a_signal_from_elsewhere"], 1);
    }

    /// Report #185 wrote BUY where the indicator said OVERWEIGHT.
    #[test]
    fn metadata_that_disagrees_with_its_own_symbol_is_listed() {
        let mut context = context(vec![markov("NESTE:xhel", 0.4)], json!({}));
        context["daily_indicators"]["signals"][0]["sentiment"] = json!("OVERWEIGHT");
        let provenance = metadata_provenance(&[trade("NESTE:xhel", 0.4)], Some(&context));
        assert_eq!(finding(&provenance, 0), "own_symbol");
        assert_eq!(
            provenance["trades"][0]["disagreements"][0]["field"],
            "technical.sentiment"
        );
        assert_eq!(provenance["trades_with_disagreeing_metadata"], 1);
    }

    /// The block over every stored report, measured rather than assumed.
    ///
    /// ```text
    /// JEV_PROMPTS_PATH=all.json JEV_METADATA_OUT=out.json \
    ///   cargo test --release across_the_stored_reports -- --ignored
    /// ```
    #[test]
    #[ignore]
    fn across_the_stored_reports() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let sources: Vec<JsonValue> =
            serde_json::from_slice(&std::fs::read(&path).expect("prompts")).expect("a JSON array");
        let mut findings: std::collections::BTreeMap<String, usize> = Default::default();
        let mut disagreements: std::collections::BTreeMap<String, usize> = Default::default();
        let mut flagged = Vec::new();
        let (mut reports, mut trades) = (0usize, 0usize);
        for entry in &sources {
            let (Some(report), Some(request)) = (entry.get("report"), entry.get("request")) else {
                continue;
            };
            let context = crate::xai_decision::decision_prompt_user_payload(request);
            if !context.is_object() {
                continue;
            }
            let suggested = report
                .get("suggested_trades")
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default();
            reports += 1;
            trades += suggested.len();
            let provenance = metadata_provenance(&suggested, Some(&context));
            for trade in provenance["trades"].as_array().into_iter().flatten() {
                if let Some(finding) = trade["markov_signed_signal"]["finding"].as_str() {
                    *findings.entry(finding.to_string()).or_default() += 1;
                    if !super::SIGNAL_FINDINGS_THAT_PASS.contains(&finding)
                        || trade["disagreements"]
                            .as_array()
                            .is_some_and(|list| !list.is_empty())
                    {
                        flagged.push(json!({"report": entry["id"], "trade": trade}));
                    }
                } else if trade["disagreements"]
                    .as_array()
                    .is_some_and(|list| !list.is_empty())
                {
                    flagged.push(json!({"report": entry["id"], "trade": trade}));
                }
                for disagreement in trade["disagreements"].as_array().into_iter().flatten() {
                    *disagreements
                        .entry(
                            disagreement["field"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        )
                        .or_default() += 1;
                }
            }
        }
        let result = json!({
            "reports_with_a_context": reports,
            "trades": trades,
            "signal_findings": findings,
            "disagreements_by_field": disagreements,
            "flagged": flagged,
        });
        println!("{}", serde_json::to_string_pretty(&json!({
            "reports_with_a_context": reports, "trades": trades,
            "signal_findings": result["signal_findings"], "disagreements_by_field": result["disagreements_by_field"],
        })).expect("json"));
        if let Ok(out) = std::env::var("JEV_METADATA_OUT") {
            std::fs::write(out, serde_json::to_string_pretty(&result).expect("json"))
                .expect("write");
        }
    }

    /// Recorded beside the checks. The audit's checks, status and score --
    /// which reach Hermes' preflight -- are exactly what they were.
    #[test]
    fn provenance_does_not_move_the_audit_status() {
        let dte = 0.23313425481319427;
        let suggested = vec![trade("NESTE:xhel", dte)];
        let report = json!({"suggested_trades": suggested});
        let context = context(vec![markov("DTE:xetr", dte)], json!({}));
        let with = completion_quality_audit(&report, None, Some(&context));
        let mut without_markov = context.clone();
        without_markov["markov_method"]["signals"] = json!([markov("NESTE:xhel", dte)]);
        let clean = completion_quality_audit(&report, None, Some(&without_markov));
        assert_eq!(with["metadata_provenance"]["status"], "review");
        assert_eq!(clean["metadata_provenance"]["status"], "consistent");
        assert_eq!(with["checks"], clean["checks"]);
        assert_eq!(with["status"], clean["status"]);
        assert_eq!(with["score"], clean["score"]);
    }
}
