---
type: wiki-log
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-07-13
---

# Wiki Log

Append-only timeline for project wiki maintenance. Use headings with the format `## [YYYY-MM-DD] kind | summary` so agents and shell tools can parse the log.

## [2026-07-13] performance | Lightweight Decision Report dashboard reads

- Split Decision Report database projections into metadata summaries and full detail records.
- Normal dashboard renders no longer fetch heavyweight prompt, request, provider-response, or normalized-report JSON for historical rows; full payloads load only for the active report or prompt detail view.
- The recent-report table now labels unloaded trade counts rather than implying missing data, and regression coverage protects the compact SQL projection.

## [2026-07-13] safety | Git-verified deployment provenance

- Docker release builds now receive the full committed Git SHA and bake it into the Rust binary; `/api/health` returns the immutable build revision.
- `post-deploy-guard` records the expected SHA in non-secret deploy metadata and fails closed unless the running revision contains that requested commit. This catches stale images even when their mutable tag appears correct.
- Updated the build/deploy runbook with the provenance check and the requirement to prefer the guard target after a deploy.

## [2026-07-13] security | Dashboard database display redaction

- Replaced the Runtime panel's raw database URL with a structured display label shared by startup logging.
- PostgreSQL displays only host, port, and database name; SQLite uses a generic local label. URL user-info, connection query parameters, and filesystem paths are excluded.
- Added regression coverage using a secret-bearing PostgreSQL URL to ensure the display value contains neither credentials nor query parameters.

## [2026-07-13] improvement | Decision Report candidate scoring waterfall

- Added a read-only Decision Reports waterfall over the stored `trading_manager_runs.manager_json` preflight, advice delta, and final manager outcomes.
- It renders only compact deterministic manager context: market/risk eligibility, technical confluence, Markov freshness/signal, Hermes quantity effect, normalized gate code, and final outcome.
- New manager runs persist stable gate codes; historical rows use local fallback classification. Raw Hermes rationale, broker payloads, and raw execution errors remain excluded, and the view performs no provider or Saxo call.

## [2026-07-11] roadmap | Build, deploy, and repo hygiene review

- Reviewed `Dockerfile.api`, the deploy script, and the working tree; added a "Build, Deploy, And Repo Hygiene" roadmap subsection.
- Build: no dependency-layer caching (`COPY . .` recompiles ~500 crates every deploy) — proposed cargo-chef/dummy-main layering with BuildKit cache mounts; `screenshots/` (12 MB, new today) is in neither `.dockerignore` nor `.gitignore` and regresses the 4 MB build context.
- Deploy: proposed content-addressed image tags (git SHA + dirty marker, feeding the deploy-provenance P0 row), skipping the per-deploy CNPG helm upgrade when unchanged, parallel rollout waits, and digest-based restart short-circuiting.
- Repo: enumerated the removable legacy Python surface (main.py, web_main.py, src/saxo_daytrader_xai, .venv 425 MB) while noting the backup scripts stay load-bearing via the `daytrader-backup` CronJob image; flagged 12 GB of live RustFS object-store data living inside the repo tree as a data-safety hazard, plus root-level Positioner CSVs (17-maj file preserved as the cost-basis repair source), legacy ledger.db, and empty dirs.
- Verified `cargo build --release` currently emits zero warnings.

## [2026-07-11] roadmap | Saxo OpenAPI capability review

- Reviewed the Saxo OpenAPI reference docs and streaming architecture against the runtime's current usage (port snapshots, chart history, infoprice polling, trade v2 orders/precheck, ref lookups).
- Added a "Saxo OpenAPI Capabilities To Adopt" subsection to the roadmap: streaming price subscriptions to replace the 1-minute quote poller, ENS activities subscriptions for near-instant fill/order events instead of fast-poll broker sync, `/port/v1/closedpositions` as a broker-computed realized-P/L cross-check (would have caught the cost-basis corruption within a day), FX-spot infoprices as the concrete source for the live-FX roadmap row, `hist` performance timeseries for independent verification of the Performance tab, and later balances/positions streaming.
- Streaming verified reachable on SIM via the OpenAPI Explorer (plain WebSocket, ContextId + ReferenceId subscriptions, delta messages up to 3/s, `?messageid=` resume, `PUT /streaming/ws/authorize` re-auth) using the same OAuth session.
- Reviewed the learn-section pages (high-level overview, request/response conventions, batching, streaming): multipart batching is explicitly obsolete in favor of HTTP/2, which exposed that several runtime call sites build a fresh `reqwest::Client` per request; added a unified-Saxo-HTTP-client roadmap row (shared HTTP/2 client, gzip, uniform 429 handling, correlation ids) plus a row folding the documented order-placement return codes and pre-trade disclaimers into the error taxonomy.
- Reviewed the rate-limiting page and added a rate-limit-aware throttling row with the concrete numbers: 120 requests/minute per session per service group (the nightly Markov run paces exactly at this limit today), 1 order/second per session, 10M requests/day per application, `X-RateLimit-*` headers for adaptive pacing, unique `x-request-id` on POST/PATCH to avoid the 15-second duplicate 409 and to make order retries idempotent, and the rule that entry + related orders must be bundled in one request.
- Reviewed the planned-changes page: pre-trade disclaimer handling is mandatory for all OpenAPI apps and the runtime has none (SIM tolerates it today; flagged as a required implementation in the disclaimers roadmap row, and added to LIVE readiness); `root/v1/user` retires 2026-09-01 but the runtime does not call it (verified by source grep); the client-onboarding (2027) and proxy-voting (2026-05) changes do not apply to this app.
- Reviewed the environments page: SIM is a restricted LIVE copy (some market data/reporting unavailable, lower support priority, possibly newer API versions than LIVE), app key/secret are per-environment, and dev-portal one-day tokens are SIM-only. Added a SIM-limitations note plus a LIVE-readiness checklist row (separate app registration, redirect URIs, live auth/gateway/streaming hosts, secrets, entitlements, `require_approval_live` re-enabled, safeguard verification) so SIM quirks like reference-data lag stop being chased as bugs and a future LIVE switch is a checklist, not an improvisation.
- Reviewed the ENS, Trade, and order-placement learn pages: refined the ENS roadmap row with the concrete subscription/replay model (streaming-only, SequenceId/FromDateTime replay, 3-day streaming retention, 14-day GET retention at 50 msg/s) and added three rows — a scheduled 14-day ENS activities backfill as a broker-authored reconciliation source, unknown-state timeout handling (`TradeNotCompleted` means state-unknown, not failed; reconcile by ExternalReference before retrying), and tradable `Prices` (vs display `InfoPrices`) for limit-order anchoring plus routing `/trade/v1/messages` into ops alerts.

