---
type: capability
tags:
  - daytrader/wiki
  - roadmap
  - maintained-by-llm
updated: 2026-07-14
---

# Daytrader Roadmap

This roadmap collects potential improvements for the Rust daytrader runtime, Hermes self-improvement loop, operations, UX, and long-term maintainability. It is intentionally a planning map, not a commitment list. Prioritize work that improves safety, observability, and reproducibility before increasing automation.

## Guiding Principles

- Preserve SIM/live separation and keep broker mutations auditable.
- Fail closed on AI, schema, session, market-scope, and broker uncertainty.
- Prefer small, testable changes with clear before/after evidence.
- Let Hermes propose, but keep activation gated through explicit code/config/database controls.
- Keep the wiki as the durable memory for operational lessons, experiments, and strategy behavior.

## Recently Landed

- 2026-07-15: Added a database-backed broker-sync not-found fixture. When both open-order and audit-activity lookup are absent, local `broker_working` state remains unchanged while the missing visibility and broker order id are persisted as an auditable event.
- 2026-07-14: Added a database-backed broker terminal-state fixture. A synthetic Saxo `Expired` response records a normalized broker event, persists `broker_expired`, preserves the prior local result, and stores broker-sync context without any broker HTTP.
- 2026-07-14: Added a database-backed execution-order claim race fixture using isolated in-memory SQLite. Two concurrent local claim attempts yield exactly one `submitting_to_broker` transition while clearing a stale local error; it never loads a Saxo session or calls broker HTTP.
- 2026-07-14: Added a database-backed Trading Manager queue fixture using isolated in-memory SQLite tables. It verifies idempotent local order/event persistence and manager-run linkage for a scheduled report without a Saxo session, broker HTTP, or a live database.
- 2026-07-14: Added an execution-queue admission regression guard. The pure gate preserves the existing precedence and response shape: only explicit live Saxo execution without dry-run or required approval can reach session loading and broker mutation; all other configurations fail closed before that boundary.
- 2026-07-14: Added always-visible EU and US decision-pulse health to the Operations banner. Each chip reports the latest normalized status and last successful run timestamp; accepted deterministic xAI fallback reports count as successful, matching the Trading Manager's scheduled-report eligibility.
- 2026-07-14: Added the first scheduled-report hand-off regression guard. Trading Manager now accepts only a positive-id completed/fallback report with a scheduled pulse and a parseable timestamp inside its configured freshness window; malformed or undated reports fail closed before they can queue broker work.
- 2026-07-14: Made Operations-banner Markov, Quiver, and Indicators freshness schedule-aware. Weekday-only jobs now render neutral `idle (weekend)` off-hours and `waiting` before their local due time; they warn only when the last successful run precedes the latest configured due date. Current schedule metadata is read from active config, not an old run record.
- 2026-07-14: Made dashboard decision chips age-aware. Portfolio and watchlist labels now use a relative age, explicitly say `Stale` after the configured `strategy.swing.position_decision_stale_after_days` horizon (default seven days), and fail closed to an `Undated` stale state when a source timestamp is absent. This is display-only context; it cannot change a recommendation, queue an order, or affect broker behavior.
- 2026-07-14: Restored scheduler-history retention in the Rust scheduler. Each completed cycle now prunes rows older than the configured age and then applies the configured row cap, matching the legacy policy without blocking the cycle if pruning fails. Physical database space reclamation remains an operator maintenance task.
- 2026-07-14: Added server-side pagination for the Scheduler Cycles table. The Execution tab reads a 12-row bounded projection with a total and Previous/Next navigation instead of loading unbounded `SELECT *` cycle history.
- 2026-07-13: Added server-side pagination for the latest Quiver run's signal table. The tab loads 40 rows per page with a bounded offset and run-scoped total; its success/error pills now use the run’s aggregate metrics rather than the current page.
- 2026-07-13: Added server-side pagination for the latest Markov run's signal table. The tab loads 40 rows per page with a bounded offset and run-scoped total; its success/error pills now use the run’s aggregate metrics rather than the current page.
- 2026-07-13: Added the first paginated dashboard table. Execution now reads 25 orders per selected server-side page, bounds Overview to its 12 most recent queue entries, and bounds other health-strip views to 20 recent orders. Attribution enrichment therefore scales with the displayed page rather than a fixed 80 rows.
- 2026-07-13: Continued the P0 dashboard read-model split. Tab-exclusive collections for Hermes, Execution detail, Markov, Quiver, Watchlists, and End-of-Day now load only when their tab is selected; compact EU/US decision-pulse status remains shared because the Operations banner is persistent across views.
- 2026-07-13: Continued the P0 dashboard payload split. Full portfolio performance history now loads only for the Performance tab and respects its selected range rather than fetching 5,000 rows for every SSR view. The standalone performance API remains unchanged.
- 2026-07-13: Started the P0 dashboard payload split. Routine SSR now loads only compact Decision Report metadata (five rows on Overview, one on other tabs, twenty on the report view); heavyweight prompt/request/response/report JSON is fetched only for the active Decision Reports or AI Prompts detail. The report-history table explicitly marks unloaded counts until a row is selected.
- 2026-07-13: Added deploy provenance verification. Docker now embeds the full committed Git SHA in the Rust binary, `/api/health` exposes it, and `post-deploy-guard` fails unless the running revision contains the requested commit. Image tags alone can no longer validate a safeguard rollout.
- 2026-07-13: Removed database credentials from dashboard-visible runtime metadata. PostgreSQL now displays only host, port, and database name; SQLite displays a generic local label. User-info, connection query parameters, and filesystem paths are excluded, with regression coverage for secret-bearing PostgreSQL URLs.
- 2026-07-13: Added a read-only Decision Report candidate scoring waterfall. It reconstructs the stored Trading Manager preflight and final outcome per candidate, showing compact market/risk, technical, Markov, Hermes quantity effect, normalized gate code, and result. New manager runs persist stable gate codes; historical runs are locally classified but raw Hermes rationale, broker payloads, and raw errors are not rendered.
- 2026-07-13: Added non-mutating Hermes counterfactual tracking. When conservative advice blocks or reduces a candidate, the prevented quantity enters a quote-to-quote shadow ledger, active shadow symbols join the read-only price monitor, and the Hermes tab shows reference/latest price plus estimated directional return. The ledger explicitly excludes broker execution, fees, FX, slippage, taxes, and realised P/L.
- 2026-07-13: Added a normalized Hermes advice delta to every Trading Manager run. Each candidate records matching precedence, requested/resulting quantity, whether advice allowed/reduced/blocked/reviewed/no-oped it, and its final local manager outcome; the delta excludes Hermes rationale, raw broker payloads, and raw execution errors so advisory impact can be measured safely.
- 2026-07-10: Completed the Hermes advice self-check safety gate: Trading Manager advisory prompts require Hermes to state whether it reviewed the latest report, Markov signals, EOD report, positions, and active experiments; stored advice normalizes missing sources; the audit table shows the self-check status; and conservative mode blocks automatic queueing whenever the self-check is incomplete.
- 2026-07-10: Added Hermes's normalized, persisted decision preflight bundle: the exact manager-cycle snapshot now supplies report/candidate waterfall, candidate-relevant exposure, capital and circuit-breaker state, Markov freshness, active experiments, and classified recent failures while excluding Saxo sessions, raw broker payloads, and raw error text.
- 2026-07-10: Added the first decision-action regression guard: manual decision-report actions now share an explicit live/dry-run mode helper, and unit tests prove completed dry-run reports cannot trigger Trading Manager or Saxo execution side effects.
- 2026-07-10: Added an operator acknowledgment/override path for the monthly-loss circuit breaker: the Trading Manager now records threshold breach vs active halt separately, current-month overrides are persisted in runtime settings, and the Overview cash deployment panel can resume or clear BUY resumption for the month with notes.
- 2026-07-10: Added an operator override path for active instrument quarantines: exact symbol/action/signature overrides are persisted in runtime settings, the Trading Manager only bypasses a quarantine when the exact override is active, and the Overview quarantine panel exposes row-level override/clear controls with notes.
- 2026-07-10: Added operator acknowledgments for current Overview integrity mismatches/warnings: each issue receives a stable issue key, acknowledgments are persisted in runtime settings with notes, and the Overview integrity panel can acknowledge or clear current issue acknowledgments without marking the underlying check healthy.
- 2026-07-10: Added broker exposure aggregate reconciliation to Overview integrity: Saxo exposure P/L is compared with dashboard unrealised P/L, exposure quantities are checked against broker position quantities, and drift is surfaced as warning-level integrity rows with stable acknowledgement keys.
- 2026-07-09: Started the price-monitor market-hours-aware polling slice: Saxo session validation now happens before per-poll DB/instrument work, known closed exchanges are skipped before infoprice batching, and extra watch symbols are not resolved while their exchange is closed.
- 2026-07-09: Added persisted price-monitor status and UI visibility for market-hours-aware quote pauses: the Market tab now shows latest quote-monitor status/skipped closed symbols, and the Operations Quotes chip treats closed-market pauses as intentional instead of stale/unknown.
- 2026-07-09: Completed the first price-monitor market-hours-aware polling pass with an explicit `off_hours_poll_interval_minutes` setting; closed-market refresh summaries now slow the scheduler-side quote heartbeat from 1 minute to 15 minutes by default.
- 2026-07-09: Started the Markov coverage cleanup with a persistent Saxo instrument negative cache; definitively unresolvable symbols now skip daily Saxo reference retries for 7 days while stored broker/position instruments still bypass the cache.
- 2026-07-09: Added Saxo share-class symbol variants for Markov, daily indicators, and execution lookup, so symbols like `CARL-B:xcse`, `VOLV-B:xsto`, and `BRK-B:xnys` can match Saxo's compact `CARLb`/`VOLVb`/`BRKb` style without weakening exchange checks.
- 2026-07-09: Added config-driven read-only Markov/daily-indicator symbol aliases for known stale mappings (`COST:xnys`, `HON:xnys`, `LIN:xnys`, `SHELL:xlon`) so chart analytics can recover coverage while persisted rows remain keyed by the original symbol and execution orders are not rewritten.
- 2026-07-09: Added scheduler-driven operational Slack alerts and Hermes dashboard age visibility for stale `pending_review` experiment proposals, so one-variable self-improvement hypotheses page the operator after 14 days instead of silently accumulating.
- 2026-07-10: Added backend duplicate detection for Hermes experiment proposals: exact same `changed_variable_path` now returns `409 Conflict` while an active or pending experiment already covers that variable.
- 2026-07-09: Added scheduler-driven operational Slack alerts for monthly-loss circuit-breaker activation and clearing, based on transitions between latest Trading Manager runs.
- 2026-07-09: Added scheduler-driven operational Slack alerts for newly active derived instrument quarantines, using normalized symbol/action/signature rows without raw broker error payloads.
- 2026-07-09: Added scheduler-driven operational Slack alert routing for overview integrity issues, including high-severity mismatches and overdue DayOrders waiting for broker-sync confirmation.
- 2026-07-09: Added a non-mutating `expiry_pending_broker_sync` lifecycle marker, overview integrity payload, Operations banner health, and Overview Integrity panel for active DayOrders whose exchange-calendar expiry is already past the broker-sync grace window.
- 2026-07-09: Continued the execution-order lifecycle reconciler: Saxo broker sync now records whether state came from the open-order endpoint, audit-activity fallback, or a missing lookup probe, and execution tooltips surface that provenance for stuck `broker_working` orders.
- 2026-07-09: Added execution-order DayOrder lifecycle visibility: active broker orders now carry `order_duration_type`, expected exchange-calendar expiry, market/timezone metadata, and the Overview/Execution tables show an Expiry column with lifecycle tooltip detail.
- 2026-07-09: Surfaced the derived instrument quarantine in the Overview sidebar so active symbol/action blocks, failure signatures, counts, expiry time, and gate config are visible after each Trading Manager run.
- 2026-07-09: Added a derived instrument quarantine in the Trading Manager: repeated identical hard execution failures over the configured lookback window now skip matching symbol/action candidates before queueing, and active quarantines are recorded in manager-run JSON.
- 2026-07-08: Repaired the May 18 import cost-basis corruption end to end: verified all 18 position_snapshots/position_lots rows against the exact old-parser corruption of the original Positioner CSV, restored true values, and recomputed 22 SELL ledger rows via FIFO replay (import lot + subsequent buys). Corrected realised P/L since the reset is +69,251 DKK; per-symbol expectancy analytics are now trustworthy.
- 2026-07-08: Added the monthly-loss circuit breaker: when month P/L breaches `strategy.capital.monthly_loss_halt_dkk` (default -10,000), the Trading Manager skips all new BUYs (SELLs unaffected), records breaker state in manager runs, and the decision prompt tells the model buys are suspended. Verified active on deploy with month P/L -28,277.
- 2026-07-08: Added the commission-efficiency floor for BUYs: `execution.max_commission_pct_per_side` (default 0.3%) turns each exchange's minimum commission into a minimum clip size (XNAS/XNYS ≈ 7,021 DKK, XCSE ≈ 4,667 DKK), enforced in the manager and published per exchange in the decision prompt capital plan.

