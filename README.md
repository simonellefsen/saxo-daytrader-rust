# saxo-daytrader-xai

Rust/Dioxus conversion of the Saxo day-trading dashboard. The active runtime is now a single Rust binary, `saxo-rust`, built with Axum for HTTP/API routes and Dioxus SSR for the dashboard UI.

The previous Python/FastAPI and Next.js implementation is still present as legacy source while the deeper trading engines are ported. Live trading mutations intentionally return `501 not_ported` from the Rust runtime until Saxo order placement, replace/cancel, and reconciliation logic are reimplemented with the same audit guarantees as the Python version.

## Current Rust Runtime

- Rust 2024 project at [Cargo.toml](/Users/lindau/codex/rust_daytrader/Cargo.toml).
- Single HTTP process serving the dashboard and `/api/*` JSON endpoints on port `8000`.
- Dioxus-rendered dashboard in [src/main.rs](/Users/lindau/codex/rust_daytrader/src/main.rs).
- Workspace-local Cargo cache supported through `CARGO_HOME=.cargo-home`.
- Docker image built from [Dockerfile.api](/Users/lindau/codex/rust_daytrader/Dockerfile.api).
- The Rust app runs in Kubernetes namespace `saxo-rust`.
- The existing CloudNativePG database remains in namespace `saxo`; the Rust app connects to it through the cross-namespace service DNS name `daytrader-postgres-rw.saxo.svc.cluster.local`.
- Kubernetes now deploys `daytrader-api`, a `daytrader-frontend` service pointing at that Rust app, and `daytrader-scheduler` from the Rust image; the separate Next.js deployment is no longer part of the base kustomization.
- Hermes Agent self-improvement is designed as a separate, gated research/reflection workflow. See [docs/hermes-agent.md](/Users/lindau/codex/rust_daytrader/docs/hermes-agent.md) for the goal contract, one-variable experiment model, Kubernetes shape, MCP boundary, and safety invariants.
- Project knowledge is organized through an LLM-maintained wiki under [wiki/](/Users/lindau/codex/rust_daytrader/wiki), with workflow details in [docs/project-wiki.md](/Users/lindau/codex/rust_daytrader/docs/project-wiki.md).

## Legacy Phase 42 Surface

- Python 3.11+ project scaffold
- Local SQLite support at `ledger.db`
- PostgreSQL support for Kubernetes deployments through `portfolio.database_url`
- CloudNativePG deployment with one primary and one standby instance for Docker Desktop Kubernetes
- S3-compatible CloudNativePG backups for local development with MinIO or rustFS
- SQLite-to-PostgreSQL migration job for existing `ledger.db` data
- SIM/LIVE trading-environment metadata, account metadata, app-user metadata, and account-access tables prepared for future multi-account access control
- Configurable `config.yaml` with placeholders for API keys, Saxo credentials, exclusions, tax brackets, and commission settings
- CSV importer for the attached Saxo position export
- Strict exclusion of `NOVOb:xcse` and `TSLA:xnas`
- Tax-lot tracking based on imported holdings
- Danish share-income tax engine with 27% / 42% brackets
- Configurable commission and FX-conversion cost handling
- Immutable trade ledger and lot-realization records
- xAI decision engine using the official Responses API
- Structured JSON decision reports with step-by-step rationale, watchlist focus, and suggested trades
- Decision report persistence with prompt, raw response, parsed report, and error tracking
- APScheduler-based background worker for recurring analysis cycles
- Exchange-calendar driven market hours, holiday closures, and daylight-saving aware session timing
- Simulation execution queue with immutable ledger updates
- Live-mode approval queue with dry-run protection
- Saxo OpenAPI session cache with refresh-token reuse
- Saxo instrument lookup, precheck, and order submission for approved live orders
- Saxo broker-status synchronization for submitted live orders
- Local ledger reconciliation when Saxo reports a confirmed final fill
- Incremental local ledger reconciliation for confirmed partial fills
- Immutable `execution_fills` records for broker fill history and deduplication
- Immutable `execution_order_events` records for broker-side amendments, cancellations, rejections, and working-order state changes
- Broker-side amendment reconciliation that updates local working order quantity and price from Saxo
- Scheduler-driven daily performance summary generation
- Optional Slack webhook and SMTP email delivery for daily, weekly, monthly, quarterly, and YTD summaries
- Immutable `notification_deliveries` records and notification history in the UI
- Renderable `systemd` and `launchd` service templates for unattended local deployment
- Live Saxo order-management actions for broker-side replace and cancel requests
- Notification throttling, retry backoff, and richer structured daily-summary formatting
- Weekly and monthly digest generation with independent scheduling and per-kind notification deduplication
- Quarterly and year-to-date digest generation using the same immutable notification pipeline
- Broker alert notifications for fills, rejections, and cancel confirmations
- Per-kind delivery routing so digests and broker alerts can target different Slack webhooks or email recipient lists
- Severity-based broker alert suppression so repeated low-signal events can be throttled without disabling higher-value alerts
- Named route profiles so several digest or alert kinds can share one delivery destination without repeated config
- Grouped broker alerts so several broker updates for the same order can be collapsed into one delivery
- Autonomous app launcher mode that starts the web UI and background scheduler together for hands-off simulation trading
- Scheduler heartbeat and last-cycle status persisted to SQLite and shown in the web UI
- One-click scheduler cycle controls in the web UI for live or mock manual runs
- Route-profile formatting so subject prefixes, message preambles, and summary style can be shared across notification kinds
- Immutable scheduler cycle history with recent-cycle visibility in the web UI
- Scheduler stale-worker detection with bounded auto-restart for launcher-managed autonomous mode
- Configurable scheduler cycle-history retention by age and row count
- Detection and repair of invalid legacy simulation trades that exceed available holdings
- Whole-share execution enforcement so queued and submitted equity orders use integer quantities only
- Audit bundle CSV export for ledger, decisions, executions, and tax records
- FastAPI backend plus Next.js web UI with:
  - portfolio summary in DKK
  - holdings allocation table with live quote refresh support
  - daily refreshed Nordic and global watchlists
  - news, earnings, and macro headline tabs
  - market status and analysis-window detection
  - realised gain / tax summary from the trade ledger
  - a Decision Report tab that can auto-run during analysis windows or run on demand
  - an Execution tab for queued orders, live approvals, Saxo submission status, broker sync, and audit export
  - live broker order replace/cancel controls for manageable Saxo orders
  - a Notifications tab with summary preview and delivery history