## [2026-07-11] roadmap | UI performance and live-system review additions

- Reviewed the running system, database, and the operator's dashboard screenshots after the 2026-07-09/10 roadmap implementation wave (Markov aliases cut daily resolution errors 38→27; breaker/quarantine/integrity alerts and overrides landed; Hermes duplicate rejection and stale-experiment alerts landed while the pending-review queue still grew to five).
- New P0 rows: redact the database connection string rendered with password on the Overview Runtime panel (`DashboardView.db_label` uses the raw URL); per-tab lazy read models for UI performance — measured every view at 0.9-2.1 s server time because `load_dashboard` fetches all decision reports (~1 MB/row, 19 MB table) and 5,000 portfolio-history rows for every tab, with `?view=prompts` shipping 1 MB of HTML; deploy provenance (git SHA in `/api/health` checked by smoke) after the 2026-07-09 stale-image window executed four BUYs that the then-missing breaker and commission floor would have blocked.
- New P1 rows: enforce `history_max_rows`/retention for `scheduler_cycle_history` (9,228 rows / 51 MB vs a 250-row cap) and vacuum `audit_log` (65 MB, zero live tuples); remove the dormant 2026-05-05 `runtime_settings` cash-buffer override that stores a zero buffer.
- UI section additions: paginated per-tab read models with a 300 ms/200 KB target, age-labels for stale per-position decision chips (screenshots show "HOLD 2026-05-08" rendered as current), market-aware ops-banner staleness (Quiver/Indicators warn "stale" on weekends despite being weekday-only by design), and collapsing the AI Prompts dump behind on-demand sections.

## [2026-07-10] improvement | Monthly-loss breaker operator override

- Added a month-scoped runtime override for the monthly-loss circuit breaker so an operator can deliberately resume BUYs before month end while preserving the threshold-breach evidence in Trading Manager run JSON.
- The Overview cash deployment panel now shows breached/active/overridden breaker state and posts either "Resume BUYs This Month" or "Clear Override" with operator notes.
- Updated the roadmap to mark the acknowledgment path as landed and leave only future override-history/audit UX as a possible follow-up.

## [2026-07-13] policy | Monthly-loss circuit-breaker threshold raised

- Updated `strategy.capital.monthly_loss_halt_dkk` from `-10,000 DKK` to `-50,000 DKK` in the local and Kubernetes runtime configuration at operator request.
- The guardrail continues to block only new BUYs once the batch-scoped month P/L breaches the configured floor; SELLs remain available for risk reduction. The change is deployed by applying the ConfigMap and restarting the Rust API and singleton scheduler, without deploying unrelated application code.

## [2026-07-10] improvement | Instrument quarantine operator override

- Added exact symbol/action/signature runtime overrides for active instrument quarantines; the Trading Manager continues to block by default and only bypasses the quarantine when the exact override is active.
- The Overview Instrument Quarantine panel now shows active, blocked, and overridden counts, and each active row can be overridden or cleared with notes.
- Updated the roadmap to mark the quarantine acknowledgment path as landed and leave only future override-history persistence as a possible follow-up.

## [2026-07-08] fix | Cost-basis repair, monthly-loss breaker, commission floor

- Repaired the May 18 import corruption: the old importer stripped dot-decimals, storing values inflated by 10^(decimal digits). Verified every stored `position_snapshots`/`position_lots` value against the exact old-parser corruption of the original `Positioner_17-maj-2026_13_39_46.csv` before updating (abort-on-mismatch guard), restored true cost bases, and recomputed all 22 post-reset SELL rows via FIFO replay against the corrected import lot plus subsequent ledger buys. Corrected realised P/L since the reset: +69,251 DKK (was showing millions of phantom losses). Repair script and audit trail in the session scratchpad; ledger rows carry a repair note.
- Landed the monthly-loss circuit breaker (`strategy.capital.monthly_loss_halt_dkk`, default -10000): the Trading Manager suspends new BUYs while month P/L is below the floor, SELLs are never blocked, breaker state is recorded in every manager run, and the decision prompt capital plan carries the same status. Verified active post-deploy with month P/L -28,277 DKK.
- Landed the commission-efficiency floor (`execution.max_commission_pct_per_side`, default 0.003): BUYs below `exchange minimum commission / pct` are rejected (XNAS/XNYS ≈ 7,021 DKK, XCSE ≈ 4,667 DKK, XLON ≈ 23,200 DKK) and the per-exchange floors are published in the decision prompt so the model sizes clips economically. Added to the Hermes experiment variable allowlist.
- All 138 tests pass; `make post-deploy-smoke` clean.

