---
type: capability
tags:
  - daytrader/wiki
  - roadmap
  - urgent
  - maintained-by-llm
updated: 2026-08-02
---

# Daytrader Urgent Todo

This page holds the small set of items where current evidence says the system is exposed **now**. It is deliberately short and ranked. [roadmap](roadmap.md) remains the long-horizon planning map; this page is the subset that should not wait for its turn there. Move an item to the roadmap's `Recently Landed` section once it lands, and delete its row here.

Entry criteria for this page: a verified gap between what the runtime claims or is configured to do and what it actually enforces, or an exposure that grew because of a recent change. Speculative improvements belong in the roadmap.

Reviewed 2026-07-25 against `config.yaml`, `src/*.rs`, and the current roadmap.

Re-reviewed **2026-08-02** against live production data (Postgres), the live Saxo SIM API, and the Saxo OpenAPI reference documentation. U9–U15 come from that pass. Two of them (U9, U11) are conditions that already exist in production rather than latent risks, and U9 is the only item on this page whose next step is an operator decision rather than a code change.

## Ranked Items

| # | Item | Why now | Exit criteria |
| --- | --- | --- | --- |
| U1 | Finish broker-hosted protective stops (SIM-first) | Downside protection currently depends entirely on the model proposing a SELL at one of two weekday pulses and that SELL clearing the technical gate. Nothing exits between pulses, and nothing exits across a weekend. The 2026-07-25 work landed a read-only coverage audit plus a manually confirmed SIM lifecycle test; the automatic path after a confirmed BUY fill is still unbuilt. `strategy.ladder.submit_stop_loss_after_fill` is still `false` and unread; `stop_loss_atr_multiple` (2.0) was equally dead until slice 1 wired it into the proposed stop level. The roadmap's own live evidence records a −24.4k DKK week in which the defensive exits did not fire. | **Slice 1 landed 2026-07-25:** `strategy.ladder.stop_loss_atr_multiple` is now live — the coverage audit computes a concrete proposed stop per unprotected position from stored close and ATR14, sized to the uncovered quantity only, and the Execution table shows it beside each exception. Read-only: no Saxo call, nothing placed, not tick-normalized. **Slice 2 (operator, blocking):** `protective_stop_prechecks` and `protective_stop_lifecycle_tests` are both still empty — the broker path has never executed once. Run the precheck, then one SIM placement/cancel/reconcile, using the proposed levels. **Slice 2b landed 2026-07-25:** operator-confirmed bulk placement from the exceptions table, sequential, 1.1s spacing for Saxo's 1 order/second limit, halting the whole batch on the first rejection, error, or ambiguous response. Stale `placement_preparing` orphans are reconciled against Saxo and abandoned only when the broker never saw them. **Slice 2c landed 2026-07-25:** the scheduler confirms placements read-only each cycle, which promoted all nine stragglers from `placement_submitted` to `broker_working` on its first run; all twelve positions are now broker-confirmed protected. **Slice 3a landed 2026-07-25** — stops became real `execution_orders` rows, so a fill is visible, and they yield to a decided sell (points 1, 5, 6). **Slice 3b landed 2026-07-26** — one declarative sweep places, re-sizes, and ratchets every held position's stop each cycle (points 2, 3, 4). U1 is complete except point 7, ENS streaming, which is a latency optimisation and belongs on the roadmap. Default to `Stop`, not `StopLimit`. A stop cannot guarantee price through a gap. |
| U2 | Config-contract audit | Several risk keys in `config.yaml` had zero references in `src/` — see [Unwired Risk Configuration](#unwired-risk-configuration). This is the same failure class as the retired 2026-05-05 `cash_buffer` override, but silent by construction: an unwired key is indistinguishable from an enforced one when reading the config. | **Landed 2026-07-27.** The audit classifies every shipped key, reports uncontracted additions, and surfaces unused risk keys in Overview integrity. The final inactive bracket/take-profit switches were deleted, reducing the shipped unused risk-surface inventory from 27 to 0. New risk settings now require an explicit runtime contract entry and regression coverage. |
| U3 | Reconcile the Hermes goal contract with enforced reality | `AppState::hermes_goal_contract_value` sent Hermes `max_drawdown: 0.20`, `min_sharpe: 1.0`, `slippage_tolerance: 0.02`, and `max_positions` as *constraints*. Only the monthly-loss DKK floors were enforced in `src/trading_manager.rs`. Portfolio drawdown was computed but consumed only by display and evidence packs, so Hermes reasoned about a risk envelope the runtime did not apply. The return half of this was fixed 2026-07-25 — see [Return Goal](#return-goal). | Landed 2026-07-26. `src/drawdown_guard.rs` makes `max_drawdown` real: a soft band reduces the cycle BUY budget, a hard floor suspends new BUYs, SELLs are never blocked. The contract now reads its limit from `strategy.capital.drawdown_halt_pct`, the same key the gate applies, so the advertised and enforced numbers cannot drift. `strategy.swing.max_holdings` now applies the published `max_positions` constraint to new-symbol BUYs using persisted positions plus same-cycle reservations. Every remaining objective and constraint carries an explicit `enforcement` status (`runtime_enforced` / `evaluation_only` / `structural` / `documentation` / `not_enforced`), and a test fails the build if a field is added without one. `gas_reserve` was deleted as a crypto-template leftover. **Remaining debt, now named rather than implied:** `slippage_tolerance` and `require_backtest_before_activation` are declared `not_enforced`. |
| U7 | Hermes can experiment on a dead variable | `deploy/k8s/base/hermes.yaml` listed `strategy.swing.cash_buffer_pct` in `experiment_policy.supported_variables`, and the config-contract audit proved nothing reads it. Hermes could therefore propose, run in SIM, observe, and promote a one-variable experiment whose variable had no effect — and attribute whatever the portfolio did to it. | Landed 2026-07-25, fully reconciled 2026-07-26: `strategy.swing.cash_buffer_pct` is removed from the Rust capabilities payload, Kubernetes ConfigMap, shipped configs, reflection prompts, runbooks, Trading Manager overlay loader, and historical runtime settings. The legacy Python reference now also reads the active `strategy.capital.min_cash_buffer_pct` reserve. `SUPPORTED_EXPERIMENT_VARIABLES` is the runtime source for overlay acceptance and is cross-checked against the config contract by test, so a variable classified `unused` cannot be offered or applied. Verified the guard rejects the retired path and other unpublished variables. |
| U4 | Prompt-injection screen for editorial research | The editorial-research path feeds third-party RSS titles and summaries into the decision prompt (`src/xai_decision.rs`) and into Hermes context. `normalize_text` (`src/editorial_research.rs`) strips HTML tags and collapses whitespace; it does not screen for instruction-shaped content. Every earlier prompt input — Markov, daily indicators, Quiver — is numeric and runtime-computed, so this is the first attacker-influenceable free text in the pipeline. It should be hardened before the configured feed catalog expands to Yahoo Finance, CNBC, and Reuters. | Landed 2026-07-25, and none too soon — the feature began ingesting live items into the decision prompt that morning. A narrow marker list detects text addressed at a model rather than a reader; flagged items are retained for operator review but excluded at the context boundary, which also covers rows stored before screening existed. The prompt now carries an explicit security-boundary instruction labelling the section untrusted. Two tests: one proves injection-shaped text never reaches the prompt, the other proves ordinary market language ("upgrade to Buy", "sell off", "disregard one month of data") is not flagged. |
| U5 | CI on every push | The suite ran only when someone typed `make validate`; deploy provenance alone could not prove the intended commit passed tests. | **Landed 2026-07-25 and verified 2026-07-26.** `.github/workflows/ci.yml` runs `cargo fmt --check`, `cargo check --all-targets`, and `cargo test` with `-D warnings` on push, pull request, and manual dispatch. `origin` uses SSH, and the stale global HTTPS OAuth rewrite was removed on 2026-07-26; see [Credential Hygiene](#credential-hygiene). |
| U8 | `strategy_type` is never set on Trading Manager orders | `CandidateOrder::from_json` (`src/trading_manager.rs:1904`) reads `strategy_type` out of the model's suggested-trade JSON, but the decision-report schema has no such field — the string `strategy_type` does not appear anywhere in `src/xai_decision.rs`. So it is NULL on every order the Rust Trading Manager has ever queued: 101 of 156 rows, most recent 2026-07-23. The three populated values come from other paths (`portfolio_sync` and `clean_reconciliation` from `src/portfolio_reset.rs`, `manual` from the manual order path). Two live consequences: the Execution table renders `fallback_text(row, "strategy_type", "manual")` (`src/ui.rs:4261`), so **every automated order is displayed to the operator as "manual"**; and `execution_source_label` (`src/notifications.rs:1476`) falls through to "Execution" instead of "Trading Manager" in Slack. See [Orphaned strategy_type](#orphaned-strategy_type). | Landed 2026-07-25. The runtime now sets `TRADING_MANAGER_STRATEGY_TYPE` (`swing`) at insert and ignores any value the model supplies; a startup backfill scoped to `report_id IS NOT NULL` repaired the 101 historical rows; the UI fallback is now `unknown` rather than `manual`. Two tests cover it, including one asserting the backfill cannot touch adoption, reconciliation, or manual rows. |
| U9 | Portfolio is 1.4% away from a full BUY halt | The drawdown guardrail landed 2026-07-26 and has been in its soft band ever since, but the band has only tightened: 16.57% (07-29) → 17.27% → 18.25% → 19.04% → **18.999% on the last run (07-31)** against a 20.00% halt. Peak 297,463 DKK (2026-06-30), current 241,281. A halt fires at 237,970 — **3,311 DKK away, or one −1.4% day**. The guardrail is working exactly as designed; the point is that nobody has decided what happens when it trips. See [Drawdown Approach](#drawdown-approach). | Operator decision, not a code change, and it should be made before the halt rather than during it. Three options: accept the halt (SELLs continue, so the book drains to cash), widen `strategy.capital.drawdown_halt_pct`, or grant a scoped override with a recorded peak. Doing nothing is also a choice — it just means the choice gets made by the market on an arbitrary morning. |
| U10 | The book is 63% unhedged USD and the runtime reports its FX exposure as exactly zero — **measurement landed 2026-08-02, policy open** | Live exposures: **USD 87,892 DKK (63%)**, DKK 41,889 (30%), NOK 9,323 (7%). USD/DKK fell from 7.0215 (07-02) to 6.4837 (08-02) — **−7.66% in one month** — against a book whose reporting currency is DKK. `trade_ledger.fx_gain_dkk` is a **hardcoded `0` literal** in the `INSERT` at `src/saxo_order.rs:2506`, and `price_gain_dkk` is bound to `realised_gain_dkk`, so every sale reports 100% price / 0% FX by construction. The real formula exists only in the retired Python (`src/saxo_daytrader_xai/tax_engine.py:273`). Currency is the one exposure nothing gates, nothing measures, and no signal source (Markov, Quiver, indicators, editorial) observes. See [Currency Exposure](#currency-exposure). **Measurement landed 2026-08-02.** `crate::fx::split_realised_gain` decomposes the realised gain exactly: price is `(net_local − cost_local) × sale_rate`, currency is `cost_local × (sale_rate − cost_rate)`, and the two reconstruct the total with no cross-term stranded — a test asserts the identity on four real production rows in both directions. A second defect surfaced while fixing the first: `realised_gain_local` was `realised_gain_dkk / sale_rate`, which is the DKK gain restated at the sale rate rather than the gain in local terms, making the split circular; it is now the genuine local figure. A startup backfill recomputes historical rows **from columns already stored on each row**, so it involves no rate lookup and cannot drift with today's FX. It derives the cost rate as `cost_basis_sold_dkk / cost_basis_sold_local` rather than reading `cost_basis_fx_rate_to_dkk`, deliberately: that column is 100x too small on pre-2026-07-09 rows, and trusting it would have invented ~+3,095 DKK of currency gain on one 2,356 DKK profit. It fails to the old behaviour rather than to nonsense — no usable local cost basis means the whole gain stays classified as price. A **plausibility guard** was added after the first backfill run exposed the real hazard: with a corrupt cost basis the split stays internally exact while being entirely fictional. Production holds derived cost rates of `128.2545` and `31.8992` against a ~7.02 sale rate, plus exact zeros; attributing currency from those would have produced large, confident, wrong numbers. A cost rate outside a 2x band either side of the sale rate is refused, because no currency in this book moves by half between purchase and sale. 41 of 45 sales attribute cleanly, 3 are refused as corrupt, 1 has no local basis. The FX cache-staleness half of this — `static_fx_rate_to_dkk` (`src/fx.rs:29`) hardcodes **USD at 7.0215** and was being reached silently on every conversion for over two days — was its own defect with its own root cause; **see U16, landed 2026-08-02.** **Remaining:** surface currency concentration next to the drawdown guardrail and decide whether currency becomes a gate input (`strategy.concentration.max_assets_per_currency` is wired but unlimited) — a policy choice, but the measurement it needs now exists. |
| U16 | ~~The FX cache went stale for two days straight and every conversion fell back to an 8%-off literal, silently~~ **Landed 2026-08-02** | Found while closing U10. `currency_fx_rates` for all six major pairs (USD, EUR, GBP, NOK, SEK, PLN) last refreshed **2026-07-31T19:39:20Z** with a 30-minute TTL — over two days stale in production. Root cause: `refresh_best_effort_fx_rates` was only ever called from `refresh_portfolio_prices` (`src/price_monitor.rs:198`), which returns early — before reaching that call — whenever every watched exchange is closed (`market_closed`, `src/price_monitor.rs:176`) or the Saxo session is unavailable (`no_session`, line 142). FX trades nearly continuously; the equity exchanges this runtime watches do not, so a plain weekend was enough to silently exhaust the cache. `cached_or_static_fx_rate_to_dkk` then fell through to `static_fx_rate_to_dkk` (`src/fx.rs:29`), a literal pinned to 2026-07-02 — **USD 7.0215 against a live 6.4837, ~8% off** — used in every `trade_ledger` write, `hist`-facing valuation, and drawdown/exposure figure, with no signal anywhere that it had happened. | **Landed 2026-08-02.** `crate::fx::run_fx_rate_refresh_cycle` is now called unconditionally from the main scheduler cycle (`src/scheduler.rs`), which runs every 10 minutes — 1 while orders are outstanding — independent of market hours or weekday, before any broker or ledger read in the same cycle. The existing 30-minute cache TTL is what actually throttles the Saxo network call, so this keeps rates within about half an hour of live, tighter than the hourly cadence asked for, without a second cadence to maintain. A missing or expired Saxo session now degrades straight to the ECB daily fallback rather than skipping the step entirely, so a broken session — which the runbooks say can last hours — no longer also stops FX from updating. The price-monitor call site is untouched and still runs its own refresh when a market is open; the two are redundant by design, not competing. `static_fx_rate_to_dkk` remains as a true last-resort default and was deliberately left un-updated: the fix is that the cache should now almost never be empty enough to reach it. |
| U11 | ~~28 of 201 universe symbols have been permanently unanalysable, including all of Stockholm~~ **Landed 2026-08-02** | Every Markov and daily-indicator run for weeks reports exactly `201 assets / 173 success / 28 error`, all `No tradable Saxo instrument match found`, all negative-cached. Verified live against Saxo SIM: **every one is our own symbol mapping, not a Saxo limitation.** 18 are `:xsto` — Saxo's Stockholm suffix is **`:xome`** (`ERICb:xome`, `VOLVb:xome`, `ABB:xome`, `TELIA:xome` all resolve immediately). Separately, `exchange_id_for_suffix` (`src/markov_method.rs:1440`) returns **MICs** where Saxo's `ExchangeId` is a proprietary code — `XSTO` returns nothing, `SSE` returns Volvo; `XNAS`→`NASDAQ`, `XCSE`→`CSE`, `XETR`→`FSE`. **All 15 entries are wrong**, which makes the exchange-scoped fallback dead code that has never matched once. See [Instrument Resolution](#instrument-resolution). **Landed 2026-08-02.** All 27 correctable symbols fixed in both configs; each replacement was verified individually against the live SIM `/ref/v1/instruments` endpoint rather than inferred. `exchange_id_for_suffix` now returns Saxo's real `ExchangeId` codes, pinned by a test that also asserts no entry returns its own MIC — the specific mistake that made the fallback dead. `base_lookup_variants` now emits both Saxo share-class spellings (`ERICb` **and** `ESSITY_B`), because Saxo uses both and the symbol alone does not say which; that keeps the convention out of configuration, where it would need per-symbol maintenance. No negative-cache purge was needed: every corrected symbol is a new string, so none carries a cached failure. **Follow-up:** resolve the exchange map from `/ref/v1/exchanges` instead of hardcoding it. The data is *already in the database* — `saxo_exchange_snapshots` has held `code=XSTO, exchange_id=SSE, mic=XOME` since 2026-05-17, which is exactly the mapping whose absence caused this. The hardcoded table was duplicating, incorrectly, a correct fact the runtime already stored. **One trap for whoever does it:** the snapshot's `XNYS` row resolves to `exchange_id=AMEX`, `mic=XASE` — that is NYSE *American*, not NYSE, so a naive swap to the stored `exchange_id` would break NYSE lookups. Reconcile per code against `/ref/v1/exchanges` rather than trusting one row. Market-open detection is unaffected either way: it keys on `code`/`iso_mic` (`XSTO`), which remains correct, so `analysis_pulses.exchange_codes` should keep `XSTO` and was deliberately left alone. |
| U12 | The decision prompt has doubled in size and is mostly raw diagnostic data | Average `prompt_text`: **271 KB (May) → 429 KB (June) → 527 KB (July)**, max 696 KB — roughly 130k+ tokens per call, twice a weekday. The bulk is Markov `recent_labels`: the latest prompt carries **1,240 individual daily observations** (close, regime, rolling_return) and 1,350 `"close"` values across 20 symbols. That array is UI diagnostic data; the decision inputs are the current state, the transition distribution, and the conviction, all of which are already present separately. This is a live cost line and a plausible decision-quality problem — the model is handed 62 days of per-day labels per symbol and asked for a judgement. | Drop `recent_labels` and the raw per-day arrays at the prompt boundary while keeping them in storage for the UI. Expect a 40–60% prompt reduction. Measure win rate and suggestion quality before and after rather than assuming it is purely a saving. |
| U13 | ~~Statistics are 33 days stale, so the query planner is working from wrong row counts~~ **Landed 2026-08-02/03** | Every `last_autoanalyze` in `pg_stat_user_tables` was from **2026-06-30**. The planner believed `audit_log` had **0 rows** (it had 67,578), `trade_ledger` 47 (118), `decision_reports` 85 (137). Nothing was visibly slow yet because the tables are small, but plan choice was arbitrary and this is the failure mode that appears suddenly under growth rather than gradually. | **Landed.** `AppState::tune_append_heavy_table_autovacuum` runs an immediate `ANALYZE` and lowers `autovacuum_analyze_scale_factor` to `0.02` (from Postgres's 0.10 default) with `autovacuum_analyze_threshold = 50` on the nine append-heavy tables (`audit_log`, `decision_reports`, `trading_manager_runs`, `trade_ledger`, `execution_orders`, `execution_order_events`, `markov_asset_signals`, `scheduler_cycle_history`, `portfolio_value_history`). Deliberately **per-table `ALTER TABLE ... SET (...)`, not a CNPG Cluster-level default**: a cluster-wide `autovacuum_analyze_scale_factor` change needs a CNPG reconcile and touches every table including ones with no staleness problem, where per-table reloptions apply immediately via ordinary DDL and are scoped to the tables that actually grow this way. Guarded to Postgres only (`database_url_is_postgres`) since SQLite has no autovacuum and rejects the syntax; runs on every pod startup as part of the existing schema-migration function, idempotent like its neighbours. `audit_log` is included even though it is slated for deletion under U14/the Python removal plan — tuning it now costs nothing and covers the case where that deletion is delayed. |
| U14 | `audit_log` is 65 MB of dead Python exhaust — 38% of the database | Largest table in the database. **Nothing has been written since 2026-05-10**, the Python→Rust cutover; every row is `broker_*_refreshed` spam from the legacy runtime (15,717 rows each for balance/positions/exposures/account, at ~1 row per 30s). `seq_scan = 0`, `idx_scan = 1` — the Rust runtime neither writes nor reads it. No retention policy, no index beyond the pkey. | Drop the table with the Python removal (see [Python Removal Plan](#python-removal-plan)). If any audit obligation exists it should be re-established deliberately in Rust with a retention policy, not inherited from a dead process. |
| U15 | Useful Saxo endpoints that would replace things we compute less reliably ourselves | Verified live against SIM: **`/port/v1/closedpositions` → 200** (broker-authoritative realised P/L, an independent check on the ledger that just produced U10's zeroed FX split) and **`/hist/v4/performance/timeseries` → 200** (the broker's own account-value series back to 2021, against which our `portfolio_value_history` peak — the one driving U9's halt — could be validated). `/ca/v2/events` returns 403 in SIM, so corporate actions stay unverified: **nothing in the runtime handles a split, dividend, or merger**, and a split silently corrupts cost basis. Also unused: `/ref/v1/instruments/tradingschedule/{Uic}/{AssetType}` (authoritative session hours, incl. half-days) and the `mkt` service group. See [Unused Saxo Surface](#unused-saxo-surface). | Take these in value order, not API order: closed positions first (it validates the ledger), then corporate actions (it prevents silent corruption), then performance (it validates the guardrail input). Trading schedule and `mkt` are conveniences. |
| U6 | Saxo rate-limit pacing for the unlimited nightly runs | `strategy.markov.max_symbols` and `strategy.swing.daily_indicators.max_symbols` were both raised to `0` (unlimited, ~199 symbols) on 2026-07-16. Saxo documents 120 requests/minute per session per service group. Two unlimited jobs ran back to back at 23:30 and 23:45 against the same limit, and the only defence was a fixed 500 ms sleep in the Markov chart loop chosen when the cap was 20. | Landed 2026-07-26 (`src/saxo_rate_limit.rs`). Pacing is keyed by Saxo service group (the first path segment) and installed in the shared `saxo_get_json`, so the Markov and daily-indicator sweeps share one budget instead of guessing independently. **Even spacing, not a token bucket** — a bucket of 100 would let a sweep fire a hundred requests back to back and then stall, the burstiest way to spend the quota; 100/min becomes one request per 600 ms, already more conservative than the 500 ms sleep it replaces. The pacer also reads the `X-RateLimit-*` headers and derives spacing from remaining quota over remaining time, tightening on its own as quota depletes rather than waiting for a 429; the tightest reported dimension wins. **Scope limit:** state is per process, and the API and scheduler pods share one Saxo session, so they cannot see each other's usage. Both sweeps run in the scheduler, so the real exposure is covered. |

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
- `max_drawdown: 0.20` is enforced as of 2026-07-26 (U3). It remains loose relative to a 15%/year target; the soft band at 10% is what actually bites first.

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

**1. Stops belong in `execution_orders`, not the lifecycle-test table. — Landed 2026-07-25.** This was the load-bearing decision. `sync_saxo_broker_orders` already runs twice per scheduler cycle — every 10 minutes, dropping to 1 minute while `outstanding_order_count > 0` — but reads `execution_orders` only, and `protective_stop_lifecycle_tests` appeared zero times in that path. A stop filling there produced no ledger row, no position update, and no Trading Manager awareness.

`AppState::adopt_protective_stops_into_execution_orders` now runs each scheduler cycle, straight after the read-only confirmation step so a stop just promoted to `broker_working` is adopted in the same cycle. It writes local rows only — the broker order already exists — and is idempotent on `broker_order_id`, with a unique `strategy_key` as the second guard against two scheduler pods racing during a rollout. From adoption onward the stop is an ordinary `execution_orders` row and inherits broker sync, fill reconciliation, the trade ledger, position updates, and the execution alert. The lifecycle-test table stays what it was built for: the placement audit trail.

Adoption forced four other consumers to learn what a protective stop is, and each was a live defect waiting for the first adopted row:

- The stale-order integrity check flags anything `broker_working` for over 24 hours. A GoodTillCancel stop is *supposed* to rest for the life of the position, so every adopted stop would have become a permanent warning — the same false positive adopted positions produced before it was fixed earlier the same day, and a panel that always warns is a panel nobody reads. `RESTING_PROTECTIVE_STOP_EXCLUSION` scopes the age branch only; `broker_state_unknown` and executed-without-a-ledger-row still apply, because those are real faults for a stop too.
- `outstanding_order_count` drives the scheduler's fast poll. Twelve resting stops would have held it above zero forever and silently converted a 10-minute cadence into a permanent 1-minute one.
- `active_sell_reservations` would have counted each stop as reserving the whole holding, making every discretionary exit look impossible. See point 6.
- The instrument-quarantine scan treats any row with `error_text` as an instrument fault, and `update_order_broker_status` writes `error_text` for every `broker_cancelled` row. Since releasing a stop before a sell is routine, the runtime would have accumulated quarantine strikes against precisely the symbols it was trading *successfully*, and eventually refused to trade them. Quarantine is for instruments the broker keeps rejecting, not for our own housekeeping.

The pattern worth remembering: a new row type in a shared table inherits every query ever written against that table, including the ones that assume the old population.

**2, 3, 4. Placement, re-sizing, and the trailing ratchet. — Landed 2026-07-26 as one sweep.** These were designed as three triggers and became one declarative reconciliation, which is a better shape than the original plan. `run_automatic_protective_stop_sweep` compares each held position's desired protective state against its actual one every scheduler cycle and closes the gap. That single path covers a new BUY fill, a partial exit that left a residual holding, a stop released for a discretionary sell, and a placement that failed on an earlier cycle. Nothing hooks a specific fill event, so there is no event to miss while the process is restarting: a missed event is silent, a missed reconciliation is corrected on the next cycle.

`decide_stop_action` is a pure function and holds two invariants, enforced where the price is computed rather than at the call site:

- **A stop never moves down.** A replacement price is always at least the resting price, so the resting order is its own high-water mark and the ratchet is monotonic without a separately stored peak. Re-sizing after a partial exit therefore cannot quietly give back level.
- **A stop always sits below the last close.** A stop at or above the market fires on acceptance and converts protection into an unplanned market sell.

It fails closed. Missing or non-finite close/ATR, a sub-one-share position, or a computed level that is not below the close all yield `Hold`. An unprotected position is visible in the coverage audit; a stop placed at a fabricated level is not.

Guards on the sweep itself, because this is the first path in the runtime that places a broker order with nobody confirming it: gated on `strategy.ladder.submit_stop_loss_after_fill`; SIM-only, inherited from the verified-SIM-session check; skips exchanges not currently accepting orders, since a rejection for a shut market is indistinguishable at the broker from a real one and would halt the sweep for every symbol behind it; capped at five actions per cycle so a systemic fault cannot become an unbounded run of orders; and halts on the first failure rather than repeating a mistake down a list.

`min_ratchet_atr_fraction` (0.25) is new config. Without hysteresis, ATR drift would rewrite twelve broker orders a day for no protective gain, and each rewrite costs a real window in which the position carries no stop at all. Since the level derives from the nightly indicator run, the practical ceiling is about one replacement per symbol per day.

**Both multiples are 2.0, deliberately.** The config shipped `trail_stop_atr_multiple: 1.25` against `stop_loss_atr_multiple: 2.0`, and the first live sweep showed what that pair actually means: all twelve positions were sitting at exactly 2.00 ATR and every one of them came back ratchet-eligible on the first pass. They were idle only because the exchanges were shut. That would have cut breathing room across the whole book by 37.5% at the next open — caused by the relationship between two config values, not by a single position gaining ground, and 1.25 ATR sits inside a normal day's range on a swing horizon (AMD's stop would have moved 27 DKK/share against a daily ATR of 36.58).

Set to 2.0 on 2026-07-26 so a stop moves up only when the price makes new ground, which is what the operator requirement — "adjusted as the stock hopefully increases in value" — actually asks for. A test pins the semantics: with equal multiples a flat price holds and a one-ATR advance moves the stop by one ATR. Revisit the pair as a pair; a tighter trail is a legitimate choice, but it takes effect against every resting stop at once, not gradually.

**5. Fast-poll exclusion. — Landed 2026-07-25.** A resting GTC stop keeps `outstanding_order_count` above zero indefinitely, which would pin the scheduler at 1-minute polling forever. `outstanding_order_count` now excludes `strategy_type = 'protective_stop'`, with a test asserting a resting stop contributes zero.

**6. SELL reservation conflict. — Landed 2026-07-25.** A stop covering the full position reserves that quantity, which would block the model's own discretionary exits. It turned out Saxo settles the question for us: the `SellOrdersAlreadyExistForOwnedContracts` rejection observed during slice 2b proves the broker permits exactly one resting sell per owned holding, so a stop and a discretionary SELL genuinely cannot coexist and layering was never an option. Both halves therefore landed together: protective stops are excluded from `active_sell_reservations`, and `cancel_protective_stops_before_sell` clears the resting stop at the single chokepoint in `execute_order`, before the sell payload is built.

Three properties make that automatic cancellation defensible. It is scoped to rows this runtime marked `protective_stop` on exactly the symbol being sold. It does not trust Saxo's acceptance of the DELETE — an accepted cancellation is a request, not a completed state change — so it reads the order back and refuses to proceed while the stop is still working, failing the sell with a clear reason rather than letting the broker reject it. And it leaves both tables agreed, releasing the lifecycle-test row so `symbols_with_active_protective_stops` does not block the position from being re-protected afterwards.

This is the one automatic broker mutation the protective-stop machinery performs. The boundary set on 2026-07-25 — the scheduler may *observe* stop state, only a confirmed action may change it — still holds: the mutation here is not the scheduler acting on its own, it is a decided exit reclaiming the slot its own protection occupies. Standing protection yields to a decision; it is never cancelled on a timer or a hunch.

The gap this left after 3a — a discretionary sell cancels a stop and nothing re-protects the remainder — is closed by the sweep above, which sees the residual holding as unprotected on the next cycle.

**7. Lower latency, later.** ENS activity streaming (already on the roadmap) takes fill-to-ledger from minutes to seconds. Polling is the correct first implementation; streaming is an optimisation once stops are proven.

Sequencing note: none of this should land before one placement has completed and reconciled end to end against Saxo, which slice 2b now makes reachable in a single operator action.

## Unwired Risk Configuration

Reference for U2. Established by extracting every `strategy.*`, `risk.*`, and `taxation.*` config access path from `src/*.rs` — both the `&["a", "b", "c"]` slice form and chained `.get("a").and_then(...)` — and comparing against the leaf keys in both shipped configs. Leaf-name grep alone over-counts badly (`cash_buffer_pct` appears to have 54 hits because it is a substring of `min_cash_buffer_pct`), so trust path extraction, not name matching.

The audit implemented for U2 reports this automatically. After the 2026-07-26 automation-switch, exclusion, tax-estimate, risk-sizing, BUY-cost, candidate-ceiling, per-symbol exposure-cap, maximum-holdings, post-gate BUY-selection-cap, duplicate-cash-buffer retirement, explicit report-freshness-policy, Quiver-cadence, minimum-selection-floor, unsupported-sector-cap, duplicate-position-weight, session-flatten, legacy-benchmark, and inactive bracket/take-profit retirements, the deployed configuration has **34 enforced, 30 advisory, 26 unused, 0 unused risk-surface keys**, and 0 uncontracted. Local configuration carries the same Quiver policy rather than silently falling back to code defaults.

`strategy.ladder.submit_bracket_with_entry` and `submit_take_profit_after_fill` were retired on 2026-07-27 because Rust never implemented them. The automatic protective-stop controls and their ATR settings remain enforced under U1. A future bracket/target feature must return with a Saxo SIM-tested bundled-order and parent/child-lifecycle design, rather than a dormant switch.

A further 17 keys are unused without implying a missing safeguard (`strategy.mode`, `max_candidates`, `min_holdings`, the remaining `ladder.*` entries, the unported weekly/monthly journal cycle, `trading_manager.use_ai`, `trading_manager.due_window_minutes`, `analysis_pulses.timezone`, `taxation.share_income.currency`).

Enforced-by-contrast examples: `strategy.capital.min_cash_buffer_pct`, `strategy.capital.max_deployment_pct`, the `markov_gate` thresholds, `daily_indicators.min_confluences`/`min_reward_risk`, and `strategy.swing.never_trade_symbols` (`src/trading_manager.rs:3323`).

## Credential Hygiene

**Resolved 2026-07-26.** No git config scope on the development machine holds a
credential any more (`local`, `global`, and `system` all clean).

What the problem was: a stale GitHub OAuth token (`gho_`, 41 chars) sat in the
**global** git config as a URL rewrite rule,

```
url.https://<token>:@github.com/.insteadOf  https://github.com/
```

which is worse than a token in a remote URL — it was not tied to any repository,
so it silently applied to every `https://github.com/...` URL on the machine and
was readable by anything that dumps git configuration.

Two details that made it hard to find. It is an **OAuth** token, not a personal
access token, so it never appears under *Developer settings → Personal access
tokens*; OAuth grants live under *Settings → Applications → Authorized OAuth
Apps*. And it was **not** the token the `gh` CLI was using — fingerprints
differed — so it was an orphan from an earlier login, which is why removing it
broke nothing.

Current state:

- `origin` is SSH (`git@github.com:...`); pushes carry no credential.
- `gh` authenticates from the macOS keyring, git protocol `ssh`.
- HTTPS git operations are brokered by `gh auth git-credential` for both
  `github.com` and `gist.github.com`, so no token is written to a config file.
- CI has run green on every push since 2026-07-25.

The orphaned token was also **revoked**, not merely unset. Both tokens were
issued under the same GitHub CLI OAuth grant, so revoking that app authorization
invalidated the leaked one too. Cost was one `gh auth login`; git itself never
stopped working, because SSH key authentication is independent of the OAuth
token. Nothing is outstanding.

## Drawdown Approach

Reference for U9. Live query against production on 2026-08-02.

Daily closes, DKK:

| Date | Close | Cash | Positions |
| --- | --- | --- | --- |
| 2026-06-30 | **297,463** (peak) | 15,258 | 14 |
| 2026-07-19 | 255,599 | 10,425 | 13 |
| 2026-07-27 | 253,378 | 10,886 | 13 |
| 2026-07-31 | 241,281 | **93,789** | 10 |

Two things are happening at once, and they compound. The book is falling *and* de-risking into cash: cash went from 4.1% of the portfolio on 2026-07-19 to **38.9%** on 2026-07-31. That is partly the guardrail working as intended — the soft band halves the BUY budget, so sales are not fully redeployed — and partly the model selling into weakness. The effect is that the remaining equity has to work harder to recover the drawdown, while the drawdown percentage itself is measured against a peak that includes the cash.

Worth being explicit about a design consequence nobody has had to face yet: **at the halt, SELLs still execute and BUYs do not.** That is the correct direction of failure for a risk control, but sustained it converts the portfolio to cash and there is no re-entry rule. The guardrail was built to stop a bleeding book; it has no opinion about how a book resumes. That is the actual decision in U9, and it is worth making deliberately rather than discovering it.

## Realised Outcomes

Reference for U9 and U10. Every closed round trip since the Rust cutover:

| Holding period | Positions | Net DKK |
| --- | --- | --- |
| 1–9 days | 4 | −6,812 |
| 20–36 days | 7 | +16,610 |
| 48–83 days | 6 | −9,131 |

Seventeen closed positions: **2 winners, 15 losers**. The winners are `GOOGL` (+36,355, 20 days) and `AMD` (+5,929, 83 days). Total realised is **+1,187 DKK** — that is, the entire result of the strategy to date is one Alphabet trade, and without it the book is down ~35,000 DKK on closed positions alone.

An 11.8% win rate is not automatically wrong; a long-tail strategy can be profitable on a handful of large winners. But that is not what this configuration claims to be — the target is +15%/year with a −3%/−6% monthly loss floor, which needs a far steadier distribution than one 30x outlier carrying 15 losers. With n=17 there is no statistical case either way yet; the honest reading is that **the strategy has not demonstrated an edge**, and the single result that makes it look flat rather than bad is not repeatable evidence.

Two patterns worth naming:

- **Round trips of 1–2 days.** `AJG` bought 07-29, sold 07-30 (−7.0%). `JNJ` bought 07-28, sold 07-30 (−7.6%). `DSV` bought 07-21, sold 07-23 (−14.9%). Whatever thesis justified the buy did not survive 48 hours. On a swing horizon this is the model contradicting itself across two pulses, and each round trip pays spread and commission twice.
- **The losses cluster in the long tail.** Six positions held 48–83 days net −9,131. Protective stops (U1) landed 2026-07-26 and should truncate this going forward; that hypothesis is now testable and has not yet been tested.

## Currency Exposure

Reference for U10. Live exposures 2026-08-02, converted at current rates:

| Currency | Positions | Cost basis DKK | Share |
| --- | --- | --- | --- |
| USD | 5 | 87,892 | **63%** |
| DKK | 4 | 41,889 | 30% |
| NOK | 1 | 9,323 | 7% |

USD/DKK observed in our own ledger: **7.0215** (2026-07-02 sale) → **6.5072** (2026-07-31 sale) → **6.48371** (current spot). A −7.66% move against 63% of the book is roughly −4.8% of portfolio value from currency alone, on a −18.9% total drawdown.

That number is an estimate, and it should not have to be. Both `cost_basis_fx_rate_to_dkk` and `sale_fx_rate_to_dkk` are stored on every ledger row; the split is computable exactly, for history as well as going forward. It reads as zero only because the column is written as a literal.

Two smaller observations from the same query:

- `currency_fx_rates` holds 30 pairs, but only 6 come from `saxo_fx_spot`. The other 24 are ECB daily rates whose `expires_at` was **2026-07-23** — expired for ten days. They are not currently load-bearing (the book holds only USD/DKK/NOK) but they will be the moment a position is opened in one of them.
- Historical rows before 2026-07-09 store `cost_basis_fx_rate_to_dkk` **100x too small** (0.0713 where the rate was 7.0221; 0.0100 for DKK). The derived `cost_basis_sold_dkk` is correct, so realised P/L is unaffected — this is a display-only artifact of the legacy Python path, fixed by the Rust rewrite but never backfilled.

## Instrument Resolution

Reference for U11. Probed live against Saxo SIM `/ref/v1/instruments` on 2026-08-02.

The 28 permanent failures decompose cleanly:

| Cause | Count | Fix |
| --- | --- | --- |
| Stockholm suffix is `xome`, not `xsto` | 20 | `ABB`, `ALFA`, `ASSA-B`, `ATCO-A`, `BOL`, `ELUX-B`, `ERIC-B`, `ESSITY-B`, `HEXA-B`, `HM-B`, `INVE-B`, `NDA-SE`, `SAND`, `SCA-B`, `SEB-A`, `SHB-A`, `SKF-B`, `SWED-A`, `TELIA`, `VOLV-B` → `:xome` |
| Wrong ticker | 5 | `SAP:xetr`→`SAPG:xetr`, `DB1:xetr`→`DB1Gn:xetr`, `SCHP:xcse`→`SCHO:xcse`, `SHOP:xnys`→`SHOP_NEW:xnas`, `AKRBP:xosl`→`AKERBP:xosl` |
| Wrong exchange | 1 | `WMT:xnys` → `WMT:xnas` (Walmart moved to Nasdaq) |
| Merged into a successor | 1 | `NZYM-B:xcse` → `NSIS-B:xcse` (Novozymes merged into Novonesis) |
| Intentional, leave alone | 1 | `SPCX:xnas` is a documented pending entry — SpaceX listed on Live 2026-06-12 and Saxo SIM reference data has not synced it. It carries an ISIN and activates automatically. Not a defect. |

The share-class variant generator is *not* the problem — `base_lookup_variants("ERIC-B")` correctly produces `ERICb`, which is exactly Saxo's format. It fails only because the suffix appended to it is `:xsto`.

The second defect is more interesting because it hid the first. `lookup_instrument` tries symbol keywords first, then falls back to an exchange-scoped search. That fallback passes `exchange_id_for_suffix(...)`, which returns the **ISO MIC**; Saxo's `ExchangeId` is a proprietary code. Verified directly:

```
ExchangeId=XSTO  →  NO MATCH
ExchangeId=SSE   →  VOLVb:xome | VOLV_A:xome | VOLCAR:xome
```

The correct values are `XNAS`→`NASDAQ`, `XNYS`→`NYSE`, `XCSE`→`CSE`, `XETR`→`FSE`, `XHEL`→`HSE`, `XOSL`→`OSE`, `XLON`→`LSE_SETS`, `ARCX`→`NYSE_ARCA`, and Stockholm is `SSE` with MIC `XOME`. **None of the 15 hardcoded entries is right**, so the fallback has never resolved a single instrument. It looks like a safety net in the code and is not one — the 173 symbols that work do so entirely on the first keyword attempt.

Note this is also why the failures are *silent*: the negative cache (30 entries, 7-day retry) is doing its job correctly, caching a genuine failure. The cache is not the bug; it just makes a permanent misconfiguration look like a transient upstream problem.

## Unused Saxo Surface

Reference for U15. Endpoints currently called: `/chart/v3/charts`, `/cs/v1/audit/orderactivities`, `/ens/v1/activities`, `/port/v1/{accounts,balances,positions,orders,exposure}`, `/ref/v1/{exchanges,instruments}`, `/trade/v1/infoprices/list`, `/trade/v2/orders{,/precheck}`.

Probed live in SIM:

| Endpoint | Result | Why it matters here |
| --- | --- | --- |
| `/port/v1/closedpositions` | **200**, 6 rows | Broker's own realised P/L. An independent check on exactly the ledger arithmetic that U10 found zeroed. |
| `/hist/v4/performance/timeseries` | **200**, series from 2021 | Broker's own account-value history. Would validate the `portfolio_value_history` peak that U9's halt threshold is measured against. |
| `/ca/v2/events` | **403** in SIM | Corporate actions. Unverifiable here, but nothing in the runtime handles splits, dividends, or mergers today — and a split silently corrupts cost basis, which would look exactly like a large unexplained loss. |
| `/ref/v1/instruments/tradingschedule/{Uic}/{AssetType}` | not called | Authoritative per-instrument session hours including half-days; currently inferred from `/ref/v1/exchanges`. |
| `mkt` service group | not called | Exchange winners/losers. A candidate source, not a gap. |

`/hist/v3/perf` returns 404 — v4 is the live version, so any older integration notes referring to v3 are stale.

## Python Removal Plan

The Next.js frontend is **already gone** — no `.ts`, `.tsx`, `.jsx`, `package.json`, or `next.config.*` remains anywhere in the tree. What is left is Python: **91 tracked files, 28,353 lines**.

Three categories, and only the first is load-bearing:

**Keep — live in production.** `scripts/create_postgres_backup.py` and `scripts/prune_postgres_backups.py`. These run as CronJobs (`daytrader-postgres-backup-schedule`, `-retention`) and were observed completing minutes ago. `Dockerfile.backup` builds them on `python:3.13-alpine` with just `boto3` and `requests`. They are ~2 files and have no dependency on the rest of the Python tree. Porting them to Rust is possible but buys little and risks the backup path; leaving them is the right call, and `requirements.txt` should be reduced to the two packages they actually import rather than the twelve it currently pins.

**Delete — dead application code.**

| Path | Files | Note |
| --- | --- | --- |
| `src/saxo_daytrader_xai/` | 30 | The entire legacy app: `api/app.py`, `saxo_openapi.py`, `execution_engine.py`, `portfolio.py`, `tax_engine.py`, `db.py`. Fully superseded. |
| `main.py`, `web_main.py` | 2 | Old Streamlit/FastAPI entrypoints. |
| `scripts/validate_phase*.py` | ~43 | Phase validators for a runtime that no longer exists. |
| `scripts/run_scheduler.py` | 1 | Superseded by the Rust scheduler. |
| `deploy/systemd/*.tmpl`, `deploy/launchd/*.tmpl` | 4 | Reference `main.py` and `scripts/run_scheduler.py`; superseded by Kubernetes. |
| `audit_log` table | — | 65 MB, 38% of the database, written only by this code, nothing since 2026-05-10. See U14. |

**Archive then delete — one-shot migration.** `scripts/migrate_sqlite_to_postgres.py` and `deploy/k8s/postgres/sqlite-migration-job.template.yaml`. The migration completed months ago and cannot be re-run meaningfully. Tag the commit before deleting so it stays recoverable from history.

**Sequencing.** Do the documentation first, because it is the part that is actively causing harm: `AGENTS.md:100-118` has a section titled *Legacy Python/Next.js Structure* that instructs agents to use `src/saxo_daytrader_xai/api/app.py` as "the reference for API behavior" and `saxo_openapi.py` as the porting reference for token handling and tick-size normalization. **The Rust implementations are now the authority and are further along**, so that section currently points every future agent at stale code. `README.md:823` still draws the package in the tree diagram.

Then delete in this order, one commit each so any of them can be reverted independently: phase validators → systemd/launchd templates → `main.py`/`web_main.py` → `src/saxo_daytrader_xai/` → `requirements.txt` reduction → `audit_log` drop. The order matters only in that the package goes late, since it is the thing most likely to be referenced by something unnoticed.

**One caveat worth checking before the package is deleted.** `tax_engine.py:273` holds the only correct FX-attribution formula in the repository, and U10 needs it. Port that arithmetic into Rust *first*, or the deletion removes the reference implementation for a bug that is still open. Nothing else in the package is known to be ahead of the Rust runtime, but the same question is worth asking of the tax bracket logic, since `estimated_tax_dkk` is still hardcoded to `0.0`.

## Related Pages

- [roadmap](roadmap.md) — full improvement map, including the longer-horizon shape for U3, U4, and U6.
- [runbooks/build-test-deploy](runbooks/build-test-deploy.md) — the manual validate/deploy checklist that U5 automates.
- [concepts/hermes-self-improvement](concepts/hermes-self-improvement.md) — goal-contract and experiment governance context for U3.
