use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    sync::{OnceLock, RwLock},
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use reqwest::header;
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use tracing::{error, info, warn};
use url::Url;

use crate::{
    auth,
    config::{database_url, yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{clamp_limit, json_f64, json_i64, pct, row_to_json, sql_escape, value_f64, value_i64},
    localization::LocalizationPrefs,
    models::{DashboardView, HermesExperimentRequest, HermesReflectionRequest},
};

#[derive(Clone)]
pub struct AppState {
    pub config_path: PathBuf,
    pub config: YamlValue,
    pub db_url: String,
    pub pool: AnyPool,
}

static SAXO_EXCHANGE_CALENDAR_CACHE: OnceLock<RwLock<Option<SaxoExchangeCalendarCache>>> =
    OnceLock::new();

#[derive(Clone, Debug)]
struct SaxoExchangeCalendarCache {
    checked_date: NaiveDate,
    checked_at: DateTime<Utc>,
    exchanges: HashMap<String, SaxoExchangeCalendar>,
    source: String,
}

#[derive(Clone, Debug)]
struct SaxoExchangeCalendar {
    exchange_id: String,
    name: Option<String>,
    timezone_id: Option<String>,
    sessions: Vec<SaxoExchangeSession>,
}

#[derive(Clone, Debug)]
struct SaxoExchangeSession {
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    state: String,
}

#[derive(Clone, Debug)]
struct ExchangeDaySession {
    open_at: DateTime<Utc>,
    close_at: DateTime<Utc>,
}

fn redacted_database_url(value: &str) -> String {
    // Logs should explain where the app connects without leaking credentials.
    // `Url` is a structured parser, so this is safer than replacing arbitrary text.
    let Ok(mut url) = Url::parse(value) else {
        return "<unparseable database url>".to_string();
    };
    if url.password().is_some() {
        let _ = url.set_password(Some("***"));
    }
    url.to_string()
}

fn runtime_id(prefix: &str) -> String {
    format!("{prefix}-{}", Utc::now().timestamp_micros())
}

fn sql_optional_text(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => format!("'{}'", sql_escape(value)),
        None => "NULL".to_string(),
    }
}

fn json_text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn performance_range_limit(range_key: &str) -> i64 {
    // Rust match expressions are similar to Python's match/case or a JS switch, but they
    // must cover every possible input. The final `_` arm is the default case.
    match range_key {
        "1D" => 120,
        "1W" => 600,
        "1M" => 2500,
        "3M" => 5000,
        "YTD" => 5000,
        "1Y" => 5000,
        "ALL" => 5000,
        _ => 120,
    }
}

fn hermes_experiment_next_status(current_status: &str, action: &str) -> Option<&'static str> {
    match (current_status, action.trim()) {
        ("pending_review", "approve_paper") => Some("approved_paper"),
        ("pending_review", "reject") => Some("rejected"),
        ("approved_paper", "activate_paper") => Some("active_paper"),
        ("approved_paper", "reject") => Some("rejected"),
        ("active_paper", "approve_sim") => Some("approved_sim"),
        ("active_paper", "mark_paper_failed") => Some("paper_failed"),
        ("active_paper", "reject") => Some("rejected"),
        ("approved_sim", "activate_sim") => Some("active_sim"),
        ("approved_sim", "reject") => Some("rejected"),
        ("active_sim", "ready_for_promotion") => Some("ready_for_promotion"),
        ("active_sim", "mark_sim_failed") => Some("sim_failed"),
        ("active_sim", "reject") => Some("rejected"),
        ("ready_for_promotion", "promote") => Some("promoted"),
        ("ready_for_promotion", "reject") => Some("rejected"),
        _ => None,
    }
}