## [2026-07-08] roadmap | Live-system review additions

- Reviewed the running system end to end: live API overview, decision reports, Trading Manager runs, execution orders, trade ledger, Hermes reflections/experiments/advice, Quiver runs, Markov runs, and portfolio history.
- System health is good (24/26 reports completed in 14 days, Quiver 60/60, Hermes advising 25 reports, fills within a minute) but trading performance is negative: month P/L -23,070 DKK vs +20,000 target, weekly closes bleeding 288.8k → 274.6k, cash deployed down to ~6%.
- New P0 roadmap rows with live evidence: repair the still-corrupted realised-gain data (SELLs book -3.2M DKK "realised losses" from poisoned position_lots cost basis) and commission-aware minimum order size (0.67% one-way commission drag on ~3.5k DKK average clips).
- New P1 rows: monthly-loss circuit breaker tied to goal tracking (reinvestment pressure currently keeps buying through a losing month), fix for the 38 Nordic/EU assets failing Markov instrument resolution daily, and automatic instrument quarantine after repeated identical precheck failures (ARKK:xmil commissions, DEMANT tick size, flattened-position SELLs).
- Hermes section: added "unstick the experiment review queue" — four one-variable proposals pending since 2026-06-16 with no review flow, including two near-duplicate cash-buffer raises.
- Quiver section: added alt-data conflict surfacing (bearish NVDA/AMZN Congress signals while both were held).

## [2026-07-06] improvement | Scheduler cycle duration metrics

- Continued the roadmap by recording total scheduler-cycle runtime and per-step duration metrics in each persisted `cycle_json`.
- Added a compact Runtime column to the Scheduler Cycles table so slow recent cycles are visible without opening raw JSON.
- Left explicit per-step timeout budgets as the next scheduler-hardening item; this change only measures and displays where cycle time is spent.

## [2026-07-04] cleanup | Removed legacy Next.js frontend directory

- Removed the `frontend/` Next.js app: it was never built or deployed by any Makefile target, deploy script, or Kubernetes manifest, and AGENTS.md already documented it as old and inactive. The Dioxus SSR dashboard in `src/ui.rs` is the committed UI.
- Important distinction preserved: the `daytrader-frontend` Kubernetes Service is NOT related to that directory — it is a live alias Service selecting the `daytrader-api` pods, and the shared ngrok AgentEndpoint routes `http://daytrader-frontend.saxo:8000` through it. The Service, Makefile port-forward target, and gateway route are untouched.
- Cleaned stale `frontend/` entries from `.gitignore` and `.dockerignore`, removed the AGENTS.md legacy-surface entry, updated the README deployment note, and marked the roadmap architecture decision as resolved.

## [2026-07-04] roadmap | Project review additions

- Reviewed the runtime after the June/July feature wave and added verified gaps to the roadmap.
- New P0 stabilization rows: cross-pod Saxo token refresh lease (rollouts still burn the single-use refresh token; only an in-process mutex exists today) and a live FX rate service (fx_rate_to_dkk is a hardcoded constant table feeding ledger, order verification, price monitor, and commissions).
- New P1 rows: real accounting invariants behind the currently hardcoded overview `integrity` field with Slack alerting, and market-hours-aware price-monitor polling.
- Added a gate replay harness idea (recalibrate Trading Manager thresholds offline against stored reports/contexts), real Danish share-income tax estimation (config brackets are unused; after-tax P/L currently equals pre-tax), a decision item on `frontend/` Next.js vs the Dioxus SSR dashboard, scheduler per-step timeout budgets and duration metrics, and watch-symbol lifecycle alerts for `extra_symbols` activations such as the pending SPCX listing.

## [2026-07-04] improvement | Operational scheduler alerts

- Continued the operations roadmap by adding scheduler-driven Slack alerts for repeated decision-report failures, execution-failure bursts, stale scheduler completion, and missed Hermes EOD reflection.
- Reused the existing immutable notification delivery/state tables and added Rust runtime schema creation so fresh Rust deployments do not depend on legacy Python initialization.
- Documented the new notification alert thresholds and route kind in the README and marked the roadmap item as recently landed.
- Followed up by exposing execution-notification and operational-notification status in the Scheduler Cycles table, with a UI regression test for nested cycle JSON status extraction.
- Continued with backend-backed decision pulse health rows for Nordic/EU, US, and manual reports so the Decision Reports tab shows latest report, last success, last failure, and 7-day attempt count even when recent report history is noisy.

## [2026-07-04] fix | Docker build context hygiene

- Aligned `.dockerignore` with local-only repository ignores, including `rustfs/`, qmd/Obsidian state, Python caches, generated spreadsheet exports, Rust backup files, and mutation-test output.
- Verified Docker now transfers a 4.11 MB build context instead of including local RustFS object-store data.
- Confirmed a production-style `Dockerfile.api` image build completes after the context change; only the pre-existing `xai_decision.rs` dead-code warnings remain.
- Added `make post-deploy-smoke` for read-only rollout, internal endpoint, health, overview, scheduler, Saxo-session, MCP tool-discovery, and Hermes gateway health checks after deployment.

## [2026-07-04] improvement | Diagnostics artifact capture

- Continued the operations roadmap by adding an opt-in diagnostics artifact mode.
- Added `make diagnostics-artifact`, which runs the existing read-only diagnostics bundle and saves the output to `.diagnostics/daytrader-diagnostics-<utc timestamp>.log`.
- Ignored `.diagnostics/` in git and Docker build context so captured incident bundles remain local by default.

