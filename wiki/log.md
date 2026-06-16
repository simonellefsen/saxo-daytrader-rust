---
type: wiki-log
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-06-16
---

# Wiki Log

Append-only timeline for project wiki maintenance. Use headings with the format `## [YYYY-MM-DD] kind | summary` so agents and shell tools can parse the log.

## [2026-06-16] operations | Kubernetes namespace and backup helper cleanup

- Documented that app, Hermes, MCP, and CloudNativePG resources now run in the consolidated `saxo` namespace.
- Updated runbooks and Hermes configuration examples so in-cluster URLs use `.saxo` service DNS.
- Investigated `daytrader-postgres-backup-*` `StartError` pods and found the backup CronJobs were still invoking Python scripts inside the Rust runtime image.
- Added a dedicated backup helper image path for the Python `requests`/`boto3` backup scripts so the Rust app image can stay Python-free.

## [2026-05-25] implementation | Hermes daily EOD reflection

- Added suspended `CronJob/hermes-daily-reflection` for weekday end-of-day Hermes reflection.
- Kept `CronJob/hermes-weekly-reflection` for weekly self-improvement and one-variable experiment proposals.
- Updated README and wiki runbooks with daily and weekly Hermes reflection commands.

## [2026-05-23] ingest | LLM Wiki pattern

- Read the LLM Wiki source now archived at [wiki/sources/llm-wiki.md](/Users/lindau/codex/rust_daytrader/wiki/sources/llm-wiki.md).
- Created the initial project wiki structure under [wiki/](/Users/lindau/codex/rust_daytrader/wiki).
- Added schema, index, source note, and concept pages.
- Added [docs/project-wiki.md](/Users/lindau/codex/rust_daytrader/docs/project-wiki.md) for repo-level workflow documentation.

## [2026-05-23] attribution | LLM Wiki source credit

- Credited Andrej Karpathy as the author of the copied LLM Wiki idea file.
- Added the original gist URL: [karpathy/442a6bf555914893e9891c11519de94f](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).

## [2026-05-23] implementation | Hermes Kubernetes base

- Added initial Kubernetes support for Hermes Agent in `saxo-rust`.
- Added `hermes-agent`, `hermes-data`, `hermes-gateway`, and `hermes-daytrader-context`.
- Updated deployment scripting to create a separate `hermes-env` secret from a whitelist so Saxo credentials are not passed to Hermes.
- Documented that Hermes is internal-only and not yet connected to a daytrader MCP adapter or strategy promotion flow.

## [2026-05-23] implementation | Hermes HTTP adapter

- Added protected `/api/hermes/*` endpoints in `saxo-rust`.
- Added sanitized context, capabilities, reflection writes, and strategy experiment proposal writes.
- Added runtime tables for `hermes_reflections`, `strategy_experiments`, and `strategy_baselines`.
- Required `HERMES_DAYTRADER_API_KEY` for the adapter so these endpoints are not exposed as normal dashboard API routes.

## [2026-05-23] implementation | Hermes weekly reflection CronJob

- Added suspended `CronJob/hermes-weekly-reflection`.
- The CronJob submits a run to Hermes' `/v1/runs` API instead of writing reflections directly.
- The prompt instructs Hermes to fetch the protected daytrader context, create one reflection, and optionally create one one-variable experiment proposal.
- The job requires `HERMES_API_SERVER_KEY` and `HERMES_DAYTRADER_API_KEY`, and remains suspended until explicitly enabled.

## [2026-05-23] runbook | Build, test, deploy, and Saxo SIM checks

- Added `wiki/runbooks/build-test-deploy.md`.
- Documented Rust build, formatting, unit tests, integration/regression tests, local smoke tests, Kubernetes deployment and smoke tests, Hermes smoke tests, Saxo SIM testing order, live trading safety gates, and qmd/Obsidian-compatible wiki maintenance.

## [2026-05-23] smoke | Hermes in-cluster reflection

