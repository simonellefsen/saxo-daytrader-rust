---
type: capability
tags:
  - daytrader/wiki
  - roadmap
  - maintained-by-llm
updated: 2026-06-29
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
| P0 | Decision reports | Add a non-mutating "dry run report" endpoint that submits and parses a decision report without Trading Manager or execution queue side effects. | Makes schema/provider fixes testable without risking new orders. |
| P0 | Decision reports | Keep the OpenRouter structured-output schema registry current whenever new strict schemas are added, including Hermes prompts if they use strict schemas later. | Prevents repeat `invalid_json_schema` outages. |
| P0 | Execution safety | Show broker precheck and placement failure details consistently in Execution Queue tooltips and order event views. | Reduces guesswork when orders fail. |
| P0 | Scheduler | Add explicit "last successful scheduled report by pulse" status cards. | Makes missed EU/US pulses visible immediately. |
| P1 | Testing | Add integration tests for manual report, scheduled report, Hermes advice, Trading Manager queueing, and execution queue dry-run paths. | Protects the most critical cross-module workflows. |

## High-Leverage Workflow Improvements

These are specific changes that could improve the quality of trading decisions while keeping the system safe and measurable.

### Trading Quality

| Idea | Shape | Measurement |
| --- | --- | --- |
| Trade thesis ledger | For every suggested trade, persist a short thesis with expected holding window, invalidation condition, target catalyst, and risk reason. | Compare realized outcome to thesis after 1 day, 5 days, and exit. |
| Candidate scoring waterfall | Score each candidate through gates: market open, instrument tradable, Markov freshness, technical confluence, concentration, liquidity, cash budget, Hermes advice. | Show why candidates were accepted or rejected and which gate blocks most trades. |
| Post-trade attribution | Attach P/L back to decision pulse, report id, Hermes advice state, Markov state, and strategy role. | Detect which pulses and filters actually add value. |
| Missed-trade shadow book | Track high-scoring candidates that were blocked by Hermes, budget, market scope, or stale data. | Measure whether blocks avoided losses or missed gains. |
| Adaptive cash deployment | Replace fixed "cash above buffer means buy" with tiers based on volatility, drawdown, signal quality, and number of independent qualifying assets. | Improve cash utilization without forcing weak trades. |
| Position lifecycle state | Track whether each holding is starter, add candidate, hold, trim, exit candidate, or blocked. | Prevent random re-entry/rebuy behavior and make position sizing intentional. |
| Cooldown and churn guard | Add symbol-level cooldowns after failed orders, recent exits, or conflicting reports. | Reduce repeated attempts in noisy names and failed instruments. |

### Hermes Advisory Loop

| Idea | Shape | Measurement |
| --- | --- | --- |
| Hermes preflight contract | Give Hermes a compact normalized preflight bundle: report summary, candidate waterfall, current exposure, Markov freshness, pending experiments, recent failures. | Higher advice consistency and fewer irrelevant comments. |
| Advice delta audit | Store exactly what Hermes changed: allowed, blocked, reduced, required review, or no-op. | Quantify Hermes impact separately from model/report impact. |
| Counterfactual tracking | When Hermes blocks/reduces a trade, track the hypothetical performance of the original report order. | Decide whether conservative Hermes advice improves outcomes. |
| Proposal quality rubric | Score Hermes proposals by evidence strength, one-variable purity, safety, measurable metric, and duplicate risk. | Promote fewer vague proposals and more testable experiments. |
| Learning memory compression | Summarize EOD/weekly reflections into stable lessons and stale lessons with expiry dates. | Prevent old lessons from overweighting new market conditions. |
| Hermes self-check | Before writing advice, require Hermes to state whether it saw latest report, Markov run, EOD report, current positions, and active experiments. | Detect missing context before advice is trusted. |

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
| Order lifecycle reconciler | Reconcile local orders with Saxo open orders, audit activities, fills, expiries, and cancellations every scheduler cycle. | Fewer stuck `broker_working` or stale local statuses. |
| Tick and currency normalizer | Centralize price rounding, display currency, order currency, and estimated DKK conversion. | Prevent DKK/USD display mistakes and tick-size rejections. |
| Session health preflight | Before reports and queue processing, assert access token, refresh token, account key, and environment are valid. | Avoid false trade failures caused by reauth drift. |
| Saxo error taxonomy | Map raw Saxo errors into stable categories and remediation hints. | Better UI, alerts, and Hermes learning from execution failures. |

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

## Operations And Deployment

Local Docker Desktop Kubernetes should stay easy to inspect and recover.

- Add a one-command diagnostics bundle that collects pod status, rollouts, recent scheduler cycles, latest report statuses, latest order failures, Hermes jobs, CNPG health, RustFS backup status, and ngrok route health.
- Add smoke tests after deploy: health, overview, scheduler status, Saxo session status, decision schema validation, Hermes API auth check, and MCP tool list.
- Add a post-deploy guard that confirms the live image tag changed on API, scheduler, MCP, and Hermes where expected.
- Add backup/restore rehearsal docs for CNPG + RustFS.
- Add alerting for repeated decision-report failures, repeated broker execution failures, stale scheduler heartbeat, and missed EOD reflection.
- Keep public ngrok route ownership in the shared gateway and app-owned internal AgentEndpoint ownership in this repo.

## Security And Secrets

Security posture should assume model prompts, broker payloads, and external docs are untrusted.

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