## [2026-07-04] improvement | Post-deploy smoke schema and image checks

- Added a read-only `/api/decision/schema` endpoint that reports strict OpenRouter decision-report schema health from the active Rust schema registry.
- Expanded `make post-deploy-smoke` to fail when decision-report schema health is not ok.
- Added optional image drift checks for API, scheduler, MCP, and Hermes deployments through `EXPECTED_DAYTRADER_IMAGE` or per-deployment `EXPECTED_*_IMAGE` environment variables.

## [2026-07-04] runbook | CNPG and RustFS backup restore rehearsal

- Added `wiki/runbooks/backup-restore.md` for CloudNativePG and RustFS backup verification, manual backup rehearsal, object inspection, and safe restore rehearsal into a throwaway namespace.
- Linked the new runbook from the runbook index and main wiki index.
- Kept restore instructions non-destructive by default and explicitly warned against restoring over the live `saxo/daytrader-postgres` cluster.

## [2026-07-04] improvement | Post-deploy image guard

- Added `scripts/post_deploy_guard.sh` and `make post-deploy-guard`.
- Updated the deploy script to write non-secret image/context metadata to `.run/last_deploy.env` after successful rollouts.
- The guard reuses the post-deploy smoke checks and verifies API, scheduler, MCP, and Hermes deployment images against the last deploy metadata unless overridden by `EXPECTED_*_IMAGE` environment variables.

## [2026-07-04] verification | QuiverQuant live subscription

- Verified the QuiverQuant subscription is active in the deployed `saxo` Kubernetes runtime.
- Triggered manual Quiver signal runs through `POST /api/actions/quiver-signals`; the latest verified run completed with 60 assets, 60 successes, and 0 errors.
- Updated `docs/quiver-signals.md` to record live status and clarified that manual refresh responses are compact summaries while full event details remain available through `GET /api/quiver/signals`.

## [2026-07-03] implementation | QuiverQuant advisory signals

- Added a Rust QuiverQuant advisory signal path for US portfolio/watchlist assets using Congress trading data.
- Wired Quiver into scheduler runs, API/dashboard surfaces, decision-report context, Hermes context, and MCP tool discovery.
- Documented the integration in `docs/quiver-signals.md`; signals are advisory only and cannot place or approve Saxo orders.

## [2026-07-02] implementation | Diagnostics bundle

- Continued the roadmap by adding `scripts/diagnostics_bundle.sh` and `make diagnostics`.
- The bundle collects read-only Kubernetes status, rollouts, resource usage, recent events, scheduler/API/Hermes logs, RustFS backup state, shared ngrok status, and a sanitized app API summary.
- Kept the bundle non-mutating: it does not trigger reports, process execution queues, place orders, or expose raw Saxo broker payloads.

## [2026-07-01] implementation | Execution-order attribution

- Continued the roadmap by adding per-order attribution for recent execution orders.
- The attribution connects each order to its source decision report, latest Trading Manager run, matching Hermes decision advice, latest daily indicator summary, and latest Markov signal summary.
- Added an Execution table disclosure so operators can inspect whether an order was Hermes-allowed, manager-only, reduced, skipped, or review-overridden without opening raw JSON.

## [2026-06-29] fix | Broker-authoritative Trading Manager sell caps

- Investigated `ORSTED:xcse` and `NNIT:xcse` SELL failures from decision report `116`.
- Found that the imported May 18 `position_snapshots` batch still showed ORSTED 108 and NNIT 100, while later executed broker orders had already sold those quantities down to zero.
- Changed Trading Manager SELL sizing to prefer current `broker_position_snapshots` when available, using broker-authoritative sellable quantity before creating execution queue rows; imported snapshots remain only a fallback when no broker read model exists.
- Kept the Saxo execution guard as a second safety net before precheck/place.

## [2026-06-27] implementation | OpenRouter schema validation registry

- Continued the roadmap by adding a reusable Rust validator for OpenRouter strict structured-output schemas.
- Added a current-schema registry test for the active daytrader decision-report response schema.
- The validator reports actionable paths for missing `additionalProperties: false`, incomplete `required` arrays, stale required entries, and nested object issues across properties, arrays, unions, and definitions.

## [2026-06-26] improvement | Saxo tick-size and expired-order diagnostics

- Continued the roadmap by porting broker-aware Saxo limit-price normalization into the Rust order payload path.
- The Rust Saxo order path now prefers configured tick overrides, then Saxo instrument details and tick-size schemes, before falling back to exchange defaults.
- Changed Saxo `Expired` and `DoneForDay` broker sync states into explicit local terminal statuses instead of generic `execution_failed`, so unfilled DayOrders are visible as broker expiry cases.
- Added Rust and UI regression tests for DEMANT-like tick-size normalization and broker-expired execution classification.

## [2026-06-25] implementation | Sanitized decision-report debug payloads

- Continued the roadmap by adding expandable sanitized prompt, request, provider-response, and normalized-report payloads to the Decisions view.
- Added recursive redaction for token-like fields and common secret/account/session keys before debug payloads are rendered.
- Added UI unit tests that verify OpenRouter/Saxo-style sensitive fields are redacted while non-sensitive model/report context remains visible.

## [2026-06-25] implementation | Cash deployment diagnostics

- Continued the roadmap by exposing the latest Trading Manager `reinvestment_diagnostics` in a read-only Cash Deployment panel on the Overview tab.
- The panel explains whether cash is being held by policy, blocked BUY candidates, missing BUY candidates, or approved reinvestment candidates.
- Added UI unit tests for cash deployment status/tone classification and summary extraction from the latest manager run.