- Deployed Hermes Agent to Docker Desktop Kubernetes in namespace `saxo-rust`.
- Used `BACKUP_OBJECT_STORE=rustfs` because the local `daytrader_rustfs` container already owns ports `9000-9001`.
- Verified `daytrader-api` health from inside the cluster.
- Enabled Hermes API server with cluster-only generated keys and verified `/health`, `/v1/capabilities`, and the protected daytrader `/api/hermes/capabilities` endpoint.
- First Hermes run failed because the persisted Hermes default model was inaccessible; switching `/opt/data/config.yaml` to provider `xai` and model `grok-4` fixed model execution.
- Added a Hermes pod startup hook that applies `HERMES_MODEL` and `HERMES_INFERENCE_PROVIDER` to `/opt/data/config.yaml`.
- Manual reflection run `run_d56aacdb4f0e45b0abfda8dfd2145957` completed after approving internal HTTP adapter calls for the session.
- The run wrote reflection `hermes-reflection-1779537409085596` and created no experiment because closed-trade evidence was insufficient.

## [2026-05-23] runbook | Kubernetes diagnostics

- Added `wiki/runbooks/k8s-diagnostics.md`.
- Documented simple one-liners for Docker Desktop Kubernetes diagnostics, pod debugging, rollouts, in-cluster smoke tests, CloudNativePG, ngrok, Hermes, and RustFS.
- Clarified that RustFS is the normal S3-compatible storage backend and runs in the Docker context to use a local filesystem bind mount.

## [2026-05-23] implementation | Hermes review dashboard

- Added a read-only `Hermes` dashboard tab at `/?view=hermes`.
- Loaded recent `hermes_reflections` and `strategy_experiments` into the server-rendered dashboard model.
- Displayed the latest reflection summary, proposed actions, reflection history, experiment proposal status, one-variable path, and evidence preview.
- Kept the UI review-only; it does not approve, activate, promote, or mutate strategy baselines.

## [2026-05-23] implementation | Hermes SIM/paper overlays

- Added Trading Manager support for one approved Hermes experiment overlay in paper/simulation or Saxo SIM.
- Allowed only `approved_sim`, `active_sim`, `approved_paper`, and `active_paper` experiment statuses.
- Limited overlays to cash buffer, minimum trade value, and daily technical minimum confluence variables.
- Recorded the applied overlay in Trading Manager run JSON and queued order request JSON for auditability.
- Kept overlays disabled for `execution.mode=live` with `saxo.environment=LIVE`.

## [2026-05-23] maintenance | Remove duplicate root LLM Wiki source

- Kept the project copy of the LLM Wiki source note under [wiki/sources/llm-wiki.md](/Users/lindau/codex/rust_daytrader/wiki/sources/llm-wiki.md).
- Removed the duplicate root-level `llm-wiki.md`.
- Updated wiki metadata and docs to point at the wiki source note and original Andrej Karpathy gist.

## [2026-05-23] implementation | Hermes experiment lifecycle

- Added dashboard actions for Hermes experiment lifecycle transitions from `pending_review` through paper, SIM, readiness, rejection/failure, and promotion.
- Added an operator Rust transition path that records actor, action, status transition, notes, timestamp, and promoted baseline id in `approval_json`.
- Promotion creates a `strategy_baselines` audit record and supersedes prior active baseline records.
- Kept promotion as an audit/control-plane record only; it does not activate live broker behavior.

## [2026-05-23] implementation | Hermes baseline context

- Added active baseline visibility to the dashboard `Hermes` tab.
- Included the active `strategy_baselines` audit record in the protected Hermes context adapter.
- Included the active baseline payload in xAI decision prompts and required decision reports to return `strategy_baseline_id`.
- Kept baseline context advisory only; it does not approve orders, mutate Saxo sessions, or enable live overlays.

## [2026-05-23] implementation | Daytrader MCP adapter

