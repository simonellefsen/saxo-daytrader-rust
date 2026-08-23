use std::{env, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Form, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::{Value as JsonValue, json};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, error, info, warn};

use crate::{
    auth::{self, SsoSession},
    config::{public_base_path, yaml_string},
    localization::LocalizationPrefs,
    models::{
        AiApiKeyRequest, AiPromptItem, AiPromptsPayload, AiSettingsRequest, CashBufferRequest,
        CashBufferSettings, DecisionGateReplayPayload, DecisionLatestPayload,
        DecisionReportListPayload, DrawdownGuardOverrideRequest, ExecutionPayload,
        HermesExperimentRequest, HermesExperimentTransitionRequest, HermesExperimentsPayload,
        HermesReflectionRequest, HermesReflectionsPayload, InstrumentQuarantineOverrideRequest,
        LimitParams, LocalizationSettingsRequest, MarketStatusPayload, MarketWatchlistsPayload,
        MarkovSignalsPayload, MonthlyLossBreakerOverrideRequest,
        OverviewIntegrityAcknowledgementRequest, PerformanceParams, PerformancePayload,
        PortfolioPositionsPayload, PortfolioTradesPayload,
        ProtectiveStopLifecycleCancellationRequest, ProtectiveStopLifecyclePlacementRequest,
        ProtectiveStopLifecycleReconcileRequest, ProtectiveStopPrecheckRequest,
        QuiverSignalsPayload, RuntimeHealth, SaxoCallbackParams, SchedulerPayload,
        StrategyJournalPayload, ViewParams,
    },
    saxo_error::classify_execution_error,
    saxo_order::{
        cancel_sim_protective_stop_lifecycle_test, place_sim_protective_stop_lifecycle_test,
        precheck_sim_protective_stop, protective_stop_lifecycle_error_is_state_unknown,
        reconcile_sim_protective_stop_lifecycle_test, run_saxo_execution_queue,
    },
    state::AppState,
    trading_manager::run_trading_manager_cycle,
    ui::render_index,
    xai_decision,
};

// Axum routes are declared as data: a `Router` maps URL patterns to async
// handler functions. This is close to Express/Next route registration, but each
// handler's inputs are typed extractors such as `State`, `Path`, and `Query`.
pub fn router(state: Arc<AppState>) -> Router {
    let base_path = public_base_path(&state.config);
    let routes = app_routes();
    let routes = if base_path.is_empty() {
        routes
    } else {
        routes.clone().nest(&base_path, routes)
    };
    routes
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

fn app_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index))
        .route("/assets/app.css", get(css))
        .route("/favicon.svg", get(favicon_svg))
        .route("/icon.svg", get(favicon_svg))
        .route("/favicon.ico", get(favicon_ico))
        .route("/api/health", get(health))
        .route("/api/overview", get(overview))
        .route("/auth/session", get(auth_session))
        .route("/api/auth/session", get(auth_session))
        .route("/api/localization", get(localization))
        .route(
            "/api/settings/cash-buffer",
            get(cash_buffer_settings).post(update_cash_buffer),
        )
        .route(
            "/api/settings/monthly-loss-breaker",
            post(update_monthly_loss_breaker_override),
        )
        .route(
            "/api/settings/drawdown-guardrail",
            post(update_drawdown_guard_override),
        )
        .route(
            "/api/settings/instrument-quarantine",
            post(update_instrument_quarantine_override),
        )
        .route(
            "/api/settings/overview-integrity",
            post(update_overview_integrity_acknowledgement),
        )
        .route(
            "/api/settings/localization",
            post(update_localization_settings),
        )
        .route("/api/settings/ai", post(update_ai_settings))
        .route("/api/settings/ai-key", post(update_ai_api_key))
        .route("/api/saxo/auth/status", get(saxo_auth_status))
        .route(
            "/api/saxo/auth/start",
            get(saxo_auth_start_redirect).post(saxo_auth_start),
        )
        .route("/api/saxo/auth/callback", get(saxo_auth_callback))
        .route("/api/saxo/session", get(saxo_session))
        .route("/api/saxo/session/refresh", post(saxo_session_refresh))
        .route("/api/saxo/session/logout", post(saxo_session_logout))
        .route(
            "/api/saxo/session/disconnect",
            post(saxo_session_disconnect),
        )
        .route("/api/portfolio/positions", get(portfolio_positions))
        .route(
            "/api/asset-ladder-history/{symbol}",
            get(asset_ladder_history),
        )
        .route("/api/ladder-chart/{symbol}", get(asset_ladder_history))
        .route("/api/portfolio/trades", get(portfolio_trades))
        .route("/api/performance", get(performance))
        .route("/api/markov/signals", get(markov_signals))
        .route("/api/quiver/signals", get(quiver_signals))
        .route("/api/market/status", get(market_status))
        .route("/api/market/watchlists", get(market_watchlists))
        .route("/api/prompts", get(prompts))
        .route("/api/decision/latest", get(decision_latest))
        .route("/api/decision/reports", get(decision_reports))
        .route(
            "/api/decision/reports/{report_id}/debug",
            get(decision_report_debug),
        )
        .route("/api/decision/gate-replay", get(decision_gate_replay))
        .route("/api/decision/schema", get(decision_schema))
        .route("/api/strategy-journal", get(strategy_journal))
        .route("/api/execution", get(execution))
        .route(
            "/api/execution/orders/{order_id}/events",
            get(execution_order_events),
        )
        .route("/api/scheduler", get(scheduler))
        .route("/api/hermes/capabilities", get(hermes_capabilities))
        .route("/api/hermes/context", get(hermes_context))
        .route(
            "/api/hermes/reflections",
            get(hermes_reflections).post(create_hermes_reflection),
        )
        .route(
            "/api/hermes/experiments",
            get(hermes_experiments).post(create_hermes_experiment),
        )
        .route(
            "/api/hermes/experiments/{experiment_id}/transition",
            post(transition_hermes_experiment),
        )
        .route(
            "/api/actions/decision-report",
            post(action_generate_decision_report),
        )
        .route(
            "/api/actions/decision-report-dry-run",
            post(action_generate_decision_report_dry_run),
        )
        .route("/api/actions/queue-process", post(action_process_queue))
        .route(
            "/api/protective-stops/precheck",
            post(precheck_protective_stop),
        )
        .route(
            "/api/protective-stops/lifecycle/place",
            post(place_protective_stop_lifecycle_test),
        )
        .route(
            "/api/protective-stops/lifecycle/place-batch",
            post(place_protective_stop_batch),
        )
        .route(
            "/api/protective-stops/lifecycle/cancel",
            post(cancel_protective_stop_lifecycle_test),
        )
        .route(
            "/api/protective-stops/lifecycle/reconcile",
            post(reconcile_protective_stop_lifecycle_test),
        )
        .route(
            "/api/actions/daily-indicators",
            post(action_run_daily_indicators),
        )
        .route(
            "/api/actions/performance-benchmarks",
            post(action_run_performance_benchmarks),
        )
        .route(
            "/api/actions/quiver-signals",
            post(action_run_quiver_signals),
        )
        .route("/api/actions/sync-broker", post(action_not_ported))
        .route("/api/actions/retry-failed", post(action_not_ported))
        .route("/api/actions/reconcile-broker", post(action_not_ported))
        .route(
            "/api/actions/adopt-broker-portfolio",
            post(action_not_ported),
        )
        .route(
            "/api/actions/sync-saxo-sim-portfolio",
            post(action_not_ported),
        )
        .route("/api/actions/scheduler-cycle", post(action_not_ported))
        .route(
            "/api/orders/{order_id}/manage",
            post(manage_order_not_ported),
        )
        .route(
            "/api/portfolio/reset-from-live-csv",
            post(reset_sim_from_live_csv),
        )
        .fallback(get(index))
}

async fn index(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ViewParams>,
) -> Html<String> {
    // `State(...)` unwraps the shared app state that was attached to the router.
    let sso_session = json!(SsoSession::from_headers(&headers));
    let localization = state
        .localization_for_user(
            LocalizationPrefs::from_headers_and_config(&headers, &state.config),
            &sso_session,
        )
        .await;
    let active_view = normalize_view(params.view.as_deref());
    let performance_range = normalize_performance_range(params.range_key.as_deref());
    let execution_page = normalize_execution_page(params.execution_page);
    let markov_page = normalize_markov_page(params.markov_page);
    let markov_filter =
        crate::markov_method::normalize_markov_filter(params.markov_filter.as_deref());
    let hermes_section = crate::state::normalize_hermes_section(params.hermes_section.as_deref());
    let quiver_page = normalize_quiver_page(params.quiver_page);
    let scheduler_page = normalize_scheduler_page(params.scheduler_page);
    info!(
        view = %active_view,
        locale = %localization.locale,
        time_zone = %localization.time_zone,
        "rendering dashboard view"
    );
    let base_path = public_base_path(&state.config);
    Html(render_index(
        state
            .dashboard_view(
                localization,
                sso_session,
                active_view,
                performance_range,
                params.report_id,
                execution_page,
                markov_page,
                markov_filter,
                hermes_section,
                quiver_page,
                scheduler_page,
            )
            .await,
        &base_path,
    ))
}

async fn css() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
        crate::ui::CSS,
    )
}

async fn favicon_svg() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "image/svg+xml; charset=utf-8",
        )],
        crate::ui::FAVICON_SVG,
    )
}

async fn favicon_ico() -> Redirect {
    Redirect::permanent("/favicon.svg")
}

async fn health() -> Json<RuntimeHealth> {
    Json(health_payload())
}

fn health_payload() -> RuntimeHealth {
    RuntimeHealth {
        status: "ok".to_string(),
        runtime: "rust-dioxus".to_string(),
        git_sha: crate::build_info::git_sha().to_string(),
    }
}

async fn overview(State(state): State<Arc<AppState>>) -> Response {
    json_result(state.overview_payload().await)
}

async fn auth_session(headers: HeaderMap) -> Json<SsoSession> {
    Json(SsoSession::from_headers(&headers))
}