## [2026-06-25] implementation | Decision report quality panel

- Continued the roadmap by adding a read-only Decision Report Quality panel to the Decisions tab.
- The quality score checks report completion, strict provider schema, normalized section presence, suggested-trade order shape, and market-scope enforcement metadata.
- Added UI unit tests for a clean report and a schema-valid report that still needs review because of bad trade shape and filtered market-scope symbols.

## [2026-06-24] implementation | Hermes decision advice audit

- Continued the roadmap by adding a read-only Hermes Decision Advice Audit table to the Hermes dashboard tab.
- Added a dashboard read model that joins recent decision reports with persisted Hermes advice, latest Trading Manager run status, and queued/executed/failed order counts.
- Added UI classification helpers and tests for received advice, order-specific conservative restrictions, and conservative timeout review fallback.

## [2026-06-24] implementation | Decision report dry-run action

- Started implementing the roadmap by adding a non-mutating decision report dry-run action.
- The dry-run path submits/parses/persists a manual decision report without running the Trading Manager or Saxo execution queue.

## [2026-06-24] implementation | Decision pulse health cards

- Added Decisions view pulse-health cards for Nordic/EU, US, and Manual/Dry Run reports.
- Cards show the latest report status and latest successful report per pulse from recent decision report history.

## [2026-06-24] fix | OpenRouter schema self-hardening

- Added a defensive OpenRouter schema sanitizer before request submission so every object schema is strict even if a nested helper omits strict fields.
- Extended decision-report schema tests to cover the `capital_plan` failure path and union branches.

## [2026-06-24] implementation | Decision report diagnostics panel

- Replaced the raw decision prompt/request preview in the Decisions view with compact provider diagnostics.
- The panel shows model, response format, schema strictness, payload size, response id/presence, and categorized error details without rendering the full prompt context.

## [2026-06-23] planning | Project roadmap

- Added [wiki/roadmap.md](/Users/lindau/codex/rust_daytrader/wiki/roadmap.md) as a forward-looking improvement map for reliability, decision reports, Hermes, Trading Manager, execution, strategy, UX, architecture, operations, security, and documentation.
- Linked the roadmap from [wiki/index.md](/Users/lindau/codex/rust_daytrader/wiki/index.md).
- Expanded the roadmap with high-leverage trading, Hermes, decision-report, and Saxo API workflow improvements.

## [2026-06-23] fix | OpenRouter decision schema strictness

- Fixed the Rust OpenRouter decision-report JSON schema so every object uses `additionalProperties: false`.
- Made nullable optional-looking fields required where strict structured outputs need all declared properties listed in `required`.
- Added a recursive schema regression test so future nested object additions cannot reintroduce provider-side `invalid_json_schema` failures.

## [2026-06-22] improvement | Hermes conservative advice enforcement

- Hardened Hermes decision-advice attachment so Trading Manager looks up advice by both `source_session_id` and `decision_report_id`.
- Switched Kubernetes Trading Manager advisory mode to `conservative` with a longer wait window.
- Documented that conservative advice may only block, reduce, or require review, and missing/timed-out advice fails closed to review.

## [2026-06-18] improvement | Hermes Trading Manager advice

- Added an audited `hermes_decision_advice` store for per-decision-report Hermes advisory records.
- Added the Hermes-safe MCP write tool `create_decision_advice`.
- Wired the Rust Trading Manager to submit a bounded Hermes advisory run before queueing orders from a fresh decision report.
- Default mode is `record_only`; optional `conservative` mode can only block, reduce, or require review and cannot add trades, increase size, approve live orders, or call Saxo mutation endpoints.
- Updated Hermes docs, README env examples, and the build/test/deploy runbook.

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

## [2026-06-18] improvement | Hermes proposal loop

- Changed the Hermes goal contract from disabled reflection-only posture to enabled `recommend_only` learning mode.
- Updated daily and weekly Hermes CronJob prompts so Hermes may create pending-review one-variable experiment proposals from concrete learnings, while still writing exactly one reflection.
- Kept the safety boundary: proposals must use the audited experiment table, avoid duplicate active/pending variables, prefer the supported overlay variable allowlist, and never place or approve Saxo orders.
- Updated Hermes documentation, wiki concept notes, and build/test/deploy runbooks to describe daily and weekly learning/proposal behavior.

## [2026-06-24] improvement | Execution diagnostics visibility

- Continued the roadmap by improving Execution page diagnostics for broker order failures and pending Saxo states.
- Added UI classification for precheck rejection, market closed, Saxo auth, rate limits, instrument resolution, insufficient cash, tick-size/price-shape issues, invalid quantity, broker rejection, and broker-working waits.
- Changed recent execution events to use the same diagnostic formatter instead of concatenating message and error text.
- Kept sanitized raw execution payloads available in collapsible order diagnostics without exposing token-like keys or broker account/client/user identifiers.

## [2026-06-24] improvement | Operations health banner

- Continued the roadmap by adding a compact dashboard operations banner for Saxo session, scheduler heartbeat, decision-report, Markov, daily-indicator, and quote freshness health.
- Added a latest daily-indicator run read model so the UI can flag missing, stale, failed, or partial technical-indicator runs beside Markov freshness.
- Added UI tests for Saxo reauth status, stale scheduler heartbeats, partial/stale runtime runs, and quote freshness thresholds.

## [2026-07-04] improvement | Dependency and CVE hygiene