- 2026-07-08: Replaced the overview `integrity` stub with real invariant checks for portfolio identity, ledger-vs-history cash drift, broker cash drift, implausible position-lot unit cost, and stale/unreconciled execution orders.
- 2026-07-08: Added a database-backed Saxo session refresh lease on the `saxo_sessions` singleton so API, scheduler, and MCP pods do not race Saxo's single-use refresh token during rollouts; auth status, explicit refresh, user logout keepalive, and broker session ensure paths now route through the lease.
- 2026-07-08: Upgraded the FX cache to prefer Saxo FX spot infoprices, falling back to ECB daily rates and then static constants; decision-report indicator context, Markov context, Trading Manager BUY value verification, overview read models, price snapshots, and broker-fill ledger rows now use the cached FX path where async DB access is available.
- 2026-07-07: Added a database-backed `currency_fx_rates` cache fed by ECB daily rates with staleness bounds, static fallback, and cache freshness short-circuiting; price-monitor DKK snapshots and broker-fill ledger rows now use cached FX rates instead of hardcoded active valuation rates.
- 2026-07-06: Added scheduler-cycle duration metrics: each persisted cycle now includes `duration_ms` plus per-step `step_durations`, and the dashboard shows recent cycle runtime in the Scheduler Cycles table.
- 2026-07-04: Added scheduler-driven operational Slack alerts for repeated decision-report failures, execution-failure bursts, stale scheduler completion, and missed Hermes EOD reflection, with runtime notification table creation in Rust.
- 2026-07-04: Exposed operational notification status in the Scheduler Cycles table so alert dispatch health is visible without opening raw cycle JSON.
- 2026-07-04: Added backend-backed decision pulse health rows for Nordic/EU, US, and manual reports, including latest report, last success, last failure, and 7-day attempts.
- 2026-07-04: Added `make post-deploy-guard` backed by non-secret `.run/last_deploy.env` metadata so deploys can verify the cluster is running the expected API, scheduler, MCP, and Hermes images.
- 2026-07-04: Added a CNPG/RustFS backup and restore rehearsal runbook with manual backup checks, RustFS object inspection, safe restore namespace pattern, and verification queries.
- 2026-07-04: Expanded `make post-deploy-smoke` with decision-report schema health validation and optional expected-image checks for API, scheduler, MCP, and Hermes deployments.
- 2026-07-04: Added `make diagnostics-artifact` so read-only diagnostics can be captured to timestamped `.diagnostics/` logs for Slack/GitHub issue sharing without changing the default diagnostic behavior.
- 2026-07-04: Added dependency and CVE hygiene workflow with `make deps-dry-run`, `make security-scan`, RustSec advisory scanning, Trivy filesystem/image CVE scanning, and Trivy secret scanning.
- 2026-07-04: Tightened Docker build-context hygiene so local RustFS object-store data is excluded, then added `make post-deploy-smoke` for read-only rollout, health, overview, scheduler, Saxo-session, MCP tool-discovery, and Hermes gateway checks after deploy.
- 2026-07-02: Added a read-only `make diagnostics` bundle for pod status, rollouts, scheduler/API/Hermes logs, resource usage, CNPG health, RustFS backup state, shared ngrok routing, and sanitized app performance/execution summaries.
- 2026-07-01: Added execution-order attribution linking orders back to decision report, Trading Manager decision, Hermes advice delta, latest daily indicators, and latest Markov state in the Execution view.
- 2026-06-29: Changed Trading Manager SELL sizing to prefer current Saxo broker snapshots over stale imported position snapshots, preventing already-flattened holdings such as ORSTED and NNIT from being queued again.
- 2026-06-27: Added an OpenRouter structured-output schema registry and reusable strict-schema validator so all current response schemas are checked for `additionalProperties: false`, complete `required` lists, union branches, arrays, and stale required entries.
- 2026-06-26: Improved Saxo limit-price handling by using configured overrides, Saxo instrument-details tick schemes, and explicit broker-expired status mapping for unfilled DayOrders.
- 2026-06-25: Added a Decision Report Quality panel that scores completion, schema strictness, normalized section presence, order shape, and market-scope enforcement warnings.
- 2026-06-25: Added an Overview Cash Deployment panel that explains latest Trading Manager reinvestment pressure, buy budget, BUY candidates, approved BUYs, and blocked BUYs.
- 2026-06-25: Added a sanitized decision-report debug panel with expandable prompt, request, provider response, and normalized report payloads plus redaction tests.
- 2026-06-24: Added a dry-run decision report action so provider/schema changes can be tested without Trading Manager or execution side effects.
- 2026-06-24: Added EU/US/manual decision pulse health cards to make missed or failed scheduled reports visible.
- 2026-06-24: Added Execution page broker diagnostics that classify common Saxo failures and render recent order events through the same tooltip/detail formatter.
- 2026-06-24: Added a compact operations banner for Saxo session, scheduler heartbeat, report, Markov, daily indicator, and quote freshness health.
- 2026-06-24: Added a Hermes Decision Advice Audit table that shows whether recent reports received advice, the recommendation, order-advice counts, manager status, queued/executed/failed order counts, and conservative impact.