async fn localization(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<LocalizationPrefs> {
    let sso_session = json!(SsoSession::from_headers(&headers));
    let prefs = state
        .localization_for_user(
            LocalizationPrefs::from_headers_and_config(&headers, &state.config),
            &sso_session,
        )
        .await;
    Json(prefs)
}

async fn cash_buffer_settings(State(state): State<Arc<AppState>>) -> Json<CashBufferSettings> {
    Json(state.cash_buffer_settings())
}

async fn update_cash_buffer(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CashBufferRequest>,
) -> Json<CashBufferSettings> {
    Json(cash_buffer_preview(
        state.cash_buffer_settings(),
        request.min_cash_buffer_pct,
    ))
}

fn cash_buffer_preview(
    mut settings: CashBufferSettings,
    min_cash_buffer_pct: f64,
) -> CashBufferSettings {
    settings.min_cash_buffer_pct = min_cash_buffer_pct;
    settings.source = "request_preview".to_string();
    settings
}

async fn update_monthly_loss_breaker_override(
    State(state): State<Arc<AppState>>,
    Form(request): Form<MonthlyLossBreakerOverrideRequest>,
) -> Response {
    let action = request.action.trim();
    let enable = match action {
        "resume_buys" => true,
        "clear_override" => false,
        _ => {
            return json_result(Err(anyhow::anyhow!(
                "Unsupported monthly-loss breaker action: {action}"
            )));
        }
    };
    match state
        .save_monthly_loss_breaker_override(enable, request.notes.unwrap_or_default().trim())
        .await
    {
        Ok(value) => {
            info!(
                enabled = value
                    .get("enabled")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
                month_key = %value.get("month_key").and_then(JsonValue::as_str).unwrap_or(""),
                "monthly-loss breaker override updated"
            );
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("monthly-loss breaker override update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

/// Grant or clear the drawdown guardrail override.
///
/// Granting requires the peak the exemption is granted against -- normally the
/// `peak_value_dkk` the Trading Manager report shows -- because that anchor is
/// what lets the grant expire on its own once the book makes a new high. The
/// operator reads it from the drawdown_guardrail block of the latest run.
async fn update_drawdown_guard_override(
    State(state): State<Arc<AppState>>,
    Form(request): Form<DrawdownGuardOverrideRequest>,
) -> Response {
    let action = request.action.trim();
    let enable = match action {
        "resume_buys" => true,
        "clear_override" => false,
        _ => {
            return json_result(Err(anyhow::anyhow!(
                "Unsupported drawdown guardrail action: {action}"
            )));
        }
    };
    match state
        .save_drawdown_guard_override(
            enable,
            request.peak_value_dkk,
            request.notes.unwrap_or_default().trim(),
        )
        .await
    {
        Ok(value) => {
            info!(
                enabled = enable,
                peak_value_dkk = value
                    .get("peak_value_dkk")
                    .and_then(JsonValue::as_f64)
                    .unwrap_or(0.0),
                "drawdown guardrail override updated"
            );
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("drawdown guardrail override update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn update_instrument_quarantine_override(
    State(state): State<Arc<AppState>>,
    Form(request): Form<InstrumentQuarantineOverrideRequest>,
) -> Response {
    let enable = match request.operation.trim() {
        "override" => true,
        "clear_override" => false,
        operation => {
            return json_result(Err(anyhow::anyhow!(
                "Unsupported instrument quarantine operation: {operation}"
            )));
        }
    };
    match state
        .save_instrument_quarantine_override(
            &request.symbol,
            &request.side,
            &request.signature,
            enable,
            request.notes.unwrap_or_default().trim(),
        )
        .await
    {
        Ok(value) => {
            info!(
                symbol = %request.symbol,
                side = %request.side,
                signature = %request.signature,
                override_count = value
                    .get("overrides")
                    .and_then(JsonValue::as_array)
                    .map_or(0, Vec::len),
                "instrument quarantine override updated"
            );
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("instrument quarantine override update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn update_overview_integrity_acknowledgement(
    State(state): State<Arc<AppState>>,
    Form(request): Form<OverviewIntegrityAcknowledgementRequest>,
) -> Response {
    let enable = match request.operation.trim() {
        "acknowledge" => true,
        "clear_acknowledgement" => false,
        operation => {
            return json_result(Err(anyhow::anyhow!(
                "Unsupported overview integrity operation: {operation}"
            )));
        }
    };
    match state
        .save_overview_integrity_acknowledgement(
            &request.issue_key,
            &request.code,
            &request.severity,
            enable,
            request.notes.unwrap_or_default().trim(),
        )
        .await
    {
        Ok(value) => {
            info!(
                issue_key = %request.issue_key,
                code = %request.code,
                severity = %request.severity,
                acknowledgement_count = value
                    .get("acknowledgements")
                    .and_then(JsonValue::as_array)
                    .map_or(0, Vec::len),
                "overview integrity acknowledgement updated"
            );
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("overview integrity acknowledgement update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

/// A prepared lifecycle test only counts as blocking once the broker has been
/// asked about it. Axum drops a handler future when the client disconnects, so a
/// double-clicked placement can commit the prepared row and never reach Saxo;
/// that orphan otherwise blocks its precheck permanently.
///
/// Each stale row is reconciled first. Only rows Saxo does not know about are
/// abandoned — a row is never expired on a timer alone, because the same
/// interruption could have happened *after* a successful placement.
/// Confirms placed stops the broker has not yet been asked about.
///
/// A stop sitting at `placement_submitted` is invisible to the coverage audit,
/// so its position keeps appearing as an exception and a later batch retries it
/// -- which Saxo rejects, because the stop it does not know about is already
/// resting. Reconciling promotes it to the state the audit actually counts.
pub(crate) async fn confirm_unconfirmed_protective_stops(state: &AppState) {
    const CONFIRM_AFTER_SECONDS: i64 = 15;
    let pending = match state
        .unconfirmed_protective_stop_placements(CONFIRM_AFTER_SECONDS)
        .await
    {
        Ok(rows) => rows,
        Err(err) => {
            warn!("could not read unconfirmed protective stops: {err:#}");
            return;
        }
    };
    for test in pending {
        let test_id = test
            .get("id")
            .and_then(JsonValue::as_i64)
            .unwrap_or_default();
        match reconcile_sim_protective_stop_lifecycle_test(state, &test).await {
            Ok(result) => {
                let status = result
                    .get("status")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("reconciliation_pending");
                if let Err(err) = state
                    .record_protective_stop_lifecycle_reconciliation(
                        test_id,
                        status,
                        result.get("broker_order_id").and_then(JsonValue::as_str),
                        &result,
                    )
                    .await
                {
                    warn!(test_id, "could not persist stop confirmation: {err:#}");
                } else {
                    info!(test_id, status, "confirmed protective stop with broker");
                }
            }
            Err(err) => {
                warn!(test_id, "could not confirm protective stop: {err:#}");
            }
        }
    }
}

async fn resolve_stale_protective_stop_preparations(state: &AppState) {
    const STALE_AFTER_SECONDS: i64 = 90;
    let stale = match state
        .stale_protective_stop_preparations(STALE_AFTER_SECONDS)
        .await
    {
        Ok(rows) => rows,
        Err(err) => {
            warn!("could not read stale protective-stop preparations: {err:#}");
            return;
        }
    };
    for test in stale {
        let test_id = test
            .get("id")
            .and_then(JsonValue::as_i64)
            .unwrap_or_default();
        match reconcile_sim_protective_stop_lifecycle_test(state, &test).await {
            Ok(result) => {
                let status = result
                    .get("status")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("reconciliation_pending");
                let visibility = result
                    .get("broker_visibility")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if status == "reconciliation_pending" && visibility == "not_found" {
                    if let Err(err) = state.abandon_protective_stop_preparation(test_id).await {
                        warn!(test_id, "could not abandon unreached preparation: {err:#}");
                    } else {
                        info!(
                            test_id,
                            "abandoned protective-stop preparation the broker never received"
                        );
                    }
                } else {
                    // The broker does know about it. Persist what it said and
                    // leave the row active for the operator.
                    if let Err(err) = state
                        .record_protective_stop_lifecycle_reconciliation(
                            test_id,
                            status,
                            result.get("broker_order_id").and_then(JsonValue::as_str),
                            &result,
                        )
                        .await
                    {
                        warn!(test_id, "could not persist stale reconciliation: {err:#}");
                    }
                    warn!(
                        test_id,
                        status, "stale protective-stop preparation exists at the broker"
                    );
                }
            }
            Err(err) => {
                warn!(
                    test_id,
                    "could not reconcile stale protective-stop preparation: {err:#}"
                );
            }
        }
    }
}

async fn precheck_protective_stop(
    State(state): State<Arc<AppState>>,
    Form(request): Form<ProtectiveStopPrecheckRequest>,
) -> Response {
    let symbol = request.symbol.trim().to_string();
    let return_to = safe_return_to(request.return_to.as_deref());
    if request.confirm_sim_precheck.as_deref() != Some("true") {
        let _ = state
            .record_protective_stop_precheck(
                &symbol,
                request.quantity,
                request.stop_price_local,
                "confirmation_required",
                &json!({
                    "accepted": false,
                    "message": "SIM confirmation was required. No Saxo request was sent."
                }),
            )
            .await;
        return redirect_to_app(&state, return_to).into_response();
    }

    match precheck_sim_protective_stop(&state, &symbol, request.quantity, request.stop_price_local)
        .await
    {
        Ok(result) => {
            if let Err(err) = state
                .record_protective_stop_precheck(
                    &symbol,
                    request.quantity,
                    request.stop_price_local,
                    "precheck_ok",
                    &result,
                )
                .await
            {
                warn!(
                    symbol,
                    "could not record successful protective-stop precheck: {err:#}"
                );
            }
            info!(
                symbol,
                "SIM protective-stop precheck completed without placing an order"
            );
        }
        Err(err) => {
            let taxonomy = classify_execution_error("execution_failed", &err.to_string());
            warn!(symbol, "SIM protective-stop precheck rejected: {err:#}");
            if let Err(record_err) = state
                .record_protective_stop_precheck(
                    &symbol,
                    request.quantity,
                    request.stop_price_local,
                    "precheck_rejected",
                    &json!({
                        "accepted": false,
                        "error": taxonomy,
                        "message": "Saxo rejected the SIM protective-stop precheck. No order was placed."
                    }),
                )
                .await
            {
                warn!(symbol, "could not record rejected protective-stop precheck: {record_err:#}");
            }
        }
    }
    redirect_to_app(&state, return_to).into_response()
}

async fn place_protective_stop_lifecycle_test(
    State(state): State<Arc<AppState>>,
    Form(request): Form<ProtectiveStopLifecyclePlacementRequest>,
) -> Response {
    let return_to = safe_return_to(request.return_to.as_deref());
    if request.confirm_sim_placement.as_deref() != Some("true") {
        warn!(
            source_precheck_id = request.source_precheck_id,
            "SIM protective-stop placement confirmation missing"
        );
        return redirect_to_app(&state, return_to).into_response();
    }
    // Clear orphans a cancelled duplicate submit may have left behind, so a
    // retry is not blocked by a record the broker never received.
    resolve_stale_protective_stop_preparations(&state).await;
    let prepared = match state
        .prepare_protective_stop_lifecycle_test(request.source_precheck_id)
        .await
    {
        Ok(prepared) => prepared,
        Err(err) => {
            warn!(
                source_precheck_id = request.source_precheck_id,
                "could not prepare SIM protective-stop lifecycle test: {err:#}"
            );
            return redirect_to_app(&state, return_to).into_response();
        }
    };
    let test_id = prepared
        .get("id")
        .and_then(JsonValue::as_i64)
        .unwrap_or_default();
    match place_sim_protective_stop_lifecycle_test(&state, &prepared).await {
        Ok(result) => {
            let broker_order_id = result.get("broker_order_id").and_then(JsonValue::as_str);
            let status = if broker_order_id.is_some() {
                "placement_submitted"
            } else {
                "broker_state_unknown"
            };
            if let Err(err) = state
                .record_protective_stop_lifecycle_placement(
                    test_id,
                    status,
                    broker_order_id,
                    &result,
                )
                .await
            {
                warn!(
                    test_id,
                    "could not persist SIM protective-stop placement result: {err:#}"
                );
            }
            info!(
                test_id,
                ?broker_order_id,
                "manual SIM protective-stop lifecycle placement submitted"
            );
        }
        Err(err) => {
            let uncertain = protective_stop_lifecycle_error_is_state_unknown(&err);
            let status = if uncertain {
                "broker_state_unknown"
            } else {
                "placement_failed"
            };
            let result = json!({
                "accepted": false,
                "error": classify_execution_error("execution_failed", &err.to_string()),
                "safety": if uncertain {
                    "broker_state_unknown_no_automatic_retry_or_duplicate_placement"
                } else {
                    "SIM placement rejected_before_broker_confirmation"
                }
            });
            warn!(
                test_id,
                status, "SIM protective-stop lifecycle placement failed: {err:#}"
            );
            if let Err(record_err) = state
                .record_protective_stop_lifecycle_placement(test_id, status, None, &result)
                .await
            {
                warn!(
                    test_id,
                    "could not persist SIM protective-stop placement failure: {record_err:#}"
                );
            }
        }
    }
    redirect_to_app(&state, return_to).into_response()
}

/// Parsed bulk-placement form.
struct ProtectiveStopBatchForm {
    /// Upper-cased, de-duplicated symbols from the checked rows.
    symbols: Vec<String>,
    confirmed: bool,
    return_to: Option<String>,
}

/// Parses the bulk-placement body.
///
/// A checkbox column submits one repeated `symbols` field per checked row, and
/// `serde_urlencoded` -- which axum's `Form` extractor uses -- cannot
/// deserialize repeated keys into a `Vec`. It fails the whole request with
/// `invalid type: string ..., expected a sequence`, so the body is parsed
/// directly instead.
fn parse_protective_stop_batch_form(body: &str) -> ProtectiveStopBatchForm {
    let mut symbols: Vec<String> = Vec::new();
    let mut confirmed = false;
    let mut return_to = None;
    for (key, value) in form_urlencoded::parse(body.as_bytes()) {
        match key.as_ref() {
            "symbols" => {
                let symbol = value.trim().to_ascii_uppercase();
                if !symbol.is_empty() && !symbols.contains(&symbol) {
                    symbols.push(symbol);
                }
            }
            "confirm_sim_batch_placement" => confirmed = value == "true",
            "return_to" => {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    return_to = Some(value);
                }
            }
            _ => {}
        }
    }
    ProtectiveStopBatchForm {
        symbols,
        confirmed,
        return_to,
    }
}

/// Places protective stops for several positions in one operator action.
///
/// Deliberately conservative, because a bulk path turns one mistake into many:
///
/// - SIM only, and every symbol must appear as an unprotected position in the
///   read-only coverage audit with a computed stop level. Operator-supplied
///   prices are never accepted here.
/// - Strictly sequential, with Saxo's documented 1 order/second placement limit
///   respected between orders.
/// - Fail-fast. The first rejection, error, or ambiguous broker response stops
///   the whole batch. An ambiguous response is never retried, and never
///   followed by another placement, because the safe assumption is that an
///   order may already exist.
async fn place_protective_stop_batch(State(state): State<Arc<AppState>>, body: String) -> Response {
    let request = parse_protective_stop_batch_form(&body);
    let return_to = safe_return_to(request.return_to.as_deref());
    if !request.confirmed {
        warn!("SIM protective-stop batch confirmation missing; nothing was sent");
        return redirect_to_app(&state, return_to).into_response();
    }
    let environment = yaml_string(&state.config, &["saxo", "environment"])
        .unwrap_or_else(|| "sim".to_string())
        .to_ascii_lowercase();
    if environment != "sim" {
        warn!(
            environment,
            "refusing protective-stop batch placement outside SIM"
        );
        return redirect_to_app(&state, return_to).into_response();
    }

    resolve_stale_protective_stop_preparations(&state).await;
    confirm_unconfirmed_protective_stops(&state).await;

    // Saxo permits one resting sell per owned holding, so never attempt a second
    // one -- not even when local coverage still lags behind the broker.
    let already_protected = state
        .symbols_with_active_protective_stops()
        .await
        .unwrap_or_else(|err| {
            warn!("could not read active protective stops: {err:#}");
            Vec::new()
        });

    // The audit is the only source of symbols, quantities, and stop levels.
    let coverage = match state.protective_stop_coverage().await {
        Ok(coverage) => coverage,
        Err(err) => {
            warn!("could not load protective-stop coverage for batch: {err:#}");
            return redirect_to_app(&state, return_to).into_response();
        }
    };
    let requested = request.symbols.clone();
    let mut targets = Vec::new();
    for exception in coverage
        .get("exceptions")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let symbol = exception
            .get("symbol")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string();
        let key = symbol.trim().to_ascii_uppercase();
        if !requested.contains(&key) {
            continue;
        }
        if already_protected.contains(&key) {
            info!(
                symbol,
                "skipping batch stop: a protective stop already exists"
            );
            continue;
        }
        let Some(proposed) = exception
            .get("proposed_stop")
            .filter(|value| !value.is_null())
        else {
            warn!(symbol, "skipping batch stop: no computed stop level");
            continue;
        };
        let quantity = proposed
            .get("quantity")
            .and_then(JsonValue::as_f64)
            .unwrap_or_default();
        let stop_price = proposed
            .get("stop_price_local")
            .and_then(JsonValue::as_f64)
            .unwrap_or_default();
        if quantity > 0.0 && stop_price > 0.0 {
            targets.push((symbol, quantity, stop_price));
        }
    }
    targets.sort_by(|left, right| left.0.cmp(&right.0));

    let total = targets.len();
    info!(
        requested = requested.len(),
        eligible = total,
        "starting SIM protective-stop batch placement"
    );
    // Placement runs detached. A dozen orders take longer than a proxy will
    // hold a request open, and when the client disconnects axum drops the
    // handler future -- mid-batch that can place an order at Saxo and lose the
    // record of it. Observed 2026-07-25: a 12-symbol batch timed out and left an
    // orphaned preparation. The operator watches the lifecycle table instead.
    tokio::spawn(run_protective_stop_batch(state.clone(), targets));
    redirect_to_app(&state, return_to).into_response()
}

/// The outcome of one protective-stop placement attempt.
///
/// Both callers -- the operator batch and the automatic sweep -- stop on
/// anything that is not `Placed`. Working further down a list after a rejection
/// mostly repeats the same mistake against a rate-limited broker, and an
/// ambiguous placement in particular must never be followed by another order.
pub(crate) enum StopPlacementOutcome {
    Placed {
        test_id: i64,
        broker_order_id: String,
    },
    PrecheckFailed,
    PrecheckRejected,
    NotRecorded,
    PlacementFailed,
    /// The request may or may not have reached Saxo. No automatic retry.
    StateUnknown,
}

impl StopPlacementOutcome {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Placed { .. } => "placed",
            Self::PrecheckFailed => "precheck_failed",
            Self::PrecheckRejected => "precheck_rejected",
            Self::NotRecorded => "not_recorded",
            Self::PlacementFailed => "placement_failed",
            Self::StateUnknown => "broker_state_unknown",
        }
    }
}

/// Prechecks, places, and confirms exactly one broker-hosted protective stop.
///
/// Shared by the operator batch and the automatic sweep so both run the same
/// broker sequence and record the same audit trail. `source` is stored with the
/// precheck so a stop can always be traced back to what asked for it.
pub(crate) async fn place_one_protective_stop(
    state: &AppState,
    symbol: &str,
    quantity: f64,
    stop_price: f64,
    source: &str,
) -> StopPlacementOutcome {
    let precheck = match precheck_sim_protective_stop(state, symbol, quantity, stop_price).await {
        Ok(result) => result,
        Err(err) => {
            warn!(symbol, source, "protective-stop precheck failed: {err:#}");
            let _ = state
                .record_protective_stop_precheck(
                    symbol,
                    quantity,
                    stop_price,
                    "precheck_failed",
                    &json!({
                        "accepted": false,
                        "source": source,
                        "error": classify_execution_error("execution_failed", &err.to_string())
                    }),
                )
                .await;
            return StopPlacementOutcome::PrecheckFailed;
        }
    };
    let accepted = precheck
        .get("accepted")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let mut recorded = precheck.clone();
    if let Some(object) = recorded.as_object_mut() {
        object.insert("source".to_string(), json!(source));
    }
    let precheck_id = match state
        .record_protective_stop_precheck(
            symbol,
            quantity,
            stop_price,
            if accepted {
                "precheck_ok"
            } else {
                "precheck_rejected"
            },
            &recorded,
        )
        .await
    {
        Ok(id) => id,
        Err(err) => {
            warn!(symbol, source, "could not record precheck: {err:#}");
            return StopPlacementOutcome::NotRecorded;
        }
    };
    if !accepted {
        warn!(symbol, source, "protective-stop precheck rejected");
        return StopPlacementOutcome::PrecheckRejected;
    }

    let prepared = match state
        .prepare_protective_stop_lifecycle_test(precheck_id)
        .await
    {
        Ok(prepared) => prepared,
        Err(err) => {
            warn!(symbol, source, "could not prepare stop placement: {err:#}");
            return StopPlacementOutcome::NotRecorded;
        }
    };
    let test_id = prepared
        .get("id")
        .and_then(JsonValue::as_i64)
        .unwrap_or_default();
    match place_sim_protective_stop_lifecycle_test(state, &prepared).await {
        Ok(result) => {
            let broker_order_id = result
                .get("broker_order_id")
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            let status = if broker_order_id.is_some() {
                "placement_submitted"
            } else {
                "broker_state_unknown"
            };
            if let Err(err) = state
                .record_protective_stop_lifecycle_placement(
                    test_id,
                    status,
                    broker_order_id.as_deref(),
                    &result,
                )
                .await
            {
                warn!(test_id, "could not persist stop placement: {err:#}");
            }
            let Some(broker_order_id) = broker_order_id else {
                warn!(
                    symbol,
                    source, test_id, "stop placement returned no broker order id"
                );
                return StopPlacementOutcome::StateUnknown;
            };
            info!(
                symbol,
                source, test_id, broker_order_id, "protective stop placed"
            );
            // `placement_submitted` is not coverage. The audit counts a stop
            // only once Saxo reports it working, so confirm now rather than
            // leaving a table of unverified placements behind.
            let prepared_for_reconcile = json!({
                "id": test_id,
                "broker_order_id": broker_order_id,
                "external_reference": prepared.get("external_reference").cloned().unwrap_or(JsonValue::Null),
                "created_at": prepared.get("created_at").cloned().unwrap_or(JsonValue::Null),
            });
            match reconcile_sim_protective_stop_lifecycle_test(state, &prepared_for_reconcile).await
            {
                Ok(reconciled) => {
                    let status = reconciled
                        .get("status")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("reconciliation_pending");
                    if let Err(err) = state
                        .record_protective_stop_lifecycle_reconciliation(
                            test_id,
                            status,
                            reconciled
                                .get("broker_order_id")
                                .and_then(JsonValue::as_str),
                            &reconciled,
                        )
                        .await
                    {
                        warn!(test_id, "could not persist stop reconciliation: {err:#}");
                    }
                    info!(symbol, test_id, status, "protective stop reconciled");
                }
                Err(err) => {
                    // The order is placed; only confirmation failed. Leave it
                    // submitted rather than guessing -- the scheduler's
                    // confirmation sweep will resolve it.
                    warn!(
                        symbol,
                        test_id, "protective stop placed but not reconciled: {err:#}"
                    );
                }
            }
            StopPlacementOutcome::Placed {
                test_id,
                broker_order_id,
            }
        }
        Err(err) => {
            let uncertain = protective_stop_lifecycle_error_is_state_unknown(&err);
            let status = if uncertain {
                "broker_state_unknown"
            } else {
                "placement_failed"
            };
            let result = json!({
                "accepted": false,
                "source": source,
                "error": classify_execution_error("execution_failed", &err.to_string()),
                "safety": if uncertain {
                    "broker_state_unknown_no_automatic_retry_and_no_further_placements"
                } else {
                    "SIM placement rejected before broker confirmation"
                }
            });
            if let Err(record_err) = state
                .record_protective_stop_lifecycle_placement(test_id, status, None, &result)
                .await
            {
                warn!(test_id, "could not persist stop failure: {record_err:#}");
            }
            warn!(
                symbol,
                source, test_id, status, "protective stop placement failed: {err:#}"
            );
            if uncertain {
                StopPlacementOutcome::StateUnknown
            } else {
                StopPlacementOutcome::PlacementFailed
            }
        }
    }
}

/// Places one protective stop per target, sequentially, halting on the first
/// problem. Runs outside the request so a client timeout cannot interrupt it.
async fn run_protective_stop_batch(state: Arc<AppState>, targets: Vec<(String, f64, f64)>) {
    const PLACEMENT_SPACING_MS: u64 = 1_100;
    let total = targets.len();
    let mut placed = 0usize;
    for (index, (symbol, quantity, stop_price)) in targets.into_iter().enumerate() {
        if index > 0 {
            // Saxo permits one order per second per session.
            tokio::time::sleep(std::time::Duration::from_millis(PLACEMENT_SPACING_MS)).await;
        }
        match place_one_protective_stop(
            &state,
            &symbol,
            quantity,
            stop_price,
            "operator_confirmed_batch",
        )
        .await
        {
            StopPlacementOutcome::Placed { .. } => placed += 1,
            outcome => {
                warn!(
                    symbol,
                    outcome = outcome.label(),
                    "halting operator protective-stop batch"
                );
                break;
            }
        }
    }
    info!(
        placed,
        eligible = total,
        "SIM protective-stop batch placement finished"
    );
}

async fn cancel_protective_stop_lifecycle_test(
    State(state): State<Arc<AppState>>,
    Form(request): Form<ProtectiveStopLifecycleCancellationRequest>,
) -> Response {
    let return_to = safe_return_to(request.return_to.as_deref());
    if request.confirm_sim_cancellation.as_deref() != Some("true") {
        warn!(
            lifecycle_test_id = request.lifecycle_test_id,
            "SIM protective-stop cancellation confirmation missing"
        );
        return redirect_to_app(&state, return_to).into_response();
    }
    let test = match state
        .protective_stop_lifecycle_test(request.lifecycle_test_id)
        .await
    {
        Ok(Some(test)) => test,
        Ok(None) | Err(_) => return redirect_to_app(&state, return_to).into_response(),
    };
    let current_status = test
        .get("status")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    if !matches!(
        current_status,
        "placement_submitted" | "broker_working" | "reconciliation_pending"
    ) {
        warn!(
            lifecycle_test_id = request.lifecycle_test_id,
            current_status, "SIM protective-stop cancellation rejected by lifecycle state"
        );
        return redirect_to_app(&state, return_to).into_response();
    }
    match cancel_sim_protective_stop_lifecycle_test(&state, &test).await {
        Ok(result) => {
            if let Err(err) = state
                .record_protective_stop_lifecycle_cancellation(
                    request.lifecycle_test_id,
                    "cancellation_submitted",
                    &result,
                )
                .await
            {
                warn!(
                    lifecycle_test_id = request.lifecycle_test_id,
                    "could not record SIM stop cancellation: {err:#}"
                );
            }
        }
        Err(err) => {
            let uncertain = protective_stop_lifecycle_error_is_state_unknown(&err);
            let status = if uncertain {
                "reconciliation_pending"
            } else {
                "cancellation_failed"
            };
            let result = json!({
                "accepted": false,
                "error": classify_execution_error("execution_failed", &err.to_string()),
                "safety": "no_automatic_cancellation_retry_operator_must_reconcile"
            });
            if let Err(record_err) = state
                .record_protective_stop_lifecycle_cancellation(
                    request.lifecycle_test_id,
                    status,
                    &result,
                )
                .await
            {
                warn!(
                    lifecycle_test_id = request.lifecycle_test_id,
                    "could not record SIM stop cancellation failure: {record_err:#}"
                );
            }
        }
    }
    redirect_to_app(&state, return_to).into_response()
}

async fn reconcile_protective_stop_lifecycle_test(
    State(state): State<Arc<AppState>>,
    Form(request): Form<ProtectiveStopLifecycleReconcileRequest>,
) -> Response {
    let return_to = safe_return_to(request.return_to.as_deref());
    let test = match state
        .protective_stop_lifecycle_test(request.lifecycle_test_id)
        .await
    {
        Ok(Some(test)) => test,
        Ok(None) | Err(_) => return redirect_to_app(&state, return_to).into_response(),
    };
    match reconcile_sim_protective_stop_lifecycle_test(&state, &test).await {
        Ok(result) => {
            let status = result
                .get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or("reconciliation_pending");
            let broker_order_id = result.get("broker_order_id").and_then(JsonValue::as_str);
            if let Err(err) = state
                .record_protective_stop_lifecycle_reconciliation(
                    request.lifecycle_test_id,
                    status,
                    broker_order_id,
                    &result,
                )
                .await
            {
                warn!(
                    lifecycle_test_id = request.lifecycle_test_id,
                    "could not record SIM stop reconciliation: {err:#}"
                );
            }
        }
        Err(err) => {
            let result = json!({
                "status": "reconciliation_pending",
                "error": classify_execution_error("execution_failed", &err.to_string()),
                "safety": "read_only_reconciliation_no_automatic_retry"
            });
            if let Err(record_err) = state
                .record_protective_stop_lifecycle_reconciliation(
                    request.lifecycle_test_id,
                    "reconciliation_pending",
                    None,
                    &result,
                )
                .await
            {
                warn!(
                    lifecycle_test_id = request.lifecycle_test_id,
                    "could not record SIM stop reconciliation failure: {record_err:#}"
                );
            }
        }
    }
    redirect_to_app(&state, return_to).into_response()
}

async fn update_localization_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(request): Form<LocalizationSettingsRequest>,
) -> Response {
    let sso_session = json!(SsoSession::from_headers(&headers));
    let value = json!({
        "locale": clean_setting(request.locale, "en-DK"),
        "time_zone": clean_setting(request.time_zone, "Europe/Copenhagen"),
        "hour_cycle": clean_setting(request.hour_cycle, "24"),
        "week_start": clean_setting(request.week_start, "monday"),
        "group_separator": clean_setting(request.group_separator, ","),
        "decimal_separator": clean_setting(request.decimal_separator, "."),
        "measurement_system": clean_setting(request.measurement_system, "metric"),
    });
    match state.save_localization_settings(&sso_session, value).await {
        Ok(_) => {
            info!("localization settings updated");
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("localization settings update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn update_ai_settings(
    State(state): State<Arc<AppState>>,
    Form(request): Form<AiSettingsRequest>,
) -> Response {
    match state
        .save_ai_settings(&clean_setting(request.model, "openai/gpt-5.5"))
        .await
    {
        Ok(settings) => {
            info!(
                model = %settings.get("model").and_then(JsonValue::as_str).unwrap_or(""),
                "AI settings updated"
            );
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("AI settings update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn update_ai_api_key(
    State(state): State<Arc<AppState>>,
    Form(request): Form<AiApiKeyRequest>,
) -> Response {
    // The submitted key must never reach logs or the response body; only
    // the masked status is observable.
    match state
        .save_ai_api_key(request.api_key.as_deref().unwrap_or(""))
        .await
    {
        Ok(status) => {
            info!(
                source = %status.get("source").and_then(JsonValue::as_str).unwrap_or(""),
                configured = status.get("configured").and_then(JsonValue::as_bool).unwrap_or(false),
                "AI API key override updated"
            );
            redirect_to_app(&state, safe_return_to(request.return_to.as_deref())).into_response()
        }
        Err(err) => {
            warn!("AI API key update failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn saxo_auth_status(State(state): State<Arc<AppState>>) -> Json<auth::SaxoAuthStatus> {
    Json(state.saxo_auth_status().await)
}

async fn saxo_auth_start(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match auth::start_saxo_auth(&state.config, &state.config_path, &headers).await {
        Ok(start) => {
            info!(
                environment = %start.environment,
                auth_mode = %start.auth_mode,
                redirect_uri = %start.redirect_uri,
                "Saxo OAuth start created"
            );
            Json(json!({
                "status": "redirect",
                "environment": start.environment,
                "auth_mode": start.auth_mode,
                "authorize_url": start.authorize_url,
                "redirect_uri": start.redirect_uri,
                "message": "Redirecting to Saxo authorization."
            }))
            .into_response()
        }
        Err(err) => {
            warn!("Saxo OAuth start failed: {err:#}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"status": "error", "detail": err.to_string()})),
            )
                .into_response()
        }
    }
}

async fn saxo_auth_start_redirect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    match auth::start_saxo_auth(&state.config, &state.config_path, &headers).await {
        Ok(start) => {
            info!(
                environment = %start.environment,
                auth_mode = %start.auth_mode,
                redirect_uri = %start.redirect_uri,
                "redirecting to Saxo OAuth"
            );
            Redirect::temporary(&start.authorize_url).into_response()
        }
        Err(err) => {
            warn!("Saxo OAuth redirect failed: {err:#}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"status": "error", "detail": err.to_string()})),
            )
                .into_response()
        }
    }
}

async fn saxo_auth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SaxoCallbackParams>,
) -> Response {
    let mut return_to = "/".to_string();
    let result = async {
        if let Some(error) = params.error.as_deref() {
            anyhow::bail!("Saxo returned an authorization error: {error}");
        }
        let code = params
            .code
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Saxo OAuth callback did not include a code."))?;
        let state_value = params
            .state
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Saxo OAuth callback did not include state."))?;
        auth::finish_saxo_auth(&state.config, &state.config_path, code, state_value).await
    }
    .await;

    match result {
        Ok(target) => {
            return_to = target;
            if let Err(err) = state
                .persist_saxo_session_file_to_db("oauth_callback")
                .await
            {
                warn!("Saxo OAuth callback completed but database persistence failed: {err:#}");
            }
            info!(return_to = %return_to, "Saxo OAuth callback completed");
            Html(auth::oauth_callback_html(
                true,
                "Saxo authorization complete",
                "The Saxo session has been renewed and stored for the backend.",
                &return_to,
            ))
            .into_response()
        }
        Err(err) => {
            warn!("Saxo OAuth callback failed: {err:#}");
            (
                StatusCode::BAD_REQUEST,
                Html(auth::oauth_callback_html(
                    false,
                    "Saxo authorization failed",
                    &err.to_string(),
                    &return_to,
                )),
            )
                .into_response()
        }
    }
}

async fn saxo_session(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    Json(state.saxo_session_value().await)
}

async fn saxo_session_refresh(State(state): State<Arc<AppState>>) -> Response {
    match state.refresh_saxo_session().await {
        Ok(value) => {
            info!(
                status = value
                    .get("status")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("unknown"),
                "Saxo session refresh endpoint completed"
            );
            Json(value).into_response()
        }
        Err(err) => {
            warn!("Saxo session refresh endpoint failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn saxo_session_logout(State(state): State<Arc<AppState>>) -> Response {
    match state.user_logout_saxo_session().await {
        Ok(value) => {
            info!("user logout completed without clearing service-level Saxo session");
            Json(value).into_response()
        }
        Err(err) => {
            warn!("Saxo session logout no-op failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn saxo_session_disconnect(State(state): State<Arc<AppState>>) -> Response {
    match state.disconnect_saxo_session().await {
        Ok(value) => {
            info!("Saxo session disconnected and removed from durable storage");
            Json(value).into_response()
        }
        Err(err) => {
            warn!("Saxo session disconnect failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn portfolio_positions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(25);
    json_result(
        state
            .position_items(limit)
            .await
            .map(portfolio_positions_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

fn portfolio_positions_payload(items: Vec<JsonValue>) -> PortfolioPositionsPayload {
    PortfolioPositionsPayload {
        total: items.len(),
        items,
    }
}

async fn asset_ladder_history(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
    Query(params): Query<PerformanceParams>,
) -> Response {
    let range_key = params.range_key.unwrap_or_else(|| "SESSION".to_string());
    let position = state
        .position_items(250)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|row| row.get("symbol").and_then(JsonValue::as_str) == Some(symbol.as_str()));
    Json(json!({
        "symbol": symbol,
        "range_key": range_key,
        "position": position,
        "ladder_summary": {"status": "not_ported", "active_orders": 0},
        "chart": {"points": [], "error": null, "source": "rust", "has_real_data": false, "first_event_at": null},
        "markers": [],
        "active_lines": [],
        "ladder_levels": [],
        "ladder_parameters": {},
        "legend": []
    }))
    .into_response()
}

async fn portfolio_trades(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(50);
    json_result(
        state
            .portfolio_trades_items(limit)
            .await
            .map(portfolio_trades_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

fn portfolio_trades_payload(items: Vec<JsonValue>) -> PortfolioTradesPayload {
    PortfolioTradesPayload { items }
}

async fn performance(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PerformanceParams>,
) -> Response {
    let range_key = params.range_key.unwrap_or_else(|| "1D".to_string());
    info!(range_key = %range_key, "loading performance payload");
    json_result(
        state
            .performance_payload(&range_key)
            .await
            .and_then(performance_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

fn performance_payload(value: JsonValue) -> Result<PerformancePayload> {
    serde_json::from_value(value).map_err(Into::into)
}

async fn markov_signals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(100);
    json_result(
        async {
            let latest_run = state.latest_markov_run().await.unwrap_or(JsonValue::Null);
            let items = state.markov_signals(limit).await?;
            serde_json::to_value(markov_signals_payload(latest_run, items)).map_err(Into::into)
        }
        .await,
    )
}

fn markov_signals_payload(latest_run: JsonValue, items: Vec<JsonValue>) -> MarkovSignalsPayload {
    MarkovSignalsPayload { latest_run, items }
}

async fn quiver_signals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(100);
    json_result(
        async {
            let latest_run = state.latest_quiver_run().await.unwrap_or(JsonValue::Null);
            let items = state.quiver_signals(limit).await?;
            serde_json::to_value(quiver_signals_payload(latest_run, items)).map_err(Into::into)
        }
        .await,
    )
}

fn quiver_signals_payload(latest_run: JsonValue, items: Vec<JsonValue>) -> QuiverSignalsPayload {
    QuiverSignalsPayload { latest_run, items }
}

async fn market_status(State(state): State<Arc<AppState>>) -> Response {
    json_result(
        state
            .market_status_payload()
            .await
            .and_then(market_status_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

fn market_status_payload(value: JsonValue) -> Result<MarketStatusPayload> {
    serde_json::from_value(value).map_err(Into::into)
}

async fn market_watchlists(State(state): State<Arc<AppState>>) -> Json<MarketWatchlistsPayload> {
    Json(
        state
            .watchlists_payload()
            .await
            .and_then(market_watchlists_payload)
            .unwrap_or_else(|err| {
                warn!("watchlist payload degraded: {err:#}");
                market_watchlists_degraded_payload(Utc::now().to_rfc3339())
            }),
    )
}

fn market_watchlists_payload(value: JsonValue) -> Result<MarketWatchlistsPayload> {
    serde_json::from_value(value).map_err(Into::into)
}

fn market_watchlists_degraded_payload(generated_at: String) -> MarketWatchlistsPayload {
    MarketWatchlistsPayload {
        generated_at,
        cache_ttl_seconds: 300,
        universe: json!({}),
        categories: Vec::new(),
    }
}

async fn prompts(State(state): State<Arc<AppState>>) -> Json<AiPromptsPayload> {
    let latest = state
        .decision_report_items(1)
        .await
        .unwrap_or_else(|err| {
            warn!("prompt latest decision lookup failed: {err:#}");
            Vec::new()
        })
        .into_iter()
        .next();
    Json(ai_prompts_payload(Utc::now().to_rfc3339(), latest))
}

fn ai_prompts_payload(
    generated_at: String,
    latest_decision_report: Option<JsonValue>,
) -> AiPromptsPayload {
    AiPromptsPayload {
        generated_at,
        items: vec![AiPromptItem {
            kind: "rust_runtime".to_string(),
            title: "Rust Runtime".to_string(),
            status: "not_ported".to_string(),
            description: "Prompt builders still need a Rust implementation.".to_string(),
        }],
        latest_decision_report,
        latest_trading_manager_run: None,
    }
}

async fn decision_latest(State(state): State<Arc<AppState>>) -> Json<DecisionLatestPayload> {
    let report = state
        .decision_report_items(1)
        .await
        .unwrap_or_else(|err| {
            warn!("latest decision lookup failed: {err:#}");
            Vec::new()
        })
        .into_iter()
        .next();
    Json(decision_latest_payload(report))
}

fn decision_latest_payload(report: Option<JsonValue>) -> DecisionLatestPayload {
    DecisionLatestPayload {
        report,
        next_report: None,
    }
}

async fn decision_reports(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(20);
    json_result(
        state
            .decision_report_items(limit)
            .await
            .map(decision_report_list_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

fn decision_report_list_payload(items: Vec<JsonValue>) -> DecisionReportListPayload {
    DecisionReportListPayload { items }
}

fn strategy_journal_payload(items: Vec<JsonValue>) -> StrategyJournalPayload {
    StrategyJournalPayload { items }
}

fn execution_payload(
    orders: Vec<JsonValue>,
    fills: Vec<JsonValue>,
    events: Vec<JsonValue>,
) -> ExecutionPayload {
    ExecutionPayload {
        orders,
        fills,
        events,
    }
}

fn scheduler_payload(status: JsonValue, cycles: Vec<JsonValue>) -> SchedulerPayload {
    SchedulerPayload { status, cycles }
}

fn hermes_reflections_payload(items: Vec<JsonValue>) -> HermesReflectionsPayload {
    HermesReflectionsPayload { items }
}

fn hermes_experiments_payload(items: Vec<JsonValue>) -> HermesExperimentsPayload {
    HermesExperimentsPayload { items }
}

async fn decision_report_debug(
    State(state): State<Arc<AppState>>,
    Path(report_id): Path<i64>,
) -> Response {
    match state.decision_report_debug_payload(report_id).await {
        Ok(Some(payload)) => Json(payload).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"status": "not_found", "detail": "Decision Report was not found."})),
        )
            .into_response(),
        Err(err) => json_result(Err(err)),
    }
}

/// Broker lifecycle timeline for one execution order, loaded on demand.
///
/// Loading per order rather than filtering the dashboard's flat event list
/// client-side is deliberate: that list is capped at 50 rows, so any order
/// older than the most recent handful would silently render an empty timeline
/// and look like an order that never reached the broker.
async fn execution_order_events(
    State(state): State<Arc<AppState>>,
    Path(order_id): Path<i64>,
) -> Response {
    match state.execution_order_events(order_id, 200).await {
        Ok(events) => Json(json!({
            "status": "ok",
            "execution_order_id": order_id,
            "event_count": events.len(),
            "events": events
        }))
        .into_response(),
        Err(err) => json_result(Err(err)),
    }
}

async fn decision_gate_replay(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(40);
    json_result(
        state
            .decision_gate_replay(limit)
            .await
            .and_then(decision_gate_replay_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

fn decision_gate_replay_payload(value: JsonValue) -> Result<DecisionGateReplayPayload> {
    serde_json::from_value(value).map_err(Into::into)
}

async fn decision_schema() -> Response {
    Json(xai_decision::decision_report_schema_health()).into_response()
}

async fn strategy_journal(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(20);
    json_result(
        state
            .strategy_journal_items(limit)
            .await
            .map(strategy_journal_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

async fn execution(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(100);
    let orders = state.execution_orders(limit).await.unwrap_or_else(|err| {
        warn!("execution orders degraded: {err:#}");
        Vec::new()
    });
    let fills = state.execution_fills(limit).await.unwrap_or_else(|err| {
        warn!("execution fills degraded: {err:#}");
        Vec::new()
    });
    let events = state.execution_events(limit).await.unwrap_or_else(|err| {
        warn!("execution events degraded: {err:#}");
        Vec::new()
    });
    Json(execution_payload(orders, fills, events)).into_response()
}

async fn scheduler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(20);
    let status = state.scheduler_status_value().await.unwrap_or_else(|err| {
        warn!("scheduler status lookup failed: {err:#}");
        JsonValue::Null
    });
    let cycles = state.scheduler_cycles(limit).await.unwrap_or_else(|err| {
        warn!("scheduler cycles lookup failed: {err:#}");
        Vec::new()
    });
    Json(scheduler_payload(status, cycles)).into_response()
}

async fn hermes_capabilities(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_hermes_api_key(&headers) {
        return response;
    }
    Json(state.hermes_capabilities_value()).into_response()
}

async fn hermes_context(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<LimitParams>,
) -> Response {
    if let Err(response) = require_hermes_api_key(&headers) {
        return response;
    }
    let limit = params.limit.unwrap_or(20);
    json_result(state.hermes_context(limit).await)
}

async fn hermes_reflections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<LimitParams>,
) -> Response {
    if let Err(response) = require_hermes_api_key(&headers) {
        return response;
    }
    let limit = params.limit.unwrap_or(20);
    json_result(
        state
            .hermes_reflections(limit)
            .await
            .map(hermes_reflections_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

async fn create_hermes_reflection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<HermesReflectionRequest>,
) -> Response {
    if let Err(response) = require_hermes_api_key(&headers) {
        return response;
    }
    if request.summary.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "detail": "summary is required"})),
        )
            .into_response();
    }
    match state.record_hermes_reflection(&request).await {
        Ok(value) => {
            info!("Hermes reflection recorded");
            (StatusCode::CREATED, Json(value)).into_response()
        }
        Err(err) => json_result(Err(err)),
    }
}

async fn hermes_experiments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<LimitParams>,
) -> Response {
    if let Err(response) = require_hermes_api_key(&headers) {
        return response;
    }
    let limit = params.limit.unwrap_or(20);
    json_result(
        state
            .hermes_experiments(limit)
            .await
            .map(hermes_experiments_payload)
            .and_then(|payload| serde_json::to_value(payload).map_err(Into::into)),
    )
}

async fn create_hermes_experiment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<HermesExperimentRequest>,
) -> Response {
    if let Err(response) = require_hermes_api_key(&headers) {
        return response;
    }
    if request.hypothesis.trim().is_empty() || request.changed_variable_path.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "detail": "hypothesis and changed_variable_path are required"
            })),
        )
            .into_response();
    }
    let review_context = match state
        .inspect_hermes_experiment_proposal(&request.changed_variable_path)
        .await
    {
        Ok(review_context) => review_context,
        Err(err) => return json_result(Err(err)),
    };
    if let Some(existing) = review_context
        .get("exact_duplicate")
        .filter(|value| !value.is_null())
    {
        warn!(
            changed_variable_path = %request.changed_variable_path,
            existing_experiment_id = %existing.get("id").and_then(JsonValue::as_str).unwrap_or(""),
            "Hermes duplicate experiment proposal rejected"
        );
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "duplicate",
                "detail": "An active or pending Hermes experiment already covers this changed_variable_path. Record the candidate in reflection proposed_actions instead of creating a duplicate proposal.",
                "changed_variable_path": request.changed_variable_path.trim(),
                "existing_experiment": existing
            })),
        )
            .into_response();
    }
    match state.record_hermes_experiment(&request).await {
        Ok(mut value) => {
            if let Some(object) = value.as_object_mut() {
                object.insert("review_context".to_string(), review_context);
            }
            info!(
                changed_variable_path = %request.changed_variable_path,
                "Hermes experiment proposal recorded"
            );
            (StatusCode::CREATED, Json(value)).into_response()
        }
        Err(err) => json_result(Err(err)),
    }
}

async fn transition_hermes_experiment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(experiment_id): Path<String>,
    Form(request): Form<HermesExperimentTransitionRequest>,
) -> Response {
    let sso = SsoSession::from_headers(&headers);
    let actor = sso
        .user
        .as_ref()
        .map(|user| user.email.as_str())
        .unwrap_or("operator");
    match state
        .transition_hermes_experiment(
            &experiment_id,
            &request.action,
            request.notes.as_deref(),
            actor,
        )
        .await
    {
        Ok(value) => {
            info!(
                experiment_id,
                action = %request.action,
                actor,
                "Hermes experiment transition recorded"
            );
            let redirect = request.return_to.as_deref().unwrap_or("/?view=hermes");
            if value
                .get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or("ok")
                == "ok"
            {
                Redirect::to(safe_return_to(Some(redirect))).into_response()
            } else {
                json_result(Ok(value))
            }
        }
        Err(err) => {
            warn!(
                experiment_id,
                action = %request.action,
                "Hermes experiment transition failed: {err:#}"
            );
            json_result(Err(err))
        }
    }
}

async fn action_not_ported() -> Response {
    warn!("blocked not-ported trading mutation endpoint");
    safe_not_ported(
        "Trading and scheduler mutations are disabled in the Rust runtime until the execution engine is fully ported.",
    )
}

async fn action_run_daily_indicators(State(state): State<Arc<AppState>>) -> Response {
    match crate::daily_indicators::run_daily_indicators_now(&state).await {
        Ok(summary) => {
            info!(
                status = summary
                    .get("status")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("unknown"),
                "manual daily indicators run completed"
            );
            json_result(Ok(summary))
        }
        Err(err) => {
            error!("manual daily indicators run failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn action_run_performance_benchmarks(State(state): State<Arc<AppState>>) -> Response {
    match crate::performance_benchmarks::run_performance_benchmarks_now(&state).await {
        Ok(summary) => {
            info!(?summary, "performance benchmark refresh completed");
            (StatusCode::OK, Json(summary)).into_response()
        }
        Err(err) => {
            warn!("performance benchmark refresh failed: {err:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status": "error", "detail": err.to_string()})),
            )
                .into_response()
        }
    }
}

async fn action_run_quiver_signals(State(state): State<Arc<AppState>>) -> Response {
    match crate::quiver::run_quiver_signals_now(&state).await {
        Ok(summary) => {
            info!(
                status = summary
                    .get("status")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("unknown"),
                "manual Quiver signal run completed"
            );
            json_result(Ok(summary))
        }
        Err(err) => {
            error!("manual Quiver signal run failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn action_generate_decision_report(State(state): State<Arc<AppState>>) -> Response {
    action_generate_decision_report_with_mode(state, DecisionReportActionMode::Live).await
}

async fn action_generate_decision_report_dry_run(State(state): State<Arc<AppState>>) -> Response {
    action_generate_decision_report_with_mode(state, DecisionReportActionMode::DryRun).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecisionReportActionMode {
    Live,
    DryRun,
}

async fn action_generate_decision_report_with_mode(
    state: Arc<AppState>,
    mode: DecisionReportActionMode,
) -> Response {
    // The full pipeline (prompt build, provider call with a 600s budget,
    // Trading Manager, execution queue) takes minutes — far longer than the
    // browser/tunnel keeps a request open, and a dropped connection would
    // cancel the pipeline mid-flight. Run it detached and return at once;
    // the decisions view polls until the new report lands.
    match state.claim_manual_decision_report().await {
        Ok(true) => {}
        Ok(false) => {
            info!("manual decision report already in flight; not starting another");
            return redirect_to_app(&state, "/?view=decisions").into_response();
        }
        Err(err) => {
            error!("manual decision report claim failed: {err:#}");
            return json_result(Err(err));
        }
    }
    let task_state = state.clone();
    tokio::spawn(async move {
        run_manual_decision_report_pipeline(&task_state, mode).await;
        if let Err(err) = task_state.release_manual_decision_report_claim().await {
            warn!("releasing manual decision report claim failed: {err:#}");
        }
    });
    redirect_to_app(&state, "/?view=decisions").into_response()
}

async fn run_manual_decision_report_pipeline(state: &AppState, mode: DecisionReportActionMode) {
    let report_result = match mode {
        DecisionReportActionMode::Live => xai_decision::submit_manual_decision_report(state).await,
        DecisionReportActionMode::DryRun => {
            xai_decision::submit_manual_dry_run_decision_report(state).await
        }
    };
    match report_result {
        Ok(report) => {
            let id = report.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
            let mut immediate = json!({"status": decision_report_action_skip_status(mode)});
            if decision_report_action_runs_immediate_pipeline(mode, &report) {
                let manager = run_trading_manager_cycle(state).await;
                let execution = match &manager {
                    Ok(_) => run_saxo_execution_queue(state).await,
                    Err(err) => Err(anyhow::anyhow!(
                        "Trading Manager failed before execution: {err:#}"
                    )),
                };
                immediate = json!({
                    "trading_manager": manager.as_ref().map(|value| value.clone()).unwrap_or_else(|err| json!({"status": "error", "error": err.to_string()})),
                    "execution_queue": execution.as_ref().map(|value| value.clone()).unwrap_or_else(|err| json!({"status": "error", "error": err.to_string()})),
                });
                if let Err(err) = manager {
                    warn!(
                        report_id = id,
                        "manual report immediate Trading Manager run failed: {err:#}"
                    );
                }
                if let Err(err) = execution {
                    warn!(
                        report_id = id,
                        "manual report immediate execution queue run failed: {err:#}"
                    );
                }
            }
            info!(
                report_id = id,
                status = report
                    .get("status")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("unknown"),
                dry_run = mode == DecisionReportActionMode::DryRun,
                immediate = %immediate,
                "manual xAI decision report pipeline completed"
            );
        }
        Err(err) => {
            error!("manual decision report generation failed: {err:#}");
        }
    }
}

fn decision_report_action_runs_immediate_pipeline(
    mode: DecisionReportActionMode,
    report: &JsonValue,
) -> bool {
    mode == DecisionReportActionMode::Live
        && report.get("status").and_then(JsonValue::as_str) == Some("completed")
        && report.get("pulse_mode").and_then(JsonValue::as_str) == Some("execution_eligible")
        && report.get("queue_eligible").and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_i64().map(|value| value > 0))
        }) == Some(true)
}

fn decision_report_action_skip_status(mode: DecisionReportActionMode) -> &'static str {
    match mode {
        DecisionReportActionMode::Live => "not_run",
        DecisionReportActionMode::DryRun => "dry_run_no_side_effects",
    }
}

async fn action_process_queue(State(state): State<Arc<AppState>>) -> Response {
    match run_saxo_execution_queue(&state).await {
        Ok(result) => {
            info!(
                submitted = result
                    .get("submitted")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0),
                failed = result
                    .get("failed")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0),
                "manual Saxo execution queue processor completed"
            );
            Json(result).into_response()
        }
        Err(err) => {
            error!("manual Saxo execution queue processor failed: {err:#}");
            json_result(Err(err))
        }
    }
}

async fn manage_order_not_ported(Path(order_id): Path<i64>) -> Response {
    warn!(order_id, "blocked not-ported order management endpoint");
    safe_not_ported(&format!(
        "Order management for order {order_id} is disabled in the Rust runtime until Saxo replace/cancel handling is ported."
    ))
}

async fn reset_sim_from_live_csv(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Response {
    // For now we do a basic check via config
    let env = yaml_string(&state.config, &["saxo", "environment"]).unwrap_or_default();
    if env.to_uppercase() != "SIM" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"status": "forbidden", "message": "This reset is only allowed when saxo.environment=SIM"})),
        ).into_response();
    }

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut cash_dkk: Option<f64> = None;
    let mut also_sync = false;
    let mut confirm = false;

    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_bytes = Some(field.bytes().await.unwrap_or_default().to_vec());
        } else if name == "cash_dkk" {
            if let Ok(text) = field.text().await {
                cash_dkk = text.parse().ok();
            }
        } else if name == "also_sync_sim_broker" {
            also_sync = true;
        } else if name == "confirm_wipe" {
            confirm = true;
        }
    }

    if !confirm {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "message": "Confirmation checkbox is required"})),
        )
            .into_response();
    }
    if cash_dkk.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "message": "cash_dkk is required"})),
        )
            .into_response();
    }
    if file_bytes.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "message": "CSV file is required"})),
        )
            .into_response();
    }

    let filename = "uploaded-live-positioner.csv"; // We can improve filename later
    match state
        .perform_sim_reset_from_live_csv(
            &file_bytes.unwrap(),
            cash_dkk.unwrap(),
            filename,
            also_sync,
        )
        .await
    {
        Ok(result) => Json(json!({
            "status": "ok",
            "batch_id": result.batch_id,
            "imported_positions": result.imported_positions,
            "cash_dkk": result.cash_dkk,
            "also_sync_sim_broker": also_sync,
            "message": "SIM portfolio has been reset from the uploaded Live export."
        }))
        .into_response(),
        Err(err) => {
            tracing::error!("SIM portfolio reset failed: {err:#}");
            let full_msg = format!("{err:#}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "message": full_msg
                })),
            )
                .into_response()
        }
    }
}