- Refreshed `Cargo.lock` within existing semver constraints after `cargo update --dry-run` showed safe transitive dependency updates were available.
- Added `make deps-dry-run` so dependency drift can be reviewed without mutating the lockfile.
- Added `make security-scan`, backed by `scripts/security_scan.sh`, to run RustSec advisory checks, Trivy filesystem/image CVE scans, and Trivy secret scans.
- Documented the dependency/CVE operating cadence and remediation policy in the build/test/deploy runbook and linked the workflow from the README.

## [2026-07-07] improvement | FX rate cache for DKK valuation

- Added a Rust `currency_fx_rates` runtime table and `src/fx.rs` cache helper for DKK conversion rates.
- The cache refreshes from ECB daily reference rates, expires rows after 30 hours, and short-circuits external fetches while the cached ECB row is still fresh.
- Price-monitor portfolio snapshots and broker-fill ledger rows now use cached FX rates with a static fallback instead of hardcoded active valuation constants.
- Kept a roadmap follow-up for switching the primary source to Saxo FX spot infoprices while retaining the ECB/static fallback chain.

## [2026-07-08] improvement | Saxo FX spot source parity

- Upgraded the FX refresh path to prefer read-only Saxo `FxSpot` instruments and `/trade/v1/infoprices/list` quotes for common DKK conversion pairs.
- Kept the fallback chain explicit: fresh Saxo cache, Saxo spot refresh, ECB daily reference refresh, then static constants at individual use sites if all cache reads fail.
- Converted async DKK conversion paths to the cache: daily-indicator prompt context, Markov context, Trading Manager BUY value verification, overview read models, price snapshots, and broker-fill ledger entries.
- Left synchronous commission-minimum fallback values static because that path has no async database access and is only a conservative local estimate.

## [2026-07-08] fix | Saxo session refresh lease

- Added nullable lease metadata to the `saxo_sessions` singleton row so token refresh is single-owner across API, scheduler, and MCP pods.
- Wrapped auth status auto-refresh, explicit refresh, broker session ensure, and user-logout keepalive paths in the lease before they call the Saxo token refresh helper.
- Waiters now restore the durable DB session and retry until the owner publishes a refreshed token or the lease expires, avoiding concurrent use of Saxo's single-use refresh token during rollouts.
- Kept `auth.rs` as the token-mechanics owner; the new coordination layer lives in `AppState` and still falls back to reauth when the refresh token is missing, expired, or marked invalid.

## [2026-07-08] improvement | Overview accounting integrity

- Continued the roadmap by replacing the hardcoded overview integrity stub with real read-model invariant checks.
- The overview payload now reports portfolio identity mismatch, ledger-vs-history cash drift, broker cash drift, implausible position-lot unit costs, and stale or unreconciled execution orders.
- Added tolerance coverage so small DKK/FX/settlement noise does not mark the dashboard unhealthy.
- Left follow-up roadmap work for UI surfacing, Slack alert routing, and deeper broker exposure aggregate reconciliation.

## [2026-07-09] improvement | Derived instrument quarantine

- Continued the roadmap by adding a Trading Manager quarantine gate for instruments with repeated identical hard execution failures.
- Active quarantines are derived from recent `execution_orders` evidence, grouped by symbol, action, and normalized failure signature.
- The first signatures cover commission setup failures, tick-size/price increment failures, already-flat SELL attempts, instrument resolution failures, and not-tradable/unsupported instruments.
- Configured defaults under `risk.instrument_quarantine`: enabled, 14-day lookback, 3 matching failures, and 14 active quarantine days.
- The manager records active quarantine config and rows in `manager_json`, and skips matching candidates before queue insertion.

## [2026-07-09] improvement | Instrument quarantine overview panel

- Surfaced the derived instrument quarantine in the Overview sidebar beside Cash Deployment.
- The panel reports whether the gate is disabled, clear, or active, plus lookback days, minimum failures, active window, and active quarantine count.
- Active rows show symbol, action, normalized failure signature, repeated-failure count, expiry time, and the sample error as a row tooltip.
- Left follow-up roadmap work for Slack activation alerts and operator acknowledgment/override flow.

## [2026-07-09] improvement | Execution DayOrder lifecycle visibility

- Continued the roadmap after investigating BAC:xnys order 204 by adding DayOrder lifecycle metadata to execution-order read models.
- Active Saxo broker orders now expose duration type, expected exchange-calendar expiry, market, timezone, and a lifecycle note when the order is a broker DayOrder.
- The Overview execution queue and full Execution table now include an Expiry column, and broker status tooltips include duration/expiry context.
- Left follow-up roadmap work for stronger broker reconciliation when Saxo open-order lookup and order-activity lookup disagree.

## [2026-07-09] improvement | Saxo broker-sync provenance

- Continued the order lifecycle reconciler by persisting broker-sync provenance for Saxo orders.
- Broker sync now records whether the current broker state came from `/port/v1/orders`, the `/cs/v1/audit/orderactivities` fallback, or a probe where both lookups returned no current state.
- Missing lookup probes create an auditable `broker_sync_not_found` execution event and leave the local order status unchanged pending later reconciliation.
- Execution status and lifecycle tooltips now show the broker visibility state and fallback note so activity-only `broker_working` rows are not confused with directly visible open orders.

## [2026-07-09] improvement | DayOrder expiry sync pending marker