## Immediate Stabilization

These items reduce operational risk and make the existing system easier to trust.

| Priority | Area | Improvement | Why It Matters |
| --- | --- | --- | --- |
| P0 | Security | Redact the database connection string in the dashboard Runtime panel. | Landed 2026-07-13: `DashboardView.db_label` now uses a structured, display-only label that excludes credentials, query parameters, and local paths; the known dashboard and startup-log display paths were audited. |
| P0 | UI performance | Continue splitting dashboard reads per tab. | The first seven slices landed 2026-07-14: Decision Report lists exclude prompt/request/response/report JSON except for active detail; portfolio history loads only on Performance; tab-exclusive collections load only on their own tabs; Execution orders, Markov signals, Quiver signals, and Scheduler Cycles are server-paginated with bounded shared views. Remaining work: paginate remaining long shared tables and add detail endpoints. |
| P0 | Deploy provenance | Verify the binary’s Git revision after every Kubernetes rollout. | Landed 2026-07-13: Docker embeds a full commit SHA, `/api/health` returns it, and `post-deploy-guard` requires the running revision to be equal to or descend from the requested commit, closing the matching-tag/stale-image gap observed on 2026-07-09. |
| P1 | Data hygiene | Remove or audit stale `runtime_settings` overrides: `strategy.capital.cash_buffer` from 2026-05-05 stores `{min_cash_buffer_pct: 0.0, max_deployment_pct: 1.0}` — currently dead (readers are config-only) but it silently activates a zero cash buffer if runtime-settings reads are ever wired back. Delete it or add an expiry/provenance convention for runtime overrides. | A dormant zero-buffer override is exactly the wrong surprise to rediscover during a future refactor. |
| P1 | Data hygiene | Scheduler cycle retention landed 2026-07-14: each Rust scheduler cycle prunes rows older than `history_retention_days` and then enforces `history_max_rows` (250/30d in Kubernetes), matching the legacy policy. Remaining work: reclaim existing PostgreSQL bloat with a scheduled low-impact maintenance plan, including the separate `audit_log` dead space. | Cycle JSON at ~5.5 KB/row grew unbounded and slowed scheduler-history queries; physical dead space still inflates backups and restore rehearsals until maintenance reclaims it. |
| P0 | Decision reports | Add a non-mutating "dry run report" endpoint that submits and parses a decision report without Trading Manager or execution queue side effects. | Makes schema/provider fixes testable without risking new orders. |
| P0 | Decision reports | Keep the OpenRouter structured-output schema registry current whenever new strict schemas are added, including Hermes prompts if they use strict schemas later. | Prevents repeat `invalid_json_schema` outages. |
| P0 | Execution safety | Show broker precheck and placement failure details consistently in Execution Queue tooltips and order event views. | Reduces guesswork when orders fail. |
| P0 | Scheduler | Per-pulse scheduled report health landed 2026-07-14: the persistent Operations banner shows EU/US latest status plus last successful run. | Makes missed EU/US pulses visible immediately. |
| P1 | Risk guardrail | Continue observing the monthly-loss circuit breaker override path added 2026-07-10; next follow-up is an audit/history view if overrides become common. Slack alerting for activation/deactivation landed 2026-07-09. | The breaker now has a month-scoped operator resumption path, but repeated overrides should be visible as risk governance evidence. |
| P1 | Accounting integrity | Continue observing the Overview integrity system after UI surfacing, Slack routing, operator acknowledgments, and broker exposure aggregate reconciliation landed. Follow-up only if runtime evidence shows additional broker/accounting invariants are needed. | The May position_lots corruption (3.2M DKK cost basis on 31 shares) sat unnoticed for a month; invariants with alerts catch the next one in hours. |
| P1 | Markov coverage | Continue fixing the assets that fail instrument resolution on daily Markov runs: negative caching, share-class compact symbol variants, and read-only configured aliases landed 2026-07-09; remaining work is verifying the next daily run and correcting any genuinely unresolved exchange mappings. | 19% of the universe was missing regime signal coverage. Weekly negative caching stops repeated dead-end lookups, share-class variants recover many `*-A`/`*-B` Nordic and Berkshire-style rows, and explicit aliases recover known stale symbols without weakening exchange checks or rewriting orders. |
| P1 | Instrument quarantine | Continue observing the derived quarantine override path added 2026-07-10; follow-up only if override history needs a separate audit table or if derived signatures become too expensive to compute. Slack alerting for active derived quarantines landed 2026-07-09. | Known-broken instruments waste daily order capacity, pollute execution stats, and re-teach the model nothing; overrides now require exact symbol/action/signature and notes. |
| P1 | Price monitor | First market-hours-aware polling pass landed 2026-07-09: session-first checks, closed-exchange skips, persisted UI visibility, and explicit slow off-hours heartbeat. Follow-up only if runtime evidence shows per-exchange cadence needs finer tuning. | Cuts ~two-thirds of infoprice calls, stops `updated_at` implying freshness for closed markets, and reduces log churn during reauth gaps. |
| P1 | Testing | Continue expanding workflow tests. Decision action, scheduled-report freshness, Hermes conservative self-check, execution-queue admission, database-backed Trading Manager queue persistence, concurrent execution-order claim, broker terminal-state, and broker-sync not-found guards have landed; next slice is a database-backed final-fill reconciliation fixture with no broker HTTP. | Protects the most critical cross-module workflows. |