- Added `saxo-rust --mcp-http`, an internal MCP endpoint for Hermes-safe daytrader tools.
- Added `Deployment/daytrader-mcp` and `Service/daytrader-mcp` in the `saxo-rust` namespace.
- Configured the Hermes pod startup hook to persist a filtered `daytrader` HTTP MCP server in `/opt/data/config.yaml`.
- Updated the weekly reflection job prompt to prefer MCP tools for context, reflection writes, and one-variable experiment proposals.
- Kept the MCP surface free of Saxo session reads, broker mutation tools, Kubernetes secret tools, and live order approval.

## [2026-05-23] smoke | Daytrader MCP in-cluster

- Deployed the Daytrader MCP adapter to Docker Desktop Kubernetes.
- Verified `daytrader-api`, `daytrader-scheduler`, `daytrader-mcp`, and `hermes-agent` rollouts.
- Verified in-cluster MCP health and Hermes persisted MCP config without printing the bearer token.
- Ran manual Hermes reflection `run_86428fcd12354288a2ffbb3d4ff9f629`; it created reflection `hermes-reflection-1779549919849580` and no experiment because evidence was insufficient.
- Added an init wait and deployment ordering so Hermes starts after `daytrader-mcp` is healthy.

## [2026-05-23] maintenance | Legacy Python Makefile cleanup

- Removed old Python convenience targets from the active Makefile surface.
- Kept legacy Python source, migration helpers, Saxo OAuth helper, and regression scripts as reference/support material while remaining broker paths are ported.
- Updated stale runtime wording in the dashboard and README.

## [2026-05-23] implementation | Markov method advisory skill

- Added a daily Rust Markov regime skill for portfolio and watchlist assets.
- Stored run summaries in `markov_signal_runs` and asset signals in `markov_asset_signals`.
- Exposed the latest signals through dashboard/API, Hermes context/MCP, and xAI decision prompt context.
- Added [wiki/sources/markov-hedge-fund-method.md](sources/markov-hedge-fund-method.md) for the source method.

## [2026-05-23] implementation | Hermes report access

- Added explicit Hermes MCP tools for recent decision reports, daily end-of-day reports, and Markov signals.
- Updated the Hermes Kubernetes tool allowlist and weekly reflection prompt so deployed Hermes can read those sources.
- Clarified that Hermes should treat Markov, decision reports, and EOD journals as advisory evidence and still cannot access Saxo secrets or broker mutation tools.

## [2026-05-25] implementation | Saxo-backed market calendars

- Changed the Rust market status path to refresh Saxo `/ref/v1/exchanges` once per UTC date and derive exchange windows from `ExchangeSessions` when available.
- Wired the refreshed calendar into dashboard market status, scheduled xAI report gating, Trading Manager queue gating, and live Saxo execution queue gating.
- Kept a no-secret configured holiday fallback for known 2026 exchange holidays, including Whit Monday closures for Copenhagen and Oslo, so temporary Saxo session/API failures do not incorrectly reopen known closed markets.

## [2026-05-25] implementation | Shared ngrok base path

- Made the Rust dashboard base-path aware for the shared ngrok endpoint at `/saxo-daytrader`.
- Added prefixed link, asset, form, and Saxo OAuth callback handling while keeping root routes available for local development and for prefix-stripping ngrok forwarding.
- Updated the ngrok manifest to preserve shared routing for `/danske-spil` and `/saxo-daytrader`, and added the internal `saxo-daytrader.internal` AgentEndpoint to the repo-managed manifests.
- Hardened Saxo OAuth start so callback URL generation prefers the configured public ngrok base URL over any internal forwarded host from shared endpoint routing.

## [2026-05-26] fix | Markov dashboard probability rendering