- Added a read-model lifecycle marker for active Saxo DayOrders whose expected exchange-calendar expiry has passed while local status is still an active broker state.
- The marker is intentionally non-mutating: it labels the order `expiry_pending_broker_sync` for operator visibility, but does not mark it expired unless Saxo confirms a terminal broker status.
- Execution status and lifecycle tooltips now call out the pending expiry sync state so overdue DayOrders do not look like ordinary in-session `broker_working` orders.
- Added a 10-minute grace window, an overview integrity warning payload, and an Operations banner `Execution` warning when any active DayOrder remains overdue after the grace window.
- Surfaced the overview integrity payload in the dashboard model, added an Operations banner `Integrity` chip, and added an Overview Integrity panel listing warnings, mismatches, and expiry-pending orders.

## [2026-07-09] improvement | Integrity operational Slack alerts

- Continued the accounting-integrity roadmap by routing overview integrity issues into the existing scheduler-driven operational Slack alert path.
- Integrity alerts now cover high-severity overview mismatches and medium-severity warnings, including overdue DayOrders that need broker-sync confirmation.
- Alert scope keys are stable across scheduler cycles for the same issue set and expiry-pending order ids, so persistent conditions do not spam Slack every heartbeat.
- Added `notifications.alerts.integrity_alert_enabled` to the Kubernetes config and unit coverage for clear, warning, and mismatch integrity payloads.

## [2026-07-09] improvement | Instrument quarantine operational Slack alerts

- Continued the instrument-quarantine roadmap by routing active derived quarantines into the existing scheduler-driven operational Slack alert path.
- Alerts summarize blocked symbol/action/failure-signature rows, failure counts, latest failure time, and quarantine expiry without including raw broker error payloads.
- Alert scope keys are based on the active quarantine set, so the same active set is sent once while newly activated signatures or count changes can page the operator.
- Added `notifications.alerts.instrument_quarantine_alert_enabled` to the Kubernetes config and unit coverage for disabled, clear, and active quarantine payloads.

## [2026-07-09] improvement | Monthly-loss circuit breaker operational alerts

- Continued the risk-guardrail roadmap by routing monthly-loss circuit-breaker activation and clearing into the scheduler-driven operational Slack alert path.
- Alerts compare the latest two Trading Manager runs and fire only on state transitions, avoiding repeated pages while the breaker remains active.
- Alert messages summarize month P/L, halt threshold, latest manager run, and whether BUY suspension is active; SELLs remain explicitly unaffected.
- Added `notifications.alerts.monthly_loss_circuit_breaker_alert_enabled` to the Kubernetes config and unit coverage for activation, repeated-active suppression, and clearing.

## [2026-07-09] improvement | Price monitor market-hours polling

- Started the price-monitor market-hours roadmap item by validating the Saxo service session before loading positions or resolving extra watch symbols.
- The price monitor now refreshes/reads the exchange-calendar cache, skips known closed exchanges before Saxo infoprice batching, and returns a `market_closed` heartbeat summary when every known exchange is closed.
- Extra watch symbols are no longer resolved through Saxo while their configured exchange is closed; unknown exchanges still poll so unsupported suffixes do not silently drop data.
- Added unit coverage for Saxo symbol exchange parsing and closed-market filtering.

## [2026-07-09] improvement | Price monitor closed-market visibility

- Added a persisted `price_monitor_status` singleton row so the latest sanitized quote-monitor outcome survives pod boundaries and page refreshes.
- The Market tab now shows Quote Monitor status, last update time, and skipped known-closed symbols from the latest monitor refresh.
- The Operations banner Quotes chip now treats `market_closed` monitor summaries as intentional closed-market pauses instead of stale or unknown quote data.
- Added UI unit coverage for closed-market quote status and skipped-symbol label formatting.

## [2026-07-09] improvement | Price monitor slow off-hours heartbeat

- Added `price_monitor.off_hours_poll_interval_minutes` to local and Kubernetes config, defaulting to 15 minutes while the regular in-hours quote heartbeat remains 1 minute.
- The Rust price-monitor loop now sleeps on the slower interval only when the latest refresh summary is `market_closed`; normal, partial, and no-session cycles keep the regular interval.
- Added unit coverage for the closed-market sleep-interval selector.

## [2026-07-09] improvement | Markov instrument negative cache

- Continued the Markov coverage roadmap by adding a persistent `saxo_instrument_negative_cache` table for definitive Saxo instrument lookup misses.
- Markov and daily-indicator instrument resolution now skip symbols with a fresh cached no-tradable-match result until the configured retry window expires.
- The cache defaults to a 7-day retry interval via `strategy.markov.instrument_negative_cache_retry_days`; stored broker/position instruments still bypass and clear cached negative rows.
- This reduces repeated daily dead-end Saxo reference lookups while leaving a slow retry path for symbols that later become available in SIM.

## [2026-07-10] improvement | Decision-report dry-run regression guard

- Continued the roadmap testing work by centralizing manual decision-report action behavior behind an explicit live vs dry-run mode.
- Added Rust unit coverage proving a completed dry-run report does not run the Trading Manager or Saxo execution queue, while a completed live report still can.
- This is the first slice of the broader workflow-test roadmap; scheduled reports, Hermes advice, Trading Manager queueing, and execution dry-run paths remain future slices.

## [2026-07-10] improvement | Hermes advisory context self-check

- Continued the Hermes advisory-loop roadmap by adding a structured context self-check to per-report Hermes advice.
- The Trading Manager now instructs Hermes to report whether it reviewed the latest decision report, Markov signals, EOD report, current positions, and active experiments before recording advice.
- The `create_decision_advice` MCP schema accepts `context_self_check`; the recorder normalizes `complete`, `missing`, and `required` fields into the advice raw payload.
- The Hermes Decision Advice Audit table now shows self-check status with a tooltip for missing sources.
- Conservative mode now blocks automatic queueing whenever the self-check is incomplete, even if Hermes supplies an `allow` or `reduce` order action; the Trading Manager records the gate reason and self-check in its run JSON.
- The Hermes audit impact label now identifies this outcome as a context review gate rather than a normal restriction or no-op.