## Install

```bash
make install
```

## Run

```bash
make run
```

Useful options:

```bash
API_PORT=8001 make run
make scheduler
```

The Rust app reads `DAYTRADER_CONFIG` when set and otherwise uses `config.yaml`. It prefers `DATABASE_URL` for Kubernetes/PostgreSQL and falls back to `portfolio.database_path` for local SQLite.

## Web UI

- Rust Axum/Dioxus app at `http://127.0.0.1:8000`
- Health check at `http://127.0.0.1:8000/api/health`
- Read-only API compatibility endpoints under `/api/*`
- SSO session JSON at `/auth/session` and `/api/auth/session`, populated from ngrok-injected `x-daytrader-user-*` headers.
- Saxo session endpoints at `/api/saxo/auth/status`, `/api/saxo/auth/start`, `/api/saxo/auth/callback`, `/api/saxo/session`, `/api/saxo/session/refresh`, `/api/saxo/session/logout`, and `/api/saxo/session/disconnect`.
- The Saxo session is persisted in the `saxo_sessions` database table so a Kubernetes rollout can restore the refresh token into a new pod. The local `/tmp/daytrader/saxo_session.json` file is only an ephemeral working copy inside each pod. Dashboard SSO logout deliberately does not clear this service-level session; use `/api/saxo/session/disconnect` only when the Saxo refresh token should be removed.
- Localization defaults live under `localization` in config and drive thousands separators, decimal separators, week start, time zone, and 12/24-hour display.
- Dashboard tabs are server-rendered at `/?view=performance`, `/?view=market`, `/?view=watchlists`, and `/?view=decisions`.
- The Rust scheduler proactively checks the Saxo session every scheduler interval and refreshes the token when it is inside the safety margin.

The mutation endpoints for decision generation, queue processing, broker sync, and live order management are deliberately disabled in Rust until those trading-critical paths are fully ported.

## Docker Desktop Kubernetes

The repository includes a local Docker Desktop Kubernetes deployment with app resources in namespace `saxo-rust` and the CNPG database in namespace `saxo`. It runs two Rust workloads:

- `daytrader-api`: Rust Axum/Dioxus app on port `8000`.
- `daytrader-frontend`: Kubernetes Service pointing at `daytrader-api` for the ngrok public endpoint.
- `daytrader-scheduler`: Rust scheduler placeholder using the same config and database-backed session state.

The API and scheduler use the existing `saxo/daytrader-postgres` CloudNativePG cluster as the live database via `DATABASE_URL`. Saxo OAuth state is persisted in the `saxo_sessions` table in Postgres; pods use `/tmp/daytrader/saxo_session.json` only as an ephemeral helper file while reading or refreshing the token. The table contains Saxo tokens, so database access should be treated as credential access.

The deployment also creates:

- `daytrader-postgres`: a two-instance CloudNativePG cluster with one primary and one standby in namespace `saxo`.
- S3-compatible CloudNativePG backup target: either a Docker-managed local MinIO container or an existing rustFS container.
- `saxo/daytrader-postgres-app`: CNPG app-user secret; deploy also mirrors a `DATABASE_URL`-only `daytrader-postgres-app` secret into `saxo-rust` because Kubernetes secret references cannot cross namespaces.

The `saxo-rust/daytrader-frontend` service is exposed through the ngrok Kubernetes operator with Google OAuth and an email allow-list.

Required `.env` values:

```bash
NGROK_API_KEY=
NGROK_AUTHTOKEN=
NGROK_DOMAIN=your-domain.ngrok.app
NGROK_OAUTH_PROVIDER=google
NGROK_ALLOWED_EMAILS=you@example.com,another@example.com
MINIO_HOST_PATH=
MINIO_ROOT_USER=daytrader
MINIO_ROOT_PASSWORD=change-me
MINIO_API_PORT=9000
MINIO_CONSOLE_PORT=9001
MINIO_ENDPOINT_URL=http://host.docker.internal:9000
BACKUP_OBJECT_STORE=minio
BACKUP_BUCKET=daytrader-cnpg
RUSTFS_ENDPOINT_URL=http://host.docker.internal:9000
RUSTFS_ACCESS_KEY=rustfsadmin
RUSTFS_SECRET_KEY=rustfsadmin
POSTGRES_APP_USER=daytrader
POSTGRES_APP_PASSWORD=change-me
HERMES_API_SERVER_ENABLED=false
HERMES_API_SERVER_HOST=0.0.0.0
HERMES_API_SERVER_KEY=
HERMES_API_SERVER_CORS_ORIGINS=http://127.0.0.1:8000
HERMES_INFERENCE_PROVIDER=xai
HERMES_MODEL=grok-4
HERMES_DASHBOARD=false
HERMES_DASHBOARD_TUI=false
HERMES_DAYTRADER_API_KEY=
HERMES_DAYTRADER_API_BASE_URL=http://daytrader-api.saxo-rust:8000
```

