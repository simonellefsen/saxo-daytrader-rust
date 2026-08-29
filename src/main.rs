mod api;
mod auth;
mod build_info;
mod config;
mod config_contract;
mod daily_indicators;
mod db;
mod debug_redaction;
mod decision_provider;
mod decision_schema;
mod drawdown_guard;
mod editorial_research;
mod execution_state;
mod fx;
mod hermes_state;
mod localization;
mod markov_method;
mod markov_state;
mod mcp;
mod models;
mod notifications;
mod performance_benchmarks;
mod performance_state;
mod portfolio_reset;
mod price_monitor;
mod protective_stops;
mod quiver;
mod quiver_state;
mod saxo_error;
mod saxo_http;
mod saxo_order;
mod saxo_portfolio;
mod saxo_rate_limit;
mod scheduler;
mod scheduler_state;
mod state;
mod strategy_journal;
mod strategy_journal_state;
mod trading_manager;
mod ui;
mod xai_decision;

use std::{env, sync::Arc};

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::{
    api::router, mcp::run_mcp_http, saxo_order::sync_saxo_broker_orders, scheduler::run_scheduler,
    state::AppState,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Rust binaries usually return a `Result` from `main` in real applications.
    // That gives us Python-like `raise` behavior through `?`, while still making
    // the error type explicit.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("saxo_rust=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // sqlx's `AnyPool` can talk to SQLite or PostgreSQL, but the drivers must be
    // registered once before opening the first connection.
    sqlx::any::install_default_drivers();

    let args = env::args().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--scheduler") {
        info!("starting process in scheduler mode");
        return run_scheduler().await;
    }
    if args.iter().any(|arg| arg == "--mcp-http") {
        info!("starting process in daytrader MCP HTTP mode");
        return run_mcp_http().await;
    }
    if args.iter().any(|arg| arg == "--sync-saxo-broker-orders") {
        info!("starting one-shot Saxo broker order sync");
        let state = AppState::load().await?;
        let result = sync_saxo_broker_orders(&state).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // `Arc` is Rust's thread-safe shared pointer. Axum clones this cheap pointer
    // into request handlers instead of cloning the whole app state.
    info!("starting process in web mode");
    let state = Arc::new(AppState::load().await.map_err(|err| {
        error!("application state failed to load: {err:#}");
        err
    })?);
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8000".to_string());
    let app = router(state);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    info!("serving Rust Dioxus app on http://{bind_addr}");
    axum::serve(listener, app).await.context("serving app")
}