## High-Leverage Workflow Improvements

These are specific changes that could improve the quality of trading decisions while keeping the system safe and measurable.

### Trading Quality

| Idea | Shape | Measurement |
| --- | --- | --- |
| Trade thesis ledger | For every suggested trade, persist a short thesis with expected holding window, invalidation condition, target catalyst, and risk reason. | Compare realized outcome to thesis after 1 day, 5 days, and exit. |
| Candidate scoring waterfall | Landed 2026-07-13: selected Decision Reports show the stored manager preflight and outcome as a compact sanitized waterfall, including market/risk, technical, Markov, Hermes quantity effect, result, and stable gate code. Follow-up only if replay data shows a missing deterministic gate needs to be added. | Show why candidates were accepted or rejected and which gate blocks most trades. |
| Post-trade attribution | Attach P/L back to decision pulse, report id, Hermes advice state, Markov state, and strategy role. | Detect which pulses and filters actually add value. |
| Missed-trade shadow book | Track high-scoring candidates that were blocked by Hermes, budget, market scope, or stale data. | Measure whether blocks avoided losses or missed gains. |
| Alt-data conflict surfacing | When a strong Quiver signal opposes a current holding or a proposed BUY (live example 2026-07-08: Quiver bearish NVDA -0.68 and AMZN -0.48 with 16-17 Congress events while both were held), flag the conflict explicitly in the decision prompt and a UI panel rather than leaving it buried in the signal table. | Track resolved conflicts: did the position exited/reduced on conflict outperform holding through it? Quiver stays advisory; this only makes disagreement visible. |
| Adaptive cash deployment | Replace fixed "cash above buffer means buy" with tiers based on volatility, drawdown, signal quality, and number of independent qualifying assets. | Improve cash utilization without forcing weak trades. |
| Position lifecycle state | Track whether each holding is starter, add candidate, hold, trim, exit candidate, or blocked. | Prevent random re-entry/rebuy behavior and make position sizing intentional. |
| Cooldown and churn guard | Add symbol-level cooldowns after failed orders, recent exits, or conflicting reports. | Reduce repeated attempts in noisy names and failed instruments. |
| Gate replay harness | Re-run Trading Manager gates offline against stored decision reports and their persisted contexts, with candidate thresholds overridden (e.g. `markov_gate.min_signed_signal` 0.15 vs 0.25, `min_confluences` 3 vs 4). All inputs (reports, manager runs, indicator and Markov signals) are already persisted, so this is pure re-evaluation with no broker or model calls. | Calibrate gate thresholds from evidence instead of intuition: show exactly which historical approvals/blocks would flip under a proposed setting before promoting it as an experiment. |

### Hermes Advisory Loop