`NGROK_DOMAIN` must be a domain available in your ngrok account. `NGROK_OAUTH_PROVIDER` defaults to `google` when omitted. `BACKUP_OBJECT_STORE` defaults to `minio`; set it to `rustfs` to use an existing rustFS container instead of starting the deploy-managed MinIO container. `MINIO_HOST_PATH` defaults to `./minio-data` from the repository root when omitted. `MINIO_ENDPOINT_URL` and `RUSTFS_ENDPOINT_URL` default to `http://host.docker.internal:9000`, which is the Docker Desktop route from Kubernetes pods back to host-exposed Docker services. Keep the existing Saxo, xAI, Slack, and OpenFIGI values in `.env`; the deploy script creates the Kubernetes secret from that file.

Deploy to Docker Desktop:

```bash
make k8s-deploy
```

Useful Kubernetes targets:

```bash
make docker-build
make k8s-status
make k8s-db-status
make k8s-stop
```

`make k8s-deploy` builds a timestamped local Rust image, prepares the configured S3-compatible backup target, creates the `daytrader-cnpg` bucket, installs or upgrades the CloudNativePG and ngrok operators via Helm, applies/keeps the database resources in `saxo`, applies the Rust app resources in `saxo-rust`, and renders the ngrok OAuth endpoint to `saxo-rust/daytrader-frontend`. With `BACKUP_OBJECT_STORE=minio`, the deploy script starts or replaces the Docker MinIO container with `MINIO_HOST_PATH` bind-mounted to `/data`. With `BACKUP_OBJECT_STORE=rustfs`, it leaves the external rustFS container running and only verifies/creates the bucket. At runtime the app writes the latest Saxo session to the `saxo_sessions` table, so future rollouts can recover without another OAuth login while the refresh token remains valid.

The Kubernetes manifests use `imagePullPolicy: IfNotPresent`. This is intentional for Docker Desktop: the deploy script builds local images into the Docker Desktop image store and then updates deployments to those concrete image tags.

The S3-compatible backup target is intentionally run outside Kubernetes because Docker Desktop Kubernetes `hostPath` volumes are node-local and did not reliably mirror object files into the macOS project folder. The deploy-managed MinIO container uses a normal Docker bind mount, so backup objects should be visible under `./minio-data/daytrader-cnpg`. For rustFS, run the container separately on the host port in `RUSTFS_ENDPOINT_URL`.

CloudNativePG currently reports the built-in `barmanObjectStore` backup stanza as deprecated for a future CNPG release. It works for this local deployment, but the longer-term replacement is CNPG's Barman Cloud Plugin.

## Hermes Agent Research Loop

The Hermes integration plan keeps self-improvement outside the live broker mutation path. Hermes can observe scheduler cycles, decision reports, execution outcomes, and strategy journals through a read-mostly adapter, then propose one-variable experiments against an explicit goal contract. Proposed prompt/config/strategy changes must be recorded, reviewed, tested in backtest or SIM/paper mode, and promoted by an operator before they can become an active baseline.

The Kubernetes base now includes `Deployment/hermes-agent`, `PVC/hermes-data`, `Service/hermes-gateway`, `ConfigMap/hermes-daytrader-context`, and a suspended `CronJob/hermes-weekly-reflection`. The service is internal-only and exposes Hermes gateway port `8642` plus dashboard port `9119` if enabled. The deploy script creates a separate `hermes-env` secret from a whitelist of Hermes/model/chat variables; Saxo credentials are not included in that secret. Set `HERMES_API_SERVER_ENABLED=true` and a strong `HERMES_API_SERVER_KEY` when the internal Hermes API should be reachable. Set `HERMES_INFERENCE_PROVIDER` and `HERMES_MODEL` to a model/provider the configured Hermes account can access, and set `HERMES_DAYTRADER_API_KEY` so Hermes can call the app's protected `/api/hermes/*` adapter endpoints with `x-hermes-api-key`.

To enable the weekly Hermes reflection after deployment:

```bash
rtk kubectl --context docker-desktop -n saxo-rust patch cronjob hermes-weekly-reflection -p '{"spec":{"suspend":false}}'
```

To trigger one immediate run:

```bash
rtk kubectl --context docker-desktop -n saxo-rust create job --from=cronjob/hermes-weekly-reflection hermes-weekly-reflection-manual
```

See [docs/hermes-agent.md](/Users/lindau/codex/rust_daytrader/docs/hermes-agent.md) for the full architecture and rollout plan.

## Project Knowledge Wiki

The repository has a persistent LLM-maintained knowledge layer under [wiki/](/Users/lindau/codex/rust_daytrader/wiki). It is intended for maintained project synthesis: architecture decisions, Saxo safety lessons, Hermes reflections, strategy experiments, and operational runbooks. Use [wiki/index.md](/Users/lindau/codex/rust_daytrader/wiki/index.md) as the entry point and [wiki/schema.md](/Users/lindau/codex/rust_daytrader/wiki/schema.md) as the maintenance contract.

See [docs/project-wiki.md](/Users/lindau/codex/rust_daytrader/docs/project-wiki.md) for qmd and Obsidian setup.

PostgreSQL backup strategy:

