use serde_json::{Value as JsonValue, json};

/// Canonical JSON Schema for structured Decision Report output.
///
/// This module is intentionally pure: it defines the provider contract but
/// cannot schedule a report, call a model, alter a manager gate, or reach a
/// broker. Provider-specific strict-output enforcement stays in
/// `xai_decision.rs` while the report shape lives here.
pub(crate) fn decision_report_json_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": [
            "report_title",
            "market_view",
            "reasoning_steps",
            "capital_plan",
            "selected_assets",
            "symbol_sentiment",
            "suggested_trades",
            "strategy_status",
            "strategy_baseline_id",
            "strategy_flow",
            "change_since_earlier"
        ],
        "additionalProperties": false,
        "properties": {
            "report_title": {"type": "string"},
            "strategy_status": {"type": "string"},
            "strategy_baseline_id": {"type": ["string", "null"]},
            "strategy_flow": strategy_flow_schema(),
            "change_since_earlier": change_since_earlier_schema(),
            "market_view": market_view_schema(),
            "reasoning_steps": {"type": "array", "items": {"type": "string"}},
            "capital_plan": capital_plan_schema(),
            "selected_assets": selected_assets_schema(),
            "symbol_sentiment": symbol_sentiment_schema(),
            "suggested_trades": suggested_trades_schema()
        }
    })
}

fn change_since_earlier_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": ["status", "summary", "material_changes"],
        "additionalProperties": false,
        "properties": {
            "status": {
                "type": "string",
                "enum": ["material_change", "no_new_information", "not_available", "not_applicable"]
            },
            "summary": {"type": "string"},
            "material_changes": {"type": "array", "items": {"type": "string"}}
        }
    })
}

fn strategy_flow_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": ["portfolio", "selected", "trades"],
        "additionalProperties": false,
        "properties": {
            "portfolio": {"type": "number"},
            "selected": {"type": "number"},
            "trades": {"type": "number"}
        }
    })
}

fn market_view_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": ["bias", "summary"],
        "additionalProperties": false,
        "properties": {
            "bias": {"type": "string"},
            "summary": {"type": "string"}
        }
    })
}

fn capital_plan_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": [
            "cash_balance_dkk",
            "available_buy_budget_dkk",
            "cash_policy",
            "reinvestment_decision",
            "near_term_opportunities",
            "medium_term_watchlist"
        ],
        "additionalProperties": false,
        "properties": {
            "cash_balance_dkk": {"type": "number"},
            "available_buy_budget_dkk": {"type": "number"},
            "cash_policy": {"type": "string"},
            "reinvestment_decision": {"type": "string", "enum": ["redeploy", "wait", "risk_reduce"]},
            "near_term_opportunities": {"type": "array", "items": {"type": "string"}},
            "medium_term_watchlist": {"type": "array", "items": {"type": "string"}}
        }
    })
}

fn selected_assets_schema() -> JsonValue {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["symbol", "score", "notes"],
            "additionalProperties": false,
            "properties": {
                "symbol": {"type": "string"},
                "score": {"type": "number"},
                "notes": {"type": "string"}
            }
        }
    })
}

fn symbol_sentiment_schema() -> JsonValue {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["symbol", "sentiment", "confidence", "rationale"],
            "additionalProperties": false,
            "properties": {
                "symbol": {"type": "string"},
                "sentiment": {"type": "string", "enum": ["SELL", "UNDERWEIGHT", "HOLD", "OVERWEIGHT", "BUY"]},
                "confidence": {"type": "number"},
                "rationale": {"type": "string"}
            }
        }
    })
}

fn suggested_trades_schema() -> JsonValue {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": [
                "symbol",
                "action",
                "quantity",
                "order_type",
                "limit_price_local",
                "estimated_value_dkk",
                "strategy_key",
                "strategy_role",
                "strategy_metadata"
            ],
            "additionalProperties": false,
            "properties": {
                "symbol": {"type": "string"},
                "action": {"type": "string", "enum": ["BUY", "SELL"]},
                "quantity": {"type": "number"},
                "order_type": {"type": "string", "enum": ["Market", "Limit"]},
                "limit_price_local": {"type": ["number", "null"]},
                "estimated_value_dkk": {"type": "number"},
                "strategy_key": {"type": "string"},
                "strategy_role": {"type": "string"},
                "strategy_metadata": strategy_metadata_schema()
            }
        }
    })
}

fn strategy_metadata_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": ["technical", "markov"],
        "additionalProperties": false,
        "properties": {
            "technical": technical_metadata_schema(),
            "markov": markov_metadata_schema()
        }
    })
}

fn technical_metadata_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": [
            "status",
            "sentiment",
            "trend_bias",
            "confluence_count",
            "min_confluences"
        ],
        "additionalProperties": false,
        "properties": {
            "status": {"type": "string"},
            "sentiment": {"type": "string"},
            "trend_bias": {"type": "string", "enum": ["bullish", "neutral", "bearish"]},
            "confluence_count": {"type": "number"},
            "min_confluences": {"type": "number"}
        }
    })
}

fn markov_metadata_schema() -> JsonValue {
    json!({
        "type": "object",
        "required": ["signed_signal", "direction", "state", "run_date"],
        "additionalProperties": false,
        "properties": {
            "signed_signal": {"type": "number"},
            "direction": {"type": "string", "enum": ["long", "short"]},
            "state": {"type": "string"},
            "run_date": {"type": "string"}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::decision_report_json_schema;

    #[test]
    fn schema_keeps_the_required_trade_contract() {
        let schema = decision_report_json_schema();
        assert_eq!(schema["type"], "object");
        assert!(
            schema["required"]
                .as_array()
                .expect("required list")
                .contains(&"suggested_trades".into())
        );
        assert_eq!(
            schema["properties"]["suggested_trades"]["items"]["properties"]["strategy_metadata"]["properties"]
                ["markov"]["type"],
            "object"
        );
    }
}