| Idea | Shape | Measurement |
| --- | --- | --- |
| Unstick the experiment review queue | Four one-variable proposals have sat in `pending_review` since as early as 2026-06-16 (Markov age gate, min_confluences 3→4, and two near-duplicate cash-buffer raises) while reflections run nightly. Pending-review aging alerts through the Slack path and dashboard age highlighting landed 2026-07-09; exact same-variable duplicate rejection landed 2026-07-10. Remaining work is a weekly review digest, auto-expiry with a "stale, superseded by market conditions" status after N days, and near-duplicate semantic merging before insert. | Median time from proposal to decision; zero proposals older than 14 days; the self-improvement loop actually completes its cycle instead of accumulating unreviewed hypotheses. |
| Hermes preflight contract | Landed 2026-07-10: Hermes receives a compact normalized manager-cycle bundle with report summary, candidate waterfall, candidate-relevant exposure, Markov freshness, active experiments, and classified recent failures. The exact snapshot is retained in `trading_manager_runs.manager_json` for audit/replay; sensitive session, broker-payload, and raw-error data is excluded. | Higher advice consistency and fewer irrelevant comments. |
| Advice delta audit | Landed 2026-07-13: each manager run records the normalized per-candidate applied advisory effect, quantity delta, matching precedence, and final manager outcome without raw rationale/broker data. | Quantify Hermes impact separately from model/report impact. |
| Counterfactual tracking | Landed 2026-07-13: conservative advice blocks/reductions now create a non-mutating quote-to-quote shadow ledger for only the prevented quantity. Active rows enter the read-only price monitor and the Hermes tab displays reference/latest quote, estimated directional return, and status. Fees, FX, slippage, tax, broker execution, and realised P/L remain excluded. | Compare Hermes restrictions against transparent directional shadow outcomes without treating them as realised performance. |
| Proposal quality rubric | Score Hermes proposals by evidence strength, one-variable purity, safety, measurable metric, and duplicate risk. | Promote fewer vague proposals and more testable experiments. |
| Learning memory compression | Summarize EOD/weekly reflections into stable lessons and stale lessons with expiry dates. | Prevent old lessons from overweighting new market conditions. |
| Hermes self-check | Landed 2026-07-10: before writing advice, Hermes must submit a structured context self-check for latest report, Markov run, EOD report, current positions, and active experiments. The audit table surfaces missing context; conservative mode fail-closes automatic queueing when any required source is missing, even if per-order advice says `allow`. | Detect missing context before advice is trusted. |

### Decision Report Flow

| Idea | Shape | Measurement |
| --- | --- | --- |
| Dry-run first-class mode | Add `generate_report?mode=dry_run` that never queues or executes orders. | Safer model, schema, and prompt experiments. |
| Two-stage report | Stage 1 selects/scans candidates. Stage 2 produces trade orders only for candidates that pass deterministic gates. | Less model freedom in order construction. |
| Deterministic pre-report context | Persist the exact context snapshot used for each report: portfolio, candidates, market status, Markov, daily indicators, cash budget. | Reproducible reports and easier debugging. |
| Strict normalized output | Keep the model schema small and deterministic, then enrich locally with calculated fields. | Lower provider/schema failure rate and less hallucinated math. |
| Report confidence and quality score | Calculate parse validity, data freshness, market scope consistency, budget consistency, and number of unsupported claims. | Block low-quality reports before Trading Manager. |
| Provider/model A/B harness | Run OpenRouter models side-by-side in dry-run mode over the same context. | Compare usefulness, parse reliability, cost, and latency before switching production. |
| Prompt regression corpus | Keep a small set of known contexts and expected schema-valid outputs. | Catch prompt/schema regressions without waiting for market hours. |

### Saxo API And Broker Integration

| Idea | Shape | Measurement |
| --- | --- | --- |
| Instrument capability cache | Cache resolved instrument metadata: UIC, asset type, currency, exchange, tradability, supported order types, tick size, commission/precheck blockers. | Reduce repeated Saxo resolve/precheck failures. |
| Precheck-only probe job | Periodically precheck watchlist instruments with tiny/safe hypothetical orders where allowed, without placement. | Identify commission/tradability blockers before reports recommend them. |
| Market-hours validator | Use Saxo exchange calendars and instrument market state together before queueing orders. | Avoid orders in closed or ambiguous sessions. |
| Order lifecycle reconciler | Reconcile local orders with Saxo open orders, audit activities, fills, expiries, and cancellations every scheduler cycle. Visibility slices landed 2026-07-09: active DayOrders now show expected exchange-calendar expiry, broker-sync provenance records open-order vs audit-activity vs missing-lookup state, and overdue active DayOrders are flagged in row tooltips, overview integrity, the Operations banner, the Overview Integrity panel, and scheduler-driven Slack alerts as expiry-sync-pending without mutating broker state. | Fewer stuck `broker_working` or stale local statuses. |
| Tick and currency normalizer | Centralize price rounding, display currency, order currency, and estimated DKK conversion. | Prevent DKK/USD display mistakes and tick-size rejections. |
| Session health preflight | Before reports and queue processing, assert access token, refresh token, account key, and environment are valid. | Avoid false trade failures caused by reauth drift. |
| Saxo error taxonomy | Map raw Saxo errors into stable categories and remediation hints. | Better UI, alerts, and Hermes learning from execution failures. |

### Saxo OpenAPI Capabilities To Adopt (reviewed 2026-07-11)

The runtime currently uses a small slice of the OpenAPI: `/port/v1` snapshots, `/chart/v3` history, `/trade/v1/infoprices` polling, `/trade/v2` orders + precheck, `/ref/v1` instruments/exchanges. The reference docs offer several capabilities that map directly onto existing polling pain. Streaming is verified reachable on SIM (`wss://sim-streaming.saxobank.com/sim/oapi/streaming/ws/connect`, plain WebSocket, same OAuth token, ContextId + per-subscription ReferenceId, delta messages over a stored snapshot, resumable via `?messageid=` and re-auth via `PUT /streaming/ws/authorize`).