## [2026-07-10] improvement | Hermes normalized decision preflight

- Added an exact per-manager-run preflight snapshot before Hermes advice is requested, covering report/candidate waterfall, capital and circuit-breaker state, candidate-relevant position exposure, compact daily technical/Markov freshness, active experiment metadata, and classified recent execution failures.
- The snapshot is both sent to Hermes and persisted in `trading_manager_runs.manager_json.hermes_preflight`, enabling later audit and offline replay without another changing-state lookup.
- The bundle intentionally excludes Saxo sessions, account identifiers, raw broker payloads, and raw execution-error text; tests verify failure summaries do not leak raw error content.

## [2026-07-09] improvement | Saxo share-class symbol variants

- Continued the Markov coverage roadmap by adding deterministic share-class symbol variants to the shared Markov/daily-indicator Saxo resolver and the Saxo execution resolver.
- Symbols with a single-letter share class, such as `CARL-B:xcse`, `VOLV-B:xsto`, and `BRK-B:xnys`, now also try and accept Saxo's compact `CARLb`, `VOLVb`, and `BRKb` symbol shape.
- The matcher still requires the requested exchange alias, so the variant does not silently resolve a share class on the wrong venue.
- Added Rust regression coverage for Markov and execution resolver candidate matching.

## [2026-07-09] improvement | Markov analysis symbol aliases

- Continued the Markov coverage roadmap by adding `strategy.markov.symbol_aliases`, an explicit read-only alias map for stale portfolio/watchlist symbols.
- Markov and daily indicators now keep persisted rows keyed by the original symbol while using the configured alias only for Saxo instrument/chart lookup.
- Seeded known stale mappings for `COST:xnys`, `HON:xnys`, `LIN:xnys`, and `SHELL:xlon`; execution order resolution is intentionally unaffected.
- Markov raw payloads record `analysis_symbol` and whether an alias was applied, preserving auditability for decision prompts and operator review.

## [2026-07-09] improvement | Hermes stale experiment review visibility

- Continued the Hermes advisory-loop roadmap by routing stale `pending_review` strategy experiments into scheduler-driven operational Slack alerts.
- Added `notifications.alerts.hermes_pending_experiment_review_enabled`, `hermes_pending_experiment_review_stale_days`, and `hermes_pending_experiment_review_limit` to local and Kubernetes config.
- Alerts summarize experiment ids, changed variable paths, created timestamps, ages, and source session ids while omitting raw Hermes payloads and evidence blobs.
- Added a Hermes dashboard Age column that highlights stale `pending_review` experiment proposals after the same 14-day threshold.
- This addresses the first slice of unblocking the experiment review queue; weekly digest, auto-expiry, and duplicate merging remain future roadmap items.

## [2026-07-10] improvement | Hermes duplicate proposal guard

- Continued the Hermes experiment review queue roadmap by adding backend duplicate detection before inserting a new `strategy_experiments` proposal.
- The protected Hermes create-proposal endpoint now returns `409 Conflict` when an active or pending experiment already covers the same trimmed, case-insensitive `changed_variable_path`.
- Terminal statuses (`rejected`, `paper_failed`, `sim_failed`, `failed`) and `promoted` do not block future proposals for the same variable, preserving the ability to run later evidence-backed experiments.
- Near-duplicate semantic merging, weekly digest, and auto-expiry remain future work.

## [2026-07-10] improvement | Overview integrity acknowledgments

- Continued the accounting-integrity roadmap by adding stable issue keys to current Overview integrity mismatches and warnings.
- Added a runtime-settings backed acknowledgement lifecycle with operator notes, plus Overview controls to acknowledge or clear current issue acknowledgments.
- Acknowledged issues remain visible and still count as mismatches/warnings; the acknowledgement is only audit context, not a health override.

## [2026-07-13] improvement | Hermes decision advice delta audit

- Added a normalized `hermes_advice_delta` to `trading_manager_runs.manager_json` after each report-time Hermes advisory request.
- Each candidate keeps only matching precedence, advisory action, requested/resulting quantities, applied effect, and final local manager outcome; Hermes rationale, raw broker payloads, and raw execution errors are excluded.
- The Hermes Decision Advice Audit UI now prefers the stored delta, making conservative blocks, review gates, reductions, and record-only no-ops visible without parsing free-form skip messages.

## [2026-07-13] improvement | Hermes counterfactual tracking

- Added a durable, non-mutating `hermes_counterfactuals` ledger for only the quantity a conservative Hermes advisory blocked or reduced. It is created from the normalized manager delta and stores no Hermes rationale, Saxo session data, broker payload, or raw execution error.
- Active rows join the read-only Saxo quote monitor and calculate a directional quote-to-quote shadow return: prevented BUYs benefit from later price increases, prevented SELLs benefit from later price decreases.
- The Hermes dashboard now presents reference and latest quotes, directional shadow return/P&L, source effect, and tracking status. The values deliberately exclude broker execution, fees, FX, slippage, taxes, and realised P/L.

## [2026-07-10] improvement | Broker exposure integrity reconciliation

- Continued the accounting-integrity roadmap by comparing dashboard unrealised P/L against the latest Saxo instrument exposure aggregate.
- Added a warning-level quantity drift check between `broker_instrument_exposures` and `broker_position_snapshots`.
- New broker exposure integrity warnings receive stable issue keys, so the acknowledgement lifecycle can track them without hiding the underlying drift.
