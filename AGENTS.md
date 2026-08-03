# saxo-rust Project Guide

This is a Rust Axum + Dioxus Saxo day-trading dashboard. It replaced an earlier Python/FastAPI + Next.js implementation; that predecessor is gone from the active runtime (Next.js was removed 2026-07-04, the remaining Python is queued for removal per `wiki/urgent-todo.md`'s Python Removal Plan — the only Python still live is two Postgres backup CronJob scripts, unrelated to the trading application).

Follow the global instruction in `/Users/lindau/.codex/RTK.md`: prefix shell commands with `rtk`.

## Current Architecture

The active runtime is a single Rust binary named `saxo-rust`.

- HTTP/API server: Axum
- Server-rendered UI: Dioxus SSR
- Database access: `sqlx::AnyPool` so local SQLite and Kubernetes PostgreSQL can both be used
- Local database fallback: `ledger.db`
- Kubernetes database: existing CloudNativePG cluster in namespace `saxo`
- Kubernetes app namespace: `saxo`
- Public endpoint: shared ngrok gateway in `/Users/lindau/codex/shared-ngrok-gateway`, routing `/saxo-daytrader` to this repo's internal `saxo-daytrader.internal` AgentEndpoint

All trading-critical mutation paths run in Rust: Saxo OAuth/session handling, scheduled decision reports, Trading Manager queue creation, order precheck/placement, broker status sync, order cancel (`saxo_delete_json`; replace is cancel-and-reissue, not a PATCH-style update), fill reconciliation, and portfolio adoption. Nothing in the live path depends on the Python runtime.

## Project Knowledge Wiki

This repository uses an LLM-maintained knowledge wiki under `wiki/`. Treat it as a persistent synthesis layer for project learning, not as raw source of truth.

- Read `wiki/index.md` first for durable project knowledge.
- Follow `wiki/schema.md` when adding or updating wiki pages.
- Append wiki maintenance actions to `wiki/log.md`.
- Use `docs/project-wiki.md` for qmd and Obsidian setup.
- Keep raw sources immutable; summarize them in `wiki/sources/`.
- File durable insights from bug investigations, Hermes reflections, Saxo safety lessons, deployment work, and strategy experiments back into the wiki when they should survive the chat.
- Never store Saxo tokens, refresh tokens, `ClientKey`, `AccountKey`, API keys, database credentials, TradingView credentials, or unredacted broker responses in the wiki.

## Rust File Structure

Target these files for future Rust work:

- `src/main.rs`
  - Process startup only.
  - Initializes tracing, SQLx drivers, app state, and Axum server.
  - Dispatches `--scheduler` to the scheduler entry point.

- `src/api.rs`
  - Axum router and HTTP handlers.
  - Add or change API routes here.
  - Keep handler logic thin; move data construction into `state.rs` and broker/session mechanics into `auth.rs`.

- `src/auth.rs`
  - SSO header parsing for ngrok OAuth, Saxo OAuth start/callback, session cache inspection, refresh, and logout.
  - Target this file for Saxo authorization/session work.
  - Do not expose access or refresh tokens in JSON responses.
  - The token safety margin is intentionally proactive so page loads and scheduler heartbeats renew before expiry.
  - The pod-local `/tmp/daytrader/saxo_session.json` file is only an ephemeral working copy. Database durability is coordinated from `src/state.rs`.

- `src/localization.rs`
  - Locale, time zone, week-start, 12/24-hour clock, and number/date formatting helpers.
  - Target this file for regional display preferences before editing UI components.

- `src/state.rs`
  - Application state, config loading, database pool, and API payload builders.
  - Most dashboard API responses (execution, scheduler, decisions, positions, trades, performance, market status/watchlists, Hermes, Markov/Quiver signals — see `pub struct *Payload` in `src/models.rs`) are typed structs built here; a shrinking set of pages still return generic compatibility JSON. See the Refactoring And Architecture section of `wiki/roadmap.md` for the remaining conversions.
  - It also owns the `saxo_sessions` runtime table used to persist the refreshable Saxo session into CNPG/PostgreSQL so a Kubernetes rollout can restore the session into the next pod.

- `src/ui.rs`
  - Dioxus SSR components and formatting helpers.
  - Target this file for dashboard layout, display text, tables, and UI-only formatting.
  - UI formatting should call `localization.rs` helpers rather than formatting numbers and timestamps inline.
  - Dashboard tabs are selected by the `view` query parameter.

- `src/config.rs`
  - YAML/env config helpers.
  - Target this file for config resolution behavior.

- `src/db.rs`
  - Generic SQL row to JSON conversion and small DB/query helpers.
  - Target this file for shared database utilities.

- `src/models.rs`
  - Shared Rust structs for view models and request query/body types.
  - Add typed request/response structs here when replacing generic JSON.

- `src/scheduler.rs`
  - Rust scheduler entry point.
  - Maintains the Saxo session cache on each heartbeat; successful refreshes are persisted back to the database by `AppState`.
  - Runs scheduled OpenRouter decision reports, the Rust Trading Manager, Saxo execution queue processing, and the EOD journal cycle.

- `src/trading_manager.rs`
  - Turns fresh scheduled decision reports into local `execution_orders`.
  - Applies market-open filters, risk exclusions, minimum trade value, SELL holding caps, and technical gates before queueing.
  - It does not talk to Saxo directly; broker mutation belongs in `src/saxo_order.rs`.

- `src/saxo_order.rs`
  - Rust Saxo order placement path.
  - Restores/refreshes the service-level Saxo session, filters approved live Saxo orders, validates market tradability, guards SELL quantities against current holdings and active sell reservations, looks up instruments, builds Saxo order payloads, runs `/trade/v2/orders/precheck`, places via `/trade/v2/orders`, and writes `execution_orders` plus `execution_order_events`.
  - Target this file for order-placement bugs, Saxo payload shape changes, tick-size behavior, and queued-order processing tests.

- `assets/app.css`
  - Dashboard styling.

## Local Development

Use a workspace-local Cargo cache so builds do not need to write to the global Cargo cache:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo check
```

Common commands:

```bash
rtk make install
rtk make run
rtk make scheduler
rtk make fmt
rtk make test
rtk make check
rtk make validate
```

Local server defaults:

- App: `http://127.0.0.1:8000`
- Health: `http://127.0.0.1:8000/api/health`

For a smoke test on another port:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home BIND_ADDR=127.0.0.1:18001 cargo run --bin saxo-rust
rtk curl -sS http://127.0.0.1:18001/api/health
rtk curl -sS http://127.0.0.1:18001/api/overview
```

Stop any smoke-test server you start before ending work.

## Testing

Rust unit tests use the standard Rust pattern:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn behavior_is_verified() {}
}
```

Existing tests live beside the code they verify:

- `src/config.rs`
- `src/db.rs`
- `src/ui.rs`

Run:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo test
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo check
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo fmt --check
```

### Continuous Integration

`.github/workflows/ci.yml` mirrors `make validate` (`cargo fmt --check`, `cargo check --all-targets`, `cargo test`) on every push to `main`, every pull request, and on manual dispatch. It builds with `RUSTFLAGS: -D warnings`, so a new warning fails CI.

Keep the suite hermetic: no test may require network access, a live database, or a Saxo/OpenRouter credential. Database-backed fixtures use isolated in-memory SQLite. If a test genuinely needs an external dependency, mark it `#[ignore]` rather than weakening CI.

Do not set `CARGO_HOME` in the workflow. The repo-local `.cargo-home` is a local-development workaround and conflicts with the CI cache action.

Kubernetes manifest validation:

```bash
rtk kubectl kustomize deploy/k8s/base
rtk bash -n scripts/deploy_k8s_docker_desktop.sh
```

## Kubernetes Deployment

App resources run in namespace `saxo`.

Database resources remain in namespace `saxo`.

Important files:

- `deploy/k8s/base/namespace.yaml`
  - Creates namespace `saxo`.

- `deploy/k8s/base/kustomization.yaml`
  - Base app kustomization.
  - Namespace is `saxo`.
  - Includes app deployments, app services, PVCs, and config map.

- `deploy/k8s/base/api.yaml`
  - Rust app deployment `daytrader-api`.
  - Rust app service `daytrader-api`.
  - Frontend-compatible service `daytrader-frontend` pointing at `daytrader-api`.
  - Both services expose port `8000`.

- `deploy/k8s/base/scheduler.yaml`
  - Rust scheduler deployment `daytrader-scheduler`.
  - Runs `/app/saxo-rust --scheduler`.
  - Uses `strategy.type: Recreate` because the scheduler is a singleton that can submit broker orders; do not switch it back to rolling updates without a stronger cross-pod lock.

- `deploy/k8s/base/config.k8s.yaml`
  - Kubernetes runtime config.
  - Uses `portfolio.database_url: ENV:DATABASE_URL`.
  - Saxo session path is `/tmp/daytrader/saxo_session.json`.
  - The durable session is stored in the `saxo_sessions` database table at runtime.

- `deploy/k8s/base/ngrok-internal-agentendpoint.yaml`
  - App-owned internal ngrok `AgentEndpoint`.
  - Namespace: `saxo`.
  - Internal URL: `http://saxo-daytrader.internal:80`.
  - Target: `http://daytrader-frontend.saxo:8000`.
  - The shared public `AgentEndpoint/daytrader-frontend`, `NgrokTrafficPolicy/daytrader-oauth`, OAuth allow-list, and `/saxo-daytrader` route are owned by `/Users/lindau/codex/shared-ngrok-gateway`.

- `deploy/k8s/postgres/postgres-stack.template.yaml`
  - CloudNativePG cluster resources.
  - Namespace: `saxo`.
  - Keep this in `saxo` unless explicitly migrating the database.

- `scripts/deploy_k8s_docker_desktop.sh`
  - Main Docker Desktop deployment script.
  - Builds the Rust image.
  - Applies/keeps CNPG resources in `DB_NAMESPACE`, default `saxo`.
  - Applies app resources in `NAMESPACE`, default `saxo`.
  - Creates a `daytrader-postgres-app` secret in `saxo` containing a cross-namespace `DATABASE_URL`.
  - Applies only the app-owned internal `saxo-daytrader.internal` AgentEndpoint.

## Namespace And DNS Rules

Kubernetes secrets cannot be referenced across namespaces. Because the database is in `saxo` and the app is in `saxo`, the deploy script creates an app-local secret:

- Secret in app namespace: `saxo/daytrader-postgres-app`
- Key: `database-url`
- Value points at cross-namespace service DNS:
  - `daytrader-postgres-rw.saxo.svc.cluster.local:5432`

Do not change app pods to reference the CNPG-generated secret directly in `saxo`; that will not work across namespaces.

## Saxo Session Persistence

The Rust runtime stores the rollout-safe Saxo OAuth cache in `saxo_sessions` in the CNPG-backed `daytrader` database in namespace `saxo`. Pods use `/tmp/daytrader/saxo_session.json` only as an ephemeral working file for the OAuth helper code. On startup, API requests, scheduler heartbeats, OAuth callback, and refresh, `AppState` restores from or writes to the database row. User/SSO logout must not clear this service-level Saxo session because the scheduler renews it without a browser user; only the explicit `/api/saxo/session/disconnect` endpoint removes the durable row. The table contains tokens, so treat database access as credential access.

## ngrok

Shared public ngrok routing is owned by `/Users/lindau/codex/shared-ngrok-gateway`.
This repo must not apply the public `AgentEndpoint/daytrader-frontend` or `NgrokTrafficPolicy/daytrader-oauth`.

This repo owns only:

- `AgentEndpoint/saxo/saxo-daytrader-internal`
- internal URL: `http://saxo-daytrader.internal:80`
- upstream: `http://daytrader-frontend.saxo:8000`

The shared gateway owns:

- public endpoint domain
- Google OAuth provider and allow-list
- `/saxo-daytrader` path route
- route rewrite to `/`
- `forward-internal` to `http://saxo-daytrader.internal:80`

Required app `.env` value:

```bash
NGROK_DOMAIN=
```

`NGROK_DOMAIN` is used only to derive `DAYTRADER_PUBLIC_BASE_URL=https://$NGROK_DOMAIN/saxo-daytrader` when `DAYTRADER_PUBLIC_BASE_URL` is not set. Keep `NGROK_API_KEY`, `NGROK_AUTHTOKEN`, `NGROK_ALLOWED_EMAILS`, and `NGROK_OAUTH_PROVIDER` in the shared gateway env.

Useful shared gateway commands:

```bash
rtk make shared-ngrok-status
rtk make shared-ngrok-apply
```

## CloudNativePG

CNPG is intentionally kept in namespace `saxo`.

Primary cluster:

- `saxo/daytrader-postgres`
- writable service: `daytrader-postgres-rw.saxo.svc.cluster.local`
- app database: `daytrader`

The app receives `DATABASE_URL` from `saxo/daytrader-postgres-app`, not directly from the CNPG secret.

Useful checks:

```bash
rtk kubectl --context docker-desktop -n saxo get cluster,svc,pvc
rtk kubectl --context docker-desktop -n saxo get pods -l cnpg.io/cluster=daytrader-postgres
rtk kubectl --context docker-desktop -n saxo get pods,svc,agentendpoint,pvc
```

## Docker

The active Docker image is built by `Dockerfile.api`.

```bash
rtk make docker-build
```

The image contains:

- `/app/saxo-rust`
- `/app/config/config.yaml`

Default runtime env:

```bash
DAYTRADER_CONFIG=/app/config/config.yaml
BIND_ADDR=0.0.0.0:8000
```

## Deployment

Deploy to Docker Desktop Kubernetes:

```bash
rtk make k8s-deploy
```

Status:

```bash
rtk make k8s-status
rtk make k8s-db-status
```

Stop app resources:

```bash
rtk make k8s-stop
```

`k8s-stop` removes app resources from `saxo`. It should not delete the CNPG database in `saxo`.

## Saxo Safety Notes

When touching any Saxo trading path:

- Keep SIM and LIVE config/session paths separate.
- Never hard-code tokens, Saxo client secrets, `ClientKey`, or `AccountKey`.
- Use `AccountKey` for account-scoped trading and portfolio calls.
- Use `ClientKey` for Saxo endpoints that require client scope.
- Precheck orders before placement where Saxo supports it.
- Preserve `x-request-id` or equivalent idempotency-style headers for order mutations.
- Preserve local audit records for every precheck, placement, replace, cancel, fill, and reconciliation event.
- Normalize prices to valid Saxo tick increments before precheck/place.
- Broker status sync, cancel/reissue, fills, and reconciliation are implemented in Rust and run live; keep changes to these paths covered by tests before deploying.

For ongoing architecture direction (remaining generic-JSON-to-typed-struct conversions, `state.rs`/`ui.rs` size, module extraction candidates), see the Refactoring And Architecture section of `wiki/roadmap.md` rather than a fixed porting order — porting from Python is complete.