- Kubernetes runs `daytrader-postgres-backup-schedule` at `15` minutes past each hour from `09:15` through `23:15`, Monday through Friday, in `Europe/Copenhagen` local time.
- Weekend backups are intentionally skipped while markets are closed. The last scheduled backup before the weekend is Friday `23:15` Copenhagen time; the cycle resumes Monday `09:15` Copenhagen time.
- The backup CronJob creates CloudNativePG `Backup` resources for `daytrader-postgres` using the `barmanObjectStore` method.
- The old CNPG `ScheduledBackup` resource is not used because the installed CNPG CRD does not expose a Kubernetes-style timezone field; using a Kubernetes CronJob keeps the schedule aligned with Copenhagen local time and DST.
- `daytrader-postgres-backup-retention` runs at `30` minutes past each hour on the same weekday backup window, after the scheduled backup should have completed.
- The retention job also purges weekend backups once at least one weekday backup exists, so old weekend backups from a previous schedule are kept only as a temporary safety net until the Monday cycle resumes.
- Retention keeps the latest `24` hourly backups.
- Older backups are compacted into one backup per day for `7` days.
- Older backups are compacted into one backup per ISO week for `4` weeks.
- Older backups are compacted into one backup per month for `12` months.
- Older backups are compacted into one backup per year for `10` years.
- The retention job deletes pruned CNPG `Backup` resources, invalid CNPG `Backup` resources whose base backup object is missing, and matching S3-compatible base-backup prefixes under `daytrader-cnpg/daytrader-postgres/base/`.
- WAL retention is kept conservative at `3650d` in CNPG so long-term retained base backups remain recoverable. This uses more storage, but avoids deleting WAL segments that a retained backup may need.

## Config Reference

The project is driven by [config.yaml](/Users/lindau/codex/daytrader/config.yaml). Values written as `ENV:NAME` are loaded from `.env`.

### `app`

- `project_name`: display name used in the UI.
- `environment`: free-form environment label such as `local`.
- `dry_run`: when `true`, live broker submission and live broker management are blocked even if `execution.mode` is `live`.
- `simulation_mode`: legacy convenience flag; execution behavior is primarily controlled by `execution.mode`.
- `launch_scheduler_with_ui`: when `true`, `main.py` starts the background scheduler together with the FastAPI + Next.js UI stack.
- `scheduler_restart_on_failure`: if the launcher-managed scheduler dies or becomes stale, `main.py` may restart it.
- `scheduler_max_restarts`: maximum restart attempts per app run.
- `scheduler_restart_delay_seconds`: wait time before each restart attempt.

### `portfolio`

- `base_currency`: reporting currency. The project assumes `DKK`.
- `source_csv`: Saxo export used for the latest imported holdings baseline.
- `database_path`: SQLite database path, usually `ledger.db`.
- `database_url`: optional PostgreSQL DSN. In Kubernetes this is set to `ENV:DATABASE_URL` and takes precedence over `database_path`.
- `initial_cash_dkk`: starting cash balance used for cash-aware portfolio value and buy-side limits. Buys reduce it, sells increase it through recorded `net_amount_dkk`.

### `market_data`

- `refresh_interval_seconds`: general UI/data refresh cadence for quote-oriented functions.
- `request_timeout_seconds`: timeout for market-data HTTP calls.
- `watchlists.nordic_limit`: target number of Nordic names shown in the watchlist.
- `watchlists.uk_limit`: target number of UK names shown in the watchlist.
- `watchlists.us_limit`: target number of US names shown in the watchlist.
- `watchlists.eu_limit`: target number of continental Europe / Euronext names shown in the watchlist.
- `watchlists.global_limit`: number of combined US/Europe names supplied to decision-report context.
- `rss.market_feeds`: RSS feeds for company/market headlines.
- `rss.macro_feeds`: RSS feeds for macro and central-bank headlines.
- `rss.crypto_feeds`: RSS feeds included in macro pulse context for crypto/risk-appetite signals.

### `price_monitor`

- `enabled`: enables persisted portfolio quote polling in the background worker.
- `poll_interval_minutes`: how often the scheduler refreshes latest portfolio prices while the price monitor is active. Current default is `1`.
- `post_close_grace_minutes`: how long quote polling continues after the final tracked exchange closes before it pauses until the next exchange open. Current default is `15`.
- `reset_hour_local`: local hour used as the daily baseline reset point. Current default is `6`.
- `timezone`: timezone used for the reset boundary. Default is `Europe/Copenhagen`.
- `history_max_rows`: optional cap on stored portfolio-value history points. `0` means unlimited.
- `history_retention_days`: optional max age for stored portfolio-value history points. `0` means unlimited.

The price monitor stores latest portfolio quotes in SQLite, appends portfolio-value history points for the Performance tab, and uses the first quote after the configured reset hour as the baseline for that day. Daily P/L in the UI is then calculated relative to that baseline instead of relying only on the CSV import. Quote polling runs while at least one tracked exchange is open, and keeps running for the configured post-close grace period after the last close. After that it pauses until the next tracked exchange opens.

### `analysis_windows`

- `offset_minutes_after_open`: how long after an exchange open the system starts considering that market eligible for analysis.
- `duration_minutes`: how long the analysis window stays active after the offset.
- `calendar_refresh_interval_minutes`: how often exchange session calendars are refreshed.
- `calendar_lookback_days`: how much recent session history is cached.
- `calendar_lookahead_days`: how far future holiday/session data is cached.

Example: with `offset_minutes_after_open: 30` and `duration_minutes: 0`, a market that opens at `09:00` local will have an analysis window from `09:30` until 15 minutes before the exchange-specific tradable close.

### `scheduler`

