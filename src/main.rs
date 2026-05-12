mod api;
mod auth;
mod config;
mod db;
mod localization;
mod models;
mod scheduler;
mod state;
mod strategy_journal;
mod trading_manager;
mod ui;
mod xai_decision;

use std::{env, sync::Arc};

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::{api::router, scheduler::run_scheduler, state::AppState};

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
