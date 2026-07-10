use std::{env, sync::Arc};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value as JsonValue, json};
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::{
    models::{HermesDecisionAdviceRequest, HermesExperimentRequest, HermesReflectionRequest},
    state::AppState,
};

pub async fn run_mcp_http() -> Result<()> {
    let state = Arc::new(AppState::load().await.context("loading MCP app state")?);
    let bind_addr = env::var("BIND_ADDR")
        .or_else(|_| env::var("MCP_BIND_ADDR"))
        .unwrap_or_else(|_| "0.0.0.0:8610".to_string());
    let app = Router::new()
        .route("/health", get(mcp_health))
        .route("/mcp", post(mcp_endpoint))
        .with_state(state);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("binding daytrader MCP server on {bind_addr}"))?;
    info!("serving daytrader MCP server on http://{bind_addr}/mcp");
    axum::serve(listener, app)
        .await
        .context("serving daytrader MCP server")
}

async fn mcp_health() -> Json<JsonValue> {
    Json(json!({"status": "ok", "runtime": "daytrader-mcp"}))
}

async fn mcp_endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<JsonValue>,
) -> Response {
    if !authorized(&headers) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            rpc_error(JsonValue::Null, -32001, "unauthorized MCP request"),
        );
    }

    if let Some(items) = payload.as_array() {
        let mut responses = Vec::new();
        for item in items {
            match handle_rpc(state.clone(), item.clone()).await {
                Ok(Some(response)) => responses.push(response),
                Ok(None) => {}
                Err(err) => responses.push(rpc_error(
                    item.get("id").cloned().unwrap_or(JsonValue::Null),
                    -32603,
                    &err.to_string(),
                )),
            }
        }
        if responses.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        return json_response(StatusCode::OK, JsonValue::Array(responses));
    }

    match handle_rpc(state, payload.clone()).await {
        Ok(Some(response)) => json_response(StatusCode::OK, response),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(err) => json_response(
            StatusCode::OK,
            rpc_error(
                payload.get("id").cloned().unwrap_or(JsonValue::Null),
                -32603,
                &err.to_string(),
            ),
        ),
    }
}

async fn handle_rpc(state: Arc<AppState>, request: JsonValue) -> Result<Option<JsonValue>> {
    let id = request.get("id").cloned().unwrap_or(JsonValue::Null);
    let method = request
        .get("method")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow!("missing MCP method"))?;
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    let result = match method {
        "initialize" => json!({
            "protocolVersion": request
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(JsonValue::as_str)
                .unwrap_or("2024-11-05"),
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "daytrader-mcp",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": "Use only these Hermes-safe daytrader tools. They expose sanitized context, reflections, and one-variable experiment proposals. They do not expose Saxo sessions, broker mutation endpoints, Kubernetes secrets, or live order approval."
        }),
        "notifications/initialized" => return Ok(None),
        "ping" => json!({}),
        "tools/list" => json!({"tools": mcp_tools()}),
        "tools/call" => call_tool(state, params).await?,
        _ => {
            return Ok(Some(rpc_error(
                id,
                -32601,
                &format!("unsupported MCP method: {method}"),
            )));
        }
    };

    Ok(Some(rpc_result(id, result)))
}