- `enabled`: enables the scheduler worker.
- `poll_interval_minutes`: how often the worker wakes up and runs one scheduler cycle.
- `startup_run`: when `true`, a cycle runs immediately when the scheduler starts instead of waiting for the first interval boundary.
- `history_max_rows`: maximum scheduler-cycle history rows to retain.
- `history_retention_days`: maximum age of scheduler-cycle history rows.

### `strategy`

- `enabled`: enables the deterministic strategy overlay on top of xAI sentiment.
- `mode`: `swing` is the default disciplined swing/day strategy. Set `ladder` only to use the legacy intraday ladder engine.
- `selection_interval_minutes`: minimum spacing between strategy-driven re-selection passes.
- `max_candidates`, `min_selected_assets`, `max_selected_assets`: selection funnel sizing.
- `max_assets_per_sector`: optional diversification cap when sector labels are available.
- `capital.max_deployment_pct`: hard ceiling on deployed capital. Default `0.90` to preserve a 10% cash buffer.
- `capital.min_cash_buffer_pct`: cash reserve kept out of new swing entries. Default `0.10`.
- `swing.min_holdings` / `swing.max_holdings`: hard portfolio count guardrails, default `10` to `25`.
- `swing.min_holding_weight_pct` / `swing.max_holding_weight_pct`: hard target weight guardrails, default `5%` to `25%`.
- `swing.never_trade_symbols`: hard blacklist. Defaults include `NOVOb:xcse` and `TSLA:xnas`.
- `swing.daily_indicators`: daily-chart MA/MACD/RSI/Bollinger/Stochastic/Volume confluence settings used to filter swing entries.
- `swing.journal`: daily/weekly/monthly learning journal cadence used to feed recent lessons back into decision prompts.
- `swing.analysis_pulses`: timezone-aware daily decision triggers for the Nordic/EU open +1h15 report and the US open +1h15 report.
- `ladder.*`: legacy rung count, ATR spacing, stop/take-profit multiples, per-position weights, flatten timing, and trailing-stop behavior used only when `mode: ladder`.

### `execution`

- `mode`: `simulation` for local paper execution, `live` for Saxo broker submission/management.
- `adapter`: currently `saxo`.
- `auto_execute_simulation`: when `true`, simulation orders are executed automatically after queueing.
- `require_approval_live`: when `true`, live orders stay in approval state until manually approved.
- `min_trade_value_dkk`: ignores tiny orders below this estimated DKK size.
- `max_daily_orders`: daily cap on created execution orders.
- `delayed_price_limit_orders`: converts swing entries/exits to protective limit orders when prices may be delayed and lets the price monitor replace stale live swing limits.

### `risk`

- `excluded_symbols`: repo-safe list of blocked symbols.
- `excluded_symbols_csv`: optional comma-separated override loaded from environment.
- `max_position_weight`: maximum post-trade position weight as a fraction of total portfolio value.
- `allow_shorting`: should remain `false` for this project.

### `taxation`

- `share_income.currency`: tax reporting currency, expected to be `DKK`.
- `share_income.brackets`: Danish share-income brackets used by the sell calculator.

### `commissions`

- `default_rate`: percentage commission rate, e.g. `0.0008` for `0.08%`.
- `fx_conversion_rate`: FX conversion markup applied when trade currency is not `DKK`.
- `minimums`: per-exchange commission minima by currency and amount.

### `xai`

- `api_key`: xAI API key from `.env`.
- `base_url`: xAI API base URL.
- `model`: Grok model used for decision generation.
- `goal`: embedded objective included in every trading prompt.
- `timeout_seconds`: HTTP timeout for xAI calls.
- `auto_run_interval_minutes`: minimum spacing between automatic decision reports.
- `include_encrypted_reasoning`: when `true`, requests encrypted reasoning content from xAI.

### `saxo`

- `environment`: `SIM` or `LIVE`.
- `client_id`: Saxo app key.
- `client_secret`: Saxo app secret for secret-based auth flows.
- `client_key`: Saxo `ClientKey`, typically written by the OAuth helper.
- `account_key`: Saxo `AccountKey`, typically written by the OAuth helper.
- `session_path`: refreshable local Saxo session cache written by `--write-session`.

### `tradingview`

- `username`: TradingView username.
- `encrypted_password`: TradingView password secret.
- `totp_secret`: TOTP secret for 2FA automation.

### `openfigi`

- `enabled`: enables OpenFIGI fallback lookups when adding new assets that are not part of the imported Saxo CSV baseline.
- `api_key`: optional OpenFIGI API key from `.env`. Without a key, OpenFIGI still works but with lower rate limits.
- `base_url`: OpenFIGI API base URL.
- `timeout_seconds`: HTTP timeout for OpenFIGI mapping requests.

Important limitation: OpenFIGI's official mapping response returns FIGI metadata such as `figi`, `ticker`, `name`, `shareClassFIGI`, and `compositeFIGI`, but it does not return ISIN. In this project, Saxo metadata is therefore used first for ISIN enrichment, and OpenFIGI is used as a fallback to improve instrument naming and capture FIGI when Saxo metadata is unavailable.

### `notifications`