fn require_hermes_api_key(headers: &HeaderMap) -> std::result::Result<(), Response> {
    let expected = env::var("HERMES_DAYTRADER_API_KEY")
        .or_else(|_| env::var("DAYTRADER_HERMES_API_KEY"))
        .unwrap_or_default();
    if expected.trim().is_empty() {
        warn!("Hermes API blocked because HERMES_DAYTRADER_API_KEY is not configured");
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "disabled",
                "detail": "Hermes API key is not configured."
            })),
        )
            .into_response());
    }

    let header_key = headers
        .get("x-hermes-api-key")
        .and_then(|value| value.to_str().ok());
    let bearer_key = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    if header_key == Some(expected.as_str()) || bearer_key == Some(expected.as_str()) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"status": "unauthorized", "detail": "Invalid Hermes API key."})),
        )
            .into_response())
    }
}

fn safe_not_ported(message: &str) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"status": "not_ported", "message": message})),
    )
        .into_response()
}

fn json_result(result: Result<JsonValue>) -> Response {
    // Rust's `Result<T, E>` is the common "success or error" return type. Here
    // we convert it into an HTTP response at the API boundary.
    match result {
        Ok(value) => Json(value).into_response(),
        Err(err) => {
            error!("API handler failed: {err:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status": "error", "detail": err.to_string()})),
            )
                .into_response()
        }
    }
}