- Fixed Markov dashboard/API probability fields that rendered as zero/null because PostgreSQL `REAL` values were being read through the generic row adapter before float handling.
- Kept Markov `signed_signal`, `bull_prob`, `sideways_prob`, `bear_prob`, `rolling_return`, `threshold`, `current_close`, and `conviction` as fractional JSON values for UI, API, Hermes, and xAI context consumers.
- Changed Markov asset failure rows to persist full error chains on future runs so Saxo reference lookup, chart-history, entitlement, and HTTP failures are distinguishable.
- Made the Hermes daily and weekly Kubernetes CronJobs active in the base manifest so redeploys do not suspend reflections.
- Retried the failed Markov instrument set against Saxo SIM reference data. Most failures were transient/rate-limit related; added Markov Saxo GET pacing and HTTP 429 backoff to reduce future false failures.

## [2026-05-27] fix | Saxo sell guard position aggregation

- Investigated a Slack `execution_failed` alert for a live `MSTR:xnas` sell order from the US Open decision pulse.
- The sell guard correctly blocked broker submission before Saxo precheck, but the diagnostic exposed a parser bug: Saxo `/port/v1/positions/me` can return multiple rows for the same symbol, and the Rust guard was keeping the last row instead of summing all symbol rows.
- Changed the Saxo live position parser to aggregate duplicate symbol amounts before applying sell quantity and active reservation checks.
- Deployed the fix to Docker Desktop Kubernetes; the existing failed MSTR order remains an immutable audit row and should only be retried through an explicit live trading action.

## [2026-05-27] maintenance | Shared ngrok gateway ownership

- Updated this repo's operational docs to treat `/Users/lindau/codex/shared-ngrok-gateway` as the source of truth for the public ngrok endpoint, OAuth policy, allow-list, and `/saxo-daytrader` route.
- Kept this repo responsible only for the internal `saxo-daytrader.internal` AgentEndpoint that targets `daytrader-frontend.saxo-rust:8000`.
- Added Make targets for inspecting and applying the shared gateway from this repo without duplicating the public gateway manifests here.

## [2026-05-28] fix | Rust portfolio value snapshots

- Found that the Rust dashboard could calculate live portfolio performance from broker snapshots, but the scheduler did not persist those values into `portfolio_value_history`.
- Added Rust-side `portfolio_value_history` schema creation and a scheduler-cycle snapshot writer so performance history, EOD journals, and Hermes reflections have a durable valuation source.
- The missing 2026-05-27 valuation was not backfilled because no same-day persisted valuation existed; future scheduler cycles should record fresh snapshots before journal generation.

## [2026-05-28] fix | Positioner reset decimal parsing

- Found that the Rust SIM reset parser treated Saxo Positioner dot-decimal fields as Danish thousands-formatted values, corrupting reset cost basis for the 2026-05-18 import batch.
- Changed the reset parser to preserve dot decimals and added regression tests using an MSTR Positioner row.
- Repaired the affected reset batch rows in `position_snapshots` and `position_lots`, and corrected the MSTR/AJG sell ledger rows whose realised P/L had been calculated from the corrupted reset basis.

## [2026-05-28] improvement | Reinvestment pressure diagnostics

- Investigated cash accumulation and found the system was de-risking through SELL recommendations while recent scheduled reports supplied no actionable BUY candidates.
- Added a configurable `strategy.capital.reinvestment_pressure_threshold_pct` defaulting to 5 percentage points above the minimum cash buffer.
- Decision prompts now include explicit reinvestment pressure context and ask the model to either redeploy excess cash, wait in cash, or reduce risk with a stated reason.
- Trading Manager run records now include `reinvestment_diagnostics` so excess cash with no BUY candidates, blocked BUY candidates, and approved reinvestment candidates are distinguishable.

## [2026-05-28] config | Reduce cash buffer to 2%

- Reduced `strategy.capital.min_cash_buffer_pct` and `strategy.swing.cash_buffer_pct` from 10% to 2%.
- Raised `strategy.capital.max_deployment_pct` from 90% to 98% so the lower cash buffer is effective instead of being constrained by the deployment ceiling.
- Left `strategy.capital.reinvestment_pressure_threshold_pct` at 5 percentage points above the configured buffer, so reinvestment pressure now activates when cash is about 7% or higher.