- `daily_summary_enabled`, `weekly_summary_enabled`, `monthly_summary_enabled`, `quarterly_summary_enabled`, `ytd_summary_enabled`: enable the corresponding digest types.
- `timezone`: local timezone for dispatch timing.
- `dispatch_hour_local`, `dispatch_minute_local`: local daily dispatch time.
- `weekly_dispatch_weekday_local`: weekday index for weekly digest dispatch.
- `monthly_dispatch_day_local`, `quarterly_dispatch_day_local`, `ytd_dispatch_day_local`: day-of-period dispatch points.
- `retry_backoff_minutes`: retry delay after a failed notification send.
- `max_attempts_per_day`: per-channel retry cap.
- `channel_cooldown_minutes`: minimum spacing between repeated sends of the same summary kind on the same channel.
- `summary_style`: default digest rendering style, e.g. `structured` or `compact`.
- `slack.enabled`: enable Slack delivery.
- `slack.webhook_url`: Slack webhook URL, normally from `.env`.
- `email.*`: SMTP configuration for email delivery.
- `alerts.execution_success_enabled`: alerts when a queued trade is executed in simulation or successfully submitted to Saxo.
- `alerts.execution_warning_enabled`: alerts for execution warnings such as `pending_approval`, `blocked_by_dry_run`, or invalid quantity.
- `alerts.broker_fill_enabled`: alerts for confirmed broker fills.
- `alerts.broker_reject_enabled`: alerts for broker rejections.
- `alerts.broker_cancel_enabled`: alerts for broker cancels/expirations.
- `alerts.execution_failure_enabled`: alerts when an execution order fails locally, e.g. session or lookup failure.
- `alerts.broker_management_failure_enabled`: alerts when a live cancel/replace request fails.
- `alert_suppression.*`: cooldown rules by severity.
- `alert_grouping.enabled`: group several broker updates for one order into one notification.
- `alert_grouping.max_items_per_group`: cap on grouped alert preview items.
- `route_profiles`: reusable delivery profiles.
- `routes`: per-summary-kind and per-alert-kind overrides, including:
  - `daily`, `weekly`, `monthly`, `quarterly`, `ytd`
  - `alert_execution_success`, `alert_execution_warning`
  - `alert_broker_fill`, `alert_broker_reject`, `alert_broker_cancel`, `alert_broker_grouped`
  - `alert_execution_failed`, `alert_broker_management_failed`

## Scheduler

Run the always-on background worker:

```bash
.venv/bin/python scripts/run_scheduler.py
```

Run one mock scheduler cycle for smoke testing:

```bash
.venv/bin/python scripts/run_scheduler.py --once --mock-decisions --force-decision
```

The scheduler:

- checks the configured exchange analysis windows
- refreshes portfolio quotes every `price_monitor.poll_interval_minutes` while at least one tracked exchange is open, then continues for `price_monitor.post_close_grace_minutes` after the final close before pausing until the next open
- refreshes the exchange-calendar cache on a recurring interval
- generates xAI decision reports during eligible windows
- queues suggested trades
- auto-executes queued trades in simulation mode
- dispatches execution notifications for successes, warnings, and failures
- sends one daily summary per configured channel after the local dispatch time
- sends optional weekly, monthly, quarterly, and YTD digests after their configured local dispatch windows
- sends optional broker event alerts when fills, rejections, or confirmed cancellations appear in the local broker sync tables
- supports per-kind routing overrides for daily/weekly/monthly/quarterly/YTD digests and broker alert types
- suppresses repeated broker alerts per order scope using severity-specific cooldown windows
- supports named route profiles plus per-kind overrides for Slack webhooks and email recipients
- supports route-profile formatting for subject prefixes, message preambles, and compact vs structured summary rendering
- can group several broker updates for one execution order into a single notification payload
- records scheduler activity in `audit_log`
- exposes dead/stale worker detection in the web UI using heartbeat age plus stored scheduler PID
- can be auto-restarted by `main.py` when launched in autonomous mode, using the configured restart budget in `app.scheduler_*`
- prunes old scheduler cycle-history rows automatically according to `scheduler.history_max_rows` and `scheduler.history_retention_days`
- flags impossible simulation trades in the Execution tab and can quarantine them from the effective portfolio state
- normalizes order quantities to whole shares before queueing, simulation execution, and Saxo order submission
- resolves Saxo instruments by Saxo's own symbol and exchange aliases, so `SBUX:xnas` and `MU:xnas` map correctly during broker submission
- pushes notification alerts when live execution fails, including session, lookup, and broker submission errors
- handles broker-side cancel/replace failures cleanly in the UI and pushes notifications for management failures without crashing the web runtime
- supports configurable starting cash in DKK, shows live cash balance in the portfolio summary, and adjusts cash automatically as trades execute
- persists latest portfolio quotes, resets the intraday baseline at `06:00` Europe/Copenhagen, and recalculates daily P/L from that baseline
- records historical portfolio-value samples so the web UI can graph daily, weekly, monthly, yearly, YTD, custom-range, and all-time performance

### What One Scheduler Cycle Does

Each scheduler cycle does the following in order:

1. Updates scheduler heartbeat and status in SQLite.
2. Refreshes exchange calendars if their refresh interval has elapsed.
3. Computes current market status and whether any analysis window is active.
4. Decides whether a new xAI decision report should be generated.
5. If eligible, generates a decision report.
6. Queues trades from the latest completed decision report.
7. If `execution.mode: simulation` and `execution.auto_execute_simulation: true`, executes queued simulation orders immediately.
8. Synchronizes broker order status for live orders.
9. Dispatches due digests and broker alerts.
10. Records cycle history and prunes old scheduler history rows.

The scheduler process also runs a separate quote-refresh job for portfolio prices. By default:

- the main decision cycle runs every `10` minutes
- the quote-refresh cycle runs every `1` minute

That means price colors and daily P/L can update more frequently than decision generation.

### What `poll_interval_minutes: 10` Means

`scheduler.poll_interval_minutes: 10` means the background worker wakes up every 10 minutes and runs exactly one scheduler cycle.