fn normalize_view(value: Option<&str>) -> String {
    match value.unwrap_or("overview").to_lowercase().as_str() {
        "overview" | "portfolio" => "overview".to_string(),
        "eod" | "end-of-day" => "eod".to_string(),
        "performance" | "market" | "watchlists" | "markov" | "quiver" | "decisions"
        | "execution" | "prompts" | "hermes" | "tuning" => {
            value.unwrap_or("overview").to_lowercase()
        }
        _ => "overview".to_string(),
    }
}

fn normalize_performance_range(value: Option<&str>) -> String {
    match value.unwrap_or("1D").to_uppercase().as_str() {
        "1D" | "1W" | "1M" | "3M" | "YTD" | "1Y" | "ALL" => value.unwrap_or("1D").to_uppercase(),
        _ => "1D".to_string(),
    }
}

fn normalize_execution_page(value: Option<i64>) -> i64 {
    value.unwrap_or(1).clamp(1, 1_000)
}

fn normalize_markov_page(value: Option<i64>) -> i64 {
    value.unwrap_or(1).clamp(1, 1_000)
}

fn normalize_quiver_page(value: Option<i64>) -> i64 {
    value.unwrap_or(1).clamp(1, 1_000)
}

fn normalize_scheduler_page(value: Option<i64>) -> i64 {
    value.unwrap_or(1).clamp(1, 1_000)
}