async fn call_tool(state: Arc<AppState>, params: JsonValue) -> Result<JsonValue> {
    let name = params
        .get("name")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow!("tools/call missing name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let value = match name {
        "get_app_capabilities" => state.hermes_capabilities_value(),
        "get_goal_contract" => state.hermes_goal_contract_value(),
        "get_context" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(20);
            state.hermes_context(limit).await?
        }
        "list_reflections" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(20);
            json!({"reflections": state.hermes_reflections(limit).await?})
        }
        "create_reflection" => {
            let request = serde_json::from_value::<HermesReflectionRequest>(arguments)
                .context("parsing create_reflection arguments")?;
            state.record_hermes_reflection(&request).await?
        }
        "list_experiments" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(20);
            json!({"experiments": state.hermes_experiments(limit).await?})
        }
        "get_decision_reports" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(10);
            json!({
                "cadence": "two_daily_open_followups",
                "reports": state.hermes_decision_report_items(limit).await?
            })
        }
        "get_end_of_day_reports" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(10);
            json!({
                "cadence": "daily",
                "reports": state.hermes_end_of_day_report_items(limit).await?
            })
        }
        "get_markov_signals" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(50);
            crate::markov_method::compact_markov_context(&state, limit).await?
        }
        "get_quiver_signals" => {
            let limit = arguments
                .get("limit")
                .and_then(JsonValue::as_i64)
                .unwrap_or(50);
            crate::quiver::compact_quiver_context(&state, limit).await?
        }
        "create_experiment_proposal" => {
            let request = serde_json::from_value::<HermesExperimentRequest>(arguments)
                .context("parsing create_experiment_proposal arguments")?;
            state.record_hermes_experiment(&request).await?
        }
        "create_decision_advice" => {
            let request = serde_json::from_value::<HermesDecisionAdviceRequest>(arguments)
                .context("parsing create_decision_advice arguments")?;
            state.record_hermes_decision_advice(&request).await?
        }
        _ => {
            warn!(tool = name, "unsupported daytrader MCP tool requested");
            return Ok(tool_error(&format!("unsupported tool: {name}")));
        }
    };

    Ok(tool_result(value))
}

fn authorized(headers: &HeaderMap) -> bool {
    let Ok(expected) =
        env::var("HERMES_DAYTRADER_API_KEY").or_else(|_| env::var("DAYTRADER_HERMES_API_KEY"))
    else {
        warn!("daytrader MCP blocked because HERMES_DAYTRADER_API_KEY is not configured");
        return false;
    };
    if expected.trim().is_empty() {
        warn!("daytrader MCP blocked because HERMES_DAYTRADER_API_KEY is empty");
        return false;
    }

    let supplied_key = headers
        .get("x-hermes-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty());

    let supplied_bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty());

    supplied_key == Some(expected.as_str()) || supplied_bearer == Some(expected.as_str())
}