It does not mean:

- the app waits 10 minutes before every single trade inside a cycle
- the app trades exactly every 10 minutes
- the app generates a new xAI report every 10 minutes regardless of market state

What it really means in practice:

- the worker checks conditions every 10 minutes
- if no analysis window is active, the cycle mostly updates status and exits
- if an analysis window is active, the cycle may generate a new decision report if `xai.auto_run_interval_minutes` also allows it
- if a completed report exists, orders may be queued and simulation orders may execute in that same cycle

### Poll Interval vs Analysis Window

These settings interact:

- `analysis_windows.offset_minutes_after_open`
- `analysis_windows.duration_minutes`
- `scheduler.poll_interval_minutes`
- `xai.auto_run_interval_minutes`

Example with the current defaults:

- exchange opens at `09:00`
- analysis window starts at `10:00`
- analysis window ends at `10:45`
- scheduler polls at `09:50`, `10:00`, `10:10`, `10:20`, `10:30`, `10:40`, `10:50`

In that case:

- `09:50`: no analysis yet
- `10:00`: eligible
- `10:10`: still eligible
- `10:20`: still eligible
- `10:30`: still eligible
- `10:40`: still eligible
- `10:50`: too late

So a 10-minute poll interval gives you roughly 5 chances to catch a 45-minute analysis window.

If you make `poll_interval_minutes` too large relative to `duration_minutes`, you can miss windows entirely. For example:

- `poll_interval_minutes: 30`
- `duration_minutes: 45`

would be much easier to miss if the scheduler happens to poll just before the window opens and then only again after it closes.

### Recommended Scheduler Settings

- `poll_interval_minutes: 10` is a reasonable default for a lightweight always-on process.
- Use `5` if you want tighter reaction time and are comfortable with more frequent API/database activity.
- Avoid setting it higher than the analysis window duration unless you are comfortable occasionally missing an opportunity window.
- Keep `startup_run: true` so a restart immediately re-evaluates the system instead of waiting for the next 15-minute boundary.

## Deployment

Legacy Python `systemd` and `launchd` service examples can still be rendered explicitly:

```bash
.venv/bin/python scripts/render_service_templates.py
```

Or:

```bash
make legacy-render-services
```

This writes rendered files into `deploy/rendered/` using the current repo path and `.venv/bin/python`.

Rendered files:

- `deploy/rendered/systemd/saxo-daytrader-scheduler.service`
- `deploy/rendered/systemd/saxo-daytrader-dashboard.service`
- `deploy/rendered/launchd/com.saxo-daytrader.scheduler.plist`
- `deploy/rendered/launchd/com.saxo-daytrader.dashboard.plist`

The active deployment path is now the Rust/Kubernetes flow in the Makefile. The rendered service templates are retained only for legacy local operation.

Typical install flow:

1. Render the templates.
2. Review the generated paths, user, and port.
3. Copy the chosen service file into your OS service directory.
4. Enable the scheduler service first.
5. Optionally enable the dashboard service if you want the web UI always running.

Example `systemd` commands:

```bash
sudo cp deploy/rendered/systemd/saxo-daytrader-scheduler.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now saxo-daytrader-scheduler.service
```

Example `launchd` commands:

```bash
cp deploy/rendered/launchd/com.saxo-daytrader.scheduler.plist ~/Library/LaunchAgents/
launchctl unload ~/Library/LaunchAgents/com.saxo-daytrader.scheduler.plist 2>/dev/null || true
launchctl load ~/Library/LaunchAgents/com.saxo-daytrader.scheduler.plist
```

## Saxo OAuth helper

The project now auto-loads `.env` from the workspace root.

To discover your Saxo `ClientKey` and `AccountKey` after creating an OpenAPI app, use:

```bash
.venv/bin/python scripts/saxo_oauth_helper.py --environment sim --auth-mode pkce
```

Or with app secret:

```bash
.venv/bin/python scripts/saxo_oauth_helper.py --environment live --auth-mode secret
```

Notes:

- The same helper works for both `sim` and `live`.
- `sim` and `live` need separate Saxo app credentials.
- Your app must have a redirect URI that matches `SAXO_REDIRECT_URI` in `.env`.
- For local use, set a localhost redirect such as `http://localhost:8765/callback`.
- Use `--write-env` if you want the helper to write `SAXO_ENVIRONMENT`, `SAXO_CLIENT_KEY`, and `SAXO_ACCOUNT_KEY` back into `.env`.
- Use the Rust Saxo Login flow to persist a refreshable session into the `saxo_sessions` database table.

Examples:

```bash
.venv/bin/python scripts/saxo_oauth_helper.py --environment sim --auth-mode pkce --write-env --write-session
.venv/bin/python scripts/saxo_oauth_helper.py --environment live --auth-mode secret --write-env --write-session
```

The session cache is ignored by git and used by the live Saxo adapter to refresh access tokens automatically.

## Slack webhooks

Slack notifications use a Slack app with Incoming Webhooks.

Current setup flow, matching Slack's official documentation:

1. Go to [Slack apps](https://api.slack.com/apps).
2. Create a new app `From scratch` and select your Slack workspace.
3. In the app settings, open [Incoming Webhooks](https://docs.slack.dev/messaging/sending-messages-using-incoming-webhooks).
4. Turn `Activate Incoming Webhooks` on.
5. Click `Add New Webhook to Workspace`.
6. Choose the Slack channel that should receive notifications and authorize the app.
7. Copy the generated webhook URL. It will look like `https://hooks.slack.com/services/...`.

Add the webhook to your local `.env`:

```env
SLACK_WEBHOOK_URL=https://hooks.slack.com/services/...
```

Then enable Slack delivery in `config.yaml`:

```yaml
notifications:
  slack:
    enabled: true
    webhook_url: ENV:SLACK_WEBHOOK_URL
```

If you want different Slack destinations for different digests or broker alerts, use either per-kind routes or route profiles in `config.yaml`:

```yaml
notifications:
  route_profiles:
    ops:
      slack_webhook_url: ENV:SLACK_WEBHOOK_URL
  routes:
    weekly:
      profile: ops
    alert_broker_reject:
      profile: ops
```

Route profiles can also share formatting across multiple delivery kinds:

```yaml
notifications:
  route_profiles:
    ops:
      slack_webhook_url: ENV:SLACK_WEBHOOK_URL
      subject_prefix: "[OPS]"
      message_preamble: "Shared profile preamble"
      summary_style: compact
  routes:
    weekly:
      profile: ops
    alert_broker_fill:
      profile: ops
```

Treat webhook URLs as secrets. Do not commit them to git. Slack's documentation notes that leaked webhook URLs are actively revoked.

## Repository safety

Before pushing this project to GitHub:

- Keep real credentials only in `.env`, never in tracked files.
- Use `.env.example` as the committed template.
- Do not commit Saxo exports, position CSV files, SQLite databases, or tax/audit exports.
- The included `.gitignore` blocks `.env`, `*.csv`, and local database files by default.
- If anything sensitive has already been pushed to the public repo, removing it in a new commit is not enough. Rewrite git history and rotate the affected credentials.

## Validation

Run the Phase 36 validation script:

```bash
.venv/bin/python scripts/validate_phase36.py
```

Earlier phase validations remain available. To validate against the live xAI API:

```bash
.venv/bin/python scripts/validate_phase4.py --live
```

Expected output shape:

```text
Phase 34 validation passed.
First baseline date: 2026-04-06
MSTR daily pnl after move DKK: 3360.00
Reset baseline date: 2026-04-07
```

The exact order id values can vary slightly with the imported portfolio snapshot.

Earlier validation scripts remain available:

```bash
.venv/bin/python scripts/validate_phase1.py
.venv/bin/python scripts/validate_phase2.py
.venv/bin/python scripts/validate_phase3.py
.venv/bin/python scripts/validate_phase4.py
.venv/bin/python scripts/validate_phase5.py
.venv/bin/python scripts/validate_phase6.py
.venv/bin/python scripts/validate_phase7.py
.venv/bin/python scripts/validate_phase8.py
.venv/bin/python scripts/validate_phase9.py
.venv/bin/python scripts/validate_phase10.py
.venv/bin/python scripts/validate_phase11.py
.venv/bin/python scripts/validate_phase12.py
.venv/bin/python scripts/validate_phase13.py
.venv/bin/python scripts/validate_phase14.py
.venv/bin/python scripts/validate_phase15.py
.venv/bin/python scripts/validate_phase16.py
.venv/bin/python scripts/validate_phase17.py
.venv/bin/python scripts/validate_phase18.py
.venv/bin/python scripts/validate_phase19.py
.venv/bin/python scripts/validate_phase20.py
.venv/bin/python scripts/validate_phase21.py
.venv/bin/python scripts/validate_phase22.py
.venv/bin/python scripts/validate_phase23.py
.venv/bin/python scripts/validate_phase24.py
.venv/bin/python scripts/validate_phase25.py
.venv/bin/python scripts/validate_phase26.py
.venv/bin/python scripts/validate_phase27.py
.venv/bin/python scripts/validate_phase28.py
.venv/bin/python scripts/validate_phase29.py
```

## Project layout

```text
.
├── config.yaml
├── main.py
├── requirements.txt
├── scripts/
│   └── validate_phase1.py
│   └── validate_phase2.py
│   └── validate_phase3.py
│   └── validate_phase4.py
│   └── validate_phase5.py
│   └── validate_phase6.py
│   └── validate_phase7.py
│   └── validate_phase8.py
│   └── validate_phase9.py
│   └── validate_phase10.py
│   └── validate_phase11.py
│   └── validate_phase12.py
│   └── validate_phase13.py
│   └── validate_phase14.py
│   └── validate_phase15.py
│   └── validate_phase16.py
│   └── validate_phase17.py
│   └── validate_phase18.py
│   └── validate_phase19.py
│   └── validate_phase20.py
│   └── validate_phase21.py
│   └── validate_phase22.py
│   └── validate_phase23.py
│   └── validate_phase24.py
│   └── validate_phase25.py
│   └── validate_phase26.py
│   └── validate_phase27.py
│   └── validate_phase28.py
│   └── validate_phase29.py
│   └── validate_phase40.py
│   └── validate_phase41.py
│   └── validate_phase42.py
└── src/
    └── saxo_daytrader_xai/
        ├── api/
        │   └── app.py
        ├── config.py
        ├── db.py
        ├── execution_engine.py
        ├── fx_service.py
        ├── importer.py
        ├── market_data.py
        ├── market_news.py
        ├── market_schedule.py
        ├── market_symbols.py
        ├── notifications.py
        ├── portfolio.py
        ├── saxo_openapi.py
        ├── scheduler_service.py
        ├── strategy_engine.py
        ├── tax_engine.py
        ├── watchlists.py
        ├── xai_decision.py
```

## Next-phase todo

1. Add a launcher-visible incident counter and cooldown so repeated scheduler crashes can be surfaced more clearly in the web UI.
2. Add a one-click web UI action to prune scheduler history immediately using the current retention policy.