| Idea | Shape | Measurement |
| --- | --- | --- |
| Streaming price subscriptions | Replace the 1-minute infoprices polling loop with `POST /trade/v1/infoprices/subscriptions` bound to one WebSocket stream; keep the poller as automatic fallback when the socket drops. One stream carries all held + extra watch symbols. | Quote latency from ~60 s to seconds; infoprice REST calls near zero during market hours; price monitor becomes push-driven with a heartbeat check. |
| ENS activities subscription for fills | Subscribe via `/ens/v1/activities/subscriptions` (activity types: Orders, Positions, MarginCalls, AccountFundings) on the same WebSocket instead of fast-polling broker order sync every minute while orders are open. ENS is streaming-only (no REST snapshot); recovery uses `FromDateTime`/`SequenceId` replay — events are retained 3 days on the streaming replay and 14 days via `GET /ens/v1/activities` (replay capped at 50 msg/s). Persist the last processed SequenceId so reconnects and pod restarts resume gap-free. | Fill-to-ledger latency from ~1 minute to seconds; scheduler fast-poll mode becomes a fallback; no more missed `broker_expired` transitions between cycles. |
| ENS 14-day activities backfill | Independent of streaming, use `GET /ens/v1/activities` with `FromDateTime` as a scheduled reconciliation source: compare the broker's own order/fill/position event log for the last N days against `execution_orders`/`execution_fills`/`trade_ledger` and alert on divergence. | A broker-authored audit trail catches anything local sync missed (expired orders, out-of-band fills, manual SIM interventions) within one scheduler cycle instead of never. |
| Unknown-state order timeout handling | Per the order-placement guide, a timeout returning `TradeNotCompleted` does not mean the order failed — the current queue treats placement errors as failures, which risks duplicate submissions on retry. Treat timeout/`TradeNotCompleted` as state-unknown: hold the local order in a `broker_state_unknown` status, reconcile via open-orders/ENS lookup (keyed by our `x-request-id`/`ExternalReference`) before any retry. | Eliminates the double-order failure mode entirely; retries become provably safe. |
| Tradable Prices for limit anchoring | The Trade service distinguishes lightweight `InfoPrices` (display) from `Prices` (tradable quotes with quote ids and commission detail, streamable). The delayed-price limit-order logic currently anchors offsets on infoprices; anchoring on a tradable price request just before placement would cut the "limit too far from market" rejections and expired DayOrders. Also expose `/trade/v1/messages` (trade confirmations, margin calls, price alerts) into the ops alert path. | Fewer `broker_expired` limit orders; broker messages surface in Slack instead of only inside Saxo's UI. |
| Broker-computed realized P/L cross-check | Pull `/port/v1/closedpositions` (FieldGroups include closed P/L and open/close prices) and reconcile against locally computed FIFO realized gains per symbol/period as an accounting invariant with Slack alerting on divergence. | The 2026-07-08 cost-basis repair would have been caught within a day by a closed-position divergence alert instead of a month later. |
| Live FX rates via FX-spot infoprices | Implement the live-FX roadmap row concretely: subscribe (or poll hourly off-market) to FX spot infoprices for USDDKK/EURDKK/GBPDKK/SEKDKK/NOKDKK Uics and persist into an fx_rates table consumed by `fx_rate_to_dkk`, with the static table as bootstrap fallback. | DKK valuations track real FX; ledger rows store the actual rate at fill time. |
| Account performance history | Use the `hist` service's performance/timeseries endpoints for broker-computed account value and time-weighted returns, rendered next to the local `portfolio_value_history` series. | Independent verification of the Performance tab; discrepancies surface data bugs (frozen daily P/L, stale prices) automatically. |
| Balances/positions streaming (later) | Once price + ENS streaming are stable, move broker read-model refresh to `/port/v1` subscriptions on the same stream. | Broker snapshots stay current between scheduler cycles without extra REST load. |
| Unified Saxo HTTP client | Consolidate the several per-module `saxo_get_json` implementations (markov_method, saxo_portfolio, daily_indicators, auth) into one shared client module: a single long-lived `reqwest::Client` (today several call sites build a new client per request, defeating connection reuse), HTTP/2 with gzip/deflate `Accept-Encoding` (Saxo explicitly recommends HTTP/2 over the now-obsolete batch endpoints; chart payloads are the big win), uniform 429 backoff (currently only the Markov path retries rate limits), and a per-request correlation id logged with each call. | Fewer TLS handshakes and 429 failures during the ~200-call Markov/indicator runs; one place to tune timeouts and observe Saxo latency. Do not implement multipart batching — Saxo marks it obsolete in favor of HTTP/2. |
| Rate-limit-aware throttling | Build the documented limits into the unified client: 120 requests/minute per session per service group (the nightly Markov run's ~200 sequential chart calls at 500 ms spacing run exactly at this limit — pace to ~100/min per group via a token bucket), 1 order/second per session (space execution-queue placements ≥1.1 s apart), and adaptive slow-down using the `X-RateLimit-Session-Limit/Remaining/Reset` and `X-RateLimit-AppDay-*` response headers instead of fixed sleeps. Send a unique `x-request-id` on every POST/PATCH: Saxo returns 409 for identical requests within 15 s without one, which is also the documented idempotency mechanism for safe order-placement retries. Note: entry + related (stop/target) orders must be bundled in one request — placing them separately is rejected, which matters if `submit_bracket_with_entry` is ever enabled. | Zero avoidable 429/409 failures; Markov and indicator runs finish faster with header-driven pacing; order retries become idempotent by construction. |
| Order-placement return codes and disclaimers | Fold the learn-section "Order Placement return codes" and "Pre-Trade Disclaimers" material into the Saxo error taxonomy row so precheck/placement failures map to documented codes rather than string matching. Per the planned-changes page, pre-trade disclaimer handling is mandatory for all OpenAPI apps (enforced from May 2025; orders can be rejected when an app does not handle disclaimers even if none currently apply) — the runtime has no disclaimer handling today, so implement the check-and-confirm flow around order placement; SIM apparently does not enforce it yet, LIVE presumably will. Also from planned changes: `root/v1/user` retires 2026-09-01 (not used by this runtime — verified 2026-07-11), and the 2026/2027 client-management/proxy-voting changes do not apply. | Orders cannot start failing with disclaimer rejections on LIVE (or a future SIM enforcement wave); deprecations are tracked with evidence instead of discovered as outages. |
| SIM limitations note + LIVE readiness checklist | Per the environments page, SIM is a restricted copy of LIVE: some market data and reporting features are unavailable, support priority is lower, and SIM may run newer API versions than LIVE — which sets expectations for known SIM quirks (delayed quotes the limit-offset logic already assumes, reference-data lag such as the pending SPCX listing). Keep a wiki checklist for an eventual LIVE migration: separate application registration (app key/secret are not shared between environments), redirect URIs re-registered on the live app, live auth/gateway/streaming hosts (`live.logonvalidation.net`, `gateway.saxobank.com/openapi`, `live-streaming.saxobank.com/oapi/streaming/ws` — the code switches REST hosts on `saxo.environment` already, streaming must do the same), new k8s secrets, market-data entitlement costs, `require_approval_live: true` re-enabled, and a full safeguard verification pass (breaker, floors, quarantine) before any real order. | SIM oddities stop being investigated as bugs; a LIVE switch becomes a checklist instead of an improvisation. |

## Decision And AI Pipeline

The decision-report pipeline should become more deterministic, easier to diagnose, and cheaper to iterate.

- Split report generation into stages: build context, submit model request, parse/normalize response, persist report, run manager, process execution queue.
- Persist sanitized prompt and sanitized response excerpts in a dedicated debug view, with secrets and broker identifiers redacted.
- Add a provider capability matrix for OpenRouter models: strict schema support, plugin support, timeout profile, cost, and observed parse reliability.
- Add fallback behavior for provider/schema failure: keep the failed report but optionally retry with a known-safe model and schema after operator approval.
- Add model comparison runs that produce reports without queueing trades, so Hermes can evaluate decision quality across models.
- Add report-quality checks before Trading Manager sees a report: active market scope, suggested trade count, valid strategy keys, valid currencies, cash-budget consistency, and required Markov/daily-indicator evidence.

## Hermes Self-Improvement

Hermes should become a useful advisory layer that learns from evidence without becoming an unbounded actor.

- Add a Hermes advisory dashboard section with rows for each report: input report id, advice status, recommendation, orders allowed/reduced/blocked, timeout status, and final Trading Manager outcome.
- Add "proposal impact" tracking: when Hermes blocks or reduces an order, follow the hypothetical outcome versus the executed path.
- Give Hermes access to normalized report outcomes: report completed, manager queued, broker submitted, filled/expired/failed, and next-day/weekly P/L attribution.
- Add a daily "lessons pending review" queue for Hermes proposed actions that are not formal experiments.
- Add explicit duplicate detection for Hermes proposed experiments, including semantically equivalent variable paths.
- Add baseline promotion evidence packs: before/after metrics, affected reports/orders, drawdown, Sharpe, cash utilization, and failure count.
- Add a "one-variable audit" view that shows which live or SIM setting currently differs from baseline and why.
- Keep conservative mode as default for report-time advice; only consider stronger modes after dry-run, paper, and SIM evidence is strong.

## Trading Manager And Execution

The execution path should be more transparent and more resilient before expanding live behavior.

- Finish Rust parity for broker status sync, replace/cancel management, fill reconciliation, and local ledger reconciliation.
- Add a queue state machine with explicit statuses: `queued`, `precheck_ok`, `broker_working`, `partial_fill`, `final_fill`, `expired`, `cancelled`, `rejected`, `local_reconciled`.
- Store normalized broker failure categories alongside raw error text: auth, market closed, instrument not tradable, commission setup, insufficient cash, tick size, quantity, session expired, unknown.
- Add order expiry handling for day orders: show expected expiry time in local exchange time and reconcile expired broker orders promptly.
- Add price/tick-size diagnostics directly into execution events for limit orders.
- Add a "why this order exists" drawer that links order -> report -> Hermes advice -> Markov signal -> technical signal -> capital budget.
- Add a broker-safe simulation mode that runs all prechecks and manager decisions but never places orders.

## Strategy, Markov, And Risk

Strategy changes should be evidence-driven and reversible.

- Promote the pending confluence-count experiment only after comparing missed trades, avoided failures, and P/L impact.
- Promote or reject the Markov signal age gate after measuring stale-signal decision quality.
- Add per-symbol regime history charts: current Markov state, transition matrix, stationary distribution, signal, and signal age.
- Add sector, currency, and exchange concentration gates before queueing BUYs.
- Add realized and unrealized attribution by decision pulse: EU open, US open, manual, portfolio sync, and Hermes-influenced.
- Add cash utilization diagnostics that explain whether cash is held by policy, no qualifying candidates, conservative Hermes block, market closed, failed order, or stale signals.
- Add drawdown guardrails that reduce buy budget or require review after threshold breaches.
- Add explicit "do not trade" lists for instruments that repeatedly fail precheck, instrument resolution, commission setup, or market-scope checks.

## Portfolio And Performance Analytics

Performance should be explainable at portfolio, report, and position level.

- Reconcile dashboard aggregate P/L with broker exposure rows and local portfolio value history, and show source freshness for each.
- Add P/L attribution by symbol, currency, sector, and strategy role.
- Add daily, weekly, monthly, and since-reset performance cards with target progress and drawdown.
- Add benchmark comparison against relevant indices for active regions.
- Add trade expectancy metrics once enough closed trades exist: win rate, average win/loss, payoff ratio, holding time, and slippage.
- Add cost tracking: commissions, FX impact, spread assumptions, and slippage versus limit/market price.
- Add "performance confidence" labels when market data is delayed, approximated, stale, or missing.
- Implement real Danish share-income tax estimation: the `taxation.share_income` brackets (27%/42%) in config are currently unused — `after_tax_summary` in `state.rs` hardcodes `estimated_tax_dkk: 0.0` and reports after-tax P/L identical to pre-tax. Estimate tax on realized YTD gains via the brackets, mark unrealized gains at the marginal rate, and let goal tracking optionally show net-of-tax progress.

## UI And UX

The dashboard should make action, risk, and system state obvious without needing logs.

- Add a persistent top status strip for Saxo session, scheduler, latest EU/US report, latest Hermes advice, Markov freshness, and execution queue health.
- Add disabled/loading states for all long-running actions, with progress text and final success/failure detail.
- Add per-tab stale-data timestamps and "source" labels.
- Add report detail pages with tabs for prompt, response, normalized report, manager result, Hermes advice, and orders.
- Add execution order detail modals with timeline events and broker error details.
- Add a Hermes tab split into Overview, Advice, Reflections, Experiments, and Baselines.
- Add a Markov tab table filter for portfolio, watchlist, errors, stale signals, and high-conviction signals.
- Add keyboard-safe and mobile-safe layouts for the main monitoring views.
- Per-tab lazy read models (see P0 UI performance row): each view fetches only its own data, heavy JSON/prompt payloads load on demand, and long tables (positions, Markov signals, execution orders, scheduler cycles) paginate server-side. Target: any tab under 300 ms server time and under 200 KB HTML.
- ~~Age-label or hide stale per-position decisions~~ Landed 2026-07-14: portfolio and watchlist decision chips show relative age, become `Stale` after the configurable seven-day default horizon, and treat missing timestamps as stale rather than current advice.
- ~~Make ops-banner staleness market-aware~~ Landed 2026-07-14: weekday-only Markov, Quiver, and Indicator jobs render neutral `idle (weekend)` or `waiting` when no run is due. A warning now means a run missed its latest scheduled due date, failed, or completed only partially.
- Render the AI Prompts view as collapsed sections with copy buttons instead of a 1 MB inline dump of system prompt and payload.

## Refactoring And Architecture

The Rust runtime should keep moving away from generic JSON and legacy Python behavior references.

- Replace generic JSON dashboard payloads in `src/state.rs` with typed models in `src/models.rs`.
- Split large state builders into focused read-model modules: portfolio, reports, execution, Hermes, Markov, scheduler, settings.
- Introduce typed decision-report structs and typed Trading Manager structs instead of ad hoc JSON traversal.
- Move OpenRouter/OpenAI provider code into a provider module with a shared request/response abstraction.
- Move report schema construction into a dedicated module with schema tests.
- Keep `src/main.rs` startup-only and avoid adding business logic there.
- Continue removing Python runtime dependencies from active Kubernetes images, while keeping legacy Python files as behavior references until replaced.
- Add repository-level architecture decision records for major porting choices.
- ~~Decide the fate of the `frontend/` Next.js app versus the Dioxus SSR dashboard~~ Resolved 2026-07-04: the Next.js app was legacy (never built or deployed) and was removed; the Dioxus SSR dashboard in `src/ui.rs` is the committed UI. Note the `daytrader-frontend` Kubernetes Service is unrelated to that directory — it is a live alias Service selecting the API pods that the shared ngrok AgentEndpoint routes through, and must be kept (or renamed only together with the gateway route).
- Give the scheduler cycle per-step timeout budgets. The duration-metric half landed 2026-07-06: each persisted cycle records total `duration_ms` plus per-step `step_durations` in `cycle_json`, and the dashboard shows recent cycle runtime. The remaining work is explicit timeout budgets around slow steps.

## Operations And Deployment

Local Docker Desktop Kubernetes should stay easy to inspect and recover.

- Add alerting for repeated decision-report failures, repeated broker execution failures, stale scheduler heartbeat, and missed EOD reflection.
- Keep public ngrok route ownership in the shared gateway and app-owned internal AgentEndpoint ownership in this repo.
- Add watch-symbol lifecycle alerts: when an `extra_symbols` entry (e.g. SPCX, waiting for Saxo SIM to sync the 2026-06-12 IPO) resolves for the first time, send a Slack notice and record the activation, instead of silently starting quotes. Include the inverse alert when a previously resolvable symbol disappears from reference data.

### Build, Deploy, And Repo Hygiene (reviewed 2026-07-11)

| Idea | Shape | Measurement |
| --- | --- | --- |
| Docker dependency-layer caching | `Dockerfile.api` does `COPY . .` then a full `cargo build --release`, so every deploy recompiles all dependencies from scratch (the whole ~28k-line workspace plus ~500 crates). Split into a dependency layer (cargo-chef, or `COPY Cargo.toml Cargo.lock` + dummy-main build) and add BuildKit cache mounts for the cargo registry and target dir. | Incremental deploy builds drop from minutes to well under a minute; deploys stop being the reason to postpone small fixes. |
| Build-context hygiene | `screenshots/` (12 MB of PNGs, added 2026-07-11 and growing) is in neither `.dockerignore` nor `.gitignore`, so it inflates every build context and will land in git history. Exclude it from the Docker context and decide whether screenshots belong in the repo at all (suggest `.gitignore` + a dated archive outside the tree, or `docs/screenshots/` with deliberate curation). | Build context stays at the ~4 MB achieved by the 2026-07-04 hygiene pass instead of regressing 4x. |
| Content-addressed image tags | Tag images with the git SHA (plus a dirty-tree marker) instead of a timestamp: enables skip-build-when-unchanged, gives the deploy-provenance P0 row its comparison value for free, and makes `.run/last_deploy.env` self-explanatory. Pair with committing before deploying — the 2026-07-09 stale-image window that silently dropped the circuit breaker for a trading day was a dirty/stale-tree deploy. | Every running image is traceable to an exact commit; unchanged code never rebuilds. |
| Deploy script speedups | `deploy_k8s_docker_desktop.sh` runs `helm upgrade cnpg` and full kustomize apply on every deploy and then waits for four rollouts serially. Skip the CNPG helm step unless the chart/values changed (version pin + hash check), run the rollout waits in parallel, and short-circuit restarts when the image digest is unchanged. | Routine deploy wall-time drops by roughly half; fewer pointless pod restarts (also reduces Saxo session-burn exposure until the refresh lease lands). |
| Legacy Python surface removal plan | The retired Python runtime still occupies the tree: `main.py`, `web_main.py`, `src/saxo_daytrader_xai/` (1.7 MB), `requirements.txt`, `.venv/` (425 MB), `.pytest_cache`, and legacy `scripts/*.py` — while `scripts/create_postgres_backup.py` and `prune_postgres_backups.py` remain genuinely used by the `daytrader-backup` CronJob image. Enumerate which Python files are load-bearing (backup scripts, `saxo_oauth_helper.py`?), delete the rest in one commit, and port the two backup scripts to the Rust binary later so `Dockerfile.backup` can retire too. | AGENTS.md "legacy surface" section shrinks to the backup scripts only; no more agent time spent reading dead Python for behavior reference. |
| Working-tree data cleanup | Non-code data sits inside the repo working tree: `rustfs/` holds 12 GB of live RustFS object-store data (gitignored but one `rm -rf` away from destroying backups), root-level `Positioner_*.csv` exports (personal portfolio data; the 17-maj file is the verified source of the 2026-07-08 cost-basis repair), legacy `ledger.db`, and empty `minio-data/`/`logs/`/`data/` dirs. Move the RustFS data directory outside the repo (it is an external service's storage), archive the 17-maj CSV under `data/` with a README pointing at the repair log, delete the other two CSVs and `ledger.db`, and prune the empty dirs. | The repo tree contains code and docs only; backup storage cannot be lost to a careless repo clean; personal exports stop living at the repo root. |

## Security And Secrets

Security posture should assume model prompts, broker payloads, and external docs are untrusted.

- Run dependency and CVE hygiene regularly with `make deps-dry-run` and `make security-scan`; treat fixed HIGH/CRITICAL CVEs, RustSec advisories, and secret findings as release blockers unless there is a dated exception with a compensating control.
- Keep all Saxo tokens, AccountKey, ClientKey, OpenRouter keys, ngrok keys, and database credentials out of wiki pages and normal UI payloads.
- Add automated redaction tests for logs, report debug views, Hermes context, and MCP tool responses.
- Make Hermes capabilities explicit and deny broker mutation tools by default.
- Add role separation in UI for view-only operations, report generation, order processing, settings changes, and experiment promotion.
- Add audit records for settings changes, experiment transitions, manual report triggers, and manual queue processing.

## Knowledge And Documentation

The wiki should become the durable operating memory for both Codex and Hermes.

- Add a short "current production shape" page that states active namespace, endpoint routing, storage backend, AI provider, model, and safety modes.
- Add runbook pages for common incidents: OpenRouter schema failure, Saxo reauth required, broker order rejected, Markov stale, CNPG degraded, RustFS unavailable, ngrok route broken.
- Add a decision record when enabling or rejecting each Hermes experiment.
- Keep [schema](schema.md), [log](log.md), and [runbooks/build-test-deploy](runbooks/build-test-deploy.md) current after operational changes.
- Use `qmd` to query the wiki before making architecture or strategy changes.

## Suggested Milestones

| Milestone | Outcome | Candidate Scope |
| --- | --- | --- |
| M1: Safe Observability | Operators can see why the system did or did not trade today. | Status strip, report pulse cards, execution failure details, Hermes advice audit table. |
| M2: Non-Mutating AI Test Harness | AI/schema/model changes can be tested without queueing orders. | Dry-run report endpoint, model comparison runs, schema validation tests, sanitized prompt/response debug page. |
| M3: Conservative Hermes Loop | Hermes advice is consistently attached, measurable, and reviewable. | Advice dashboard, conservative enforcement audit, proposal impact tracking, duplicate proposal detection. |
| M4: Typed Rust Core | Critical read/write paths are typed and easier to test. | Typed report, manager, execution, and Hermes models; module split from `state.rs`. |
| M5: Strategy Evidence Loop | Experiments can be promoted or rejected from measured SIM evidence. | Baseline evidence packs, attribution, risk metrics, one-variable audit view. |

## Open Questions

- Should manual decision reports default to dry-run and require a second explicit action to queue trades?
- Which report models should be allowed in scheduled mode versus manual dry-run mode?
- What minimum evidence should be required before promoting a Hermes experiment to SIM baseline?
- Should broker precheck-only simulation become mandatory before any new instrument class is traded?
- Which UI views need operator authentication levels beyond the existing ngrok OAuth boundary?
