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
        AiSettingsRequest, CashBufferRequest, HermesExperimentRequest,
        HermesExperimentTransitionRequest, HermesReflectionRequest,
        InstrumentQuarantineOverrideRequest, LimitParams, LocalizationSettingsRequest,
        MonthlyLossBreakerOverrideRequest, OverviewIntegrityAcknowledgementRequest,
        PerformanceParams, SaxoCallbackParams, ViewParams,
    },
    saxo_order::run_saxo_execution_queue,
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
        .route("/api/decision/schema", get(decision_schema))
        .route("/api/strategy-journal", get(strategy_journal))
        .route("/api/execution", get(execution))
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
            "/api/actions/daily-indicators",
            post(action_run_daily_indicators),
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

async fn health() -> Json<JsonValue> {
    Json(health_payload())
}

fn health_payload() -> JsonValue {
    json!({
        "status": "ok",
        "runtime": "rust-dioxus",
        "git_sha": crate::build_info::git_sha(),
    })
}

async fn overview(State(state): State<Arc<AppState>>) -> Response {
    json_result(state.overview_payload().await)
}

async fn auth_session(headers: HeaderMap) -> Json<JsonValue> {
    Json(json!(SsoSession::from_headers(&headers)))
}

async fn localization(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Json<JsonValue> {
    let sso_session = json!(SsoSession::from_headers(&headers));
    let prefs = state
        .localization_for_user(
            LocalizationPrefs::from_headers_and_config(&headers, &state.config),
            &sso_session,
        )
        .await;
    Json(prefs.to_json())
}

async fn cash_buffer_settings(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    Json(state.cash_buffer_value())
}

async fn update_cash_buffer(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CashBufferRequest>,
) -> Json<JsonValue> {
    let mut value = state.cash_buffer_value();
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "min_cash_buffer_pct".to_string(),
            JsonValue::from(request.min_cash_buffer_pct),
        );
        obj.insert("source".to_string(), JsonValue::from("request_preview"));
    }
    Json(value)
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

async fn saxo_auth_status(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    Json(state.saxo_auth_status_value().await)
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
            .map(|items| json!({"total": items.len(), "items": items})),
    )
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
            .map(|items| json!({"items": items})),
    )
}

async fn performance(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PerformanceParams>,
) -> Response {
    let range_key = params.range_key.unwrap_or_else(|| "1D".to_string());
    info!(range_key = %range_key, "loading performance payload");
    json_result(state.performance_payload(&range_key).await)
}

async fn markov_signals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(100);
    json_result(
        async {
            Ok::<JsonValue, anyhow::Error>(json!({
                "latest_run": state.latest_markov_run().await.unwrap_or(JsonValue::Null),
                "items": state.markov_signals(limit).await?
            }))
        }
        .await,
    )
}

async fn quiver_signals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParams>,
) -> Response {
    let limit = params.limit.unwrap_or(100);
    json_result(
        async {
            Ok::<JsonValue, anyhow::Error>(json!({
                "latest_run": state.latest_quiver_run().await.unwrap_or(JsonValue::Null),
                "items": state.quiver_signals(limit).await?
            }))
        }
        .await,
    )
}

async fn market_status(State(state): State<Arc<AppState>>) -> Response {
    json_result(state.market_status_payload().await)
}

async fn market_watchlists(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    Json(state.watchlists_payload().await.unwrap_or_else(|err| {
        warn!("watchlist payload degraded: {err:#}");
        json!({"generated_at": Utc::now().to_rfc3339(), "categories": []})
    }))
}

async fn prompts(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    let latest = state
        .decision_report_items(1)
        .await
        .unwrap_or_else(|err| {
            warn!("prompt latest decision lookup failed: {err:#}");
            Vec::new()
        })
        .into_iter()
        .next();
    Json(json!({
        "generated_at": Utc::now().to_rfc3339(),
        "items": [{"kind": "rust_runtime", "title": "Rust Runtime", "status": "not_ported", "description": "Prompt builders still need a Rust implementation."}],
        "latest_decision_report": latest,
        "latest_trading_manager_run": null
    }))
}

async fn decision_latest(State(state): State<Arc<AppState>>) -> Response {
    let report = state
        .decision_report_items(1)
        .await
        .unwrap_or_else(|err| {
            warn!("latest decision lookup failed: {err:#}");
            Vec::new()
        })
        .into_iter()
        .next();
    Json(json!({"report": report, "next_report": null})).into_response()
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
            .map(|items| json!({"items": items})),
    )
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
            .map(|items| json!({"items": items})),
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
    Json(json!({"orders": orders, "fills": fills, "events": events})).into_response()
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
    Json(json!({"status": status, "cycles": cycles})).into_response()
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
            .map(|items| json!({"items": items})),
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
            .map(|items| json!({"items": items})),
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
    match state
        .find_duplicate_hermes_experiment(&request.changed_variable_path)
        .await
    {
        Ok(Some(existing)) => {
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
        Ok(None) => {}
        Err(err) => return json_result(Err(err)),
    }
    match state.record_hermes_experiment(&request).await {
        Ok(value) => {
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
    match xai_decision::submit_manual_decision_report(&state).await {
        Ok(report) => {
            let id = report.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
            let mut immediate = json!({"status": decision_report_action_skip_status(mode)});
            if decision_report_action_runs_immediate_pipeline(mode, &report) {
                let manager = run_trading_manager_cycle(&state).await;
                let execution = match &manager {
                    Ok(_) => run_saxo_execution_queue(&state).await,
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
                "manual deferred xAI decision report submitted"
            );
            redirect_to_app(&state, &format!("/?view=decisions&report_id={id}")).into_response()
        }
        Err(err) => {
            error!("manual decision report generation failed: {err:#}");
            json_result(Err(err))
        }
    }
}

fn decision_report_action_runs_immediate_pipeline(
    mode: DecisionReportActionMode,
    report: &JsonValue,
) -> bool {
    mode == DecisionReportActionMode::Live
        && report.get("status").and_then(JsonValue::as_str) == Some("completed")
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
        | "execution" | "prompts" | "hermes" => value.unwrap_or("overview").to_lowercase(),
        _ => "overview".to_string(),
    }
}

fn normalize_performance_range(value: Option<&str>) -> String {
    match value.unwrap_or("1D").to_uppercase().as_str() {
        "1D" | "1W" | "1M" | "3M" | "YTD" | "1Y" | "ALL" => value.unwrap_or("1D").to_uppercase(),
        _ => "1D".to_string(),
    }
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

        assert_eq!(health.get("status").and_then(JsonValue::as_str), Some("ok"));
        assert_eq!(
            health.get("runtime").and_then(JsonValue::as_str),
            Some("rust-dioxus")
        );
        assert_eq!(
            health.get("git_sha").and_then(JsonValue::as_str),
            Some(crate::build_info::git_sha())
        );
    }

    #[test]
    fn live_completed_decision_report_runs_immediate_pipeline() {
        let report = json!({"id": 42, "status": "completed"});

        assert!(decision_report_action_runs_immediate_pipeline(
            DecisionReportActionMode::Live,
            &report
        ));
    }

    #[test]
    fn dry_run_completed_decision_report_does_not_run_immediate_pipeline() {
        let report = json!({"id": 42, "status": "completed"});

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
            let report = json!({"id": 42, "status": status});

            assert!(!decision_report_action_runs_immediate_pipeline(
                DecisionReportActionMode::Live,
                &report
            ));
        }
    }
}
