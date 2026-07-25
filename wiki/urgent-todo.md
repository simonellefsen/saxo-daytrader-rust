---
type: capability
tags:
  - daytrader/wiki
  - roadmap
  - urgent
  - maintained-by-llm
updated: 2026-07-25
---

# Daytrader Urgent Todo

This page holds the small set of items where current evidence says the system is exposed **now**. It is deliberately short and ranked. [roadmap](roadmap.md) remains the long-horizon planning map; this page is the subset that should not wait for its turn there. Move an item to the roadmap's `Recently Landed` section once it lands, and delete its row here.

Entry criteria for this page: a verified gap between what the runtime claims or is configured to do and what it actually enforces, or an exposure that grew because of a recent change. Speculative improvements belong in the roadmap.

Reviewed 2026-07-25 against `config.yaml`, `src/*.rs`, and the current roadmap.

## Ranked Items

| # | Item | Why now | Exit criteria |
| --- | --- | --- | --- |
| U1 | Finish broker-hosted protective stops (SIM-first) | Downside protection currently depends entirely on the model proposing a SELL at one of two weekday pulses and that SELL clearing the technical gate. Nothing exits between pulses, and nothing exits across a weekend. The 2026-07-25 work landed a read-only coverage audit plus a manually confirmed SIM lifecycle test; the automatic path after a confirmed BUY fill is still unbuilt. `strategy.ladder.submit_stop_loss_after_fill` is still `false` and unread; `stop_loss_atr_multiple` (2.0) was equally dead until slice 1 wired it into the proposed stop level. The roadmap's own live evidence records a −24.4k DKK week in which the defensive exits did not fire. | **Slice 1 landed 2026-07-25:** `strategy.ladder.stop_loss_atr_multiple` is now live — the coverage audit computes a concrete proposed stop per unprotected position from stored close and ATR14, sized to the uncovered quantity only, and the Execution table shows it beside each exception. Read-only: no Saxo call, nothing placed, not tick-normalized. **Slice 2 (operator, blocking):** `protective_stop_prechecks` and `protective_stop_lifecycle_tests` are both still empty — the broker path has never executed once. Run the precheck, then one SIM placement/cancel/reconcile, using the proposed levels. **Slice 2b landed 2026-07-25:** operator-confirmed bulk placement from the exceptions table, sequential, 1.1s spacing for Saxo's 1 order/second limit, halting the whole batch on the first rejection, error, or ambiguous response. Stale `placement_preparing` orphans are reconciled against Saxo and abandoned only when the broker never saw them. **Slice 3 — full automation, designed below in [Automatic Protective Stops](#automatic-protective-stops).** Default to `Stop`, not `StopLimit`. A stop cannot guarantee price through a gap. |
| U2 | Config-contract audit | Several risk keys in `config.yaml` have zero references in `src/` — see [Unwired Risk Configuration](#unwired-risk-configuration). This is the same failure class as the retired 2026-05-05 `cash_buffer` override, but silent by construction: an unwired key is indistinguishable from an enforced one when reading the config. | Audit landed 2026-07-25 (`src/config_contract.rs`): every key is classified, unused risk keys appear in the Overview integrity payload, and a key added without a contract entry raises a warning. **Remaining:** each of the 27 risk-surface keys needs an operator decision to implement or delete. |
| U3 | Reconcile the Hermes goal contract with enforced reality | `AppState::hermes_goal_contract_value` sends Hermes `max_drawdown: 0.20`, `min_sharpe: 1.0`, `slippage_tolerance: 0.02`, and `max_positions` as *constraints*. Only the monthly-loss DKK floors are enforced in `src/trading_manager.rs`. Portfolio drawdown is computed (`src/state.rs:731`) but consumed only by display and evidence packs, so Hermes reasons about a risk envelope the runtime does not apply. The return half of this was fixed 2026-07-25 — see [Return Goal](#return-goal). | No field in the goal contract states a constraint the runtime does not enforce. Either implement the guardrail or relabel the field as an aspirational objective. See the roadmap `Risk governance` row for the drawdown-guard shape. |
| U7 | Hermes can experiment on a dead variable | `deploy/k8s/base/hermes.yaml` lists `strategy.swing.cash_buffer_pct` in `experiment_policy.supported_variables`, and the config-contract audit proves nothing reads it. Hermes can therefore propose, run in SIM, observe, and promote a one-variable experiment whose variable has no effect — and attribute whatever the portfolio did to it. | Cross-check `supported_variables` against the config contract; a variable classified `unused` cannot be a supported experiment variable. Ideally enforced by a test so the two lists cannot drift. |
| U4 | Prompt-injection screen for editorial research | The editorial-research path feeds third-party RSS titles and summaries into the decision prompt (`src/xai_decision.rs`) and into Hermes context. `normalize_text` (`src/editorial_research.rs`) strips HTML tags and collapses whitespace; it does not screen for instruction-shaped content. Every earlier prompt input — Markov, daily indicators, Quiver — is numeric and runtime-computed, so this is the first attacker-influenceable free text in the pipeline. It should be hardened before the configured feed catalog expands to Yahoo Finance, CNBC, and Reuters. | A deterministic instruction-pattern screen drops or flags items; feed text is structurally delimited and labelled untrusted where it enters the prompt; a regression test asserts an injection-shaped summary is rejected. |
| U5 | CI on every push | The suite is 324 `#[test]`/`#[tokio::test]` functions across 24 modules and ran only when someone typed `make validate`. Deploy provenance was hardened on 2026-07-13 so the intended commit ships, but nothing verified that commit's tests pass. | Landed 2026-07-25: `.github/workflows/ci.yml` runs `cargo fmt --check`, `cargo check --all-targets`, and `cargo test` with `-D warnings` on push, pull request, and manual dispatch. **Remaining:** the first Actions run, which needs the embedded token removed from the `origin` remote first. |
| U8 | `strategy_type` is never set on Trading Manager orders | `CandidateOrder::from_json` (`src/trading_manager.rs:1904`) reads `strategy_type` out of the model's suggested-trade JSON, but the decision-report schema has no such field — the string `strategy_type` does not appear anywhere in `src/xai_decision.rs`. So it is NULL on every order the Rust Trading Manager has ever queued: 101 of 156 rows, most recent 2026-07-23. The three populated values come from other paths (`portfolio_sync` and `clean_reconciliation` from `src/portfolio_reset.rs`, `manual` from the manual order path). Two live consequences: the Execution table renders `fallback_text(row, "strategy_type", "manual")` (`src/ui.rs:4261`), so **every automated order is displayed to the operator as "manual"**; and `execution_source_label` (`src/notifications.rs:1476`) falls through to "Execution" instead of "Trading Manager" in Slack. See [Orphaned strategy_type](#orphaned-strategy_type). | Landed 2026-07-25. The runtime now sets `TRADING_MANAGER_STRATEGY_TYPE` (`swing`) at insert and ignores any value the model supplies; a startup backfill scoped to `report_id IS NOT NULL` repaired the 101 historical rows; the UI fallback is now `unknown` rather than `manual`. Two tests cover it, including one asserting the backfill cannot touch adoption, reconciliation, or manual rows. |
| U6 | Saxo rate-limit pacing for the unlimited nightly runs | `strategy.markov.max_symbols` and `strategy.swing.daily_indicators.max_symbols` were both raised to `0` (unlimited, ~199 symbols) on 2026-07-16. The roadmap's `Rate-limit-aware throttling` row — written while the cap was 20 — documents Saxo's 120 requests/minute per session per service group and estimates ~200 sequential chart calls for the Markov run alone. Two unlimited jobs now run back-to-back at 23:30 and 23:45 against the same limit, and only the Markov path retries 429s. | Token-bucket pacing to roughly 100 requests/minute per service group, driven by the `X-RateLimit-*` response headers, shared by the Markov and daily-indicator paths. Best delivered with the roadmap's unified Saxo HTTP client row. |

## Return Goal

Resolved 2026-07-25. The operator's actual target is **+10-20% per year**; the configured goals stated three different things, all far above it.

| Source | Was | Implied annual (on a ~304,000 DKK book) |
| --- | --- | --- |
| `xai.performance_goals.monthly_target_dkk` | 20,000 DKK | ~+115% |
| `xai.performance_goals.weekly_target_dkk` | 5,000 DKK | ~+137% |
| Hermes goal contract `target_return_30d` | 0.47 with the note "10x in 6 months" | ~+10,000% |

The Hermes objective was roughly 70x the operator's target, which matters beyond documentation: `promote_only_if.return_30d_gte` used the same 0.47, so no experiment could ever clear the promotion bar on merit, and every reflection was measured against a return only reachable by taking far more risk than the loss floors permit.

Now set to **+15%/year** (the midpoint) in all five places that carried a copy — `config.yaml`, `deploy/k8s/base/config.k8s.yaml`, `deploy/k8s/base/hermes.yaml`, `docs/hermes-agent.md`, and `AppState::hermes_goal_contract_value` — as `target_return_30d: 0.0117`, 880 DKK/week, 3,800 DKK/month, `goal_version: 2`, and `failure_below_30d_return: -0.02`.

The monthly loss floors were rescaled with it: the old -25,000/-50,000 pair was -8.2%/-16.4% of the portfolio in a single month, so one bad month could erase roughly a year of target gains before the hard halt fired. Now **-9,000 soft (-3%) and -18,000 hard (-6%)**, preserving the 2:1 ratio.

Two follow-ups:

- The DKK targets encode a percentage against a ~300,000 DKK book. They drift silently as the portfolio grows or shrinks. Consider deriving them from portfolio value instead of hardcoding DKK, or add a review reminder.
- `max_drawdown: 0.20` in the goal contract is now loose relative to a 15%/year target and is still unenforced. Revisit it with U3.

## Orphaned strategy_type

Reference for U8. Found 2026-07-25 while investigating the unreconciled-orders false positive, which is itself the second consumer of this column.

Production `execution_orders` on 2026-07-25:

| `strategy_type` | Rows | Oldest | Newest | Written by |
| --- | --- | --- | --- | --- |
| `NULL` | 101 | 2026-05-12 | **2026-07-23** | Rust Trading Manager |
| `clean_reconciliation` | 27 | 2026-05-13 | 2026-05-14 | `src/portfolio_reset.rs` |
| `portfolio_sync` | 19 | 2026-05-05 | 2026-05-05 | portfolio adoption |
| `swing` | 6 | 2026-05-06 | 2026-05-07 | legacy Python runtime |
| `manual` | 3 | 2026-06-10 | 2026-06-10 | manual order path |

The `swing` rows stop on 2026-05-07 and the NULLs begin on 2026-05-12. That gap is the Python-to-Rust port: the legacy runtime set the column, the Rust Trading Manager never has. The stored timestamp format corroborates it — populated legacy rows use `+00:00`, NULL rows use `Z`.

This is not stale legacy data to be cleaned up once. It is an active defect: every order queued since the port is affected, including orders from two days ago.

Why it happened: `strategy_type` is read from the model's response rather than set by the runtime, and the field was never part of the report schema. The neighbouring `strategy_key` avoided this because `unique_strategy_key` builds it locally. The lesson generalizes — provenance should be recorded by the component that knows it, never requested from the model.

Why it matters beyond a blank column:

- Every automated order is labelled **"manual"** in the Execution table, which inverts the most important fact about an order's provenance.
- Slack execution alerts say "Execution" rather than "Trading Manager".
- The roadmap's "realized and unrealized attribution by decision pulse" item is unbuildable on this column until it is fixed and backfilled.
- It is now load-bearing: the 2026-07-25 unreconciled-orders fix keys its adoption exclusion on `COALESCE(strategy_type, '') <> 'portfolio_sync'`. That is correct today precisely because adopted rows are among the *populated* ones — but a second consumer of an unreliable column is a warning sign, not a pattern to repeat.

## Automatic Protective Stops

Design for U1 slice 3, from the operator requirement on 2026-07-25: stops must be fully automatic, adjusted on every trade, and must ratchet upward as a position appreciates. The Trading Manager must also learn when a stop fills.

**1. Stops belong in `execution_orders`, not the lifecycle-test table.** This is the load-bearing decision. `sync_saxo_broker_orders` already runs twice per scheduler cycle — every 10 minutes, dropping to 1 minute while `outstanding_order_count > 0` — but reads `execution_orders` only, and `protective_stop_lifecycle_tests` appears zero times in that path. A stop filling there today produces no ledger row, no position update, and no Trading Manager awareness. Moving stops into `execution_orders` inherits broker sync, fast polling, fill reconciliation, the trade ledger, and the coverage audit with no new plumbing. The lifecycle-test table stays what it was built for: a one-off validation harness outside the queue.

**2. Placement trigger.** On a reconciled BUY fill, queue a GTC SELL `Stop` for the filled quantity at `close - (ATR14 x stop_loss_atr_multiple)`, tick-normalized at build time. Gate on `strategy.ladder.submit_stop_loss_after_fill`, which is currently dead config — wiring it moves it from `unused_risk` to `enforced` in the config contract.

**3. Quantity changes.** Any fill that changes a position's size must amend or replace the stop so covered quantity tracks the holding. A partial SELL leaves an oversized stop; a follow-on BUY leaves an undersized one. Both are coverage failures the audit would report but nothing would fix.

**4. Trailing ratchet.** `strategy.ladder.trail_stop_atr_multiple` is another dead key. As the position appreciates, raise the stop to `high_water_close - (ATR14 x trail_multiple)`. The ratchet must be strictly monotonic — a stop may only ever move up for a long — and should act on the nightly indicator run rather than per quote, to avoid churning replace requests against Saxo's rate limits.

**5. Fast-poll exclusion.** A resting GTC stop keeps `outstanding_order_count` above zero indefinitely, which would pin the scheduler at 1-minute polling forever. The fast-poll trigger must exclude resting protective stops, or this change quietly turns a 10-minute cadence into a permanent 1-minute one.

**6. SELL reservation conflict.** A stop covering the full position reserves that quantity, which would block the model's own discretionary exits. Either exclude protective stops from the reservation calculation, or cancel-then-replace around a discretionary SELL. This is the parent/child linkage the roadmap flagged as separate design work and remains the hardest part.

**7. Lower latency, later.** ENS activity streaming (already on the roadmap) takes fill-to-ledger from minutes to seconds. Polling is the correct first implementation; streaming is an optimisation once stops are proven.

Sequencing note: none of this should land before one placement has completed and reconciled end to end against Saxo, which slice 2b now makes reachable in a single operator action.

## Unwired Risk Configuration

Reference for U2. Established by extracting every `strategy.*`, `risk.*`, and `taxation.*` config access path from `src/*.rs` — both the `&["a", "b", "c"]` slice form and chained `.get("a").and_then(...)` — and comparing against the leaf keys in both shipped configs. Leaf-name grep alone over-counts badly (`cash_buffer_pct` appears to have 54 hits because it is a substring of `min_cash_buffer_pct`), so trust path extraction, not name matching.

The audit implemented for U2 reports this automatically. Against `config.yaml` on 2026-07-25: **20 enforced, 30 advisory, 44 unused, 27 of them risk-surface**, 0 uncontracted, 8 contracted-but-absent.

The 27 keys that read as active risk controls and are not:

| Config key | Note |
| --- | --- |
| `strategy.enabled` | No master switch is read. Setting it to `false` does not stop the strategy cycle. |
| `strategy.swing.trading_manager.enabled` | No switch is read. Setting it to `false` does not stop the Trading Manager. |
| `strategy.swing.analysis_pulses.europe_open_followup.enabled` / `us_open_followup.enabled` | Neither pulse switch is read. A disabled pulse still runs. |
| `strategy.swing.risk_per_trade_pct` | Position sizing is not risk-based; quantity comes from the model suggestion bounded by budget, minimum trade value, and the commission floor. |
| `strategy.max_assets_per_sector` | No concentration gate exists; the string `sector` does not appear anywhere in `src/`. |
| `strategy.estimated_slippage_bps`, `strategy.cost_guard_multiple` | No cost model consumes either. |
| `strategy.min_selected_assets`, `strategy.max_selected_assets` | No breadth bound is enforced. |
| `strategy.swing.min_holding_weight_pct`, `max_holding_weight_pct`, `strategy.ladder.min_position_weight`, `max_position_weight`, `risk.max_position_weight` | Five separate per-position weight caps, none enforced. |
| `strategy.swing.cash_buffer_pct` | The third cash-buffer path and the second dead one. Only `strategy.capital.min_cash_buffer_pct` bounds the BUY budget; `strategy.capital.cash_buffer` was retired 2026-07-22. The Hermes `cash_buffer_policy` related-family map describes two paths; there are three. |
| `strategy.swing.trading_manager.max_symbols` | No per-run candidate cap is applied from configuration. |
| `strategy.ladder.submit_stop_loss_after_fill`, `submit_take_profit_after_fill`, `submit_bracket_with_entry`, `stop_loss_atr_multiple`, `trail_stop_atr_multiple` | No protective stop, take-profit, bracket, or trailing stop is ever placed. Directly related to U1. |
| `strategy.ladder.session_flatten_enabled`, `flatten_minutes_before_tradable_close` | No session flatten runs; nothing exits on a schedule. |
| `risk.excluded_symbols_csv` | Only the list form is read. Exclusions supplied through `RISK_EXCLUDED_SYMBOLS` have no effect. |
| `strategy.swing.journal.benchmark_indices` | No benchmark comparison is computed in Rust; performance is reported without one. |
| `taxation.share_income.brackets` | `estimated_tax_dkk` is hardcoded to `0.0` at `src/state.rs:2858`, so after-tax P/L equals pre-tax P/L and goal progress is overstated. |

A further 17 keys are unused without implying a missing safeguard (`strategy.mode`, `max_candidates`, `min_holdings`, the remaining `ladder.*` entries, the unported weekly/monthly journal cycle, `trading_manager.use_ai`, `trading_manager.due_window_minutes`, `analysis_pulses.timezone`, `taxation.share_income.currency`).

Two further config divergences the audit surfaced:

- `strategy.swing.trading_manager.max_report_age_hours` is read by the Trading Manager but supplied by **neither** shipped config, so report freshness runs on the code default.
- `strategy.quiver.*` is read by `src/quiver.rs` but exists only in `deploy/k8s/base/config.k8s.yaml`, so a local run silently uses different Quiver defaults than production.

Enforced-by-contrast examples: `strategy.capital.min_cash_buffer_pct`, `strategy.capital.max_deployment_pct`, the `markov_gate` thresholds, `daily_indicators.min_confluences`/`min_reward_risk`, and `strategy.swing.never_trade_symbols` (`src/trading_manager.rs:3323`).

## Related Pages

- [roadmap](roadmap.md) — full improvement map, including the longer-horizon shape for U3, U4, and U6.
- [runbooks/build-test-deploy](runbooks/build-test-deploy.md) — the manual validate/deploy checklist that U5 automates.
- [concepts/hermes-self-improvement](concepts/hermes-self-improvement.md) — goal-contract and experiment governance context for U3.