impl AppState {
    // Associated functions are like static/class methods. `Self` means
    // `AppState`, so this returns a fully initialized application state.
    pub async fn load() -> Result<Self> {
        let config_path =
            env::var("DAYTRADER_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        let config_path = PathBuf::from(config_path);
        info!(config_path = %config_path.display(), "loading application config");
        let config_text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading config {}", config_path.display()))?;
        let config: YamlValue = serde_yaml::from_str(&config_text)
            .with_context(|| format!("parsing config {}", config_path.display()))?;
        let db_url = database_url(&config, &config_path)?;
        let safe_db_url = redacted_database_url(&db_url);
        info!(database_url = %safe_db_url, "connecting to database");
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .with_context(|| format!("connecting to database {safe_db_url}"))?;
        info!(database_url = %safe_db_url, "database connection pool ready");
        let state = Self {
            config_path,
            config,
            db_url,
            pool,
        };
        state.ensure_runtime_state_schema().await?;
        if let Err(err) = state.sync_saxo_session_storage().await {
            warn!("Saxo session database sync skipped during startup: {err:#}");
        }
        Ok(state)
    }

    pub async fn dashboard_view(
        &self,
        localization: LocalizationPrefs,
        sso_session: JsonValue,
        active_view: String,
        performance_range: String,
        selected_report_id: Option<i64>,
    ) -> DashboardView {
        let overview = self.overview_payload().await.unwrap_or_else(|err| {
            error!("overview load failed: {err:#}");
            json!({})
        });
        let positions = self.position_items(25).await.unwrap_or_else(|err| {
            warn!("dashboard positions degraded: {err:#}");
            Vec::new()
        });
        let orders = self.execution_orders(80).await.unwrap_or_else(|err| {
            warn!("dashboard execution queue degraded: {err:#}");
            Vec::new()
        });
        let execution_fills = self.execution_fills(50).await.unwrap_or_else(|err| {
            warn!("dashboard execution fills degraded: {err:#}");
            Vec::new()
        });
        let execution_events = self.execution_events(50).await.unwrap_or_else(|err| {
            warn!("dashboard execution events degraded: {err:#}");
            Vec::new()
        });
        let mut reports = self.decision_report_items(20).await.unwrap_or_else(|err| {
            warn!("dashboard decision reports degraded: {err:#}");
            Vec::new()
        });
        let selected_decision = match selected_report_id {
            Some(report_id) => {
                let selected = if let Some(row) = reports
                    .iter()
                    .find(|row| row.get("id").and_then(JsonValue::as_i64) == Some(report_id))
                    .cloned()
                {
                    row
                } else {
                    self.decision_report_item(report_id)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(JsonValue::Null)
                };
                if !selected.is_null()
                    && !reports
                        .iter()
                        .any(|row| row.get("id").and_then(JsonValue::as_i64) == Some(report_id))
                {
                    reports.insert(0, selected.clone());
                }
                selected
            }
            None => reports.first().cloned().unwrap_or(JsonValue::Null),
        };
        let journal_entries = self.strategy_journal_items(20).await.unwrap_or_else(|err| {
            warn!("dashboard end-of-day journal degraded: {err:#}");
            Vec::new()
        });
        let scheduler_cycles = self.scheduler_cycles(12).await.unwrap_or_else(|err| {
            warn!("dashboard scheduler cycles degraded: {err:#}");
            Vec::new()
        });
        let hermes_reflections = self.hermes_reflections(20).await.unwrap_or_else(|err| {
            warn!("dashboard Hermes reflections degraded: {err:#}");
            Vec::new()
        });
        let hermes_experiments = self.hermes_experiments(20).await.unwrap_or_else(|err| {
            warn!("dashboard Hermes experiments degraded: {err:#}");
            Vec::new()
        });
        let active_strategy_baseline =
            self.active_strategy_baseline().await.unwrap_or_else(|err| {
                warn!("dashboard active strategy baseline degraded: {err:#}");
                JsonValue::Null
            });
        let markov_signals = self.markov_signals(80).await.unwrap_or_else(|err| {
            warn!("dashboard Markov signals degraded: {err:#}");
            Vec::new()
        });
        let latest_markov_run = self.latest_markov_run().await.unwrap_or_else(|err| {
            warn!("dashboard latest Markov run degraded: {err:#}");
            JsonValue::Null
        });
        let performance_history = self
            .performance_history_with_current(&performance_range, 5000)
            .await
            .unwrap_or_else(|err| {
                warn!("dashboard performance history degraded: {err:#}");
                Vec::new()
            });
        let performance_summary = self.performance_summary(&performance_history);
        let market_status = self.market_status_payload().await.unwrap_or_else(|err| {
            warn!("dashboard market status degraded: {err:#}");
            json!({"items": [], "summary": {"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []}})
        });
        let watchlists = self.watchlists_payload().await.unwrap_or_else(|err| {
            warn!("dashboard watchlists degraded: {err:#}");
            json!({"generated_at": Utc::now().to_rfc3339(), "categories": []})
        });
        let latest_decision = reports.first().cloned().unwrap_or(JsonValue::Null);
        let summary = overview
            .get("portfolio_summary")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let after_tax_summary = overview
            .get("after_tax_summary")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let execution = overview
            .get("execution")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let saxo_auth = overview
            .get("saxo_auth")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let saxo_auth_object = saxo_auth.as_object().cloned().unwrap_or_default();

        DashboardView {
            app_name: yaml_string(&self.config, &["app", "project_name"])
                .unwrap_or_else(|| "saxo-rust".to_string()),
            environment: yaml_string(&self.config, &["app", "environment"])
                .unwrap_or_else(|| "local".to_string()),
            db_label: self.db_url.clone(),
            total_value_dkk: json_f64(&summary, "total_market_value_dkk"),
            invested_value_dkk: json_f64(&summary, "invested_market_value_dkk"),
            cash_dkk: json_f64(&summary, "cash_balance_dkk"),
            initial_cash_dkk: json_f64(&summary, "initial_cash_dkk"),
            cash_from_trades_dkk: json_f64(&summary, "cash_from_trades_dkk"),
            unrealised_pnl_dkk: json_f64(&summary, "total_unrealised_pnl_dkk"),
            unrealised_after_tax_dkk: json_f64(&after_tax_summary, "unrealised_pnl_after_tax_dkk"),
            daily_pnl_dkk: json_f64(&summary, "total_daily_pnl_dkk"),
            position_count: json_i64(&summary, "position_count"),
            execution_mode: execution
                .get("mode")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown")
                .to_string(),
            execution_adapter: execution
                .get("adapter")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown")
                .to_string(),
            saxo_status: saxo_auth
                .get("status_text")
                .or_else(|| saxo_auth.get("status"))
                .and_then(JsonValue::as_str)
                .unwrap_or("not connected")
                .to_string(),
            saxo_auth: JsonValue::Object(saxo_auth_object),
            sso_session,
            localization,
            active_view,
            performance_range,
            selected_report_id,
            positions,
            orders,
            execution_fills,
            execution_events,
            reports,
            journal_entries,
            scheduler_cycles,
            hermes_reflections,
            hermes_experiments,
            active_strategy_baseline,
            markov_signals,
            latest_markov_run,
            performance_history,
            performance_summary,
            market_status,
            watchlists,
            latest_decision,
            selected_decision,
        }
    }

    pub async fn overview_payload(&self) -> Result<JsonValue> {
        // `&self` is a borrowed receiver: callers can use AppState without
        // transferring ownership, similar to passing an object reference in
        // Python or JavaScript.
        let latest_history = self
            .first_json(
                "SELECT recorded_at, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count FROM portfolio_value_history ORDER BY recorded_at DESC LIMIT 1",
            )
            .await?
            .unwrap_or_else(|| json!({}));
        let latest_batch = self.latest_batch_id().await?;
        let broker_positions_available = self.broker_positions_available().await.unwrap_or(false);
        let aggregate = if broker_positions_available {
            self.position_aggregate(latest_batch.as_deref()).await?
        } else if latest_history.as_object().is_some_and(|o| !o.is_empty()) {
            latest_history.clone()
        } else {
            self.position_aggregate(latest_batch.as_deref()).await?
        };
        let total_value = value_f64(&aggregate, "total_market_value_dkk");
        let cash_summary = self.cash_summary_from_ledger().await?;
        let initial_cash = aggregate
            .get("initial_cash_dkk")
            .map(|_| value_f64(&aggregate, "initial_cash_dkk"))
            .unwrap_or_else(|| value_f64(&cash_summary, "initial_cash_dkk"));
        let cash_from_trades = aggregate
            .get("cash_from_trades_dkk")
            .map(|_| value_f64(&aggregate, "cash_from_trades_dkk"))
            .unwrap_or_else(|| value_f64(&cash_summary, "cash_from_trades_dkk"));
        let max_daily_orders =
            yaml_i64(&self.config, &["execution", "max_daily_orders"]).unwrap_or(0);
        let executed_today = self.executed_orders_today().await.unwrap_or(0);
        let decision_refresh = crate::xai_decision::decision_pulse_summary(self);

        Ok(json!({
            "app": {
                "project_name": yaml_string(&self.config, &["app", "project_name"]),
                "environment": yaml_string(&self.config, &["app", "environment"]),
                "config_path": self.config_path.display().to_string(),
                "runtime": "rust-dioxus"
            },
            "execution": {
                "mode": yaml_string(&self.config, &["execution", "mode"]),
                "adapter": yaml_string(&self.config, &["execution", "adapter"]),
                "require_approval_live": yaml_bool(&self.config, &["execution", "require_approval_live"]).unwrap_or(true),
                "max_daily_orders": max_daily_orders,
                "daily_order_capacity": {
                    "max": max_daily_orders,
                    "used": executed_today,
                    "remaining": (max_daily_orders - executed_today).max(0)
                },
                "counts": self.execution_counts().await.unwrap_or_else(|_| json!({
                    "queued": 0,
                    "pending_approval": 0,
                    "broker_live": 0,
                    "failed": 0
                })),
            },
            "portfolio_summary": {
                "recorded_at": aggregate.get("recorded_at").cloned().unwrap_or(JsonValue::Null),
                "total_market_value_dkk": total_value,
                "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
                "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
                "initial_cash_dkk": initial_cash,
                "cash_from_trades_dkk": cash_from_trades,
                "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
                "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
                "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
                "position_count": value_i64(&aggregate, "position_count"),
            },
            "after_tax_summary": {
                "unrealised_pnl_after_tax_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
                "estimated_tax_dkk": 0.0
            },
            "goal_tracking": self.goal_tracking(total_value, initial_cash),
            "integrity": {"healthy": true, "warnings": [], "mismatches": [], "unreconciled_orders": []},
            "analysis_summary": self.market_status_payload().await.unwrap_or_else(|_| json!({"summary": {"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []}})).get("summary").cloned().unwrap_or_else(|| json!({"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []})),
            "latest_decision": self.latest_decision_summary().await.unwrap_or_else(|_| json!({"id": null, "created_at": null, "status": null})),
            "scheduler_status": self.scheduler_status_value().await.unwrap_or(JsonValue::Null),
            "scheduler_health": {"status": "ok", "message": "Rust scheduler maintains Saxo sessions, submits/polls deferred xAI decision reports, runs the Trading Manager, refreshes daily Markov regime signals when due, and creates due end-of-day journals."},
            "trading_manager": {
                "status": "available",
                "latest_run": self.latest_trading_manager_run().await.unwrap_or(JsonValue::Null)
            },
            "markov_method": {
                "status": "available",
                "config": crate::markov_method::markov_config_json_for_state(self),
                "latest_run": self.latest_markov_run().await.unwrap_or(JsonValue::Null),
            },
            "saxo_auth": self.saxo_auth_status_value().await,
            "settings": {"cash_buffer": self.cash_buffer_value()},
            "refresh": {
                "price_poll_interval_minutes": yaml_i64(&self.config, &["price_monitor", "poll_interval_minutes"]).unwrap_or(1),
                "scheduler_poll_interval_minutes": yaml_i64(&self.config, &["scheduler", "poll_interval_minutes"]).unwrap_or(10),
                "decision_cadence": "rust_dashboard",
                "decision_cadence_label": "Rust dashboard",
                "decision_pulses": decision_refresh.get("pulses").cloned().unwrap_or_else(|| json!([])),
                "next_decision_pulse_at": decision_refresh.get("next_pulse_at").cloned().unwrap_or(JsonValue::Null),
                "next_decision_pulse_label": decision_refresh.get("next_pulse_label").cloned().unwrap_or(JsonValue::Null)
            }
        }))
    }

    pub async fn performance_payload(&self, range_key: &str) -> Result<JsonValue> {
        let history = self
            .performance_history_with_current(range_key, performance_range_limit(range_key))
            .await?;
        let latest = history.last().cloned().unwrap_or_else(|| json!({}));
        let total = value_f64(&latest, "total_market_value_dkk");
        let initial_cash =
            yaml_f64(&self.config, &["portfolio", "initial_cash_dkk"]).unwrap_or(0.0);
        Ok(json!({
            "range_key": range_key,
            "history": history,
            "summary": self.performance_summary(&history),
            "goal_tracking": self.goal_tracking(total, initial_cash)
        }))
    }

    pub fn performance_summary(&self, history: &[JsonValue]) -> JsonValue {
        let first = history.first();
        let latest = history.last();
        let first_total = first
            .map(|row| value_f64(row, "total_market_value_dkk"))
            .unwrap_or(0.0);
        let latest_total = latest
            .map(|row| value_f64(row, "total_market_value_dkk"))
            .unwrap_or(0.0);
        let latest_daily = latest
            .map(|row| value_f64(row, "total_daily_pnl_dkk"))
            .unwrap_or(0.0);
        let latest_positions = latest
            .map(|row| value_i64(row, "position_count"))
            .unwrap_or(0);
        json!({
            "points": history.len(),
            "first_recorded_at": first.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
            "latest_recorded_at": latest.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
            "first_total_market_value_dkk": first_total,
            "latest_total_market_value_dkk": latest_total,
            "change_dkk": latest_total - first_total,
            "daily_pnl_dkk": latest_daily,
            "position_count": latest_positions
        })
    }

    pub async fn market_status_payload(&self) -> Result<JsonValue> {
        let calendar_refresh = match self.refresh_saxo_exchange_calendars_if_stale().await {
            Ok(value) => value,
            Err(err) => {
                warn!("Saxo exchange calendar refresh skipped: {err:#}");
                json!({"status": "error", "error": err.to_string()})
            }
        };
        let items = self.market_exchange_rows();
        let scheduler = self
            .scheduler_status_value()
            .await
            .unwrap_or(JsonValue::Null);
        let cycle = scheduler
            .get("last_cycle_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let manager_status = cycle
            .get("trading_manager")
            .and_then(|value| value.get("manager_status"))
            .cloned()
            .unwrap_or(JsonValue::Null);
        let open_active_markets = market_names_where(&items, "open_analysis_window_active");
        let close_active_markets = market_names_where(&items, "close_analysis_window_active");
        let pre_sync_markets = market_names_where(&items, "pre_analysis_sync_active");
        let active_markets = open_active_markets
            .iter()
            .chain(close_active_markets.iter())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let summary = json!({
            "analysis_window_active": !active_markets.is_empty(),
            "active_markets": active_markets,
            "active_windows": manager_status.get("active_pulses").cloned().unwrap_or_else(|| json!([])),
            "open_active_markets": open_active_markets,
            "close_active_markets": close_active_markets,
            "pre_sync_markets": pre_sync_markets,
            "last_cycle_status": scheduler.get("last_cycle_status").cloned().unwrap_or(JsonValue::Null),
            "last_heartbeat_at": scheduler.get("last_heartbeat_at").cloned().unwrap_or(JsonValue::Null),
            "next_pulse_at": manager_status.get("next_pulse_at").cloned().unwrap_or(JsonValue::Null),
            "next_pulse_label": manager_status.get("next_pulse_label").cloned().unwrap_or(JsonValue::Null),
            "calendar_refresh": calendar_refresh,
        });
        Ok(json!({
            "items": items,
            "summary": summary,
            "scheduler": scheduler
        }))
    }

    pub async fn refresh_saxo_exchange_calendars_if_stale(&self) -> Result<JsonValue> {
        let today = Utc::now().date_naive();
        if let Some(cache) = current_saxo_exchange_calendar_cache() {
            if cache.checked_date == today {
                return Ok(json!({
                    "status": "fresh",
                    "source": cache.source,
                    "checked_at": cache.checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "exchange_count": cache.exchanges.len(),
                }));
            }
        }

        let cache = self
            .fetch_saxo_exchange_calendar_cache(today)
            .await
            .context("refreshing Saxo exchange calendar cache")?;
        let result = json!({
            "status": "refreshed",
            "source": cache.source,
            "checked_at": cache.checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "exchange_count": cache.exchanges.len(),
        });
        let lock = saxo_exchange_calendar_cache_lock();
        *lock
            .write()
            .map_err(|_| anyhow!("Saxo exchange calendar cache lock is poisoned"))? = Some(cache);
        Ok(result)
    }

    async fn fetch_saxo_exchange_calendar_cache(
        &self,
        checked_date: NaiveDate,
    ) -> Result<SaxoExchangeCalendarCache> {
        self.refresh_saxo_session()
            .await
            .context("refreshing Saxo session before exchange calendar lookup")?;
        let session = auth::ensure_session_json(&self.config, &self.config_path)
            .await
            .context("loading Saxo session for exchange calendar lookup")?;
        let data = self
            .fetch_saxo_exchange_summaries(&session)
            .await
            .context("fetching Saxo ref/v1/exchanges")?;
        let mut exchanges = HashMap::new();
        for exchange in default_exchanges() {
            let Some(summary) = data
                .iter()
                .find(|item| saxo_exchange_matches(item, exchange.code))
            else {
                continue;
            };
            let exchange_id = saxo_exchange_text(summary, "ExchangeId")
                .unwrap_or_else(|| exchange.code.to_string());
            let mut detail = summary.clone();
            if parse_saxo_exchange_sessions(&detail).is_empty() {
                match saxo_reference_get_json(
                    self,
                    &session,
                    &format!("/ref/v1/exchanges/{exchange_id}"),
                    &[],
                )
                .await
                {
                    Ok(value) => detail = value,
                    Err(err) => warn!(
                        exchange = exchange.code,
                        exchange_id, "Saxo exchange detail lookup failed: {err:#}"
                    ),
                }
            }
            if let Some(calendar) = saxo_exchange_calendar_from_detail(&detail, &exchange_id) {
                exchanges.insert(exchange.code.to_string(), calendar);
            }
        }
        if exchanges.is_empty() {
            bail!("Saxo ref/v1/exchanges did not match any configured exchange MICs");
        }
        Ok(SaxoExchangeCalendarCache {
            checked_date,
            checked_at: Utc::now(),
            exchanges,
            source: "saxo_ref_v1_exchanges".to_string(),
        })
    }

    async fn fetch_saxo_exchange_summaries(&self, session: &JsonValue) -> Result<Vec<JsonValue>> {
        let mut skip = 0usize;
        let top = 1000usize;
        let mut all = Vec::new();
        loop {
            let payload = saxo_reference_get_json(
                self,
                session,
                "/ref/v1/exchanges",
                &[("$skip", skip.to_string()), ("$top", top.to_string())],
            )
            .await?;
            let page = payload
                .get("Data")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| anyhow!("Saxo ref/v1/exchanges response did not contain Data"))?;
            let page_len = page.len();
            all.extend(page.iter().cloned());
            let total_count = payload
                .get("__count")
                .and_then(JsonValue::as_u64)
                .map(|value| value as usize);
            let has_next = payload
                .get("__next")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            if page_len < top || !has_next || total_count.is_some_and(|total| all.len() >= total) {
                break;
            }
            skip += top;
            if skip > 10_000 {
                bail!("Saxo ref/v1/exchanges pagination exceeded 10000 rows");
            }
        }
        Ok(all)
    }

    pub async fn watchlists_payload(&self) -> Result<JsonValue> {
        let mut seen = HashSet::new();
        let mut monitored = Vec::new();
        for row in self.position_items(250).await.unwrap_or_default() {
            let symbol = text_value(&row, "symbol");
            if seen.insert(symbol) {
                monitored.push(row);
            }
        }
        for row in self
            .select_json(
                "SELECT symbol, updated_at, current_price_local, change_pct, currency, source, status FROM portfolio_price_snapshots ORDER BY updated_at DESC, symbol ASC",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() || !seen.insert(symbol.clone()) {
                continue;
            }
            let mut item = row.as_object().cloned().unwrap_or_default();
            item.insert("instrument_name".to_string(), JsonValue::from(symbol));
            monitored.push(JsonValue::Object(item));
        }
        for row in self
            .select_json(
                "SELECT symbol, instrument_name, updated_at, quantity, currency, average_open_price, profit_loss_on_trade, instrument_price_day_percent_change, calculation_reliability FROM broker_instrument_exposures ORDER BY updated_at DESC, symbol ASC",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() || !seen.insert(symbol) {
                continue;
            }
            monitored.push(row);
        }
        let decisions = self.latest_symbol_decisions().await.unwrap_or_default();
        for (symbol, decision) in &decisions {
            if symbol.is_empty() || !seen.insert(symbol.clone()) {
                continue;
            }
            let source = decision.get("source").cloned().unwrap_or_else(|| json!({}));
            let technical = source
                .get("technical")
                .cloned()
                .unwrap_or_else(|| json!({}));
            monitored.push(json!({
                "symbol": symbol,
                "instrument_name": instrument_name_for_symbol(symbol),
                "updated_at": decision.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "currency": technical.get("currency").cloned().unwrap_or(JsonValue::Null),
                "current_price_local": technical.get("latest_close").cloned().unwrap_or(JsonValue::Null),
                "change_pct": JsonValue::Null,
                "market_value_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "quote_status": technical.get("status").cloned().unwrap_or_else(|| JsonValue::from("decision_report")),
                "source": source.get("source").cloned().unwrap_or_else(|| JsonValue::from("decision_report")),
                "decision": decision,
                "exchange": exchange_code(symbol).to_uppercase(),
                "region": exchange_region(symbol),
            }));
        }
        for row in self
            .select_json(
                "SELECT s.symbol, s.sentiment, s.confidence, s.macro_bias, s.rationale, s.source_json, s.report_id, dr.created_at AS decision_created_at, dr.status AS decision_status, dr.analysis_pulse_key, dr.analysis_pulse_label
                 FROM swing_sentiment_snapshots s
                 LEFT JOIN decision_reports dr ON dr.id = s.report_id
                 ORDER BY s.report_id DESC, s.id DESC
                 LIMIT 600",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() || !seen.insert(symbol.clone()) {
                continue;
            }
            let source = row
                .get("source_json")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let technical = source
                .get("technical")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let decision = json!({
                "symbol": symbol,
                "report_id": row.get("report_id").cloned().unwrap_or(JsonValue::Null),
                "created_at": row.get("decision_created_at").cloned().unwrap_or(JsonValue::Null),
                "status": row.get("decision_status").cloned().unwrap_or(JsonValue::Null),
                "pulse_key": row.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
                "pulse_label": row.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
                "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null),
                "confidence": value_f64(&row, "confidence"),
                "macro_bias": row.get("macro_bias").cloned().unwrap_or(JsonValue::Null),
                "rationale": row.get("rationale").cloned().unwrap_or(JsonValue::Null),
                "source": source.clone(),
            });
            monitored.push(json!({
                "symbol": symbol,
                "instrument_name": instrument_name_for_symbol(&symbol),
                "currency": technical.get("currency").cloned().unwrap_or(JsonValue::Null),
                "current_price_local": technical.get("latest_close").cloned().unwrap_or(JsonValue::Null),
                "change_pct": JsonValue::Null,
                "market_value_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "quote_status": technical.get("status").cloned().unwrap_or_else(|| JsonValue::from("decision_history")),
                "source": "swing_sentiment_snapshots",
                "decision": decision,
                "exchange": exchange_code(&symbol).to_uppercase(),
                "region": exchange_region(&symbol),
            }));
        }
        for item in &mut monitored {
            let symbol = text_value(item, "symbol");
            if let Some(obj) = item.as_object_mut() {
                obj.entry("decision".to_string())
                    .or_insert_with(|| decisions.get(&symbol).cloned().unwrap_or(JsonValue::Null));
                obj.entry("exchange".to_string())
                    .or_insert_with(|| JsonValue::from(exchange_code(&symbol).to_uppercase()));
                obj.entry("region".to_string())
                    .or_insert_with(|| JsonValue::from(exchange_region(&symbol)));
                obj.entry("instrument_name".to_string())
                    .or_insert_with(|| JsonValue::from(instrument_name_for_symbol(&symbol)));
                obj.entry("quote_status".to_string())
                    .or_insert_with(|| JsonValue::from("ok"));
            }
        }
        let mut nordic = Vec::new();
        let mut uk = Vec::new();
        let mut us = Vec::new();
        let mut eu = Vec::new();
        for item in &monitored {
            match exchange_region(&text_value(item, "symbol")).as_str() {
                "Nordics" => nordic.push(item.clone()),
                "UK" => uk.push(item.clone()),
                "US" => us.push(item.clone()),
                _ => eu.push(item.clone()),
            }
        }
        let nordic_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "nordic_limit"]).unwrap_or(100);
        let uk_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "uk_limit"]).unwrap_or(25);
        let us_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "us_limit"]).unwrap_or(100);
        let eu_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "eu_limit"]).unwrap_or(75);
        Ok(json!({
            "generated_at": Utc::now().to_rfc3339(),
            "cache_ttl_seconds": 300,
            "categories": [
                {"key": "all", "label": "All monitored", "target_limit": monitored.len(), "total_universe": monitored.len(), "items": monitored},
                {"key": "nordic", "label": "Nordics", "target_limit": nordic_limit, "total_universe": nordic.len(), "items": nordic},
                {"key": "uk", "label": "UK", "target_limit": uk_limit, "total_universe": uk.len(), "items": uk},
                {"key": "us", "label": "US", "target_limit": us_limit, "total_universe": us.len(), "items": us},
                {"key": "eu", "label": "Europe", "target_limit": eu_limit, "total_universe": eu.len(), "items": eu}
            ],
        }))
    }

    pub async fn localization_for_user(
        &self,
        mut prefs: LocalizationPrefs,
        sso_session: &JsonValue,
    ) -> LocalizationPrefs {
        let key = localization_settings_key(sso_session);
        match self.runtime_setting(&key).await {
            Ok(Some(value)) => prefs.apply_settings_json(&value),
            Ok(None) => {}
            Err(err) => warn!(key = %key, "localization settings lookup failed: {err:#}"),
        }
        prefs
    }

    pub async fn save_localization_settings(
        &self,
        sso_session: &JsonValue,
        mut value: JsonValue,
    ) -> Result<JsonValue> {
        let key = localization_settings_key(sso_session);
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "updated_at".to_string(),
                JsonValue::from(Utc::now().to_rfc3339()),
            );
        }
        self.save_runtime_setting(&key, &value).await?;
        Ok(value)
    }

    async fn latest_batch_id(&self) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| row.try_get::<String, _>("batch_id").ok()))
    }

    async fn broker_positions_available(&self) -> Result<bool> {
        let row = self
            .first_json("SELECT COUNT(*) AS count FROM broker_position_snapshots")
            .await?
            .unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count") > 0)
    }

    async fn effective_position_rows(&self, limit: Option<i64>) -> Result<Vec<JsonValue>> {
        let latest_batch = self.latest_batch_id().await?;
        let where_clause = match latest_batch {
            Some(batch_id) => format!(
                "WHERE batch_id = '{}' AND excluded = 0",
                sql_escape(&batch_id)
            ),
            None => "WHERE excluded = 0".to_string(),
        };
        let base_rows = self
            .select_json(&format!(
                "SELECT instrument_name, symbol, isin, quantity, currency, open_price_local, open_price_local AS paid_price_local, current_price_local, cost_basis_local, cost_basis_dkk, market_value_local, market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk, allocation_pct, asset_class, market_status, value_date FROM position_snapshots {where_clause}"
            ))
            .await
            .unwrap_or_default();
        let broker_rows = self
            .select_json(
                "SELECT symbol, updated_at, instrument_name, isin, uic, asset_type, quantity, currency, open_price_local, open_price_including_costs_local, execution_time_open, value_date, market_state, can_be_closed FROM broker_position_snapshots ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default();
        if broker_rows.is_empty() {
            let mut rows = base_rows;
            rows.sort_by(|left, right| {
                value_f64(right, "market_value_dkk")
                    .partial_cmp(&value_f64(left, "market_value_dkk"))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
            });
            if let Some(limit) = limit {
                rows.truncate(clamp_limit(limit, 1, 250) as usize);
            }
            return Ok(rows);
        }

        let base_by_symbol = base_rows
            .into_iter()
            .map(|row| (text_value(&row, "symbol"), row))
            .collect::<HashMap<_, _>>();
        let price_by_symbol = self
            .select_json(
                "SELECT symbol, updated_at, current_price_local, current_fx_rate_to_dkk, baseline_price_local, baseline_fx_rate_to_dkk, change_pct, currency, status FROM portfolio_price_snapshots ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (text_value(&row, "symbol"), row))
            .collect::<HashMap<_, _>>();
        let exposure_by_symbol = self
            .select_json(
                "SELECT symbol, quantity, average_open_price, profit_loss_on_trade, instrument_price_day_percent_change, currency, calculation_reliability FROM broker_instrument_exposures ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (text_value(&row, "symbol"), row))
            .collect::<HashMap<_, _>>();
        let account_currency = self
            .first_json("SELECT account_currency FROM broker_account_snapshots WHERE singleton_key = 'main' LIMIT 1")
            .await?
            .and_then(|row| row.get("account_currency").cloned())
            .and_then(|value| value.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "DKK".to_string());
        let account_fx_rate = fx_rate_to_dkk(&account_currency);
        let cash_summary = self.cash_summary_from_ledger().await?;
        let cash_balance = value_f64(&cash_summary, "cash_balance_dkk");

        let mut rows = Vec::new();
        for broker in broker_rows {
            let symbol = text_value(&broker, "symbol");
            let quantity = value_f64(&broker, "quantity");
            if symbol.is_empty() || quantity <= 1e-9 {
                continue;
            }
            let base = base_by_symbol.get(&symbol);
            let price = price_by_symbol.get(&symbol);
            let exposure = exposure_by_symbol.get(&symbol);
            let currency = text_value(&broker, "currency")
                .trim()
                .to_string()
                .if_empty_then(|| {
                    price
                        .map(|row| text_value(row, "currency"))
                        .filter(|value| !value.is_empty())
                })
                .or_else(|| base.map(|row| text_value(row, "currency")))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "DKK".to_string());
            let broker_open_price = value_f64(&broker, "open_price_including_costs_local")
                .max(value_f64(&broker, "open_price_local"));
            let base_quantity = base.map(|row| value_f64(row, "quantity")).unwrap_or(0.0);
            let base_market_local = base
                .map(|row| value_f64(row, "market_value_local"))
                .unwrap_or(0.0);
            let base_market_dkk = base
                .map(|row| value_f64(row, "market_value_dkk"))
                .unwrap_or(0.0);
            let inferred_fx_rate = if base_market_local.abs() > 1e-9 {
                base_market_dkk / base_market_local
            } else {
                fx_rate_to_dkk(&currency)
            };
            let current_price_local = price
                .map(|row| value_f64(row, "current_price_local"))
                .filter(|value| *value > 0.0)
                .or_else(|| {
                    base.map(|row| value_f64(row, "current_price_local"))
                        .filter(|value| *value > 0.0)
                })
                .unwrap_or(broker_open_price);
            let current_fx_rate = price
                .map(|row| value_f64(row, "current_fx_rate_to_dkk"))
                .filter(|value| *value > 0.0)
                .unwrap_or(inferred_fx_rate);
            let unit_cost_dkk = if base_quantity > 0.0 {
                value_f64(base.unwrap(), "cost_basis_dkk") / base_quantity
            } else {
                broker_open_price * current_fx_rate
            };
            let cost_basis_dkk = unit_cost_dkk * quantity;
            let cost_basis_local_total = if base_quantity > 0.0 {
                let base_cost_local_total =
                    value_f64(base.unwrap(), "cost_basis_local") * base_quantity;
                if base_cost_local_total > 0.0 {
                    base_cost_local_total / base_quantity * quantity
                } else {
                    broker_open_price * quantity
                }
            } else {
                broker_open_price * quantity
            };
            let market_value_dkk = quantity * current_price_local * current_fx_rate;
            let daily_pnl_dkk = match price {
                Some(price) if value_f64(price, "baseline_price_local") > 0.0 => {
                    quantity
                        * (current_price_local * current_fx_rate
                            - value_f64(price, "baseline_price_local")
                                * value_f64(price, "baseline_fx_rate_to_dkk"))
                }
                _ if base_quantity > 0.0 => {
                    value_f64(base.unwrap(), "daily_pnl_dkk") * quantity / base_quantity
                }
                _ => 0.0,
            };
            let unrealised_pnl_dkk = exposure
                .map(|row| value_f64(row, "profit_loss_on_trade"))
                .filter(|value| value.abs() > 1e-9)
                .map(|value| value * account_fx_rate)
                .unwrap_or(market_value_dkk - cost_basis_dkk);
            rows.push(json!({
                "instrument_name": text_value(&broker, "instrument_name")
                    .if_empty_then(|| base.map(|row| text_value(row, "instrument_name")))
                    .unwrap_or_else(|| instrument_name_for_symbol(&symbol)),
                "symbol": symbol,
                "isin": broker.get("isin").cloned().unwrap_or(JsonValue::Null),
                "quantity": quantity,
                "currency": currency,
                "paid_price_local": if quantity > 0.0 { cost_basis_local_total / quantity } else { broker_open_price },
                "open_price_local": broker_open_price,
                "cost_basis_local": if quantity > 0.0 { cost_basis_local_total / quantity } else { broker_open_price },
                "current_price_local": current_price_local,
                "cost_basis_dkk": cost_basis_dkk,
                "market_value_dkk": market_value_dkk,
                "unrealised_pnl_dkk": unrealised_pnl_dkk,
                "daily_pnl_dkk": daily_pnl_dkk,
                "daily_change_pct": exposure.map(|row| value_f64(row, "instrument_price_day_percent_change")).unwrap_or(0.0),
                "total_return_pct": if cost_basis_dkk.abs() > 1e-9 { unrealised_pnl_dkk / cost_basis_dkk } else { 0.0 },
                "allocation_pct": 0.0,
                "asset_class": text_value(&broker, "asset_type")
                    .if_empty_then(|| base.map(|row| text_value(row, "asset_class")))
                    .unwrap_or_else(|| "Equity".to_string()),
                "market_status": "Saxo broker snapshot",
                "value_date": broker.get("value_date").cloned().unwrap_or(JsonValue::Null),
                "latest_quote_updated_at": price.and_then(|row| row.get("updated_at")).cloned().unwrap_or(JsonValue::Null),
                "quote_status": price.and_then(|row| row.get("status")).cloned().unwrap_or_else(|| JsonValue::from("broker_snapshot")),
                "broker_profit_loss_on_trade": exposure.map(|row| value_f64(row, "profit_loss_on_trade")).unwrap_or(0.0),
                "broker_calculation_reliability": exposure.and_then(|row| row.get("calculation_reliability")).cloned().unwrap_or(JsonValue::Null),
            }));
        }
        let invested = rows
            .iter()
            .map(|row| value_f64(row, "market_value_dkk"))
            .sum::<f64>();
        let total_value = invested + cash_balance;
        for row in &mut rows {
            let market_value_dkk = value_f64(row, "market_value_dkk");
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "allocation_pct".to_string(),
                    JsonValue::from(if total_value > 0.0 {
                        market_value_dkk / total_value
                    } else {
                        0.0
                    }),
                );
            }
        }
        rows.sort_by(|left, right| {
            value_f64(right, "market_value_dkk")
                .partial_cmp(&value_f64(left, "market_value_dkk"))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
        });
        if let Some(limit) = limit {
            rows.truncate(clamp_limit(limit, 1, 250) as usize);
        }
        Ok(rows)
    }

    async fn position_aggregate(&self, batch_id: Option<&str>) -> Result<JsonValue> {
        let rows = if self.broker_positions_available().await? {
            self.effective_position_rows(None).await?
        } else {
            let where_clause = match batch_id {
                Some(batch_id) => format!(
                    "WHERE batch_id = '{}' AND excluded = 0",
                    sql_escape(batch_id)
                ),
                None => "WHERE excluded = 0".to_string(),
            };
            self.select_json(&format!(
                "SELECT market_value_dkk, cost_basis_dkk, unrealised_pnl_dkk, daily_pnl_dkk FROM position_snapshots {where_clause}"
            ))
            .await
            .unwrap_or_default()
        };
        let invested = rows
            .iter()
            .map(|row| value_f64(row, "market_value_dkk"))
            .sum::<f64>();
        let cash_summary = self.cash_summary_from_ledger().await?;
        let cash_balance = value_f64(&cash_summary, "cash_balance_dkk");
        let initial_cash = value_f64(&cash_summary, "initial_cash_dkk");
        let cash_from_trades = value_f64(&cash_summary, "cash_from_trades_dkk");
        Ok(json!({
            "total_market_value_dkk": invested + cash_balance,
            "invested_market_value_dkk": invested,
            "cash_balance_dkk": cash_balance,
            "initial_cash_dkk": initial_cash,
            "cash_from_trades_dkk": cash_from_trades,
            "total_cost_basis_dkk": rows.iter().map(|row| value_f64(row, "cost_basis_dkk")).sum::<f64>(),
            "total_unrealised_pnl_dkk": rows.iter().map(|row| value_f64(row, "unrealised_pnl_dkk")).sum::<f64>(),
            "total_daily_pnl_dkk": rows.iter().map(|row| value_f64(row, "daily_pnl_dkk")).sum::<f64>(),
            "position_count": rows.len() as i64,
            "source": if self.broker_positions_available().await? { "saxo_broker_snapshot" } else { "position_snapshots" }
        }))
    }

    async fn cash_summary_from_ledger(&self) -> Result<JsonValue> {
        let initial_cash =
            yaml_f64(&self.config, &["portfolio", "initial_cash_dkk"]).unwrap_or(0.0);
        let row = self
            .first_json(
                "SELECT COALESCE(SUM(net_amount_dkk), 0) AS cash_from_trades_dkk FROM trade_ledger WHERE status IN ('executed', 'approved')",
            )
            .await?
            .unwrap_or_else(|| json!({}));
        let cash_from_trades = value_f64(&row, "cash_from_trades_dkk");
        Ok(json!({
            "initial_cash_dkk": initial_cash,
            "cash_from_trades_dkk": cash_from_trades,
            "cash_balance_dkk": initial_cash + cash_from_trades,
        }))
    }

    pub async fn position_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let decisions = self.latest_symbol_decisions().await.unwrap_or_default();
        let mut rows = self.effective_position_rows(Some(limit)).await?;
        for row in &mut rows {
            let symbol = text_value(row, "symbol");
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "ladder_status".to_string(),
                    json!({"text": "idle", "active_orders": 0, "filled_entry_rungs": 0, "total_entry_rungs": 0, "progress_pct": 0.0, "trailing": false}),
                );
                obj.insert(
                    "decision".to_string(),
                    decisions.get(&symbol).cloned().unwrap_or(JsonValue::Null),
                );
                obj.entry("latest_quote_updated_at".to_string())
                    .or_insert(JsonValue::Null);
            }
        }
        Ok(rows)
    }

    async fn latest_symbol_decisions(&self) -> Result<HashMap<String, JsonValue>> {
        let Some(report) = self
            .first_json(
                "SELECT dr.id, dr.created_at, dr.status, dr.analysis_pulse_key, dr.analysis_pulse_label
                 FROM decision_reports dr
                 WHERE dr.report_json IS NOT NULL
                   AND (
                     EXISTS (SELECT 1 FROM swing_sentiment_snapshots s WHERE s.report_id = dr.id)
                     OR EXISTS (SELECT 1 FROM swing_position_targets t WHERE t.report_id = dr.id)
                   )
                 ORDER BY dr.id DESC
                 LIMIT 1",
            )
            .await?
        else {
            return Ok(HashMap::new());
        };
        let report_id = value_i64(&report, "id");
        let mut decisions = HashMap::new();
        let sentiment_rows = self
            .select_json(&format!(
                "SELECT symbol, sentiment, confidence, macro_bias, rationale, catalysts_json, risk_notes_json, source_json FROM swing_sentiment_snapshots WHERE report_id = {} ORDER BY symbol ASC, id DESC",
                report_id
            ))
            .await
            .unwrap_or_default();
        for row in sentiment_rows {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            decisions.insert(
                symbol.clone(),
                json!({
                    "symbol": symbol,
                    "report_id": report_id,
                    "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
                    "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
                    "pulse_key": report.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
                    "pulse_label": report.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
                    "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null),
                    "confidence": value_f64(&row, "confidence"),
                    "macro_bias": row.get("macro_bias").cloned().unwrap_or(JsonValue::Null),
                    "rationale": row.get("rationale").cloned().unwrap_or(JsonValue::Null),
                    "catalysts": row.get("catalysts_json").cloned().unwrap_or_else(|| json!([])),
                    "risk_notes": row.get("risk_notes_json").cloned().unwrap_or_else(|| json!([])),
                    "source": row.get("source_json").cloned().unwrap_or_else(|| json!({})),
                }),
            );
        }
        let target_rows = self
            .select_json(&format!(
                "SELECT symbol, sentiment, action, current_weight_pct, target_weight_pct, current_quantity, target_quantity, estimated_delta_quantity, estimated_value_dkk, priority, confidence, rationale, risk_json FROM swing_position_targets WHERE report_id = {} ORDER BY symbol ASC, id DESC",
                report_id
            ))
            .await
            .unwrap_or_default();
        for row in target_rows {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            let entry = decisions.entry(symbol.clone()).or_insert_with(|| {
                json!({
                    "symbol": symbol,
                    "report_id": report_id,
                    "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
                    "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
                    "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null)
                })
            });
            if let Some(obj) = entry.as_object_mut() {
                obj.insert(
                    "action".to_string(),
                    row.get("action").cloned().unwrap_or(JsonValue::Null),
                );
                obj.insert(
                    "priority".to_string(),
                    row.get("priority").cloned().unwrap_or(JsonValue::Null),
                );
                obj.insert(
                    "target_confidence".to_string(),
                    JsonValue::from(value_f64(&row, "confidence")),
                );
                obj.insert(
                    "target_rationale".to_string(),
                    row.get("rationale").cloned().unwrap_or(JsonValue::Null),
                );
                obj.insert(
                    "current_weight_pct".to_string(),
                    JsonValue::from(value_f64(&row, "current_weight_pct")),
                );
                obj.insert(
                    "target_weight_pct".to_string(),
                    JsonValue::from(value_f64(&row, "target_weight_pct")),
                );
                obj.insert(
                    "risk".to_string(),
                    row.get("risk_json").cloned().unwrap_or_else(|| json!({})),
                );
            }
        }
        Ok(decisions)
    }

    pub async fn execution_orders(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_id, symbol, action, order_type, mode, status, adapter, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk, approval_required, approved_at, ledger_id, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role, error_text, broker_order_id FROM execution_orders ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_fills(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM execution_fills ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_events(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM execution_order_events ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn decision_report_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn decision_report_item(&self, report_id: i64) -> Result<Option<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label FROM decision_reports WHERE id = {} LIMIT 1",
            report_id.max(0)
        );
        self.first_json(&sql).await
    }

    pub async fn markov_signals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        crate::markov_method::latest_markov_signals(self, limit).await
    }

    pub async fn latest_markov_run(&self) -> Result<JsonValue> {
        crate::markov_method::latest_markov_run(self).await
    }

    #[allow(dead_code)]
    pub async fn generate_decision_report_fallback(&self) -> Result<JsonValue> {
        // This is a conservative Rust-side generator used by the manual button.
        // It does not call the external xAI service; instead it persists a
        // transparent deterministic report with the same database shape that the
        // Rust UI consumes. This keeps manual operator snapshots auditable when
        // the primary deferred xAI path is unavailable or bypassed.
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let report_date = Utc::now().date_naive().to_string();
        let batch_id = self.latest_batch_id().await?.unwrap_or_default();
        let positions = self.position_items(250).await.unwrap_or_default();
        let watchlists = self
            .watchlists_payload()
            .await
            .unwrap_or_else(|_| json!({}));
        let selected_assets = deterministic_selected_assets(&positions, &watchlists);
        let suggested_trades = deterministic_suggested_trades(&positions, &watchlists);
        let symbol_sentiment = deterministic_symbol_sentiment(&positions, &selected_assets);
        let report_json = json!({
            "report_title": "Manual Rust fallback Decision Report",
            "status": "rust_fallback",
            "created_at": created_at,
            "reasoning_steps": [
                "Manual trigger was requested from the Rust dashboard.",
                "The primary deferred xAI decision path was unavailable or bypassed for this fallback invocation.",
                "This fallback report uses current portfolio, watchlist, cash, and allocation data to create an auditable operator snapshot."
            ],
            "market_view": {
                "bias": "neutral",
                "summary": "Deterministic Rust fallback: review current watchlist and portfolio state before submitting trades."
            },
            "portfolio_summary": {
                "position_count": positions.len(),
                "cash_balance_dkk": self.cash_buffer_value().get("cash_balance_dkk").cloned().unwrap_or(JsonValue::Null)
            },
            "strategy_status": "Rust manual fallback generated. Review suggested trades manually; no broker orders are queued by this action.",
            "strategy_flow": {
                "portfolio": positions.len(),
                "selected": selected_assets.len(),
                "trades": suggested_trades.len()
            },
            "selected_assets": selected_assets,
            "candidate_assets": symbol_sentiment,
            "symbol_sentiment": symbol_sentiment,
            "suggested_trades": suggested_trades,
        });
        let prompt_text = json!({
            "system": "Rust dashboard manual fallback. No external model call was made.",
            "user": "Generate an auditable decision snapshot from current stored portfolio/watchlist data."
        });
        let request_json = json!({
            "source": "rust_dashboard",
            "manual": true,
            "position_count": positions.len()
        });
        let sql = format!(
            "INSERT INTO decision_reports (
                created_at, report_date, batch_id, model, status, analysis_window_active,
                response_id, prompt_text, request_json, response_json, report_json,
                error_text, analysis_pulse_key, analysis_pulse_label
            ) VALUES (
                '{}', '{}', '{}', 'rust-deterministic-fallback', 'rust_fallback', 0,
                NULL, '{}', '{}', NULL, '{}',
                '{}', '{}', '{}'
            )",
            sql_escape(&created_at),
            sql_escape(&report_date),
            sql_escape(&batch_id),
            sql_escape(&serde_json::to_string(&prompt_text)?),
            sql_escape(&serde_json::to_string(&request_json)?),
            sql_escape(&serde_json::to_string(&report_json)?),
            sql_escape(
                "Generated by Rust fallback because external xAI decision generation was unavailable or bypassed."
            ),
            sql_escape(&format!("manual:{report_date}")),
            sql_escape("Manual Decision Report")
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("inserting manual Rust fallback decision report")?;
        let report = self
            .first_json(&format!(
                "SELECT id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label FROM decision_reports WHERE created_at = '{}' ORDER BY id DESC LIMIT 1",
                sql_escape(&created_at)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(report)
    }

    async fn latest_decision_summary(&self) -> Result<JsonValue> {
        let report = self
            .first_json("SELECT id, created_at, status FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT 1")
            .await?;
        Ok(report.unwrap_or_else(|| json!({"id": null, "created_at": null, "status": null})))
    }

    pub async fn strategy_journal_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, journal_date, cadence, status, summary, metrics_json, learnings_json, source_report_id, diary_json FROM strategy_journal_entries ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn scheduler_status_value(&self) -> Result<JsonValue> {
        Ok(self
            .first_json("SELECT singleton_key, started_at, last_heartbeat_at, last_cycle_started_at, last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid FROM scheduler_status WHERE singleton_key = 'main' LIMIT 1")
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn scheduler_cycles(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM scheduler_cycle_history ORDER BY started_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub fn hermes_goal_contract_value(&self) -> JsonValue {
        json!({
            "enabled": false,
            "mode": "recommend_only",
            "goal_version": 1,
            "objective": {
                "target_return_30d": 0.47,
                "target_return_note": "Approximately 10x in 6 months if compounded monthly: 1.47^6 ~= 10.1",
                "max_drawdown": 0.20,
                "min_sharpe": 1.0,
                "failure_below_30d_return": -0.04,
                "reflection_every": "7d",
                "one_variable_only": true
            },
            "constraints": {
                "max_positions": yaml_i64(&self.config, &["strategy", "swing", "max_holdings"]).unwrap_or(25),
                "slippage_tolerance": 0.02,
                "gas_reserve": 0.05,
                "min_cash_buffer_pct": yaml_f64(&self.config, &["strategy", "capital", "min_cash_buffer_pct"]).unwrap_or(0.10),
                "allow_shorting": yaml_bool(&self.config, &["risk", "allow_shorting"]).unwrap_or(false),
                "require_human_approval": true,
                "require_backtest_before_activation": true,
                "require_paper_or_sim_observation": true
            },
            "experiment_policy": {
                "min_observation_days": 7,
                "min_closed_trades": 5,
                "promote_only_if": {
                    "return_30d_gte": 0.47,
                    "drawdown_lte": 0.20,
                    "sharpe_gte": 1.0
                },
                "rollback_if": {
                    "return_30d_lte": -0.04,
                    "drawdown_gt": 0.20,
                    "safety_violation": true
                }
            }
        })
    }

    pub fn hermes_capabilities_value(&self) -> JsonValue {
        json!({
            "status": "ok",
            "runtime": "saxo-rust",
            "namespace": "saxo-rust",
            "database_namespace": "saxo",
            "safe_endpoints": [
                "/api/hermes/capabilities",
                "/api/hermes/context",
                "/api/hermes/reflections",
                "/api/hermes/experiments",
                "/api/hermes/experiments/{id}/transition",
                "/api/health",
                "/api/overview",
                "/api/markov/signals",
                "/api/decision/latest",
                "/api/decision/reports",
                "/api/scheduler",
                "/api/execution",
                "/api/strategy-journal"
            ],
            "read_models": [
                "overview",
                "scheduler_status",
                "scheduler_cycle_history",
                "decision_reports.report_json",
                "strategy_journal_entries",
                "execution_orders",
                "execution_order_events",
                "execution_fills",
                "portfolio_value_history",
                "markov_signal_runs",
                "markov_asset_signals"
            ],
            "restricted_writes": [
                "hermes_reflections",
                "strategy_experiments"
            ],
            "supported_experiment_overlays": {
                "scope": "paper_or_saxo_sim_only",
                "statuses": ["approved_sim", "active_sim", "approved_paper", "active_paper"],
                "variables": [
                    "execution.min_trade_value_dkk",
                    "strategy.capital.min_cash_buffer_pct",
                    "strategy.swing.cash_buffer_pct",
                    "strategy.swing.daily_indicators.min_confluences"
                ]
            },
            "forbidden": [
                "saxo_sessions",
                "Saxo OAuth token/session reads",
                "order precheck/place/replace/cancel",
                "live order approval",
                "Kubernetes secret mutation",
                "live broker baseline activation"
            ],
            "notes": [
                "Hermes proposals are recommend-only until reviewed by the daytrader UI/operator flow.",
                "Promoted baselines are audit records; they do not activate live broker behavior.",
                "Strategy experiments must change exactly one variable while one_variable_only is true.",
                "Markov method signals are advisory analytics and do not place or approve orders.",
                "Scheduled decision reports target two daily open-followup pulses: Nordic/EU open +1h15 and US open +1h15.",
                "Daily end-of-day reports are exposed as sanitized strategy journal rows.",
                "The Hermes adapter intentionally excludes raw request_json/response_json payloads from decision reports."
            ],
            "goal_contract": self.hermes_goal_contract_value()
        })
    }

    pub async fn hermes_context(&self, limit: i64) -> Result<JsonValue> {
        let limit = clamp_limit(limit, 1, 50);
        let overview = self.overview_payload().await.unwrap_or_else(|err| {
            warn!("Hermes overview context degraded: {err:#}");
            json!({"status": "degraded", "detail": err.to_string()})
        });
        let scheduler_status = self.scheduler_status_value().await.unwrap_or_else(|err| {
            warn!("Hermes scheduler status degraded: {err:#}");
            json!({"status": "degraded", "detail": err.to_string()})
        });
        let scheduler_cycles = self.scheduler_cycles(limit).await.unwrap_or_default();
        let decision_reports = self.hermes_decision_report_items(limit).await?;
        let journals = self.strategy_journal_items(limit).await.unwrap_or_default();
        let end_of_day_reports = self.hermes_end_of_day_report_items(limit).await?;
        let execution_orders = self.execution_orders(limit).await.unwrap_or_default();
        let execution_failures = self.hermes_execution_failures(limit).await?;
        let execution_events = self.execution_events(limit).await.unwrap_or_default();
        let execution_fills = self.execution_fills(limit).await.unwrap_or_default();
        let performance = self
            .performance_history_with_current("1M", 500)
            .await
            .unwrap_or_default();
        let active_experiments = self.hermes_experiments(10).await.unwrap_or_default();
        let active_strategy_baseline = self
            .active_strategy_baseline()
            .await
            .unwrap_or(JsonValue::Null);
        let markov = crate::markov_method::compact_markov_context(self, limit)
            .await
            .unwrap_or_else(|err| {
                warn!("Hermes Markov context degraded: {err:#}");
                json!({"status": "degraded", "detail": err.to_string()})
            });

        Ok(json!({
            "status": "ok",
            "generated_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "capabilities": self.hermes_capabilities_value(),
            "goal_contract": self.hermes_goal_contract_value(),
            "overview": overview,
            "scheduler": {
                "status": scheduler_status,
                "cycles": scheduler_cycles
            },
            "decisions": {
                "cadence": "two_daily_open_followups",
                "pulses": crate::xai_decision::decision_pulse_summary(self).get("pulses").cloned().unwrap_or_else(|| json!([])),
                "reports": decision_reports
            },
            "end_of_day": {
                "cadence": "daily",
                "reports": end_of_day_reports
            },
            "strategy_journal": {
                "items": journals
            },
            "execution": {
                "orders": execution_orders,
                "failures": execution_failures,
                "events": execution_events,
                "fills": execution_fills
            },
            "performance": {
                "range": "1M",
                "history": performance
            },
            "markov_method": markov,
            "hermes": {
                "experiments": active_experiments,
                "active_strategy_baseline": active_strategy_baseline
            },
            "safety": {
                "saxo_sessions_excluded": true,
                "broker_mutations_excluded": true,
                "raw_oauth_payloads_excluded": true
            }
        }))
    }

    pub async fn hermes_decision_report_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_date, model, status, analysis_window_active, report_json, error_text, analysis_pulse_key, analysis_pulse_label
             FROM decision_reports
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn hermes_end_of_day_report_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, journal_date, cadence, status, summary, metrics_json, learnings_json, source_report_id, diary_json
             FROM strategy_journal_entries
             WHERE cadence = 'daily'
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn active_strategy_baseline(&self) -> Result<JsonValue> {
        Ok(self
            .first_json(
                "SELECT id, created_at, activated_at, status, goal_version, config_json, prompt_json, source
                 FROM strategy_baselines
                 WHERE status = 'active'
                 ORDER BY activated_at DESC, created_at DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn hermes_execution_failures(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_id, symbol, action, order_type, mode, status, adapter, quantity, currency, estimated_value_dkk, approval_required, strategy_type, strategy_session, strategy_key, strategy_role, error_text
             FROM execution_orders
             WHERE error_text IS NOT NULL OR lower(status) LIKE '%failed%' OR lower(status) LIKE '%error%' OR lower(status) LIKE '%rejected%'
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn hermes_reflections(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, period_start, period_end, goal_version, summary, findings_json, proposed_actions_json, source_session_id, raw_payload_json
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn record_hermes_reflection(
        &self,
        request: &HermesReflectionRequest,
    ) -> Result<JsonValue> {
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let id = runtime_id("hermes-reflection");
        let period_start = request.period_start.as_deref().unwrap_or("");
        let period_end = request.period_end.as_deref().unwrap_or("");
        let findings = request.findings.clone().unwrap_or_else(|| json!([]));
        let proposed_actions = request
            .proposed_actions
            .clone()
            .unwrap_or_else(|| json!([]));
        let raw_payload = request.raw_payload.clone().unwrap_or(JsonValue::Null);
        let sql = format!(
            "INSERT INTO hermes_reflections (
                id, created_at, period_start, period_end, goal_version, summary,
                findings_json, proposed_actions_json, source_session_id, raw_payload_json
            ) VALUES (
                '{}', '{}', '{}', '{}', {}, '{}', '{}', '{}', {}, '{}'
            )",
            sql_escape(&id),
            sql_escape(&created_at),
            sql_escape(period_start),
            sql_escape(period_end),
            request.goal_version.unwrap_or(1),
            sql_escape(request.summary.trim()),
            sql_escape(&serde_json::to_string(&findings)?),
            sql_escape(&serde_json::to_string(&proposed_actions)?),
            sql_optional_text(request.source_session_id.as_deref()),
            sql_escape(&serde_json::to_string(&raw_payload)?)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording Hermes reflection")?;
        Ok(self
            .first_json(&format!(
                "SELECT id, created_at, period_start, period_end, goal_version, summary, findings_json, proposed_actions_json, source_session_id, raw_payload_json
                 FROM hermes_reflections WHERE id = '{}' LIMIT 1",
                sql_escape(&id)
            ))
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn hermes_experiments(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
             FROM strategy_experiments
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn record_hermes_experiment(
        &self,
        request: &HermesExperimentRequest,
    ) -> Result<JsonValue> {
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let id = runtime_id("strategy-experiment");
        let evidence = request.evidence.clone().unwrap_or_else(|| json!({}));
        let raw_payload = request.raw_payload.clone().unwrap_or(JsonValue::Null);
        let sql = format!(
            "INSERT INTO strategy_experiments (
                id, created_at, status, baseline_id, goal_version, hypothesis,
                changed_variable_path, old_value_json, new_value_json, expected_effect,
                risk_notes, evidence_json, approval_json, metrics_json, source_session_id,
                raw_payload_json
            ) VALUES (
                '{}', '{}', 'pending_review', {}, {}, '{}',
                '{}', '{}', '{}', '{}',
                '{}', '{}', NULL, NULL, {}, '{}'
            )",
            sql_escape(&id),
            sql_escape(&created_at),
            sql_optional_text(request.baseline_id.as_deref()),
            request.goal_version.unwrap_or(1),
            sql_escape(request.hypothesis.trim()),
            sql_escape(request.changed_variable_path.trim()),
            sql_escape(&serde_json::to_string(&request.old_value)?),
            sql_escape(&serde_json::to_string(&request.new_value)?),
            sql_escape(request.expected_effect.trim()),
            sql_escape(request.risk_notes.as_deref().unwrap_or("")),
            sql_escape(&serde_json::to_string(&evidence)?),
            sql_optional_text(request.source_session_id.as_deref()),
            sql_escape(&serde_json::to_string(&raw_payload)?)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording Hermes strategy experiment")?;
        Ok(self
            .first_json(&format!(
                "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(&id)
            ))
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn transition_hermes_experiment(
        &self,
        experiment_id: &str,
        action: &str,
        notes: Option<&str>,
        actor: &str,
    ) -> Result<JsonValue> {
        let experiment_id = experiment_id.trim();
        if experiment_id.is_empty() {
            bail!("experiment id is required");
        }
        let experiment = self
            .first_json(&format!(
                "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(experiment_id)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        if experiment.is_null() {
            bail!("Hermes experiment not found: {experiment_id}");
        }
        let current_status = json_text(&experiment, "status");
        let next_status =
            hermes_experiment_next_status(&current_status, action).with_context(|| {
                format!("invalid Hermes experiment transition {current_status} -> {action}")
            })?;
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut baseline_id = None;
        if next_status == "promoted" {
            baseline_id = Some(
                self.promote_hermes_experiment_baseline(&experiment, &now)
                    .await?,
            );
        }
        let approval = json!({
            "action": action.trim(),
            "from_status": current_status,
            "to_status": next_status,
            "actor": actor,
            "notes": notes.unwrap_or("").trim(),
            "recorded_at": now,
            "baseline_id": baseline_id
        });
        sqlx::query(&format!(
            "UPDATE strategy_experiments
             SET status = '{}', approval_json = '{}'
             WHERE id = '{}'",
            sql_escape(next_status),
            sql_escape(&serde_json::to_string(&approval)?),
            sql_escape(experiment_id)
        ))
        .execute(&self.pool)
        .await
        .context("updating Hermes experiment transition")?;

        let updated = self
            .first_json(&format!(
                "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(experiment_id)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(json!({
            "status": "ok",
            "experiment": updated,
            "transition": approval
        }))
    }

    async fn promote_hermes_experiment_baseline(
        &self,
        experiment: &JsonValue,
        activated_at: &str,
    ) -> Result<String> {
        let baseline_id = runtime_id("strategy-baseline");
        let config_json = json!({
            "source_experiment_id": json_text(experiment, "id"),
            "goal_version": experiment.get("goal_version").cloned().unwrap_or_else(|| json!(1)),
            "changed_variable_path": json_text(experiment, "changed_variable_path"),
            "old_value": experiment.get("old_value_json").cloned().unwrap_or(JsonValue::Null),
            "new_value": experiment.get("new_value_json").cloned().unwrap_or(JsonValue::Null),
            "hypothesis": json_text(experiment, "hypothesis"),
            "expected_effect": json_text(experiment, "expected_effect"),
            "risk_notes": json_text(experiment, "risk_notes"),
            "scope": "baseline_record_only",
            "live_activation": false
        });
        let prompt_json = json!({
            "source": "hermes_experiment_promotion",
            "raw_payload": experiment.get("raw_payload_json").cloned().unwrap_or(JsonValue::Null)
        });
        sqlx::query("UPDATE strategy_baselines SET status = 'superseded' WHERE status = 'active'")
            .execute(&self.pool)
            .await
            .context("superseding prior strategy baselines")?;
        sqlx::query(&format!(
            "INSERT INTO strategy_baselines (
                id, created_at, activated_at, status, goal_version, config_json, prompt_json, source
            ) VALUES (
                '{}', '{}', '{}', 'active', {}, '{}', '{}', '{}'
            )",
            sql_escape(&baseline_id),
            sql_escape(activated_at),
            sql_escape(activated_at),
            experiment
                .get("goal_version")
                .and_then(JsonValue::as_i64)
                .unwrap_or(1),
            sql_escape(&serde_json::to_string(&config_json)?),
            sql_escape(&serde_json::to_string(&prompt_json)?),
            sql_escape(&format!(
                "hermes_experiment:{}",
                json_text(experiment, "id")
            ))
        ))
        .execute(&self.pool)
        .await
        .context("creating promoted Hermes strategy baseline")?;
        Ok(baseline_id)
    }

    pub async fn latest_trading_manager_run(&self) -> Result<JsonValue> {
        Ok(self
            .first_json(
                "SELECT id, created_at, manager_key, manager_kind, manager_label, target_at_utc, report_id, status, open_exchange_codes_json, technical_json, manager_json, queue_result_json, error_text
                 FROM trading_manager_runs
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn record_scheduler_cycle(
        &self,
        started_at: &str,
        completed_at: &str,
        status: &str,
        cycle_json: &JsonValue,
    ) -> Result<()> {
        let cycle_text =
            serde_json::to_string(cycle_json).context("serializing scheduler cycle JSON")?;
        let queue_status = cycle_json
            .get("trading_manager")
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or(status);
        let analysis_window_active = cycle_json
            .get("market")
            .and_then(|value| value.get("analysis_window_active"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let notifications_status = cycle_json
            .get("notifications")
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or("not_run");
        let broker_alerts_status = notifications_status;
        let sql = format!(
            "INSERT INTO scheduler_cycle_history (
                started_at, completed_at, status, analysis_window_active,
                generated_decision, queue_status, notifications_status, broker_alerts_status,
                cycle_json
            ) VALUES (
                '{}', '{}', '{}', {}, 0, '{}', '{}', '{}', '{}'
            )",
            sql_escape(started_at),
            sql_escape(completed_at),
            sql_escape(status),
            if analysis_window_active { 1 } else { 0 },
            sql_escape(queue_status),
            sql_escape(notifications_status),
            sql_escape(broker_alerts_status),
            sql_escape(&cycle_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording scheduler cycle")?;
        self.update_scheduler_status(started_at, completed_at, status, cycle_json)
            .await
    }

    pub async fn update_scheduler_heartbeat(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO scheduler_status (
                singleton_key, started_at, last_heartbeat_at, last_cycle_started_at,
                last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid
            ) VALUES (
                'main', '{}', '{}', NULL, NULL, 'heartbeat', '{{}}', NULL
            )
            ON CONFLICT(singleton_key) DO UPDATE SET
                last_heartbeat_at = excluded.last_heartbeat_at",
            sql_escape(&now),
            sql_escape(&now)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("updating scheduler heartbeat")?;
        Ok(())
    }

    async fn update_scheduler_status(
        &self,
        started_at: &str,
        completed_at: &str,
        status: &str,
        cycle_json: &JsonValue,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let cycle_text =
            serde_json::to_string(cycle_json).context("serializing scheduler status JSON")?;
        let sql = format!(
            "INSERT INTO scheduler_status (
                singleton_key, started_at, last_heartbeat_at, last_cycle_started_at,
                last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid
            ) VALUES (
                'main', '{}', '{}', '{}', '{}', '{}', '{}', NULL
            )
            ON CONFLICT(singleton_key) DO UPDATE SET
                last_heartbeat_at = excluded.last_heartbeat_at,
                last_cycle_started_at = excluded.last_cycle_started_at,
                last_cycle_completed_at = excluded.last_cycle_completed_at,
                last_cycle_status = excluded.last_cycle_status,
                last_cycle_json = excluded.last_cycle_json",
            sql_escape(started_at),
            sql_escape(&now),
            sql_escape(started_at),
            sql_escape(completed_at),
            sql_escape(status),
            sql_escape(&cycle_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("updating scheduler status")?;
        Ok(())
    }

    pub async fn performance_history_for_range(
        &self,
        range_key: &str,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let columns = "recorded_at, snapshot_type, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count, source";
        let limit = clamp_limit(limit, 1, 5000);
        let mut rows = Vec::new();
        let where_clause = match performance_start_at(range_key) {
            Some(start_at) => {
                let escaped_start = sql_escape(&start_at);
                let anchor_sql = format!(
                    "SELECT {columns} FROM portfolio_value_history WHERE recorded_at < '{escaped_start}' ORDER BY recorded_at DESC, id DESC LIMIT 1"
                );
                rows.extend(self.select_json(&anchor_sql).await.unwrap_or_default());
                format!("WHERE recorded_at >= '{escaped_start}'")
            }
            None => String::new(),
        };
        let remaining = (limit - rows.len() as i64).max(1);
        let sql = format!(
            "SELECT {columns} FROM portfolio_value_history {where_clause} ORDER BY recorded_at ASC, id ASC LIMIT {}",
            clamp_limit(remaining, 1, 5000)
        );
        rows.extend(self.select_json(&sql).await.unwrap_or_default());
        rows.sort_by(|left, right| {
            text_value(left, "recorded_at")
                .cmp(&text_value(right, "recorded_at"))
                .then_with(|| {
                    text_value(left, "snapshot_type").cmp(&text_value(right, "snapshot_type"))
                })
        });
        Ok(rows)
    }

    pub async fn performance_history_with_current(
        &self,
        range_key: &str,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let mut history = self.performance_history_for_range(range_key, limit).await?;
        let current = self.current_performance_row().await?;
        let latest_matches_current = history
            .last()
            .is_some_and(|latest| performance_rows_have_same_values(latest, &current));
        if !latest_matches_current {
            history.push(current);
        } else if history.len() == 1 {
            history[0] = current;
        }
        Ok(history)
    }

    async fn current_performance_row(&self) -> Result<JsonValue> {
        let latest_batch = self.latest_batch_id().await?;
        let aggregate = self.position_aggregate(latest_batch.as_deref()).await?;
        Ok(json!({
            "recorded_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "snapshot_type": "runtime_current",
            "total_market_value_dkk": value_f64(&aggregate, "total_market_value_dkk"),
            "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
            "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
            "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
            "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
            "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
            "position_count": value_i64(&aggregate, "position_count"),
            "source": text_value(&aggregate, "source"),
        }))
    }

    pub async fn portfolio_trades_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM trade_ledger ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 250)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    async fn execution_counts(&self) -> Result<JsonValue> {
        let rows = self
            .select_json("SELECT status, COUNT(*) AS count FROM execution_orders GROUP BY status")
            .await?;
        let mut queued = 0;
        let mut pending_approval = 0;
        let mut broker_live = 0;
        let mut failed = 0;
        for row in rows {
            let status = row.get("status").and_then(JsonValue::as_str).unwrap_or("");
            let count = value_i64(&row, "count");
            match status {
                "pending_execution"
                | "waiting_for_market_open"
                | "waiting_for_cash_settlement"
                | "waiting_for_virtual_cash_budget" => queued += count,
                "pending_approval" => pending_approval += count,
                "submitted_to_broker"
                | "submitting_to_broker"
                | "broker_working"
                | "broker_amended"
                | "broker_partially_filled"
                | "broker_replace_requested"
                | "broker_cancel_requested" => broker_live += count,
                "execution_failed" => failed += count,
                _ => {}
            }
        }
        Ok(
            json!({"queued": queued, "pending_approval": pending_approval, "broker_live": broker_live, "failed": failed}),
        )
    }

    async fn executed_orders_today(&self) -> Result<i64> {
        let today = Utc::now().date_naive().to_string();
        let sql = format!(
            "SELECT COUNT(*) AS count FROM execution_orders WHERE substr(created_at, 1, 10) = '{}' AND status = 'executed'",
            sql_escape(&today)
        );
        let row = self.first_json(&sql).await?.unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count"))
    }

    pub fn goal_tracking(&self, total_value: f64, initial_cash: f64) -> JsonValue {
        let weekly_target = yaml_f64(
            &self.config,
            &["xai", "performance_goals", "weekly_target_dkk"],
        )
        .unwrap_or(5000.0);
        let monthly_target = yaml_f64(
            &self.config,
            &["xai", "performance_goals", "monthly_target_dkk"],
        )
        .unwrap_or(20000.0);
        let pnl = total_value - initial_cash;
        json!({
            "weekly_target_dkk": weekly_target,
            "monthly_target_dkk": monthly_target,
            "periods": {
                "week": {"pnl_dkk": pnl, "target_dkk": weekly_target, "progress_pct": pct(pnl, weekly_target)},
                "month": {"pnl_dkk": pnl, "target_dkk": monthly_target, "progress_pct": pct(pnl, monthly_target)}
            }
        })
    }

    pub fn cash_buffer_value(&self) -> JsonValue {
        let min_cash_buffer_pct = yaml_f64(
            &self.config,
            &["strategy", "capital", "min_cash_buffer_pct"],
        )
        .unwrap_or(0.10);
        let max_deployment_pct =
            yaml_f64(&self.config, &["strategy", "capital", "max_deployment_pct"]).unwrap_or(0.90);
        json!({
            "min_cash_buffer_pct": min_cash_buffer_pct,
            "max_deployment_pct": max_deployment_pct,
            "source": "config",
            "updated_at": null,
            "config_default_min_cash_buffer_pct": min_cash_buffer_pct
        })
    }

    pub async fn saxo_auth_status_value(&self) -> JsonValue {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before auth status failed: {err:#}");
        }
        let status = auth::auth_status(&self.config, &self.config_path, true).await;
        if status
            .get("connected")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
        {
            if let Err(err) = self.persist_saxo_session_file_to_db("auth_status").await {
                warn!("Saxo session database persistence after auth status failed: {err:#}");
            }
        }
        status
    }

    pub async fn saxo_session_value(&self) -> JsonValue {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before session API failed: {err:#}");
        }
        auth::session_api(&self.config, &self.config_path).await
    }

    pub async fn refresh_saxo_session(&self) -> Result<JsonValue> {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before refresh failed: {err:#}");
        }
        let status = auth::refresh_session(&self.config, &self.config_path).await?;
        self.persist_saxo_session_file_to_db("refresh").await?;
        Ok(status)
    }

    pub async fn user_logout_saxo_session(&self) -> Result<JsonValue> {
        // User SSO and Saxo OAuth are different security domains. Logging out
        // of the dashboard user must not delete the service-level Saxo refresh
        // token, because the scheduler keeps renewing that token without any
        // browser session. This endpoint therefore reports the current Saxo
        // status and leaves the durable `saxo_sessions` row untouched.
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore during user logout no-op failed: {err:#}");
        }
        let mut status = auth::auth_status(&self.config, &self.config_path, true).await;
        if status
            .get("connected")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
        {
            if let Err(err) = self
                .persist_saxo_session_file_to_db("user_logout_keepalive")
                .await
            {
                warn!("Saxo session database persistence after user logout no-op failed: {err:#}");
            }
        }
        if let Some(obj) = status.as_object_mut() {
            obj.insert("logout_scope".to_string(), json!("user"));
            obj.insert(
                "message".to_string(),
                json!("User logout does not disconnect the service-level Saxo session."),
            );
        }
        Ok(status)
    }

    pub async fn disconnect_saxo_session(&self) -> Result<JsonValue> {
        let status = auth::logout_session(&self.config, &self.config_path)?;
        self.clear_saxo_session_from_db().await?;
        Ok(status)
    }

    async fn ensure_runtime_state_schema(&self) -> Result<()> {
        // The database is the durable runtime state for tokens and operator
        // preferences. The on-disk session file is only an ephemeral working
        // copy for the OAuth helper functions.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS saxo_sessions (
                singleton_key TEXT PRIMARY KEY,
                session_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Saxo session state table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS runtime_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating runtime settings table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_position_snapshots (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                instrument_name TEXT,
                isin TEXT,
                uic INTEGER,
                asset_type TEXT,
                quantity REAL NOT NULL,
                currency TEXT,
                open_price_local REAL,
                open_price_including_costs_local REAL,
                execution_time_open TEXT,
                value_date TEXT,
                market_state TEXT,
                can_be_closed INTEGER,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker position snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_instrument_exposures (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                uic INTEGER,
                asset_type TEXT,
                quantity REAL,
                average_open_price REAL,
                profit_loss_on_trade REAL,
                instrument_price_day_percent_change REAL,
                currency TEXT,
                calculation_reliability TEXT,
                can_be_closed INTEGER,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker instrument exposures table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_balance_snapshots (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                currency TEXT,
                cash_available_for_trading REAL,
                margin_available_for_trading REAL,
                cash_balance REAL,
                transactions_not_booked REAL,
                settlement_value REAL,
                total_value REAL,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker balance snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_account_snapshots (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                account_key TEXT,
                account_id TEXT,
                account_currency TEXT,
                is_trial_account INTEGER,
                fractional_order_enabled INTEGER,
                fractional_order_enabled_asset_types_json TEXT,
                can_use_cash_positions_as_margin_collateral INTEGER,
                use_cash_positions_as_margin_collateral INTEGER,
                legal_asset_types_json TEXT,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker account snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS strategy_baselines (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                activated_at TEXT,
                status TEXT NOT NULL,
                goal_version INTEGER NOT NULL,
                config_json TEXT NOT NULL,
                prompt_json TEXT NOT NULL,
                source TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy baselines table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_reflections (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                period_start TEXT NOT NULL,
                period_end TEXT NOT NULL,
                goal_version INTEGER NOT NULL,
                summary TEXT NOT NULL,
                findings_json TEXT NOT NULL,
                proposed_actions_json TEXT NOT NULL,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes reflections table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                baseline_id TEXT,
                goal_version INTEGER NOT NULL,
                hypothesis TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                old_value_json TEXT NOT NULL,
                new_value_json TEXT NOT NULL,
                expected_effect TEXT NOT NULL,
                risk_notes TEXT NOT NULL,
                evidence_json TEXT NOT NULL,
                approval_json TEXT,
                metrics_json TEXT,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy experiments table")?;
        for sql in crate::markov_method::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Markov method runtime tables")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_reflections_created
             ON hermes_reflections(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes reflections created index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_strategy_experiments_status
             ON strategy_experiments(status, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy experiments status index")?;
        Ok(())
    }

    async fn sync_saxo_session_storage(&self) -> Result<()> {
        let file_session = auth::export_session_json(&self.config, &self.config_path).ok();
        let db_session = self.load_saxo_session_from_db().await?;

        match (file_session, db_session) {
            (Some(file), Some(db)) => {
                if saxo_session_score(&db) >= saxo_session_score(&file) {
                    auth::import_session_json(&self.config, &self.config_path, &db)
                        .context("restoring Saxo session file from database")?;
                    info!("Saxo session file restored from database state");
                } else {
                    self.save_saxo_session_to_db(&file, "startup_file_sync")
                        .await?;
                    info!("Saxo session database state updated from local file");
                }
            }
            (Some(file), None) => {
                self.save_saxo_session_to_db(&file, "startup_file_import")
                    .await?;
                info!("Saxo session database state initialized from local file");
            }
            (None, Some(db)) => {
                auth::import_session_json(&self.config, &self.config_path, &db)
                    .context("restoring Saxo session file from database")?;
                info!("Saxo session file initialized from database state");
            }
            (None, None) => {
                info!("No Saxo session is cached in the file system or database");
            }
        }
        Ok(())
    }

    async fn load_saxo_session_from_db(&self) -> Result<Option<JsonValue>> {
        let Some(row) = self
            .first_json(
                "SELECT session_json, updated_at, source FROM saxo_sessions WHERE singleton_key = 'default' LIMIT 1",
            )
            .await?
        else {
            return Ok(None);
        };
        let value = row.get("session_json").cloned().unwrap_or(JsonValue::Null);
        if value.is_object() {
            return Ok(Some(value));
        }
        if let Some(text) = value.as_str() {
            return Ok(Some(
                serde_json::from_str(text).context("parsing Saxo session JSON from database")?,
            ));
        }
        Ok(None)
    }

    async fn save_saxo_session_to_db(&self, session: &JsonValue, source: &str) -> Result<()> {
        let session_text =
            serde_json::to_string(session).context("serializing Saxo session for database")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO saxo_sessions (singleton_key, session_json, updated_at, source)
             VALUES ('default', '{}', '{}', '{}')
             ON CONFLICT(singleton_key) DO UPDATE SET
                session_json = excluded.session_json,
                updated_at = excluded.updated_at,
                source = excluded.source",
            sql_escape(&session_text),
            sql_escape(&updated_at),
            sql_escape(source)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("persisting Saxo session to database")?;
        Ok(())
    }

    pub async fn persist_saxo_session_file_to_db(&self, source: &str) -> Result<()> {
        let session = auth::export_session_json(&self.config, &self.config_path)
            .context("reading Saxo session file for database persistence")?;
        self.save_saxo_session_to_db(&session, source).await
    }

    pub async fn clear_saxo_session_from_db(&self) -> Result<()> {
        sqlx::query("DELETE FROM saxo_sessions WHERE singleton_key = 'default'")
            .execute(&self.pool)
            .await
            .context("clearing Saxo session from database")?;
        Ok(())
    }

    async fn runtime_setting(&self, key: &str) -> Result<Option<JsonValue>> {
        let Some(row) = self
            .first_json(&format!(
                "SELECT value_json FROM runtime_settings WHERE key = '{}' LIMIT 1",
                sql_escape(key)
            ))
            .await?
        else {
            return Ok(None);
        };
        let value = row.get("value_json").cloned().unwrap_or(JsonValue::Null);
        if value.is_object() {
            return Ok(Some(value));
        }
        if let Some(text) = value.as_str() {
            return Ok(Some(
                serde_json::from_str(text).context("parsing runtime setting JSON")?,
            ));
        }
        Ok(None)
    }

    async fn save_runtime_setting(&self, key: &str, value: &JsonValue) -> Result<()> {
        let value_text = serde_json::to_string(value).context("serializing runtime setting")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO runtime_settings (key, value_json, updated_at)
             VALUES ('{}', '{}', '{}')
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            sql_escape(key),
            sql_escape(&value_text),
            sql_escape(&updated_at)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("persisting runtime setting")?;
        Ok(())
    }

    pub(crate) fn market_exchange_rows(&self) -> Vec<JsonValue> {
        let cache = current_saxo_exchange_calendar_cache();
        market_exchange_rows_for_config(&self.config, Utc::now(), cache.as_ref())
    }

    async fn first_json(&self, sql: &str) -> Result<Option<JsonValue>> {
        let row = sqlx::query(sql).fetch_optional(&self.pool).await?;
        Ok(row.map(|row| row_to_json(&row)))
    }

    async fn select_json(&self, sql: &str) -> Result<Vec<JsonValue>> {
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_json).collect())
    }
}

async fn saxo_reference_get_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
) -> Result<JsonValue> {
    let access_token = json_text(session, "access_token");
    if access_token.trim().is_empty() {
        bail!("Saxo access token is missing from session");
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!(
            "{}{}",
            saxo_openapi_base_url(state, session)?,
            path
        ))
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .query(query)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
        if let Some(error_text) = extract_saxo_error_text(&payload) {
            bail!("Saxo reference lookup failed: {error_text}");
        }
        let snippet: String = body.chars().take(300).collect();
        bail!(
            "Saxo reference lookup failed: HTTP {}: {}",
            status.as_u16(),
            snippet
        );
    }
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).context("parsing Saxo reference response")
}

fn extract_saxo_error_text(payload: &JsonValue) -> Option<String> {
    for key in ["Message", "ErrorMessage", "ErrorCode"] {
        if let Some(text) = payload.get(key).and_then(JsonValue::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    payload
        .get("ErrorInfo")
        .and_then(|value| {
            value
                .get("Message")
                .or_else(|| value.get("ErrorMessage"))
                .or_else(|| value.get("ErrorCode"))
        })
        .and_then(JsonValue::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

fn saxo_openapi_base_url(state: &AppState, session: &JsonValue) -> Result<&'static str> {
    let environment = json_text(session, "environment")
        .trim()
        .to_string()
        .to_lowercase();
    let environment = if environment.is_empty() {
        yaml_string(&state.config, &["saxo", "environment"])
            .unwrap_or_else(|| "sim".to_string())
            .to_lowercase()
    } else {
        environment
    };
    match environment.as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn saxo_exchange_calendar_cache_lock() -> &'static RwLock<Option<SaxoExchangeCalendarCache>> {
    SAXO_EXCHANGE_CALENDAR_CACHE.get_or_init(|| RwLock::new(None))
}

fn current_saxo_exchange_calendar_cache() -> Option<SaxoExchangeCalendarCache> {
    saxo_exchange_calendar_cache_lock()
        .read()
        .ok()
        .and_then(|cache| cache.clone())
}

fn market_exchange_rows_for_config(
    config: &YamlValue,
    now_utc: DateTime<Utc>,
    cache: Option<&SaxoExchangeCalendarCache>,
) -> Vec<JsonValue> {
    let offset_minutes =
        yaml_i64(config, &["analysis_windows", "offset_minutes_after_open"]).unwrap_or(30);
    let pre_sync_minutes = yaml_i64(
        config,
        &["analysis_windows", "pre_sync_minutes_before_analysis"],
    )
    .unwrap_or(5);
    let end_buffer_minutes = yaml_i64(
        config,
        &["analysis_windows", "end_buffer_minutes_before_close"],
    )
    .unwrap_or(15);
    default_exchanges()
        .into_iter()
        .map(|exchange| {
            market_exchange_row(
                &exchange,
                now_utc,
                offset_minutes,
                pre_sync_minutes,
                end_buffer_minutes,
                cache,
            )
        })
        .collect()
}

fn market_exchange_row(
    exchange: &ExchangeRuntime,
    now_utc: DateTime<Utc>,
    offset_minutes: i64,
    pre_sync_minutes: i64,
    end_buffer_minutes: i64,
    cache: Option<&SaxoExchangeCalendarCache>,
) -> JsonValue {
    let tz = exchange
        .timezone
        .parse::<Tz>()
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let local_now = now_utc.with_timezone(&tz);
    let local_date = local_now.date_naive();
    let is_weekend = local_now.weekday().number_from_monday() >= 6;
    let saxo_calendar = cache.and_then(|cache| cache.exchanges.get(exchange.code));
    let configured_holiday = if !is_weekend {
        configured_holiday_name(exchange.code, local_date)
    } else {
        None
    };
    let saxo_day_session =
        saxo_calendar.and_then(|calendar| saxo_trading_session_for_date(calendar, tz, local_date));
    let day_session = saxo_day_session.or_else(|| {
        if saxo_calendar.is_none() && !is_weekend && configured_holiday.is_none() {
            let open_local = local_session_time(tz, local_date, exchange.open_time);
            let close_local = local_session_time(tz, local_date, exchange.close_time);
            Some(ExchangeDaySession {
                open_at: open_local.with_timezone(&Utc),
                close_at: close_local.with_timezone(&Utc),
            })
        } else {
            None
        }
    });
    let holiday_name = if day_session.is_none() && !is_weekend {
        configured_holiday
    } else {
        None
    };

    let current_saxo_state = saxo_calendar.and_then(|calendar| {
        calendar
            .sessions
            .iter()
            .find(|session| session.start_at <= now_utc && now_utc < session.end_at)
            .map(|session| session.state.as_str())
    });
    let calendar_source = if saxo_calendar.is_some() {
        cache
            .map(|cache| cache.source.as_str())
            .unwrap_or("saxo_ref_v1_exchanges")
    } else if holiday_name.is_some() {
        "configured_holiday"
    } else {
        "configured"
    };
    let calendar_last_checked = cache
        .map(|cache| {
            cache
                .checked_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or_default();

    if let Some(day_session) = day_session {
        let open_local = day_session.open_at.with_timezone(&tz);
        let close_local = day_session.close_at.with_timezone(&tz);
        let tradable_close_local =
            close_local - Duration::minutes(exchange.tradable_close_offset_minutes);
        let tradable_close_at = tradable_close_local.with_timezone(&Utc);
        let is_open = current_saxo_state
            .map(is_saxo_open_state)
            .unwrap_or(now_utc >= day_session.open_at && now_utc <= day_session.close_at);
        let is_tradable = current_saxo_state
            .map(is_saxo_trading_state)
            .unwrap_or(now_utc >= day_session.open_at && now_utc < tradable_close_at)
            && now_utc < tradable_close_at;
        let open_analysis_start = open_local + Duration::minutes(offset_minutes);
        let open_analysis_end = std::cmp::max(
            open_analysis_start,
            tradable_close_local - Duration::minutes(end_buffer_minutes),
        );
        let pre_sync_start = std::cmp::max(
            open_local,
            open_analysis_start - Duration::minutes(pre_sync_minutes),
        );
        let pre_analysis_sync_active =
            local_now >= pre_sync_start && local_now < open_analysis_start;
        let open_analysis_window_active =
            local_now >= open_analysis_start && local_now <= open_analysis_end;
        let next_open = saxo_calendar
            .and_then(|calendar| next_saxo_open_time(calendar, now_utc))
            .map(|value| value.with_timezone(&tz))
            .unwrap_or_else(|| next_open_time(tz, exchange, local_now));
        let status_reason = current_saxo_state
            .map(saxo_status_reason)
            .unwrap_or_else(|| {
                if local_now < open_local {
                    "Pre-open"
                } else if local_now >= tradable_close_local && local_now <= close_local {
                    "Closed - Closing auction / post-trade"
                } else if local_now > close_local {
                    "Closed - After hours"
                } else {
                    "Open"
                }
            });
        return json!({
            "code": exchange.code,
            "market": exchange.name,
            "timezone": exchange.timezone,
            "local_time": local_now.format("%Y-%m-%d %H:%M").to_string(),
            "status_reason": status_reason,
            "holiday_name": JsonValue::Null,
            "session_open_local": open_local.format("%Y-%m-%d %H:%M").to_string(),
            "session_close_local": close_local.format("%Y-%m-%d %H:%M").to_string(),
            "tradable_close_local": tradable_close_local.format("%Y-%m-%d %H:%M").to_string(),
            "session_open_at_utc": day_session.open_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "session_close_at_utc": day_session.close_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "tradable_close_at_utc": tradable_close_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "is_open": is_open,
            "is_tradable": is_tradable,
            "pre_analysis_sync_active": pre_analysis_sync_active,
            "open_analysis_window_active": open_analysis_window_active,
            "close_analysis_window_active": false,
            "analysis_window_active": open_analysis_window_active,
            "pre_analysis_sync_start_at_utc": pre_sync_start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "open_analysis_window_start_at_utc": open_analysis_start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "open_analysis_window_end_at_utc": open_analysis_end.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "next_open_at_utc": next_open.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "next_open": next_open.format("%Y-%m-%d %H:%M").to_string(),
            "calendar_source": calendar_source,
            "calendar_last_checked": calendar_last_checked,
            "saxo_exchange_id": saxo_calendar.map(|calendar| calendar.exchange_id.clone()).unwrap_or_default(),
            "saxo_exchange_name": saxo_calendar.and_then(|calendar| calendar.name.clone()).unwrap_or_default(),
            "saxo_timezone_id": saxo_calendar.and_then(|calendar| calendar.timezone_id.clone()).unwrap_or_default(),
            "saxo_session_state": current_saxo_state.unwrap_or_default(),
        });
    }

    let next_open = saxo_calendar
        .and_then(|calendar| next_saxo_open_time(calendar, now_utc))
        .map(|value| value.with_timezone(&tz))
        .unwrap_or_else(|| next_open_time(tz, exchange, local_now));
    let status_reason = if is_weekend {
        "Closed - Weekend".to_string()
    } else if let Some(holiday) = holiday_name {
        format!("Closed - {holiday}")
    } else if saxo_calendar.is_some() {
        "Closed - No Saxo trading session".to_string()
    } else {
        let open_local = local_session_time(tz, local_date, exchange.open_time);
        if local_now < open_local {
            "Pre-open".to_string()
        } else {
            "Closed - After hours".to_string()
        }
    };

    json!({
        "code": exchange.code,
        "market": exchange.name,
        "timezone": exchange.timezone,
        "local_time": local_now.format("%Y-%m-%d %H:%M").to_string(),
        "status_reason": status_reason,
        "holiday_name": holiday_name.unwrap_or_default(),
        "session_open_local": "n/a",
        "session_close_local": "n/a",
        "tradable_close_local": "n/a",
        "session_open_at_utc": JsonValue::Null,
        "session_close_at_utc": JsonValue::Null,
        "tradable_close_at_utc": JsonValue::Null,
        "is_open": false,
        "is_tradable": false,
        "pre_analysis_sync_active": false,
        "open_analysis_window_active": false,
        "close_analysis_window_active": false,
        "analysis_window_active": false,
        "pre_analysis_sync_start_at_utc": JsonValue::Null,
        "open_analysis_window_start_at_utc": JsonValue::Null,
        "open_analysis_window_end_at_utc": JsonValue::Null,
        "next_open_at_utc": next_open.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "next_open": next_open.format("%Y-%m-%d %H:%M").to_string(),
        "calendar_source": calendar_source,
        "calendar_last_checked": calendar_last_checked,
        "saxo_exchange_id": saxo_calendar.map(|calendar| calendar.exchange_id.clone()).unwrap_or_default(),
        "saxo_exchange_name": saxo_calendar.and_then(|calendar| calendar.name.clone()).unwrap_or_default(),
        "saxo_timezone_id": saxo_calendar.and_then(|calendar| calendar.timezone_id.clone()).unwrap_or_default(),
        "saxo_session_state": current_saxo_state.unwrap_or_default(),
    })
}

fn saxo_session_score(session: &JsonValue) -> (i64, i64) {
    let now = Utc::now().timestamp();
    let refresh_invalid = non_empty_session_text(session.get("refresh_token_invalid_at")).is_some();
    let has_refresh = non_empty_session_text(session.get("refresh_token")).is_some();
    let has_access = non_empty_session_text(session.get("access_token")).is_some();
    let refresh_expires_at = parse_session_time(session.get("refresh_token_expires_at"));
    let access_expires_at = parse_session_time(session.get("access_token_expires_at"));

    // Compare health before recency. A freshly marked-invalid cache should never
    // overwrite an older cache that still has a usable refresh token.
    let health = if refresh_invalid {
        0
    } else if has_refresh && refresh_expires_at.is_none_or(|expires_at| expires_at > now) {
        3
    } else if has_access && access_expires_at.is_some_and(|expires_at| expires_at > now) {
        1
    } else {
        0
    };

    (health, saxo_session_rank(session))
}

struct ExchangeRuntime {
    code: &'static str,
    name: &'static str,
    timezone: &'static str,
    open_time: NaiveTime,
    close_time: NaiveTime,
    tradable_close_offset_minutes: i64,
}

fn default_exchanges() -> Vec<ExchangeRuntime> {
    vec![
        exchange("XCSE", "Copenhagen", "Europe/Copenhagen", 9, 0, 17, 0, 0),
        exchange("XLON", "London", "Europe/London", 8, 0, 16, 30, 0),
        exchange(
            "XETR",
            "Frankfurt / Xetra",
            "Europe/Berlin",
            9,
            0,
            17,
            30,
            0,
        ),
        exchange(
            "XAMS",
            "Amsterdam / Euronext",
            "Europe/Amsterdam",
            9,
            0,
            17,
            30,
            0,
        ),
        exchange("XNAS", "Nasdaq US", "America/New_York", 9, 30, 16, 0, 0),
        exchange("XNYS", "NYSE", "America/New_York", 9, 30, 16, 0, 0),
        exchange("XSTO", "Stockholm", "Europe/Stockholm", 9, 0, 17, 30, 0),
        exchange("XOSL", "Oslo", "Europe/Oslo", 9, 0, 16, 30, 5),
        exchange("XHEL", "Helsinki", "Europe/Helsinki", 10, 0, 18, 30, 0),
        exchange("XMIL", "Milan", "Europe/Rome", 9, 0, 17, 30, 0),
    ]
}

fn exchange(
    code: &'static str,
    name: &'static str,
    timezone: &'static str,
    open_hour: u32,
    open_minute: u32,
    close_hour: u32,
    close_minute: u32,
    tradable_close_offset_minutes: i64,
) -> ExchangeRuntime {
    ExchangeRuntime {
        code,
        name,
        timezone,
        open_time: NaiveTime::from_hms_opt(open_hour, open_minute, 0).unwrap_or(NaiveTime::MIN),
        close_time: NaiveTime::from_hms_opt(close_hour, close_minute, 0).unwrap_or(NaiveTime::MIN),
        tradable_close_offset_minutes,
    }
}

fn saxo_exchange_calendar_from_detail(
    detail: &JsonValue,
    fallback_exchange_id: &str,
) -> Option<SaxoExchangeCalendar> {
    let exchange_id = saxo_exchange_text(detail, "ExchangeId")
        .unwrap_or_else(|| fallback_exchange_id.to_string());
    let sessions = parse_saxo_exchange_sessions(detail);
    if sessions.is_empty() {
        return None;
    }
    Some(SaxoExchangeCalendar {
        exchange_id,
        name: saxo_exchange_text(detail, "Name"),
        timezone_id: saxo_exchange_text(detail, "TimeZoneId"),
        sessions,
    })
}

fn parse_saxo_exchange_sessions(detail: &JsonValue) -> Vec<SaxoExchangeSession> {
    let Some(sessions) = detail.get("ExchangeSessions").and_then(JsonValue::as_array) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|session| {
            let start = saxo_exchange_text(session, "StartTime")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())?
                .with_timezone(&Utc);
            let end = saxo_exchange_text(session, "EndTime")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())?
                .with_timezone(&Utc);
            let state =
                saxo_exchange_text(session, "State").unwrap_or_else(|| "Undefined".to_string());
            Some(SaxoExchangeSession {
                start_at: start,
                end_at: end,
                state,
            })
        })
        .collect()
}

fn saxo_exchange_matches(value: &JsonValue, code: &str) -> bool {
    ["ExchangeId", "Mic", "IsoMic", "OperatingMic"]
        .iter()
        .filter_map(|key| saxo_exchange_text(value, key))
        .any(|value| value.eq_ignore_ascii_case(code))
}

fn saxo_exchange_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn saxo_trading_session_for_date(
    calendar: &SaxoExchangeCalendar,
    tz: Tz,
    local_date: NaiveDate,
) -> Option<ExchangeDaySession> {
    let sessions = calendar
        .sessions
        .iter()
        .filter(|session| is_saxo_trading_state(&session.state))
        .filter(|session| session_overlaps_local_date(session, tz, local_date))
        .collect::<Vec<_>>();
    let open_at = sessions.iter().map(|session| session.start_at).min()?;
    let close_at = sessions.iter().map(|session| session.end_at).max()?;
    Some(ExchangeDaySession { open_at, close_at })
}

fn next_saxo_open_time(
    calendar: &SaxoExchangeCalendar,
    now_utc: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    calendar
        .sessions
        .iter()
        .filter(|session| is_saxo_trading_state(&session.state))
        .filter(|session| session.start_at > now_utc)
        .map(|session| session.start_at)
        .min()
}

fn session_overlaps_local_date(
    session: &SaxoExchangeSession,
    tz: Tz,
    local_date: NaiveDate,
) -> bool {
    let start_date = session.start_at.with_timezone(&tz).date_naive();
    let end_date = (session.end_at - Duration::seconds(1))
        .with_timezone(&tz)
        .date_naive();
    start_date <= local_date && local_date <= end_date
}

fn is_saxo_trading_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "automatedtrading"
            | "pittrading"
            | "callauctiontrading"
            | "auction"
            | "openingauction"
            | "tradingatlast"
    )
}

fn is_saxo_open_state(state: &str) -> bool {
    !matches!(
        state.to_ascii_lowercase().as_str(),
        "closed" | "break" | "halt" | "suspended" | "undefined"
    )
}

fn saxo_status_reason(state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "automatedtrading" | "pittrading" | "callauctiontrading" | "auction" | "openingauction"
        | "tradingatlast" => "Open",
        "preautomatedtrading" | "premarket" | "pretrading" => "Pre-open",
        "postautomatedtrading" | "postmarket" | "posttrading" => {
            "Closed - Closing auction / post-trade"
        }
        "break" => "Closed - Exchange break",
        "halt" => "Closed - Halted",
        "suspended" => "Closed - Suspended",
        "closed" => "Closed",
        _ => "Closed - Unknown Saxo session state",
    }
}

fn configured_holiday_name(exchange_code: &str, local_date: NaiveDate) -> Option<&'static str> {
    match (
        exchange_code,
        local_date.year(),
        local_date.month(),
        local_date.day(),
    ) {
        ("XCSE", 2026, 1, 1) => Some("New Year's Day"),
        ("XCSE", 2026, 4, 2) => Some("Maundy Thursday"),
        ("XCSE", 2026, 4, 3) => Some("Good Friday"),
        ("XCSE", 2026, 4, 6) => Some("Easter Monday"),
        ("XCSE", 2026, 5, 14) => Some("Ascension Day"),
        ("XCSE", 2026, 5, 15) => Some("Day after Ascension Day"),
        ("XCSE", 2026, 5, 25) => Some("Whit Monday"),
        ("XCSE", 2026, 6, 5) => Some("Constitution Day"),
        ("XCSE", 2026, 12, 24) => Some("Christmas Eve"),
        ("XCSE", 2026, 12, 25) => Some("Christmas Day"),
        ("XCSE", 2026, 12, 31) => Some("New Year's Eve"),
        ("XLON", 2026, 1, 1) => Some("New Year's Day"),
        ("XLON", 2026, 4, 3) => Some("Good Friday"),
        ("XLON", 2026, 4, 6) => Some("Easter Monday"),
        ("XLON", 2026, 5, 4) => Some("Early May bank holiday"),
        ("XLON", 2026, 5, 25) => Some("Spring bank holiday"),
        ("XLON", 2026, 8, 31) => Some("Summer bank holiday"),
        ("XLON", 2026, 12, 25) => Some("Christmas Day"),
        ("XLON", 2026, 12, 28) => Some("Boxing Day (substitute day)"),
        ("XETR", 2026, 1, 1) => Some("New Year's Day"),
        ("XETR", 2026, 4, 3) => Some("Good Friday"),
        ("XETR", 2026, 4, 6) => Some("Easter Monday"),
        ("XETR", 2026, 12, 24) => Some("Christmas Eve"),
        ("XETR", 2026, 12, 25) => Some("Christmas Day"),
        ("XETR", 2026, 12, 31) => Some("New Year's Eve"),
        ("XAMS", 2026, 1, 1) => Some("New Year's Day"),
        ("XAMS", 2026, 4, 3) => Some("Good Friday"),
        ("XAMS", 2026, 4, 6) => Some("Easter Monday"),
        ("XAMS", 2026, 5, 1) => Some("Labour Day"),
        ("XAMS", 2026, 12, 25) => Some("Christmas Day"),
        ("XNAS", 2026, 1, 1) => Some("New Year's Day"),
        ("XNAS", 2026, 1, 19) => Some("Martin Luther King Jr. Day"),
        ("XNAS", 2026, 2, 16) => Some("Presidents Day"),
        ("XNAS", 2026, 4, 3) => Some("Good Friday"),
        ("XNAS", 2026, 5, 25) => Some("Memorial Day"),
        ("XNAS", 2026, 6, 19) => Some("Juneteenth"),
        ("XNAS", 2026, 7, 3) => Some("Independence Day (observed)"),
        ("XNAS", 2026, 9, 7) => Some("Labor Day"),
        ("XNAS", 2026, 11, 26) => Some("Thanksgiving Day"),
        ("XNAS", 2026, 12, 25) => Some("Christmas Day"),
        ("XNYS", 2026, 1, 1) => Some("New Year's Day"),
        ("XNYS", 2026, 1, 19) => Some("Martin Luther King Jr. Day"),
        ("XNYS", 2026, 2, 16) => Some("Washington's Birthday"),
        ("XNYS", 2026, 4, 3) => Some("Good Friday"),
        ("XNYS", 2026, 5, 25) => Some("Memorial Day"),
        ("XNYS", 2026, 6, 19) => Some("Juneteenth"),
        ("XNYS", 2026, 7, 3) => Some("Independence Day (observed)"),
        ("XNYS", 2026, 9, 7) => Some("Labor Day"),
        ("XNYS", 2026, 11, 26) => Some("Thanksgiving Day"),
        ("XNYS", 2026, 12, 25) => Some("Christmas Day"),
        ("XSTO", 2026, 1, 1) => Some("New Year's Day"),
        ("XSTO", 2026, 1, 6) => Some("Epiphany"),
        ("XSTO", 2026, 4, 3) => Some("Good Friday"),
        ("XSTO", 2026, 4, 6) => Some("Easter Monday"),
        ("XSTO", 2026, 5, 1) => Some("Labour Day"),
        ("XSTO", 2026, 5, 14) => Some("Ascension Day"),
        ("XSTO", 2026, 6, 19) => Some("Midsummer Eve"),
        ("XSTO", 2026, 12, 24) => Some("Christmas Eve"),
        ("XSTO", 2026, 12, 25) => Some("Christmas Day"),
        ("XSTO", 2026, 12, 31) => Some("New Year's Eve"),
        ("XOSL", 2026, 1, 1) => Some("New Year's Day"),
        ("XOSL", 2026, 4, 2) => Some("Maundy Thursday"),
        ("XOSL", 2026, 4, 3) => Some("Good Friday"),
        ("XOSL", 2026, 4, 6) => Some("Easter Monday"),
        ("XOSL", 2026, 5, 1) => Some("Labour Day"),
        ("XOSL", 2026, 5, 14) => Some("Ascension Day"),
        ("XOSL", 2026, 5, 25) => Some("Whit Monday"),
        ("XOSL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XOSL", 2026, 12, 25) => Some("Christmas Day"),
        ("XOSL", 2026, 12, 31) => Some("New Year's Eve"),
        ("XHEL", 2026, 1, 1) => Some("New Year's Day"),
        ("XHEL", 2026, 1, 6) => Some("Epiphany"),
        ("XHEL", 2026, 4, 3) => Some("Good Friday"),
        ("XHEL", 2026, 4, 6) => Some("Easter Monday"),
        ("XHEL", 2026, 5, 1) => Some("Labour Day"),
        ("XHEL", 2026, 5, 14) => Some("Ascension Day"),
        ("XHEL", 2026, 6, 19) => Some("Midsummer Eve"),
        ("XHEL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XHEL", 2026, 12, 25) => Some("Christmas Day"),
        ("XHEL", 2026, 12, 31) => Some("New Year's Eve"),
        ("XMIL", 2026, 1, 1) => Some("New Year's Day"),
        ("XMIL", 2026, 4, 3) => Some("Good Friday"),
        ("XMIL", 2026, 4, 6) => Some("Easter Monday"),
        ("XMIL", 2026, 5, 1) => Some("Labour Day"),
        ("XMIL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XMIL", 2026, 12, 25) => Some("Christmas Day"),
        ("XMIL", 2026, 12, 31) => Some("New Year's Eve"),
        _ => None,
    }
}

fn local_session_time(tz: Tz, date: chrono::NaiveDate, time: NaiveTime) -> DateTime<Tz> {
    tz.with_ymd_and_hms(
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute(),
        0,
    )
    .single()
    .unwrap_or_else(|| Utc::now().with_timezone(&tz))
}

fn next_open_time(tz: Tz, exchange: &ExchangeRuntime, local_now: DateTime<Tz>) -> DateTime<Tz> {
    for offset in 0..10 {
        let candidate_date = local_now.date_naive() + Duration::days(offset);
        if candidate_date.weekday().number_from_monday() >= 6 {
            continue;
        }
        if configured_holiday_name(exchange.code, candidate_date).is_some() {
            continue;
        }
        let candidate = local_session_time(tz, candidate_date, exchange.open_time);
        if candidate > local_now {
            return candidate;
        }
    }
    local_session_time(
        tz,
        local_now.date_naive() + Duration::days(1),
        exchange.open_time,
    )
}

fn market_names_where(items: &[JsonValue], key: &str) -> Vec<String> {
    items
        .iter()
        .filter(|row| row.get(key).and_then(JsonValue::as_bool).unwrap_or(false))
        .filter_map(|row| row.get("market").and_then(JsonValue::as_str))
        .map(ToString::to_string)
        .collect()
}

fn performance_start_at(range_key: &str) -> Option<String> {
    let now = Utc::now();
    let start = match range_key.to_uppercase().as_str() {
        "1D" => now - Duration::days(1),
        "1W" => now - Duration::weeks(1),
        "1M" => now - Duration::days(31),
        "3M" => now - Duration::days(93),
        "YTD" => Utc
            .with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now - Duration::days(31)),
        "1Y" => now - Duration::days(366),
        "ALL" => return None,
        _ => now - Duration::days(1),
    };
    Some(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn performance_rows_have_same_values(left: &JsonValue, right: &JsonValue) -> bool {
    const EPSILON_DKK: f64 = 0.01;
    let numeric_keys = [
        "total_market_value_dkk",
        "invested_market_value_dkk",
        "cash_balance_dkk",
        "total_cost_basis_dkk",
        "total_unrealised_pnl_dkk",
        "total_daily_pnl_dkk",
    ];
    numeric_keys
        .iter()
        .all(|key| (value_f64(left, key) - value_f64(right, key)).abs() <= EPSILON_DKK)
        && value_i64(left, "position_count") == value_i64(right, "position_count")
}

fn text_value(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

trait BlankStringExt {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>;
}

impl BlankStringExt for String {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>,
    {
        if self.trim().is_empty() {
            fallback()
        } else {
            Some(self)
        }
    }
}

fn fx_rate_to_dkk(currency: &str) -> f64 {
    // Static fallback rates mirror the old Python service fallback. Price snapshots
    // carry fresher per-symbol FX rates when the market data job has populated them.
    match currency.trim().to_uppercase().as_str() {
        "DKK" => 1.0,
        "EUR" => 7.4604,
        "USD" => 7.0215,
        "GBP" => 8.70,
        "NOK" => 0.64,
        "SEK" => 0.67,
        "PLN" => 1.75,
        _ => 1.0,
    }
}

fn exchange_code(symbol: &str) -> String {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange.to_string())
        .unwrap_or_default()
}

fn exchange_region(symbol: &str) -> String {
    match exchange_code(symbol).to_lowercase().as_str() {
        "xcse" | "xsto" | "xosl" | "xhel" => "Nordics",
        "xlon" => "UK",
        "xnas" | "xnys" => "US",
        _ => "Europe",
    }
    .to_string()
}

fn localization_settings_key(sso_session: &JsonValue) -> String {
    let user_key = sso_session
        .get("user")
        .and_then(|user| user.get("email"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default");
    format!("localization:{user_key}")
}

fn instrument_name_for_symbol(symbol: &str) -> String {
    let base = symbol
        .split_once(':')
        .map(|(base, _)| base)
        .unwrap_or(symbol);
    match base.to_uppercase().as_str() {
        "AAPL" => "Apple".to_string(),
        "ADBE" => "Adobe".to_string(),
        "ADI" => "Analog Devices".to_string(),
        "AMD" => "Advanced Micro Devices".to_string(),
        "AMZN" => "Amazon.com".to_string(),
        "ASML" => "ASML ADR".to_string(),
        "AVGO" => "Broadcom".to_string(),
        "DDOG" => "Datadog".to_string(),
        "GOOGL" => "Alphabet Inc. Class A".to_string(),
        "IBM" => "IBM".to_string(),
        "INTC" => "Intel".to_string(),
        "MA" => "Mastercard".to_string(),
        "MDB" => "MongoDB".to_string(),
        "MSTR" => "MicroStrategy".to_string(),
        "NVDA" => "NVIDIA".to_string(),
        "PANW" => "Palo Alto Networks".to_string(),
        "PLTR" => "Palantir Technologies".to_string(),
        "QCOM" => "Qualcomm".to_string(),
        "SNOW" => "Snowflake".to_string(),
        "V" => "Visa".to_string(),
        other => other.to_string(),
    }
}

fn saxo_session_rank(session: &JsonValue) -> i64 {
    // The latest useful timestamp wins within the same health tier. Refreshes update
    // `last_refreshed_at`, while a fresh OAuth callback may only have `created_at`.
    [
        "last_refreshed_at",
        "created_at",
        "access_token_expires_at",
        "refresh_token_expires_at",
    ]
    .iter()
    .filter_map(|key| parse_session_time(session.get(*key)))
    .max()
    .unwrap_or(0)
}

fn parse_session_time(value: Option<&JsonValue>) -> Option<i64> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.timestamp())
}

fn non_empty_session_text(value: Option<&JsonValue>) -> Option<&str> {
    let text = value?.as_str()?;
    if text.is_empty() { None } else { Some(text) }
}

#[allow(dead_code)]
fn deterministic_selected_assets(
    positions: &[JsonValue],
    watchlists: &JsonValue,
) -> Vec<JsonValue> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for row in positions.iter().take(12) {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        selected.push(json!({
            "symbol": symbol,
            "score": (value_f64(row, "allocation_pct") * 100.0).max(50.0),
            "notes": "Existing portfolio holding included in the manual fallback review.",
            "source": "portfolio"
        }));
    }
    if let Some(categories) = watchlists.get("categories").and_then(JsonValue::as_array) {
        for category in categories {
            let Some(items) = category.get("items").and_then(JsonValue::as_array) else {
                continue;
            };
            for row in items.iter().take(6) {
                let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
                if symbol.is_empty() || !seen.insert(symbol.to_string()) {
                    continue;
                }
                selected.push(json!({
                    "symbol": symbol,
                    "score": value_f64(row, "change_pct").abs().max(50.0),
                    "notes": "Watchlist symbol included in the manual fallback review.",
                    "source": category.get("key").and_then(JsonValue::as_str).unwrap_or("watchlist")
                }));
                if selected.len() >= 20 {
                    return selected;
                }
            }
        }
    }
    selected
}

#[allow(dead_code)]
fn deterministic_symbol_sentiment(
    positions: &[JsonValue],
    selected_assets: &[JsonValue],
) -> Vec<JsonValue> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for row in positions.iter() {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        let allocation = value_f64(row, "allocation_pct");
        let daily = value_f64(row, "daily_pnl_dkk");
        let sentiment = if allocation > 0.15 {
            "UNDERWEIGHT"
        } else if daily < -500.0 {
            "UNDERWEIGHT"
        } else {
            "HOLD"
        };
        rows.push(json!({
            "symbol": symbol,
            "sentiment": sentiment,
            "confidence": 50.0,
            "rationale": "Manual Rust fallback based on current allocation and daily P/L.",
            "risk_notes": ["Review manually before creating orders."]
        }));
    }
    for row in selected_assets {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        rows.push(json!({
            "symbol": symbol,
            "sentiment": "HOLD",
            "confidence": value_f64(row, "score"),
            "rationale": row.get("notes").cloned().unwrap_or_else(|| JsonValue::from("Manual fallback candidate.")),
            "risk_notes": ["No automated broker order was created."]
        }));
    }
    rows
}

#[allow(dead_code)]
fn deterministic_suggested_trades(
    positions: &[JsonValue],
    watchlists: &JsonValue,
) -> Vec<JsonValue> {
    let mut trades = Vec::new();
    for row in positions {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() {
            continue;
        }
        let allocation = value_f64(row, "allocation_pct");
        let unrealised = value_f64(row, "unrealised_pnl_dkk");
        if allocation > 0.15 || unrealised < -4000.0 {
            trades.push(json!({
                "symbol": symbol,
                "action": "SELL",
                "priority": "medium",
                "confidence": if allocation > 0.15 { 56.0 } else { 52.0 },
                "quantity_hint": "Reduce toward target allocation",
                "target_weight_pct": 5.56,
                "rationale": "Manual fallback flagged concentration or drawdown for operator review.",
                "risk_notes": ["No automatic order was queued."]
            }));
        }
        if trades.len() >= 6 {
            return trades;
        }
    }
    if let Some(categories) = watchlists.get("categories").and_then(JsonValue::as_array) {
        for category in categories {
            let Some(items) = category.get("items").and_then(JsonValue::as_array) else {
                continue;
            };
            for row in items {
                if value_f64(row, "change_pct") <= 2.0 {
                    continue;
                }
                let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
                if symbol.is_empty() {
                    continue;
                }
                trades.push(json!({
                    "symbol": symbol,
                    "action": "BUY",
                    "priority": "medium",
                    "confidence": value_f64(row, "change_pct").min(75.0),
                    "quantity_hint": "Review for possible starter allocation",
                    "target_weight_pct": 5.56,
                    "rationale": "Manual fallback highlighted positive watchlist momentum.",
                    "risk_notes": ["Confirm thesis, liquidity, and market window before trading."]
                }));
                if trades.len() >= 6 {
                    return trades;
                }
            }
        }
    }
    trades
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saxo_session_score_prefers_refreshable_session_over_invalid_recent_session() {
        let old_refreshable = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "refresh_token_expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339(),
            "last_refreshed_at": (Utc::now() - Duration::minutes(30)).to_rfc3339(),
        });
        let recently_invalid = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "refresh_token_invalid_at": Utc::now().to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339(),
            "last_refreshed_at": Utc::now().to_rfc3339(),
        });

        assert!(saxo_session_score(&old_refreshable) > saxo_session_score(&recently_invalid));
    }

    #[test]
    fn configured_holiday_fallback_closes_copenhagen_and_oslo_on_whit_monday_2026() {
        let config = YamlValue::Null;
        let now = DateTime::parse_from_rfc3339("2026-05-25T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = market_exchange_rows_for_config(&config, now, None);
        let copenhagen = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XCSE"))
            .unwrap();
        let oslo = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XOSL"))
            .unwrap();

        assert_eq!(
            copenhagen.get("status_reason").and_then(JsonValue::as_str),
            Some("Closed - Whit Monday")
        );
        assert_eq!(
            oslo.get("status_reason").and_then(JsonValue::as_str),
            Some("Closed - Whit Monday")
        );
        assert_eq!(
            copenhagen.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            oslo.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            copenhagen
                .get("next_open_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-05-26T07:00:00Z")
        );
        assert_eq!(
            oslo.get("next_open_at_utc").and_then(JsonValue::as_str),
            Some("2026-05-26T07:00:00Z")
        );
        for (code, reason) in [
            ("XLON", "Closed - Spring bank holiday"),
            ("XNAS", "Closed - Memorial Day"),
            ("XNYS", "Closed - Memorial Day"),
        ] {
            let row = rows
                .iter()
                .find(|row| row.get("code").and_then(JsonValue::as_str) == Some(code))
                .unwrap();
            assert_eq!(
                row.get("status_reason").and_then(JsonValue::as_str),
                Some(reason)
            );
            assert_eq!(
                row.get("is_tradable").and_then(JsonValue::as_bool),
                Some(false)
            );
        }
    }

    #[test]
    fn validates_hermes_experiment_lifecycle_transitions() {
        assert_eq!(
            hermes_experiment_next_status("pending_review", "approve_paper"),
            Some("approved_paper")
        );
        assert_eq!(
            hermes_experiment_next_status("active_sim", "ready_for_promotion"),
            Some("ready_for_promotion")
        );
        assert_eq!(
            hermes_experiment_next_status("ready_for_promotion", "promote"),
            Some("promoted")
        );
        assert_eq!(
            hermes_experiment_next_status("pending_review", "promote"),
            None
        );
    }
}