fn clean_setting(value: Option<String>, fallback: &str) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn safe_return_to(value: Option<&str>) -> &str {
    let Some(value) = value else {
        return "/";
    };
    if value.starts_with('/') && !value.starts_with("//") {
        value
    } else {
        "/"
    }
}

fn redirect_to_app(state: &AppState, path: &str) -> Redirect {
    let base = public_base_path(&state.config);
    if base.is_empty() {
        Redirect::to(path)
    } else if path == "/" {
        Redirect::to(&base)
    } else {
        Redirect::to(&format!("{}{}", base.trim_end_matches('/'), path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_payload_identifies_the_runtime_and_build() {
        let health = health_payload();

        assert_eq!(health.status, "ok");
        assert_eq!(health.runtime, "rust-dioxus");
        assert_eq!(health.git_sha, crate::build_info::git_sha());

        let serialized = serde_json::to_value(&health).expect("runtime health serializes");
        assert_eq!(serialized["status"], "ok");
        assert_eq!(serialized["runtime"], "rust-dioxus");
    }

    #[tokio::test]
    async fn auth_session_serializes_only_the_header_derived_sso_contract() {
        let anonymous = auth_session(HeaderMap::new()).await.0;
        assert!(!anonymous.authenticated);
        assert!(anonymous.user.is_none());

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-daytrader-user-email",
            axum::http::HeaderValue::from_static("operator@example.com"),
        );
        headers.insert(
            "x-daytrader-user-name",
            axum::http::HeaderValue::from_static("Trading Operator"),
        );

        let session = auth_session(headers).await.0;
        assert!(session.authenticated);
        assert_eq!(
            session.user.as_ref().map(|user| user.email.as_str()),
            Some("operator@example.com")
        );
        assert_eq!(
            session.user.as_ref().map(|user| user.name.as_str()),
            Some("Trading Operator")
        );

        let serialized = serde_json::to_value(&session).expect("SSO session serializes");
        assert_eq!(serialized["authenticated"], true);
        assert_eq!(serialized["user"]["email"], "operator@example.com");
        assert_eq!(serialized["user"]["name"], "Trading Operator");
    }

    #[test]
    fn cash_buffer_preview_preserves_the_enforced_baseline_contract() {
        let settings = CashBufferSettings {
            min_cash_buffer_pct: 0.02,
            max_deployment_pct: 0.95,
            reinvestment_pressure_threshold_pct: 0.05,
            source: "config".to_string(),
            updated_at: None,
            config_default_min_cash_buffer_pct: 0.02,
        };

        let preview = cash_buffer_preview(settings, 0.04);
        assert_eq!(preview.min_cash_buffer_pct, 0.04);
        assert_eq!(preview.config_default_min_cash_buffer_pct, 0.02);
        assert_eq!(preview.source, "request_preview");
        assert!(preview.updated_at.is_none());

        let serialized = serde_json::to_value(preview).expect("cash buffer preview serializes");
        assert_eq!(serialized["min_cash_buffer_pct"], 0.04);
        assert_eq!(serialized["config_default_min_cash_buffer_pct"], 0.02);
        assert_eq!(serialized["source"], "request_preview");
    }

    #[test]
    fn localization_response_serializes_the_resolved_public_preferences() {
        let prefs = LocalizationPrefs {
            locale: "en-DK".to_string(),
            time_zone: "Europe/Copenhagen".to_string(),
            hour_cycle: crate::localization::HourCycle::H24,
            week_start: crate::localization::WeekStart::Monday,
            group_separator: ",".to_string(),
            decimal_separator: ".".to_string(),
            measurement_system: "metric".to_string(),
        };

        let serialized = serde_json::to_value(prefs).expect("localization preferences serialize");
        assert_eq!(serialized["locale"], "en-DK");
        assert_eq!(serialized["time_zone"], "Europe/Copenhagen");
        assert_eq!(serialized["hour_cycle"], "h24");
        assert_eq!(serialized["week_start"], "monday");
        assert_eq!(serialized["measurement_system"], "metric");
    }

    #[test]
    fn ai_prompts_response_keeps_the_typed_operator_envelope() {
        let payload = ai_prompts_payload(
            "2026-08-01T09:15:00Z".to_string(),
            Some(json!({"id": 42, "status": "completed"})),
        );

        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].kind, "rust_runtime");
        assert_eq!(payload.latest_trading_manager_run, None);

        let serialized = serde_json::to_value(payload).expect("AI prompts payload serializes");
        assert_eq!(serialized["generated_at"], "2026-08-01T09:15:00Z");
        assert_eq!(serialized["items"][0]["status"], "not_ported");
        assert_eq!(serialized["latest_decision_report"]["id"], 42);
        assert!(serialized["latest_trading_manager_run"].is_null());
    }

    #[test]
    fn decision_latest_response_keeps_the_typed_polling_envelope() {
        let payload = decision_latest_payload(Some(json!({"id": 42, "status": "completed"})));

        assert_eq!(
            payload
                .report
                .as_ref()
                .and_then(|report| report["id"].as_i64()),
            Some(42)
        );
        assert!(payload.next_report.is_none());

        let serialized = serde_json::to_value(payload).expect("latest decision payload serializes");
        assert_eq!(serialized["report"]["status"], "completed");
        assert!(serialized["next_report"].is_null());
    }

    #[test]
    fn decision_report_list_response_keeps_the_typed_list_envelope() {
        let payload = decision_report_list_payload(vec![json!({"id": 42}), json!({"id": 43})]);

        assert_eq!(payload.items.len(), 2);

        let serialized = serde_json::to_value(payload).expect("Decision Report list serializes");
        assert_eq!(serialized["items"][0]["id"], 42);
        assert_eq!(serialized["items"][1]["id"], 43);
    }

    #[test]
    fn portfolio_positions_response_keeps_the_typed_counted_list_envelope() {
        let payload = portfolio_positions_payload(vec![
            json!({"symbol": "TSLA:xnas"}),
            json!({"symbol": "NOVO-B:xcse"}),
        ]);

        assert_eq!(payload.total, 2);
        assert_eq!(payload.items.len(), 2);

        let serialized =
            serde_json::to_value(payload).expect("portfolio positions payload serializes");
        assert_eq!(serialized["total"], 2);
        assert_eq!(serialized["items"][0]["symbol"], "TSLA:xnas");
    }

    #[test]
    fn portfolio_trades_response_keeps_the_typed_list_envelope() {
        let payload = portfolio_trades_payload(vec![
            json!({"symbol": "TSLA:xnas", "side": "BUY"}),
            json!({"symbol": "NOVO-B:xcse", "side": "SELL"}),
        ]);

        assert_eq!(payload.items.len(), 2);

        let serialized =
            serde_json::to_value(payload).expect("portfolio trades payload serializes");
        assert_eq!(serialized["items"][0]["symbol"], "TSLA:xnas");
        assert_eq!(serialized["items"][1]["side"], "SELL");
    }

    #[test]
    fn markov_signals_response_keeps_the_typed_run_and_list_envelope() {
        let payload = markov_signals_payload(
            json!({"id": 19, "status": "completed"}),
            vec![json!({"symbol": "TSLA:xnas", "signal": "long"})],
        );

        assert_eq!(payload.latest_run["id"], 19);
        assert_eq!(payload.items.len(), 1);

        let serialized = serde_json::to_value(payload).expect("Markov signals payload serializes");
        assert_eq!(serialized["latest_run"]["status"], "completed");
        assert_eq!(serialized["items"][0]["symbol"], "TSLA:xnas");
    }

    #[test]
    fn quiver_signals_response_keeps_the_typed_run_and_list_envelope() {
        let payload = quiver_signals_payload(
            json!({"id": 23, "status": "completed"}),
            vec![json!({"symbol": "TSLA:xnas", "signal": "buy"})],
        );

        assert_eq!(payload.latest_run["id"], 23);
        assert_eq!(payload.items.len(), 1);

        let serialized = serde_json::to_value(payload).expect("Quiver signals payload serializes");
        assert_eq!(serialized["latest_run"]["status"], "completed");
        assert_eq!(serialized["items"][0]["signal"], "buy");
    }

    #[test]
    fn strategy_journal_response_keeps_the_typed_list_envelope() {
        let payload = strategy_journal_payload(vec![
            json!({"created_at": "2026-08-01T10:15:00Z", "event": "reflection"}),
            json!({"created_at": "2026-08-01T16:15:00Z", "event": "outcome"}),
        ]);

        assert_eq!(payload.items.len(), 2);

        let serialized =
            serde_json::to_value(payload).expect("strategy journal payload serializes");
        assert_eq!(serialized["items"][0]["event"], "reflection");
        assert_eq!(serialized["items"][1]["event"], "outcome");
    }

    #[test]
    fn execution_response_keeps_the_typed_read_only_envelope() {
        let payload = execution_payload(
            vec![json!({"id": 42, "status": "broker_working"})],
            vec![json!({"id": 7, "symbol": "TSLA:xnas"})],
            vec![json!({"event_type": "precheck_completed"})],
        );

        assert_eq!(payload.orders.len(), 1);
        assert_eq!(payload.fills.len(), 1);
        assert_eq!(payload.events.len(), 1);

        let serialized = serde_json::to_value(payload).expect("execution payload serializes");
        assert_eq!(serialized["orders"][0]["status"], "broker_working");
        assert_eq!(serialized["fills"][0]["symbol"], "TSLA:xnas");
        assert_eq!(serialized["events"][0]["event_type"], "precheck_completed");
    }

    #[test]
    fn scheduler_response_keeps_the_typed_status_and_cycle_envelope() {
        let payload = scheduler_payload(
            json!({"last_cycle_status": "ok", "next_due_at": "2026-08-01T17:15:00Z"}),
            vec![json!({"id": 9, "status": "ok"})],
        );

        assert_eq!(payload.status["last_cycle_status"], "ok");
        assert_eq!(payload.cycles.len(), 1);

        let serialized = serde_json::to_value(payload).expect("scheduler payload serializes");
        assert_eq!(serialized["status"]["next_due_at"], "2026-08-01T17:15:00Z");
        assert_eq!(serialized["cycles"][0]["status"], "ok");
    }

    #[test]
    fn hermes_reflections_response_keeps_the_typed_list_envelope() {
        let payload = hermes_reflections_payload(vec![json!({
            "id": "daily-reflection-2026-08-01",
            "summary": "No one-variable experiment proposed."
        })]);

        assert_eq!(payload.items.len(), 1);

        let serialized =
            serde_json::to_value(payload).expect("Hermes reflections payload serializes");
        assert_eq!(serialized["items"][0]["id"], "daily-reflection-2026-08-01");
        assert_eq!(
            serialized["items"][0]["summary"],
            "No one-variable experiment proposed."
        );
    }

    #[test]
    fn hermes_experiments_response_keeps_the_typed_list_envelope() {
        let payload = hermes_experiments_payload(vec![json!({
            "id": "experiment-2026-08-01",
            "status": "pending_review",
            "changed_variable_path": "strategy.swing.technical_gate"
        })]);

        assert_eq!(payload.items.len(), 1);

        let serialized =
            serde_json::to_value(payload).expect("Hermes experiments payload serializes");
        assert_eq!(serialized["items"][0]["id"], "experiment-2026-08-01");
        assert_eq!(serialized["items"][0]["status"], "pending_review");
    }

    #[test]
    fn market_watchlists_response_keeps_the_typed_outer_contract() {
        let payload = market_watchlists_payload(json!({
            "generated_at": "2026-08-01T18:00:00Z",
            "cache_ttl_seconds": 300,
            "universe": {"source": "configured_analysis_universe"},
            "categories": [{
                "key": "nordic",
                "label": "Nordics",
                "items": [{"symbol": "NOVO-B:xcse"}]
            }],
        }))
        .expect("watchlists compatibility payload has the public contract");

        let serialized =
            serde_json::to_value(payload).expect("market watchlists payload serializes");
        assert_eq!(serialized["cache_ttl_seconds"], 300);
        assert_eq!(
            serialized["universe"]["source"],
            "configured_analysis_universe"
        );
        assert_eq!(
            serialized["categories"][0]["items"][0]["symbol"],
            "NOVO-B:xcse"
        );
    }

    #[test]
    fn market_watchlists_degraded_payload_keeps_the_empty_category_contract() {
        let payload = market_watchlists_degraded_payload("2026-08-01T18:00:00Z".to_string());

        assert_eq!(payload.cache_ttl_seconds, 300);
        assert!(payload.categories.is_empty());

        let serialized =
            serde_json::to_value(payload).expect("degraded market watchlists payload serializes");
        assert_eq!(serialized["generated_at"], "2026-08-01T18:00:00Z");
        assert_eq!(serialized["categories"], json!([]));
    }

    #[test]
    fn market_status_response_keeps_the_typed_outer_contract() {
        let payload = market_status_payload(json!({
            "items": [{"market": "US", "open_analysis_window_active": true}],
            "summary": {"analysis_window_active": true, "active_markets": ["US"]},
            "scheduler": {"last_cycle_status": "ok"},
            "price_monitor": {"status": "fresh"},
        }))
        .expect("market status compatibility payload has the public contract");

        let serialized = serde_json::to_value(payload).expect("market status payload serializes");
        assert_eq!(serialized["items"][0]["market"], "US");
        assert_eq!(serialized["summary"]["active_markets"], json!(["US"]));
        assert_eq!(serialized["scheduler"]["last_cycle_status"], "ok");
        assert_eq!(serialized["price_monitor"]["status"], "fresh");
    }

    #[test]
    fn performance_response_keeps_the_typed_outer_contract() {
        let payload = performance_payload(json!({
            "range_key": "1D",
            "history": [{"recorded_at": "2026-08-01T18:00:00Z", "total_market_value_dkk": 300000.0}],
            "summary": {
                "points": 2,
                "first_recorded_at": "2026-08-01T10:00:00Z",
                "latest_recorded_at": "2026-08-01T12:00:00Z",
                "first_total_market_value_dkk": 295000.0,
                "latest_total_market_value_dkk": 300000.0,
                "change_dkk": 5000.0,
                "daily_pnl_dkk": 250.0,
                "position_count": 20,
                "range_return_pct": 1.6949152542,
                "range_max_drawdown_pct": -0.5,
                "confidence": {
                    "status": "current",
                    "valid_points": 2,
                    "latest_recorded_at": "2026-08-01T12:00:00Z",
                    "latest_snapshot_type": "runtime_current",
                    "latest_source": "test",
                    "age_minutes": 0,
                    "scope": "account_value_only",
                },
                "unreliable_cost_basis_points": 0,
            },
            "benchmarks": {"status": "available"},
            "goal_tracking": {
                "weekly_target_dkk": 880.0,
                "monthly_target_dkk": 3800.0,
                "basis": "Local portfolio-value history baseline.",
                "periods": {
                    "week": {
                        "status": "ready",
                        "pnl_dkk": 440.0,
                        "target_dkk": 880.0,
                        "progress_pct": 0.5,
                        "baseline_value_dkk": 299560.0,
                        "period_start_utc": "2026-07-27T00:00:00Z",
                    },
                    "month": {
                        "status": "pending_baseline",
                        "pnl_dkk": null,
                        "target_dkk": 3800.0,
                        "progress_pct": null,
                        "baseline_value_dkk": null,
                        "period_start_utc": "2026-08-01T00:00:00Z",
                    },
                    "since_reset": {
                        "status": "ready",
                        "pnl_dkk": 5000.0,
                        "return_pct": 1.6949152542,
                        "baseline_value_dkk": 295000.0,
                        "baseline_recorded_at": "2026-08-01T10:00:00Z",
                    },
                },
            },
            "snapshot_evidence": {
                "status": "partial",
                "range_key": "1D",
                "aggregate_snapshot_count": 4,
                "covered_snapshot_count": 2,
                "missing_legacy_snapshot_count": 2,
                "coverage_pct": 50.0,
                "snapshots_with_position_rows": 2,
                "position_evidence_row_count": 40,
                "first_covered_at": "2026-08-01T10:00:00Z",
                "latest_covered_at": "2026-08-01T12:00:00Z",
                "latest_snapshot": {
                    "status": "available",
                    "snapshot": {
                        "snapshot_id": 42,
                        "recorded_at": "2026-08-01T12:00:00Z",
                        "snapshot_type": "scheduler_cycle",
                        "source": "test",
                        "position_count": 2,
                        "invested_market_value_dkk": 1500.0,
                        "total_cost_basis_dkk": 1200.0,
                        "total_unrealised_pnl_dkk": 300.0,
                    },
                    "items": [{
                        "symbol": "EXMPL:xnas",
                        "isin": "US0000000001",
                        "currency": "USD",
                        "quantity": 10.0,
                        "price_local": 100.0,
                        "fx_rate_to_dkk": 6.5,
                        "cost_basis_local": 75.0,
                        "cost_basis_dkk": 4875.0,
                        "market_value_dkk": 6500.0,
                        "unrealised_pnl_dkk": 1625.0,
                    }],
                    "safety": "local_retained_position_snapshot_read_no_provider_hermes_gate_or_order_authority",
                    "interpretation": "Stored evidence, not a live broker portfolio.",
                },
                "latest_change": {
                    "status": "available",
                    "current_snapshot": {
                        "snapshot_id": 42,
                        "recorded_at": "2026-08-01T12:00:00Z",
                        "snapshot_type": "scheduler_cycle",
                        "source": "test",
                        "position_count": 2,
                        "invested_market_value_dkk": 1500.0,
                        "total_cost_basis_dkk": 1200.0,
                        "total_unrealised_pnl_dkk": 300.0,
                    },
                    "previous_snapshot": {
                        "snapshot_id": 41,
                        "recorded_at": "2026-08-01T10:00:00Z",
                        "snapshot_type": "scheduler_cycle",
                        "source": "test",
                        "position_count": 1,
                        "invested_market_value_dkk": 1000.0,
                        "total_cost_basis_dkk": 900.0,
                        "total_unrealised_pnl_dkk": 100.0,
                    },
                    "opened": [{
                        "symbol": "NEW:xnas",
                        "quantity_before": 0.0,
                        "quantity_after": 2.0,
                        "quantity_change": 2.0,
                        "market_value_change_dkk": 500.0,
                        "cost_basis_change_dkk": 300.0,
                    }],
                    "closed": [],
                    "resized": [],
                    "opened_count": 1,
                    "closed_count": 0,
                    "resized_count": 0,
                    "unchanged_quantity_count": 1,
                    "net_market_value_change_dkk": 500.0,
                    "net_cost_basis_change_dkk": 300.0,
                    "safety": "local_retained_position_snapshot_comparison_no_provider_hermes_gate_or_order_authority",
                    "interpretation": "Stored comparison only.",
                },
                "detail_retention": "all_cycle_snapshots_for_90_days_then_final_stored_snapshot_per_utc_date",
                "integrity": {
                    "status": "aligned",
                    "checked_snapshot_count": 2,
                    "structural_mismatch_count": 0,
                    "structural_mismatches": [],
                    "broker_derived_unrealised_difference_count": 1,
                    "broker_derived_unrealised_differences": [{
                        "snapshot_id": 42,
                        "recorded_at": "2026-08-01T12:00:00Z",
                        "difference_dkk": 25.0,
                        "aggregate_unrealised_pnl_dkk": 325.0,
                        "recomputed_unrealised_pnl_dkk": 300.0,
                        "interpretation": "aggregate_uses_broker_derived_unrealised_pnl",
                    }],
                    "tolerance": {"absolute_dkk": 0.01, "relative": 0.000001},
                    "safety": "local_aggregate_and_position_snapshot_comparison_no_provider_hermes_gate_or_order_authority",
                },
                "safety": "local_snapshot_evidence_read_no_provider_hermes_gate_or_order_authority",
                "interpretation": "Coverage identifies retained position evidence.",
            },
        }))
        .expect("performance compatibility payload has the public contract");

        let serialized = serde_json::to_value(payload).expect("performance payload serializes");
        assert_eq!(serialized["range_key"], "1D");
        assert_eq!(serialized["history"][0]["total_market_value_dkk"], 300000.0);
        assert_eq!(serialized["summary"]["change_dkk"], 5000.0);
        assert_eq!(serialized["summary"]["confidence"]["status"], "current");
        assert_eq!(serialized["benchmarks"]["status"], "available");
        assert_eq!(
            serialized["goal_tracking"]["periods"]["week"]["progress_pct"],
            0.5
        );
        assert!(serialized["goal_tracking"]["periods"]["month"]["pnl_dkk"].is_null());
        assert_eq!(serialized["snapshot_evidence"]["coverage_pct"], 50.0);
        assert_eq!(serialized["snapshot_evidence"]["covered_snapshot_count"], 2);
        assert_eq!(
            serialized["snapshot_evidence"]["latest_snapshot"]["snapshot"]["snapshot_id"],
            42
        );
        assert_eq!(
            serialized["snapshot_evidence"]["latest_snapshot"]["items"][0]["market_value_dkk"],
            6500.0
        );
        assert_eq!(
            serialized["snapshot_evidence"]["latest_change"]["status"],
            "available"
        );
        assert_eq!(
            serialized["snapshot_evidence"]["latest_change"]["previous_snapshot"]["snapshot_id"],
            41
        );
        assert_eq!(
            serialized["snapshot_evidence"]["latest_change"]["opened"][0]["quantity_change"],
            2.0
        );
        assert_eq!(
            serialized["snapshot_evidence"]["integrity"]["broker_derived_unrealised_difference_count"],
            1
        );
        assert_eq!(
            serialized["snapshot_evidence"]["integrity"]["broker_derived_unrealised_differences"]
                [0]["difference_dkk"],
            25.0
        );
    }

    #[test]
    fn decision_gate_replay_response_keeps_the_typed_outer_contract() {
        let payload = decision_gate_replay_payload(json!({
            "status": "available",
            "run_count": 3,
            "scenarios": [{"variable_path": "strategy.swing.markov_gate.min_signed_signal"}],
            "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
            "interpretation": "A target-gate clear is not an approval.",
            "support_risk_evidence": {"status": "collecting"},
        }))
        .expect("decision gate replay compatibility payload has the public contract");

        let serialized =
            serde_json::to_value(payload).expect("decision gate replay payload serializes");
        assert_eq!(serialized["status"], "available");
        assert_eq!(serialized["run_count"], 3);
        assert_eq!(
            serialized["scenarios"][0]["variable_path"],
            "strategy.swing.markov_gate.min_signed_signal"
        );
        assert_eq!(serialized["support_risk_evidence"]["status"], "collecting");
    }

    #[test]
    fn live_completed_decision_report_runs_immediate_pipeline() {
        let report = json!({
            "id": 42,
            "status": "completed",
            "pulse_mode": "execution_eligible",
            "queue_eligible": true,
        });

        assert!(decision_report_action_runs_immediate_pipeline(
            DecisionReportActionMode::Live,
            &report
        ));
    }

    #[test]
    fn dry_run_completed_decision_report_does_not_run_immediate_pipeline() {
        let report = json!({
            "id": 42,
            "status": "completed",
            "pulse_mode": "shadow",
            "queue_eligible": false,
        });

        assert!(!decision_report_action_runs_immediate_pipeline(
            DecisionReportActionMode::DryRun,
            &report
        ));
        assert_eq!(
            decision_report_action_skip_status(DecisionReportActionMode::DryRun),
            "dry_run_no_side_effects"
        );
    }

    #[test]
    fn live_non_completed_decision_report_does_not_run_immediate_pipeline() {
        for status in ["deferred", "xai_error", "provider_error", ""] {
            let report = json!({
                "id": 42,
                "status": status,
                "pulse_mode": "execution_eligible",
                "queue_eligible": true,
            });

            assert!(!decision_report_action_runs_immediate_pipeline(
                DecisionReportActionMode::Live,
                &report
            ));
        }
    }

    #[test]
    fn completed_shadow_decision_report_never_runs_the_immediate_pipeline() {
        let report = json!({
            "id": 42,
            "status": "completed",
            "pulse_mode": "shadow",
            "queue_eligible": false,
        });
        assert!(!decision_report_action_runs_immediate_pipeline(
            DecisionReportActionMode::Live,
            &report
        ));
    }

    #[test]
    fn normalizes_execution_page_to_a_bounded_positive_value() {
        assert_eq!(normalize_execution_page(None), 1);
        assert_eq!(normalize_execution_page(Some(0)), 1);
        assert_eq!(normalize_execution_page(Some(4)), 4);
        assert_eq!(normalize_execution_page(Some(9_999)), 1_000);
    }

    #[test]
    fn normalizes_markov_page_to_a_bounded_positive_value() {
        assert_eq!(normalize_markov_page(None), 1);
        assert_eq!(normalize_markov_page(Some(-3)), 1);
        assert_eq!(normalize_markov_page(Some(7)), 7);
        assert_eq!(normalize_markov_page(Some(9_999)), 1_000);
    }

    #[test]
    fn normalizes_quiver_page_to_a_bounded_positive_value() {
        assert_eq!(normalize_quiver_page(None), 1);
        assert_eq!(normalize_quiver_page(Some(-3)), 1);
        assert_eq!(normalize_quiver_page(Some(7)), 7);
        assert_eq!(normalize_quiver_page(Some(9_999)), 1_000);
    }

    #[test]
    fn normalizes_scheduler_page_to_a_bounded_positive_value() {
        assert_eq!(normalize_scheduler_page(None), 1);
        assert_eq!(normalize_scheduler_page(Some(-3)), 1);
        assert_eq!(normalize_scheduler_page(Some(7)), 7);
        assert_eq!(normalize_scheduler_page(Some(9_999)), 1_000);
    }

    /// A checkbox column submits one repeated `symbols` field per checked row.
    /// axum's `Form` extractor uses `serde_urlencoded`, which cannot map
    /// repeated keys onto a `Vec` and fails the request outright with
    /// `invalid type: string "LMND:xnys", expected a sequence` -- observed by
    /// the operator on 2026-07-25.
    #[test]
    fn batch_form_parses_repeated_symbol_fields() {
        let body = "return_to=%2F%3Fview%3Dexecution&symbols=LMND%3Axnys&symbols=ASML%3Axnas\
                    &symbols=lmnd%3Axnys&symbols=+&confirm_sim_batch_placement=true";
        let parsed = parse_protective_stop_batch_form(body);
        assert_eq!(
            parsed.symbols,
            vec!["LMND:XNYS".to_string(), "ASML:XNAS".to_string()],
            "symbols are decoded, upper-cased, de-duplicated, and blanks dropped"
        );
        assert!(parsed.confirmed);
        assert_eq!(parsed.return_to.as_deref(), Some("/?view=execution"));
    }

    /// Confirmation is opt-in. Anything other than an explicit `true` must place
    /// nothing, including a missing field or an unchecked box.
    #[test]
    fn batch_form_requires_explicit_confirmation() {
        assert!(!parse_protective_stop_batch_form("symbols=V%3Axnys").confirmed);
        assert!(
            !parse_protective_stop_batch_form("symbols=V%3Axnys&confirm_sim_batch_placement=false")
                .confirmed
        );
        assert!(parse_protective_stop_batch_form("").symbols.is_empty());
    }
}