fn mcp_tools() -> Vec<JsonValue> {
    vec![
        tool_schema(
            "get_app_capabilities",
            "Return the sanitized daytrader capability and safety contract for Hermes.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
        ),
        tool_schema(
            "get_goal_contract",
            "Return the versioned Hermes self-improvement objective and constraints.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
        ),
        tool_schema(
            "get_context",
            "Return sanitized scheduler, decision, execution, journal, performance, experiment, and active baseline context.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "list_reflections",
            "List recent Hermes reflections from the audited daytrader database.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "create_reflection",
            "Create exactly one audited Hermes reflection. Use when evidence is insufficient for an experiment too.",
            json!({
                "type": "object",
                "required": ["summary"],
                "properties": {
                    "period_start": {"type": "string"},
                    "period_end": {"type": "string"},
                    "goal_version": {"type": "integer"},
                    "summary": {"type": "string"},
                    "findings": {},
                    "proposed_actions": {},
                    "source_session_id": {"type": "string"},
                    "raw_payload": {}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "list_experiments",
            "List recent one-variable Hermes experiment proposals and lifecycle states.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "get_decision_reports",
            "Return recent sanitized scheduled decision reports. The scheduler targets two daily open-followup reports when market calendars make them due.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "get_end_of_day_reports",
            "Return recent sanitized daily end-of-day strategy journal reports.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "get_markov_signals",
            "Return recent advisory Markov regime signals for portfolio and watchlist assets.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "get_quiver_signals",
            "Return recent advisory QuiverQuant alternative-data signals for US portfolio and watchlist assets.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "create_experiment_proposal",
            "Create one pending-review strategy experiment proposal. The proposal must change exactly one variable.",
            json!({
                "type": "object",
                "required": [
                    "hypothesis",
                    "changed_variable_path",
                    "old_value",
                    "new_value",
                    "expected_effect"
                ],
                "properties": {
                    "baseline_id": {"type": "string"},
                    "goal_version": {"type": "integer"},
                    "hypothesis": {"type": "string"},
                    "changed_variable_path": {"type": "string"},
                    "old_value": {},
                    "new_value": {},
                    "expected_effect": {"type": "string"},
                    "risk_notes": {"type": "string"},
                    "evidence": {},
                    "source_session_id": {"type": "string"},
                    "raw_payload": {}
                },
                "additionalProperties": false
            }),
        ),
        tool_schema(
            "create_decision_advice",
            "Record advisory Hermes input for one decision report. This is an audited advisory artifact only; it cannot place, approve, add, or execute orders.",
            json!({
                "type": "object",
                "required": [
                    "decision_report_id",
                    "overall_recommendation",
                    "summary"
                ],
                "properties": {
                    "decision_report_id": {"type": "integer"},
                    "source_session_id": {"type": "string"},
                    "overall_recommendation": {"type": "string", "enum": ["proceed", "stand_down", "review"]},
                    "summary": {"type": "string"},
                    "order_advice": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["action", "reason"],
                            "properties": {
                                "strategy_key": {"type": "string"},
                                "symbol": {"type": "string"},
                                "side": {"type": "string", "enum": ["BUY", "SELL"]},
                                "action": {"type": "string", "enum": ["allow", "reduce", "stand_down", "review"]},
                                "max_quantity": {"type": "number"},
                                "reason": {"type": "string"}
                            },
                            "additionalProperties": false
                        }
                    },
                    "learning_notes": {},
                    "context_self_check": {
                        "type": "object",
                        "required": [
                            "latest_report",
                            "markov_signals",
                            "end_of_day_report",
                            "current_positions",
                            "active_experiments"
                        ],
                        "properties": {
                            "latest_report": {"type": "boolean"},
                            "markov_signals": {"type": "boolean"},
                            "end_of_day_report": {"type": "boolean"},
                            "current_positions": {"type": "boolean"},
                            "active_experiments": {"type": "boolean"},
                            "sources": {
                                "type": "array",
                                "items": {"type": "string"}
                            },
                            "notes": {"type": "string"}
                        },
                        "additionalProperties": true
                    },
                    "raw_payload": {}
                },
                "additionalProperties": false
            }),
        ),
    ]
}

fn tool_schema(name: &str, description: &str, input_schema: JsonValue) -> JsonValue {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn tool_result(value: JsonValue) -> JsonValue {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": value,
        "isError": false
    })
}

fn tool_error(message: &str) -> JsonValue {
    json!({
        "content": [
            {
                "type": "text",
                "text": message
            }
        ],
        "isError": true
    })
}

fn rpc_result(id: JsonValue, result: JsonValue) -> JsonValue {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn rpc_error(id: JsonValue, code: i64, message: &str) -> JsonValue {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn json_response(status: StatusCode, value: JsonValue) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_exposes_only_hermes_safe_tools() {
        let names = mcp_tools()
            .into_iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();

        assert!(names.contains(&"get_context".to_string()));
        assert!(names.contains(&"get_decision_reports".to_string()));
        assert!(names.contains(&"get_end_of_day_reports".to_string()));
        assert!(names.contains(&"get_markov_signals".to_string()));
        assert!(names.contains(&"get_quiver_signals".to_string()));
        assert!(names.contains(&"create_reflection".to_string()));
        assert!(names.contains(&"create_experiment_proposal".to_string()));
        assert!(names.contains(&"create_decision_advice".to_string()));
        assert!(!names.iter().any(|name| name.contains("saxo")));
        assert!(!names.iter().any(|name| name.contains("order")));
        assert!(!names.iter().any(|name| name.contains("secret")));
    }
}