## [2026-06-16] config | Switch decision reports and Hermes to OpenRouter

- Changed active decision-report configuration to use `OPENROUTER_API_KEY`, provider `openrouter`, base URL `https://openrouter.ai/api/v1`, and model `openai/gpt-5.5`.
- Updated the Rust decision-report transport so OpenRouter Chat Completions are recorded as completed reports immediately, while the old xAI deferred polling path is skipped unless the provider is explicitly set back to `xai`.
- Changed Hermes defaults to `HERMES_INFERENCE_PROVIDER=openrouter` and `HERMES_MODEL=openai/gpt-5.5`, and kept Hermes secrets limited to the Hermes/model/chat whitelist.
- Removed `XAI_API_KEY` from the app secret generation path after the OpenRouter migration so stale provider credentials are not carried into Kubernetes.
- Moved Hermes model/provider/MCP configuration from a `postStart` hook into the container startup wrapper and corrected the local MCP URL to `daytrader-mcp.saxo`, so Hermes reads the current provider and namespace before gateway startup.
- Fixed manual decision-report generation after it still surfaced the old xAI deferred parser error from a stale image and then timed out OpenRouter responses at 30 seconds. The Rust resolver now honors `xai.timeout_seconds`, local config uses a 600-second report timeout, and provider parse/body failures are stored as `xai_error` report rows instead of returning a raw handler error. Verified report `95` completed through OpenRouter after redeploy.

## [2026-06-16] fix | Reject malformed limit orders before execution

- Investigated execution orders 105-110 from report `95` and found every failed row was a local validation failure: the decision report emitted `Limit` orders without `limit_price_local`, so no Saxo precheck or broker placement was attempted.
- Added a Trading Manager order-shape gate that rejects unsupported order types, requires limit/stop prices where applicable, and only uses `price_local` as a positive fallback for limit prices.
- Updated the decision-report prompt schema to require `limit_price_local` whenever `order_type` is `Limit`, and to prefer `Market` when no explicit limit is intended.
- Hardened Saxo session handling by serializing in-pod refresh attempts and routing broker snapshots, price monitoring, Markov, daily indicators, execution, and order sync through the state-level database-backed session loader.
- Confirmed the Saxo 401 state required a manual SIM OAuth login; after reauth, the scheduler reported a healthy Saxo session and refreshed broker snapshots again.
- After report `97` successfully retried the `PLTR:xnas` sell, found that `BAC:xnys`, `CSCO:xnas`, and `ARM:xnas` starter BUYs were skipped because the duplicate-starter guard counted earlier `execution_failed` rows from report `95`.
- Changed the duplicate-starter guard to count only non-terminal BUY orders, so immutable failed audit rows do not block later same-day retries while pending, submitted, or executed orders still suppress duplicates.
- Added a Web UI runtime setting for the OpenRouter decision-report model, stored in `runtime_settings` and defaulting to `xai.model` from config. The settings form suggests `openrouter/fusion` as an operator-selectable model.
- Fixed manual decision-report redirects to stay under `/saxo-daytrader` and changed completed manual reports to immediately run the Trading Manager and Saxo execution queue instead of waiting for the next scheduler heartbeat.

## [2026-06-16] fix | Hermes reflection watchdog

- Found that `CronJob/hermes-weekly-reflection` and `CronJob/hermes-daily-reflection` were active and completing, but they only submitted asynchronous Hermes `/v1/runs` requests and did not verify that a reflection row was written.
- Confirmed the latest persisted Hermes reflection was still from 2026-05-23 before a manual weekly run on 2026-06-16.
- Triggered a manual weekly reflection after the OpenRouter/Hermes configuration fixes; Hermes wrote a current 2026-06-16 weekly reflection.
- Updated both reflection CronJobs to instruct Hermes to write a deterministic `source_session_id`, wait for that row, and write a watchdog reflection through the protected daytrader adapter if Hermes starts a run but does not persist a reflection inside the watchdog window.
