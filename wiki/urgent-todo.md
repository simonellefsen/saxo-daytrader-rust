---
type: capability
tags:
  - daytrader/wiki
  - roadmap
  - urgent
  - maintained-by-llm
updated: 2026-07-26
---

# Daytrader Urgent Todo

This page holds the small set of items where current evidence says the system is exposed **now**. It is deliberately short and ranked. [roadmap](roadmap.md) remains the long-horizon planning map; this page is the subset that should not wait for its turn there. Move an item to the roadmap's `Recently Landed` section once it lands, and delete its row here.

Entry criteria for this page: a verified gap between what the runtime claims or is configured to do and what it actually enforces, or an exposure that grew because of a recent change. Speculative improvements belong in the roadmap.

Reviewed 2026-07-25 against `config.yaml`, `src/*.rs`, and the current roadmap.

## Ranked Items

| # | Item | Why now | Exit criteria |
| --- | --- | --- | --- |
| U1 | Finish broker-hosted protective stops (SIM-first) | Downside protection currently depends entirely on the model proposing a SELL at one of two weekday pulses and that SELL clearing the technical gate. Nothing exits between pulses, and nothing exits across a weekend. The 2026-07-25 work landed a read-only coverage audit plus a manually confirmed SIM lifecycle test; the automatic path after a confirmed BUY fill is still unbuilt. `strategy.ladder.submit_stop_loss_after_fill` is still `false` and unread; `stop_loss_atr_multiple` (2.0) was equally dead until slice 1 wired it into the proposed stop level. The roadmap's own live evidence records a −24.4k DKK week in which the defensive exits did not fire. | **Slice 1 landed 2026-07-25:** `strategy.ladder.stop_loss_atr_multiple` is now live — the coverage audit computes a concrete proposed stop per unprotected position from stored close and ATR14, sized to the uncovered quantity only, and the Execution table shows it beside each exception. Read-only: no Saxo call, nothing placed, not tick-normalized. **Slice 2 (operator, blocking):** `protective_stop_prechecks` and `protective_stop_lifecycle_tests` are both still empty — the broker path has never executed once. Run the precheck, then one SIM placement/cancel/reconcile, using the proposed levels. **Slice 2b landed 2026-07-25:** operator-confirmed bulk placement from the exceptions table, sequential, 1.1s spacing for Saxo's 1 order/second limit, halting the whole batch on the first rejection, error, or ambiguous response. Stale `placement_preparing` orphans are reconciled against Saxo and abandoned only when the broker never saw them. **Slice 2c landed 2026-07-25:** the scheduler confirms placements read-only each cycle, which promoted all nine stragglers from `placement_submitted` to `broker_working` on its first run; all twelve positions are now broker-confirmed protected. **Slice 3a landed 2026-07-25** — stops became real `execution_orders` rows, so a fill is visible, and they yield to a decided sell (points 1, 5, 6). **Slice 3b landed 2026-07-26** — one declarative sweep places, re-sizes, and ratchets every held position's stop each cycle (points 2, 3, 4). U1 is complete except point 7, ENS streaming, which is a latency optimisation and belongs on the roadmap. Default to `Stop`, not `StopLimit`. A stop cannot guarantee price through a gap. |
| U2 | Config-contract audit | Several risk keys in `config.yaml` have zero references in `src/` — see [Unwired Risk Configuration](#unwired-risk-configuration). This is the same failure class as the retired 2026-05-05 `cash_buffer` override, but silent by construction: an unwired key is indistinguishable from an enforced one when reading the config. | Audit landed 2026-07-25 (`src/config_contract.rs`): every key is classified, unused risk keys appear in the Overview integrity payload, and a key added without a contract entry raises a warning. **Remediations landed 2026-07-26:** the strategy, Trading Manager, EU/US scheduled-pulse, `RISK_EXCLUDED_SYMBOLS`, Danish share-income estimate, ATR-based risk-per-trade, deterministic BUY cost controls, the per-report candidate ceiling, the ladder per-symbol exposure cap, maximum-holdings cap, post-gate BUY-selection cap, duplicate inert cash-buffer path, and inert minimum-selection floor are now resolved, reducing the risk-surface inventory from 27 to 12 keys. **Remaining:** each of the 12 keys needs an operator decision to implement or delete. |
| U3 | Reconcile the Hermes goal contract with enforced reality | `AppState::hermes_goal_contract_value` sent Hermes `max_drawdown: 0.20`, `min_sharpe: 1.0`, `slippage_tolerance: 0.02`, and `max_positions` as *constraints*. Only the monthly-loss DKK floors were enforced in `src/trading_manager.rs`. Portfolio drawdown was computed but consumed only by display and evidence packs, so Hermes reasoned about a risk envelope the runtime did not apply. The return half of this was fixed 2026-07-25 — see [Return Goal](#return-goal). | Landed 2026-07-26. `src/drawdown_guard.rs` makes `max_drawdown` real: a soft band reduces the cycle BUY budget, a hard floor suspends new BUYs, SELLs are never blocked. The contract now reads its limit from `strategy.capital.drawdown_halt_pct`, the same key the gate applies, so the advertised and enforced numbers cannot drift. `strategy.swing.max_holdings` now applies the published `max_positions` constraint to new-symbol BUYs using persisted positions plus same-cycle reservations. Every remaining objective and constraint carries an explicit `enforcement` status (`runtime_enforced` / `evaluation_only` / `structural` / `documentation` / `not_enforced`), and a test fails the build if a field is added without one. `gas_reserve` was deleted as a crypto-template leftover. **Remaining debt, now named rather than implied:** `slippage_tolerance` and `require_backtest_before_activation` are declared `not_enforced`. |
| U7 | Hermes can experiment on a dead variable | `deploy/k8s/base/hermes.yaml` listed `strategy.swing.cash_buffer_pct` in `experiment_policy.supported_variables`, and the config-contract audit proved nothing reads it. Hermes could therefore propose, run in SIM, observe, and promote a one-variable experiment whose variable had no effect — and attribute whatever the portfolio did to it. | Landed 2026-07-25, fully reconciled 2026-07-26: `strategy.swing.cash_buffer_pct` is removed from the Rust capabilities payload, Kubernetes ConfigMap, shipped configs, reflection prompts, runbooks, Trading Manager overlay loader, and historical runtime settings. The legacy Python reference now also reads the active `strategy.capital.min_cash_buffer_pct` reserve. `SUPPORTED_EXPERIMENT_VARIABLES` is the runtime source for overlay acceptance and is cross-checked against the config contract by test, so a variable classified `unused` cannot be offered or applied. Verified the guard rejects the retired path and other unpublished variables. |
| U4 | Prompt-injection screen for editorial research | The editorial-research path feeds third-party RSS titles and summaries into the decision prompt (`src/xai_decision.rs`) and into Hermes context. `normalize_text` (`src/editorial_research.rs`) strips HTML tags and collapses whitespace; it does not screen for instruction-shaped content. Every earlier prompt input — Markov, daily indicators, Quiver — is numeric and runtime-computed, so this is the first attacker-influenceable free text in the pipeline. It should be hardened before the configured feed catalog expands to Yahoo Finance, CNBC, and Reuters. | Landed 2026-07-25, and none too soon — the feature began ingesting live items into the decision prompt that morning. A narrow marker list detects text addressed at a model rather than a reader; flagged items are retained for operator review but excluded at the context boundary, which also covers rows stored before screening existed. The prompt now carries an explicit security-boundary instruction labelling the section untrusted. Two tests: one proves injection-shaped text never reaches the prompt, the other proves ordinary market language ("upgrade to Buy", "sell off", "disregard one month of data") is not flagged. |
| U5 | CI on every push | The suite is 324 `#[test]`/`#[tokio::test]` functions across 24 modules and ran only when someone typed `make validate`. Deploy provenance was hardened on 2026-07-13 so the intended commit ships, but nothing verified that commit's tests pass. | Landed 2026-07-25: `.github/workflows/ci.yml` runs `cargo fmt --check`, `cargo check --all-targets`, and `cargo test` with `-D warnings` on push, pull request, and manual dispatch. **Verified 2026-07-26:** Actions has run green on every push since 2026-07-25, so the workflow is live and no longer merely committed. The `origin` remote is now SSH (`git@github.com:...`), so pushes no longer carry a credential. **Still open, and not what U5 was about:** the OAuth token was never actually removed — it lives in the *global* git config as a `url.https://<token>:@github.com/.insteadOf https://github.com/` rewrite rule, so it silently applies to every HTTPS GitHub URL on this machine and still leaks through `git config --list` in any diagnostics capture. Switching this repo to SSH sidestepped it; it did not fix it. Operator action, see [Credential Hygiene](#credential-hygiene). |
| U8 | `strategy_type` is never set on Trading Manager orders | `CandidateOrder::from_json` (`src/trading_manager.rs:1904`) reads `strategy_type` out of the model's suggested-trade JSON, but the decision-report schema has no such field — the string `strategy_type` does not appear anywhere in `src/xai_decision.rs`. So it is NULL on every order the Rust Trading Manager has ever queued: 101 of 156 rows, most recent 2026-07-23. The three populated values come from other paths (`portfolio_sync` and `clean_reconciliation` from `src/portfolio_reset.rs`, `manual` from the manual order path). Two live consequences: the Execution table renders `fallback_text(row, "strategy_type", "manual")` (`src/ui.rs:4261`), so **every automated order is displayed to the operator as "manual"**; and `execution_source_label` (`src/notifications.rs:1476`) falls through to "Execution" instead of "Trading Manager" in Slack. See [Orphaned strategy_type](#orphaned-strategy_type). | Landed 2026-07-25. The runtime now sets `TRADING_MANAGER_STRATEGY_TYPE` (`swing`) at insert and ignores any value the model supplies; a startup backfill scoped to `report_id IS NOT NULL` repaired the 101 historical rows; the UI fallback is now `unknown` rather than `manual`. Two tests cover it, including one asserting the backfill cannot touch adoption, reconciliation, or manual rows. |
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

The audit implemented for U2 reports this automatically. After the 2026-07-26 automation-switch, exclusion, tax-estimate, risk-sizing, BUY-cost, candidate-ceiling, per-symbol exposure-cap, maximum-holdings, post-gate BUY-selection-cap, duplicate-cash-buffer retirement, explicit report-freshness-policy, Quiver-cadence, and minimum-selection-floor remediations, the deployed configuration has **34 enforced, 30 advisory, 29 unused, 12 of them risk-surface**, 0 uncontracted. Local configuration carries the same Quiver policy rather than silently falling back to code defaults.

The remaining 12 keys that read as active risk controls and are not:

| Config key | Note |
| --- | --- |
| `strategy.max_assets_per_sector` | No concentration gate exists; the string `sector` does not appear anywhere in `src/`. |
| `strategy.swing.min_holding_weight_pct`, `max_holding_weight_pct`, `strategy.ladder.min_position_weight`, `risk.max_position_weight` | Four separate per-position weight caps remain unused. `strategy.ladder.max_position_weight` is now the enforced 4% total per-symbol BUY-exposure ceiling. |
| `strategy.ladder.submit_take_profit_after_fill`, `submit_bracket_with_entry` | No take-profit or entry bracket is implemented. Automatic protective stops and their ATR controls are enforced under U1. |
| `strategy.ladder.session_flatten_enabled`, `flatten_minutes_before_tradable_close` | No session flatten runs; nothing exits on a schedule. |
| `strategy.swing.journal.benchmark_indices` | No benchmark comparison is computed in Rust; performance is reported without one. |

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

## Related Pages

- [roadmap](roadmap.md) — full improvement map, including the longer-horizon shape for U3, U4, and U6.
- [runbooks/build-test-deploy](runbooks/build-test-deploy.md) — the manual validate/deploy checklist that U5 automates.
- [concepts/hermes-self-improvement](concepts/hermes-self-improvement.md) — goal-contract and experiment governance context for U3.
