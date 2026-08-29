---
type: wiki-log
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-08-29
---

# Wiki Log

Append-only timeline for project wiki maintenance. Use headings with the format `## [YYYY-MM-DD] kind | summary` so agents and shell tools can parse the log.

## [2026-08-29] architecture | Type End-of-Day benchmark readthrough

- Changed the EOD benchmark-readthrough card from a generic diary document to a small local projection of status, valid rendered benchmark references, and caveat. Unrelated diary fields and malformed reference rows do not reach the renderer.
- The complete retained diary document remains in the established read-only journal detail view. This changes no benchmark collection, EOD scheduling, Hermes context, Decision Report, manager gate, queue, precheck, or Saxo behavior.

## [2026-08-29] architecture | Render typed portfolio decision badge directly

- Removed the portfolio-position Decision Badge's generic-JSON re-serialization of its typed advisory projection. Sentiment, action, timestamp, rationale, and original-thesis fields now remain compiler-checked through SSR.
- Trend stays on the existing typed sparkline path, and direct regression coverage preserves the stored advisory fields. This is display-only: quote refresh, Decision Reports, Hermes, manager gates, queues, prechecks, and Saxo behavior are unchanged.

## [2026-08-29] architecture | Render typed execution orders directly

- Removed the Execution and Overview order renderers' generic-JSON re-serialization of their compiler-checked execution-order payloads. Stable display fields now remain typed through SSR.
- Persisted lifecycle-result and Hermes-attribution documents remain explicit diagnostics-only inputs, with a direct regression test covering protective-stop, broker-visibility, and attribution display semantics. This changes no queue claim, precheck, placement, cancellation, replacement, reconciliation, or Saxo execution behavior.

## [2026-08-29] architecture | Render selected Decision Report directly

- Removed the Decisions view's internal generic-JSON re-serialization of the selected typed Decision Report. Its stable lifecycle, cadence, authority, model, and error fields now flow straight from the compiler-checked dashboard payload.
- The prompt, provider request/response, and normalized-report documents remain explicit inputs only to the established detail, diagnostics, and redacted debug views. This is a read-only UI boundary change: report scheduling, provider calls, Hermes, manager gates, queues, prechecks, and Saxo execution are unchanged.

## [2026-08-28] architecture | Type Support Risk Evidence panel

- Changed the Decisions support-risk outcome evidence and renderer from generic JSON to an allowlisted local count, label, one/five-run observation, and confidence projection.
- Raw daily-indicator documents and unused analytical detail remain internal. The panel remains descriptive, non-causal, and non-gating; it cannot change reports, Hermes, configuration, queues, or Saxo behavior.

## [2026-08-28] architecture | Type Protective Stop Coverage lifecycle tests

- Changed the Execution SIM lifecycle-test rows from generic persisted JSON to an allowlisted local ID, requested stop, lifecycle status, and visible broker-order reference projection.
- Placement, cancellation, reconciliation, request, and broker-response documents remain internal. Existing reconcile/cancel forms retain their handler-side SIM reload and explicit cancellation acknowledgement safeguards; no queue or Saxo behavior changed.

## [2026-08-28] architecture | Type Protective Stop Coverage SIM prechecks

- Changed the Execution recent-SIM-precheck rows from generic persisted JSON to an allowlisted local ID, requested stop, status, compact result-label, and safety projection.
- The stored Saxo precheck response remains internal. Its placement form still sends only the local precheck ID to the established explicit SIM confirmation and reload/validation path; no precheck, placement, cancellation, reconciliation, queue, or Saxo behavior changed.

## [2026-08-28] architecture | Type Protective Stop Coverage exception rows

- Changed the Execution protection-exception rows and their computed proposal display from generic JSON to an allowlisted symbol, uncovered quantity, reason, operator guidance, and stored-indicator price/ATR projection.
- Broker and indicator source documents remain internal. The checkbox still submits only a symbol to the established explicit SIM confirmation, reload, precheck, and placement path; no coverage computation, lifecycle, queue, cancellation, reconciliation, or Saxo behavior changed.

## [2026-08-28] architecture | Type Overview Instrument Quarantine rows

- Changed the Overview's active instrument-quarantine rows from generic JSON to allowlisted symbol, action, failure category/count, expiry, override, and bounded sample-error evidence.
- Unallowlisted row fields remain internal; malformed rows degrade independently while valid blocks remain visible. The panel remains read-only and does not alter quarantine policy, manager selection, queues, or Saxo behavior.

## [2026-08-28] architecture | Type latest Trading Manager run envelope

- Changed the Overview's latest Trading Manager run from a generic persisted row to allowlisted run identity, timing, report-linkage, lifecycle, and manager-identity metadata.
- Root-level raw error, technical, queue-result, and exchange documents remain internal. The established Cash Deployment and Instrument Quarantine gate diagnostics remain staged and read-only; manager selection, queues, and Saxo behavior are unchanged.

## [2026-08-28] architecture | Type public Watchlist decision summary

- Changed the public Watchlist Decision Report evidence and its dashboard badge/sparkline renderer from generic JSON to a display-only summary: sentiment, action, timestamp, rationale, and report-local trend bias.
- Report identifiers, queue eligibility, strategy metadata, and provider/source documents remain internal. This cannot generate a report, alter candidate membership, add a queue entry, or mutate a Saxo order.

## [2026-08-28] architecture | Type public Watchlist support-risk evidence

- Changed the public Watchlist support-risk document and renderer from generic JSON to a fixed local daily-indicator projection: run/status, levels, downside, risk label/value, confidence, history coverage, and touch count.
- Raw indicator diagnostics remain internal. This read-only support display remains separate from every decision/manager gate, quote collection, queue, and Saxo order path.

## [2026-08-28] architecture | Type public Watchlist row shell

- Changed public Watchlist rows and their dashboard renderer from generic JSON to an allowlisted identity, market, quote/value/change, and lifecycle projection.
- Unallowlisted source/raw-quote fields remain internal. Nested Decision Report and support-risk evidence intentionally remain staged while their schemas evolve; quote collection, candidate membership, reports, queues, and Saxo order behavior are unchanged.

## [2026-08-28] architecture | Type public Market Status exchange rows

- Changed public Market Status exchange rows and the dashboard market table from generic JSON to typed operator-facing session/window fields, preserving explicit absent holiday names.
- Saxo provider exchange identifiers and names remain internal. The read-only calendar refresh/cache, scheduler and price-monitor documents, Decision Reports, queues, and Saxo order behavior are unchanged.

## [2026-08-28] architecture | Type Decision Gate Replay scenarios

- Changed public Decision Gate Replay scenarios and their changed-outcome rows from generic JSON to typed, allowlisted historical evidence; the Decisions dashboard now renders the same typed fields.
- Raw Trading Manager/provider documents stay outside this boundary. Threshold value documents and the independently evolving support-risk evidence remain staged JSON; this is still an offline historical comparison with no report, gate, configuration, queue, or Saxo mutation authority.

## [2026-08-27] architecture | Type public Markov signal list

- Changed `GET /api/markov/signals` from a generic collector/run document to an allowlisted typed lifecycle and rendered regime/probability projection.
- Collector configuration/summary documents, transition counts/matrices, forecasts, raw payloads, and provider diagnostics remain in internal read models. Malformed rows fail closed at the public boundary; regime collection, Decision Reports, manager gates, queues, and Saxo behavior are unchanged.

## [2026-08-27] architecture | Type public Quiver signal list

- Changed `GET /api/quiver/signals` from a generic collector/run document to an allowlisted typed lifecycle and rendered-signal projection.
- Collector configuration/summary documents, source-status/top-event documents, and provider diagnostics remain in internal read models. Malformed rows fail closed at the public boundary; collection, Decision Reports, manager gates, queues, and Saxo behavior are unchanged.

## [2026-08-27] security | Narrow Saxo session refresh response

- Changed `POST /api/saxo/session/refresh` to return the same typed, sanitized session-health contract as the read-only session-status endpoint after a successful refresh and durable-session persistence.
- Local session paths and free-form auth errors remain internal. The scheduler now reads typed lifecycle fields and serializes the same sanitized status into its retained cycle evidence; refresh lease handling, token renewal, durable persistence, invalid-session audit persistence, and all broker/execution behavior are unchanged.

## [2026-08-27] architecture | Type asset ladder history placeholder

- Changed the public asset-ladder-history endpoint from a generic stub document to a typed read-only placeholder contract. Its optional position now uses the bounded dashboard position projection, and malformed matching rows degrade to absent position evidence.
- The explicit `not_ported` ladder state, empty chart/marker/line/level evidence, and non-mutating semantics are unchanged. This cannot refresh broker data, queue work, precheck, place, cancel, replace, or reconcile a Saxo order.

## [2026-08-27] architecture | Type protected Hermes context envelope

- Changed `GET /api/hermes/context` and the Hermes-safe MCP context tool to construct the same typed read-only envelope. Stable context sections, pulse/EOD cadence, performance range, capability declaration, and exclusion guarantees are now compiler-checked.
- Detailed retained read models remain staged JSON because their schemas evolve independently. The endpoint and MCP tool remain protected/advisory-only and exclude Saxo sessions, raw OAuth payloads, and broker mutations.

## [2026-08-27] architecture | Type protected Hermes capabilities

- Changed `GET /api/hermes/capabilities` from a generic JSON document to a typed advisory-boundary contract. Stable endpoint, read-model, restricted-write, advice self-check, experiment-overlay, forbidden-capability, and note lists are now compiler-checked.
- The evolving configuration-derived goal contract deliberately remains a staged document. This endpoint remains protected and read-only; Hermes cannot add trades, increase size, approve live orders, or call Saxo mutation endpoints.

## [2026-08-27] security | Type and narrow public Saxo auth status

- Changed `GET /api/saxo/auth/status` from the wider internal auth object to a typed health-only projection. It retains environment, token/refresh lifecycle, expiry, reauthorization, and stable status fields.
- Local session-storage paths and free-form loader errors remain available only to trusted internal diagnostics. The endpoint is still read-only; no OAuth refresh, session storage, precheck, placement, cancellation, or reconciliation behavior changed.

## [2026-08-27] security | Type and narrow public Saxo session status

- Changed `GET /api/saxo/session` from compatibility JSON to a typed health-only contract. It retains environment, token/refresh lifecycle, expiry, reauthorization state, auth mode, and non-secret client/account-key presence signals.
- Local session-storage paths, free-form session errors, default-account identifiers, and client display metadata no longer cross the API boundary. The endpoint remains read-only; no OAuth refresh, session storage, precheck, placement, cancellation, or reconciliation behavior changed.

## [2026-08-27] architecture | Type public per-order execution timeline

- Changed `GET /api/execution/orders/{order_id}/events` to return a typed, allowlisted lifecycle timeline while preserving its chronological order and broker-status, substatus, quantity, price, and broker-order-reference evidence.
- Raw Saxo payloads, account identifiers, and local audit signatures remain outside the API. This remains observation only and cannot replay, reconcile, precheck, queue, place, cancel, or replace an order.

## [2026-08-27] architecture | Type public AI prompts latest-report metadata

- Changed `/api/prompts` to load and return only the existing stable Decision Report lifecycle summary; it no longer reads the full persisted report merely to show the latest item.
- Prompt text, request/response/provider documents, report JSON, and free-form errors remain on their dedicated internal paths. A malformed latest summary degrades to absent metadata and this cannot generate a report, invoke Hermes, change a queue, or reach Saxo.

## [2026-08-27] architecture | Type public scheduler API

- Changed `/api/scheduler` to return a typed scheduler-status snapshot and bounded, allowlisted cycle evidence rather than generic database rows.
- Retained cycle/provider documents, broker-alert columns, and local process metadata remain in the audit store. Status and cycle-list failures degrade independently and this cannot schedule work, change queues, invoke a provider, or mutate a Saxo order.

## [2026-08-27] architecture | Type public execution list

- Changed `/api/execution` to return typed, allowlisted order, reconciled-fill, and lifecycle-event summaries rather than generic persisted rows.
- Broker payloads, lifecycle-result documents, attribution, and free-form broker errors stay in the local audit store; each list preserves its existing independent degraded-to-empty behavior. This cannot claim, precheck, place, cancel, replace, reconcile, or otherwise mutate a Saxo order.

## [2026-08-26] architecture | Type protected Hermes experiment list

- Changed `GET /api/hermes/experiments` to return typed stable identity, lifecycle, baseline, goal-version, changed-path, and source-session metadata only.
- Proposed values, evidence, approvals, metrics, and raw model payloads remain in the local audit store and internal read models. The MCP advisory list retains its stricter pending-review value exclusion; this cannot create, transition, promote, queue, precheck, or reach Saxo.

## [2026-08-26] architecture | Type protected Hermes reflection list

- Changed `GET /api/hermes/reflections` to return typed stable reflection identity, timing, goal-version, summary, and source-session metadata only.
- Detailed findings, proposed actions, and raw model payloads remain in the local audit store and internal read models. The protected watchdog retains its source-session check; this cannot invoke Hermes, create a proposal, change an experiment, queue, precheck, or reach Saxo.

## [2026-08-26] architecture | Type public strategy-journal list

- Changed `/api/strategy-journal` to return typed, stable identity, timing, cadence, status, summary, and source-report metadata rows.
- Detailed metrics, learnings, and diary documents remain on the internal EOD dashboard read model; malformed metadata fails closed at the public API boundary without invoking Hermes, scheduling work, queueing, or reaching Saxo.

## [2026-08-26] architecture | Type nested portfolio decision display

- Replaced the compatibility decision document in typed portfolio rows with explicit advisory badge and trend fields, redacting and bounding rationale text.
- Full Decision Report documents remain on their dedicated selected-report path; this display data cannot change a report, queue, precheck, or Saxo order.

## [2026-08-26] architecture | Type dashboard execution queue rows

- Changed the shared overview/Execution order rows to consume typed identity, lifecycle, instrument, price, quantity, and strategy fields, with capped/redacted error text.
- Existing lifecycle-result and attribution documents remain compatibility JSON only for detailed diagnostics; no execution mutation path changed.
- Malformed rows degrade to an empty local page and cannot claim, precheck, place, cancel, replace, or reconcile a Saxo order.

## [2026-08-26] architecture | Type dashboard portfolio position rows

- Changed the overview position list to consume typed instrument, price, valuation, P/L, allocation, and quote-freshness fields.
- The nested advisory decision remains compatibility JSON for the existing decision badge and trend chart; broker/provider payloads remain outside SSR.
- Malformed position metadata degrades to an empty local overview list and cannot refresh quotes, change a Decision Report, queue, precheck, or submit a Saxo order.

## [2026-08-26] architecture | Type dashboard Decision Report summaries

- Changed the overview and Decisions list to consume typed report identity, lifecycle, model, and pulse metadata from the existing lightweight SQL projection.
- Full report, prompt, request, response, error, and debug documents remain on the selected-report and lazy debug paths; malformed list metadata degrades to an empty local list.
- This preserves the existing Decision Report selection and pulse-status fallback behavior and cannot generate a report, change queue eligibility, invoke Hermes, or mutate a Saxo order.

## [2026-08-26] architecture | Type dashboard Quiver signal rows

- Changed the paginated Quiver signals table to consume explicit rendered market, Congress aggregate, date, status, and bounded sanitized-error fields.
- Source-status, top-event, and provider documents remain outside SSR; malformed rows degrade to the existing empty local page.
- This preserves pagination and read-only display behavior and cannot refresh Quiver data, change a Decision Report, manager gate, queue, precheck, or Saxo order.

## [2026-08-26] architecture | Type dashboard Markov signal rows

- Changed the paginated Markov signals table to consume explicit rendered fields, including a flattened stationary distribution and bounded sanitized error text.
- Transition matrices, forecasts, raw payloads, and provider diagnostics remain outside SSR; malformed rows degrade to the existing empty local page.
- This preserves existing filters and pagination and cannot refresh Markov data or alter a Decision Report, manager gate, queue, precheck, or Saxo order.

## [2026-08-26] architecture | Type dashboard Hermes baseline evidence

- Changed the Hermes Baselines view to consume typed promoted-baseline metadata, redacted/capped configuration display text, exact-overlay activity counts, and bounded local observation windows.
- Prompt/source, experiment, manager, broker, and other retained documents remain outside SSR; malformed evidence degrades to the existing unavailable or empty local state.
- This preserves the read-only, non-causal evidence boundary and cannot change experiment or baseline lifecycle, configuration, queues, prechecks, or Saxo orders.

## [2026-08-26] architecture | Type dashboard Hermes experiment proposals

- Changed the Hermes Experiment Proposals table to consume explicit proposal display fields and redacted, capped old/new/evidence strings through a typed Rust contract.
- Approval, metrics, source-session, and raw provider documents remain outside SSR; malformed proposal metadata degrades to an empty local table.
- The retained proposal ID/status preserve only the existing explicit operator transition form. This display contract cannot approve, activate, promote, reject, queue, precheck, or mutate an order.

## [2026-08-26] architecture | Type dashboard Hermes decision-advice audit

- Changed the Hermes Decision Advice Audit table to consume retained report/advice metadata, derived self-check/action/impact counts, manager status, and local order totals through a typed Rust contract.
- Raw provider payloads, persisted advice documents, Trading Manager JSON, and broker mutation data are now used only to derive bounded display evidence and remain outside SSR; malformed evidence degrades to an empty local table.
- This preserves Hermes as advisory-only and cannot change advice, queues, manager runs, prechecks, or Saxo orders.

## [2026-08-26] architecture | Type dashboard missed-trade shadow aggregate evidence

- Changed the Missed Trade Shadow evidence panel to consume typed aggregate status, sample counts, equal-weighted directional-return summaries, and per-gate breakdowns.
- Raw shadow rows, scan metadata, and safety marker remain outside SSR; malformed evidence degrades to the existing unavailable panel state.
- The aggregate remains observational only, excludes execution, fees, FX, slippage, and tax, and cannot override a manager gate or mutate a Saxo order.

## [2026-08-26] architecture | Type dashboard missed-trade shadow evidence

- Changed the Missed Trade Shadows table to consume typed quote-to-quote observation fields and its distinct manager-gate reason.
- Manager-gate audit data and broker payloads remain outside SSR; malformed shadow evidence degrades to an empty local table.
- These remain observational estimates, not a recommendation to override a gate or execute a trade; they exclude execution, fees, FX, and slippage and cannot mutate Saxo orders.

## [2026-08-26] architecture | Type dashboard Hermes counterfactual evidence

- Changed the Counterfactual Tracking table to consume typed quote-to-quote observation fields rather than generic JSON.
- Manager/advice documents and broker payloads remain outside SSR; malformed counterfactual evidence degrades to an empty local table.
- These remain observational estimates excluding execution, fees, FX, and slippage; this change cannot create, place, cancel, or modify a Saxo order.

## [2026-08-26] fix | Load and type Hermes proposal-quality evidence on Overview

- Corrected the Hermes Overview query path: Proposal Quality Review now loads the bounded experiment evidence it scores instead of deriving from the Experiments-only list, which was always empty on Overview.
- Changed the rendered rows to a typed rubric contract containing only displayable scores, checks, duplicate counts, and gap labels; persisted experiment/evidence documents remain outside SSR.
- This preserves the advisory-only Hermes boundary and performs no proposal approval, lifecycle transition, configuration change, queue, precheck, or broker-order action.

## [2026-08-26] architecture | Type dashboard Hermes one-variable audit evidence

- Changed the Hermes One-Variable Audit table to consume typed audit metadata and bounded display strings for baseline/candidate values.
- Source baseline, overlay, and manager-run JSON remain outside SSR; malformed audit metadata degrades to an empty local table.
- This preserves the advisory-only Hermes boundary and performs no experiment approval, activation, promotion, rollback, configuration change, queue, precheck, or broker-order action.

## [2026-08-26] architecture | Type dashboard Hermes learning-memory evidence

- Changed the Hermes Learning Memory table to consume typed, already-redacted lesson status, aggregate observations, cadence, and expiry metadata.
- Raw reflections and internal safety markers remain outside SSR; malformed memory evidence or an unknown lifecycle status degrades to an empty local table.
- This preserves the advisory-only Hermes boundary and performs no lesson promotion, agent invocation, experiment transition, configuration change, queue, precheck, or broker-order action.

## [2026-08-26] architecture | Type dashboard Hermes lesson evidence

- Changed the Hermes Lessons Pending Review table to consume typed, already-redacted lesson text plus optional period, reflection-summary, and source-session metadata.
- Raw reflection/provider payloads and detailed proposed-action documents remain in the local audit store; malformed lesson metadata degrades to an empty local table.
- This preserves the advisory-only Hermes boundary and performs no lesson approval, agent invocation, experiment transition, configuration change, queue, precheck, or broker-order action.

## [2026-08-26] architecture | Type dashboard Hermes reflection evidence

- Changed the Hermes Reflections section to consume typed time, goal version, summary, aggregate finding/action counts, and optional source-session reference.
- Raw Hermes/provider payloads and detailed findings/actions remain in the local audit store; malformed reflection metadata degrades to an empty local table.
- This preserves the advisory-only Hermes boundary and performs no agent invocation, experiment transition, configuration change, queue, precheck, or broker-order action.

## [2026-08-24] architecture | Type dashboard scheduler-cycle evidence

- Changed the Execution-tab scheduler history to consume typed cycle time/status, decision/queue/notification fields, and flattened duration plus two local health labels.
- Retained cycle documents, including provider and detailed operations diagnostics, stay outside SSR; missing or malformed evidence degrades to an empty local table.
- This preserves read-only scheduler observability and performs no scheduler action, data refresh, gate change, queue, precheck, or broker-order action.

## [2026-08-24] architecture | Type dashboard broker-event evidence

- Changed the flat Execution-tab event list to consume only typed time, local order linkage, event type, optional broker status, and a whitelisted local failure-stage label.
- Persisted raw Saxo payloads and free-form broker error text stay outside SSR; missing or malformed evidence degrades to an empty local table rather than fabricating lifecycle detail.
- This preserves read-only event observability and performs no reconciliation, replay, queue, precheck, or broker-order action.

## [2026-08-24] architecture | Type dashboard broker-fill evidence

- Changed Recent Broker Fills to consume typed local order linkage, broker reference, symbol/side/status, quantity/price/currency, and optional ledger-link fields.
- Raw Saxo fill payloads stay outside the SSR model; malformed evidence degrades to an empty local table instead of pretending a fill was reconciled.
- This preserves read-only fill observability and performs no reconciliation, replay, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Decision Pulse lifecycle status

- Changed the Decision Report pulse cards and shared operations banner to consume typed pulse identity, enablement, compact latest/success/failure references, and seven-day attempt counts.
- Prompt, provider, and detailed report fields are discarded at the SSR boundary; absent or malformed status evidence remains explicit as unavailable or unknown health rather than a fabricated success.
- This preserves read-only operational status and performs no report generation, queue, gate, Hermes, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Quiver conflict evidence

- Changed Held-Position Quiver Conflicts to consume typed status, held-symbol count, bearish threshold, review rows, safety, and interpretation fields directly.
- Unexpected provider detail is discarded at the SSR boundary; absent and malformed evidence render explicit not-loaded or unavailable states rather than a fabricated clear result.
- This preserves advisory-only review evidence and performs no collector, scheduler-job, provider, Decision Report, Hermes, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard latest signal-run summaries

- Changed Markov, Quiver, and daily-indicator operations health plus the Markov/Quiver panels to consume typed run availability, lifecycle, coverage, and success/error fields.
- Detailed run configuration and analysis summaries remain staged JSON; absent or malformed rows become an explicit no-run state.
- This preserves read-only run observability and performs no collector, scheduler-job, provider, Decision Report, Hermes, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard scheduled-run timing

- Changed Markov, Quiver, and daily-indicator operational health and Quiver schedule display to consume typed availability, enablement, cadence, and scheduled-target metadata.
- Collector-specific configuration remains outside the dashboard timing model; missing or malformed schedule metadata becomes an explicit unknown state.
- This preserves read-only operational status and performs no scheduler-job, provider, Decision Report, Hermes, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard data-freshness strip

- Changed the dashboard staleness strip to consume typed source identity, owner-tab, observation/age, threshold, and state fields.
- Per-source missing-data degradation remains explicit, so an unavailable table still appears as `missing` instead of suppressing the diagnostic.
- This preserves read-only operational evidence and performs no scheduler, provider-refresh, Decision Report, Hermes, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard sanitized AI settings

- Changed the settings menu to consume typed provider/model provenance and masked API-key status directly.
- The typed SSR contract excludes unknown raw-key fields; malformed settings degrade to config-derived model metadata and a missing-key state.
- This preserves display-only settings behavior and performs no API-key storage, provider request, model-selection, Hermes, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard SSO session

- Changed the top-bar identity and settings menu to use the existing typed header-derived SSO session directly.
- The dashboard retains only the authenticated flag and optional name/email; localization persistence remains on its separate request compatibility boundary.
- This preserves read-only identity display and performs no ngrok OAuth/SSO, user-settings, Saxo, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard sanitized Saxo session status

- Changed the dashboard and Operations health panel to consume a typed display-only Saxo session-status contract.
- The contract retains connection, SIM/LIVE environment, token-validity, expiry, re-auth, and status text while omitting the local session path and credential-adjacent fields at this UI boundary.
- This preserves read-only observability and performs no OAuth, refresh, SIM/LIVE admission, Trading Manager, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard latest-Decision status

- Changed the global Decision health badge and AI Prompts view to consume typed latest-report ID, timestamp, status, model, and error metadata directly.
- Detailed normalized/provider report data remains compatibility JSON; absent or malformed metadata becomes an explicit no-report state.
- This preserves read-only status rendering and performs no report generation, Trading Manager, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Protective Stop Coverage consumption

- Changed the Execution tab's protective-stop audit panel to consume typed coverage status, summary, position/exception/test list boundaries, safety, and interpretation directly.
- Broker- and lifecycle-specific rows remain compatibility JSON; malformed coverage becomes explicit unavailable local evidence.
- This preserves the read-only audit. The separate SIM precheck, placement, cancellation, queue, and broker-order paths are unchanged.

## [2026-08-23] architecture | Type dashboard Decision Gate Replay consumption

- Changed the Decisions tab's Gate Replay and Support/Risk calls to consume the typed replay status, count, scenario-list, safety, interpretation, and staged support-risk boundary directly.
- Scenario and support-risk details remain compatibility JSON; malformed replay evidence becomes an explicit unavailable local contract rather than arbitrary dashboard JSON.
- This preserves read-only historical analysis and performs no Decision Report generation, configuration, Trading Manager gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Watchlists consumption

- Changed the Watchlists tab to consume the typed watchlist refresh metadata and category-list boundary directly.
- Quote- and decision-derived category/item fields remain compatibility JSON; malformed tab evidence degrades to an empty local watchlist rather than arbitrary dashboard JSON.
- This preserves read-only watchlist presentation and performs no quote refresh, candidate-membership, Decision Report, Hermes, gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Trading Manager consumption

- Changed Cash Deployment and Instrument Quarantine to consume the typed Trading Manager availability/latest-run envelope directly.
- The evolving persisted manager-run diagnostics remain compatibility JSON, while malformed overview data degrades to an explicit unavailable local envelope.
- This preserves read-only dashboard observability and performs no decision generation, Trading Manager gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Integrity consumption

- Changed the dashboard Integrity panel and Operations health calls to use a typed outer integrity contract for health, timestamps, acknowledgement count, and staged issue-list boundaries.
- Check-specific findings and acknowledgement metadata remain compatibility JSON; malformed overview evidence becomes an explicit local warning rather than arbitrary dashboard JSON.
- This preserves read-only integrity observability and performs no integrity-check, scheduler, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard market-status consumption

- Changed the dashboard, Market tab, and Operations health calls to use the typed outer market-status contract, passing its summary and price-monitor subdocuments directly to staged diagnostic helpers.
- A malformed status payload now degrades to a local inactive/unavailable shape rather than leaving arbitrary JSON at the dashboard boundary.
- This preserves read-only market observability and performs no Saxo calendar or quote request, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance reconciled SELL outcomes

- Added a typed dashboard model for local reconciled SELL accounting evidence: individual rows, symbol/currency/route aggregates, collecting versus preliminary sample status, totals, and explicit evidence limitations.
- The detailed panel now consumes that typed model directly; missing or malformed SSR evidence renders an explicit unavailable state without fabricating accounting results.
- This preserves read-only Performance accounting evidence and performs no Saxo provider request, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance stored-exposure attribution

- Changed the Stored Exposure P/L Attribution panel to consume typed per-symbol and per-instrument-currency stored Saxo exposure data directly.
- Availability, quantity, reliability, timestamp, account currency, and per-instrument FX basis remain explicit; no-snapshot or malformed SSR data renders an unavailable panel.
- This preserves read-only Performance diagnostics and performs no Saxo provider request, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance unrealised-P/L reconciliation

- Changed the Unrealised P/L Sources panel to consume a typed dashboard/latest-history/stored-Saxo-exposure reconciliation model directly.
- Source availability, snapshot timestamp, account currency, and instrument-currency FX basis remain explicit; malformed or unavailable SSR data produces an unavailable panel rather than a fabricated comparison.
- This preserves read-only Performance diagnostics and performs no Saxo provider request, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance snapshot-evidence consumption

- Changed the Repairable Snapshot Evidence panel to consume `PerformanceSnapshotEvidencePayload` and its retained metadata, item, change, and integrity fields directly.
- Unavailable or malformed SSR evidence now renders an explicit unavailable state; optional composition-change values render as `n/a` rather than fabricated zeroes.
- This preserves read-only historical Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance goal-tracking consumption

- Changed the weekly/monthly target cards and since-reset context card to consume `PerformanceGoalTrackingPayload` and its typed periods directly.
- Ready, pending-baseline, and unavailable SSR states remain distinct; missing baseline evidence cannot render as zero P/L or zero progress.
- This preserves read-only Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance benchmark consumption

- Changed the Performance benchmark panel and each configured ETF-proxy row to consume typed benchmark fields directly, including pending return/close/timestamp/freshness values. The End-of-Day journal validates its persisted compatibility rows before reusing the same typed renderer.
- The SSR boundary preserves an explicit unavailable comparison and keeps the existing native-currency price-return caveat and freshness labels unchanged.
- This preserves read-only Performance behavior and performs no Saxo collection, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance summary consumption

- Changed the Performance metrics, confidence badge, and local drawdown/cost-basis context panel to consume the typed `PerformanceSummaryPayload` directly.
- The SSR boundary preserves explicit absence outside the Performance view and degrades malformed data to an unavailable badge and `n/a` metrics, never to fabricated zero-value portfolio evidence.
- This preserves read-only Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type dashboard Performance history consumption

- Changed the Performance chart/table path to consume `PerformanceHistoryRowPayload` directly, removing generic JSON field reads for all local account-value observations.
- The dashboard validates the complete selected range after its existing summary/benchmark calculations; an invalid legacy row produces an empty chart/table rather than a misleading partial range or fabricated zero fields.
- This preserves read-only Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type latest Performance benchmark run

- Typed the optional persisted benchmark-run metadata: identity, creation time, run date, status, and configured-reference/success/error counts. No run remains an explicit null for disabled and pre-sync states.
- This is local operational coverage evidence only; it neither changes proxy returns nor introduces a benchmark, Hermes, decision, manager, or broker input.
- This preserves read-only Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type selected-range Performance history rows

- Typed the fixed local account-value observation fields: record time, snapshot type, nullable historical source, DKK aggregate/invested/cash/cost-basis/unrealised/daily-P&L values, and position count.
- These rows continue to represent local account-value evidence including cash, not broker-computed time-weighted performance or a live security quote. The dashboard SSR compatibility model remains separately staged.
- This preserves read-only Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type Performance benchmark comparison

- Typed the selected-range benchmark status, account boundaries/return, coverage/freshness counters, caveat, and configured ETF-proxy comparison rows. Pending proxy history remains explicit optional return/close/timestamp/freshness fields.
- The retained benchmark-run record remains compatibility JSON because it is provider/run evidence rather than the stable public read-model contract. The proxy comparison remains native-currency price-return context, not normalized TWR/total return.
- This preserves read-only Performance behavior and performs no Saxo provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type Performance goal tracking

- Typed weekly/monthly local-baseline goal periods and since-reset performance: targets, ready/pending state, P/L, progress, baseline value/time, and since-reset return.
- Goal baselines remain scoped to the active import batch, so portfolio-reset history cannot enter the current periods. Missing baselines remain explicit `pending_baseline` values rather than fabricated zero progress.
- This preserves read-only local portfolio-value context and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type Performance summary and confidence provenance

- Typed the selected-range local-account-value summary: point/timestamp/value/change/daily-P&L/position/range-return/drawdown fields plus the confidence status, valid-point count, freshness, snapshot/source provenance, and scope.
- The summary is explicitly a deterministic projection of persisted/current account-value history. It does not claim broker-computed time-weighted performance or a real-time quote; history rows, benchmarks, and goal tracking remain separately staged read models.
- This preserves read-only Performance behavior and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Complete typed Performance snapshot-evidence diagnostics

- Typed the remaining aggregate/detail integrity fields: absolute/relative tolerance, structural mismatch rows with aggregate/detail counts and DKK deltas, and broker-derived unrealised-P/L difference rows.
- The response preserves the diagnostic distinction between a structural mismatch and the stored broker valuation basis. Performance snapshot evidence is now fully typed while history, benchmarks, and goal tracking remain separate staged read models.
- This preserves local historical diagnostics and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type retained Performance composition-change rows

- Extended the typed Performance snapshot-evidence boundary with opened, closed, and resized symbol rows: before/after quantity, quantity change, and stored DKK market-value/cost-basis deltas.
- The fields remain observational comparisons over two retained snapshots; value movement includes price, FX, and quantity effects and does not assert trades, fills, or causality. Only mismatch rows remain compatibility JSON for a separately bounded conversion.
- This preserves local read-only evidence semantics and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type retained Performance position rows

- Extended the typed Performance snapshot-evidence boundary with retained per-position symbol, optional ISIN, currency, quantity, local price, FX, local/DKK cost basis, DKK market value, and recomputed unrealised P/L.
- These are immutable inputs and derived values from one stored observation, not a fresh Saxo portfolio read. Per-symbol composition-change and mismatch rows remain compatibility JSON for independently bounded conversion.
- This preserves local read-only evidence semantics and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type Performance snapshot-integrity evidence

- Extended the typed Performance snapshot-evidence boundary with aggregate-versus-position integrity status, checked count, structural mismatch count, broker-derived unrealised-P/L difference count, and safety.
- Individual mismatch rows and tolerance details remain compatibility JSON for an independently bounded conversion. Broker-derived valuation differences remain distinct from structural mismatch status.
- This preserves local read-only diagnostics and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type retained Performance composition-change summary

- Extended Performance snapshot evidence with a typed two-snapshot change envelope: current/previous retained metadata, aggregate opened/closed/resized and unchanged counts, and stored DKK market-value/cost-basis movements.
- Collecting states preserve absent snapshot/count fields explicitly. Per-symbol change rows and integrity findings remain compatibility JSON for independently bounded conversion.
- This preserves read-only historical evidence semantics and performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] architecture | Type latest retained Performance snapshot

- Extended the typed Performance snapshot-evidence boundary with explicit latest-snapshot availability, safety, interpretation, and retained metadata: identity, record time, source, position count, and stored DKK totals.
- The collecting state retains an absent snapshot as an explicit optional field. Per-position historical rows, composition-change detail, and integrity findings remain compatibility JSON for independently bounded follow-up conversions.
- This preserves response semantics and is read-only: no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action changed.

## [2026-08-23] architecture | Type Performance snapshot-evidence API contract

- Replaced the public Performance API's generic snapshot-evidence envelope with `PerformanceSnapshotEvidencePayload`. Its stable selected-range coverage, retention, timestamp, safety, and interpretation fields are now compiler-checked.
- Nested historical snapshot, composition-change, and integrity payloads deliberately remain compatibility JSON for staged conversion. The serialized response values are unchanged and a focused API regression pins the typed contract.
- This is an API/read-model boundary only; it performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] observability | Compare retained position compositions

- Extended Performance snapshot evidence with a bounded comparison between the two newest retained per-position snapshots in the selected range. It reports opened, closed, resized, and unchanged-quantity counts plus net stored market-value and cost-basis movement.
- Symbol matching is case-normalized and limited to the retained records. The projection explicitly distinguishes quantity composition from price/FX movement, and does not label a difference as a trade, fill, or causal event.
- This reads local durable snapshot tables only; it performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] observability | Show latest retained position composition in Performance

- Extended repairable snapshot coverage with a bounded historical composition table for the latest retained snapshot inside the selected range. It shows the stored quantity, local price, FX rate, DKK cost basis, DKK market value, and recomputed unrealised P/L for each retained position.
- The panel is explicit that it is stored historical evidence, not a fresh Saxo portfolio read. It intentionally does not fall back to current positions when no retained composition exists, and aggregate charts/valuation retain their existing source semantics.
- This is a local durable-snapshot projection only; it performs no Saxo, provider, Hermes, decision-gate, queue, precheck, or broker-order action.

## [2026-08-23] observability | Show repairable portfolio-snapshot coverage in Performance

- The Performance view and typed API now report selected-range aggregate snapshot coverage by retained per-position evidence. It keeps legacy aggregate-only rows explicit: they remain chartable but cannot be recomputed or repaired from detail that was never stored.
- Coverage includes retained position-row counts, first/latest covered snapshots, the 90-day/full-then-daily retention policy, and the existing bounded aggregate/detail integrity result. Broker-derived aggregate unrealised P/L differences remain a distinct valuation-method observation, not structural drift.
- This is a local read-only projection over durable snapshot tables. It does not call Saxo, a provider, Hermes, gates, queues, prechecks, or broker orders.

## [2026-08-23] correctness | Check aggregate and position snapshot integrity

- The scheduler now compares the latest bounded aggregate snapshots with their linked position evidence. Position count, market value, and cost basis use strict monetary tolerances; a mismatch is surfaced as `attention_required` in the Scheduler Cycles table.
- Aggregate unrealised P/L deliberately remains the broker-derived field while position evidence recomputes it from market value less local cost basis. The diagnostic reports that divergence separately as `broker_derived_unrealised_difference`, preventing a legitimate valuation-method difference from being misrepresented as structural corruption.
- The check reads local durable snapshot rows only and has no provider, Hermes, decision-gate, queue, Saxo precheck, or broker-order authority.

## [2026-08-23] observability | Surface shadow outcome-ledger integrity in Tuning

- The existing shadow pulse table now separates ordinary collection from a completed candidate report that has no corresponding `shadow_report_outcomes` rows. It uses the same pure BUY/SELL/positive-quantity predicate as the bounded scheduler repair, so UI coverage cannot silently drift from ledger eligibility.
- Reference evidence now states captured, retroactively unavailable, awaiting-reference, and missing-ledger counts. A missing ledger report is labelled `shadow_outcome_ledger_gap`; an existing row awaiting its immediate quote remains the distinct, non-terminal `awaiting_saxo_reference_quote` state.
- The projection reads only persisted decision-report and shadow-outcome rows. It neither calls Saxo nor invokes a provider, Hermes, gates, queues, prechecks, or order mutations.

## [2026-08-23] correctness | Start repairable per-position portfolio snapshots

- Added the first `portfolio_position_snapshots` dual-write slice. Each new aggregate portfolio snapshot and its per-position evidence commit together, preventing another aggregate-only history gap from being recorded on a partial write.
- Detail rows retain quantity, local price, FX rate, local/DKK cost basis, recomputed market value, and recomputed unrealised P/L. The aggregate and detail derive from one effective-position read, while current dashboard/performance readers remain unchanged on the aggregate table.
- This records only data already held locally after existing portfolio reads; it does not request Saxo data, alter a queue, invoke a provider or Hermes, or mutate broker orders. Retention and aggregate/detail integrity checking remain the explicitly sequenced next slices.

## [2026-08-23] maintenance | Bound per-position snapshot retention

- The scheduler now keeps full per-cycle position evidence for the 90-day drawdown window and removes only older detail rows that have a later aggregate snapshot on the same UTC date. The final retained observation is described honestly as a stored daily point, not an exchange-close claim.
- Aggregate `portfolio_value_history` is untouched, so current performance/drawdown readers and legacy evidence retain their complete history. The scheduler stores the local prune result in its cycle record for diagnostics.
- This is a bounded local database cleanup: it performs no Saxo, provider, Hermes, queue, gate, precheck, or order action.

## [2026-08-20] observability | Add the first typed Tuning pulse comparison

- Added a lazily loaded, read-only Tuning tab with a typed 30-day pulse-comparison payload for EU open, EU shadow, US open, and US shadow reports.
- The first slice reports report reliability and durable shadow-ledger coverage: candidates, captured reference quotes, 1/5/20-session outcomes, and five-session estimated-after-cost positive rate. It labels execution-eligible rows as outside this initial shadow-ledger outcome join rather than mixing them with simulated results.
- All maturity, window, denominator, as-of, and safety context are surfaced. The path reads local decision/shadow tables only; it has no provider, Hermes, gate, broker, or order authority. Remaining Phase 4 joins stay explicitly scoped by evidence type.

## [2026-08-20] observability | Add separate execution evidence to Tuning

- Extended the typed Tuning payload with a same-window execution-attribution lane for the EU and US opening pulses, reusing the existing bounded local order/fill/ledger/daily-close evidence builder with an explicit 30-day order cutoff.
- The UI shows attributed orders, reconciled BUY fill movement at one/five sessions, and reconciled SELL gain/commission/tax separately. It does not call either result shadow performance, combine currencies, or imply causal impact.
- This is a local read-only projection; it does not invoke Saxo, OpenRouter, Hermes, deterministic gates, or any order mutation. The existing Execution tab keeps its previous recent-history semantics.

## [2026-08-20] observability | Add shadow candidate novelty evidence to Tuning

- Extended the same typed, 30-day shadow pulse rows with canonical-symbol new-versus-repeat counts against the persisted same-market opening report.
- Candidates with no earlier opening reference are excluded from the novelty denominator, and a zero-candidate shadow report remains distinct from a `no_new_information` assessment.
- The dashboard reads only local persisted shadow evidence; it adds no provider, Hermes, gate, queue, broker, or order authority.

## [2026-08-20] observability | Add shadow decision-time gate evidence to Tuning

- Added a separate shadow-only Tuning lane that reports candidate counts by persisted decision-time gate source and result, including an explicit unclassified count for unknown historical records.
- The lane is a compact technical/Markov replay over saved prompt evidence, not a Trading Manager approval, queue state, broker precheck, or execution simulation.
- It reads local persisted outcome rows only and adds no provider, Hermes, gate, queue, broker, or order authority.

## [2026-08-20] observability | Add shadow Hermes record-only evidence to Tuning

- Added a separate shadow-only Tuning lane for persisted record-only Hermes effects, context-self-check coverage, approved-policy-source coverage, and unknown legacy effects.
- `allow`, `reduce`, `stand_down`, and `review` remain advisory labels only; they do not describe a manager result, prevented quantity, broker action, or simulated execution.
- The lane reads local persisted outcome rows only and adds no provider, Hermes, gate, queue, broker, or order authority.

## [2026-08-20] observability | Add normalized shadow-change evidence to Tuning

- Added a shadow-only Tuning lane for the server-normalized change assessment: material change, explicit no new information, unavailable opening reference, not applicable, invalid, missing, and unknown states.
- The no-new-information rate has an explicit available-comparison denominator; candidate count or absence cannot manufacture an assessment state.
- The lane reads local persisted Decision Reports only and adds no provider, Hermes, gate, queue, broker, or order authority.

## [2026-08-20] observability | Add shadow Support/Risk context to Tuning

- Added a shadow-only Tuning lane for saved decision-time Support/Risk snapshot coverage, low/moderate/high break-risk buckets, and complete-context average break risk, confidence, and history coverage.
- Missing and unknown snapshots remain visible, and the table is labelled observational context rather than a forecast, gate, or execution signal.
- The lane reads local persisted shadow outcomes only and adds no provider, Hermes, gate, queue, broker, or order authority.

## [2026-08-19] safety | Exclude pending Hermes experiments from advisory context

- Closed Phase 0 prerequisite 1 from the shadow-report decision record. `pending_review` proposals remain visible to operators through the dashboard/API and continue to block duplicate proposals, but their changed variable and proposed values are no longer supplied to Hermes advisory context.
- The Trading Manager preflight and MCP `list_experiments` projection now carry only operator-approved/active lifecycle rows plus an audit-only pending count. Prompt instructions explicitly prohibit using pending-review values for `allow`, `reduce`, `stand_down`, or `review` advice.
- Added pure, database-backed, manager-preflight, and MCP-contract regressions. This does not change broker authority, order placement, strategy overlays, or the normal operator lifecycle flow.

## [2026-08-19] safety | Expire stale discretionary orders before Saxo submission

- Closed Phase 0 prerequisite 2 from the shadow-report decision record. A completed report can become stale while its order waits for the next market session or virtual-capital condition; the executor now expires that discretionary queue row after the explicit 360-minute policy rather than automatically submitting old intent.
- Expiry runs before the executor opens a Saxo session, fetches quotes, prechecks, or places an order. It records terminal `expired_local` state, a sanitized `expired_before_submission` event, taxonomy/remediation, and a zero-broker-mutation audit flag. Concurrent claims remain protected by the terminal update condition.
- `protective_stop` GoodTillCancel rows, already broker-submitted rows, and ambiguous broker states remain outside this timer. Dashboard and Slack failure observability distinguish local queue expiry from broker DayOrder expiry.

## [2026-08-04] ui | Split the Hermes tab into sections with per-section data loading

- The tab rendered nine sections in one 335-line scroll and ran eleven separate queries on every load regardless of what the operator was looking at. Now Overview / Advice / Reflections / Experiments / Baselines, as plain links carrying their own query string.
- The substance is the data gating, not the visual grouping: each of the eleven datasets is bound to the one section that renders it, extending the existing per-tab lazy-read policy a level deeper. Measured live: default Overview is 22.5 KB against roughly 327 KB for all sections combined, about a 93% reduction on first load. Advice (165 KB) and Reflections (88 KB) are the heavy ones and are now paid for only when opened.
- **The header pills had to be gated the same way, and this was the subtle part.** They read counts from all eleven datasets; with only the active section loaded, sitting on Baselines would have rendered "Reflections: 0" when twenty exist. A zero that means "not loaded" reads as "none exist" — a worse defect than the one being fixed. Each section now shows only the pills whose data it loads. Verified live: Overview shows only `One-variable: 1`, Advice only `Counterfactuals: 19`, Baselines no count pills, Reflections the true `Reflections: 20 / Lessons: 28`.
- A test asserts every dataset gates to exactly one section — gated to none renders a permanently empty table, gated to several defeats the split — and that nothing loads while another view is active.
- 543 tests pass; fmt and `-D warnings` clean; 0 smoke warnings.

## [2026-08-04] ui | On-demand broker event timeline for execution orders

- Answers "what actually happened with this order" in the UI — the question the operator had to ask in chat on 2026-08-03 about orders 272-274.
- Built as an inline expandable cell, not the modal the roadmap wording suggested. The Execution row already carries the order's own fields across 16 columns and the Error column already holds broker error details with the failure-stage taxonomy; the only thing genuinely missing was the broker lifecycle, and an expandable cell keeps it beside the row it describes rather than behind a dialog.
- `GET /api/execution/orders/{id}/events`, loaded on demand following the existing `data-decision-debug` pattern. **Per-order rather than filtering the dashboard's flat event list client-side**, because that list is capped at 50 rows — any order older than the most recent handful would have rendered an empty timeline and looked like an order that never reached the broker.
- The endpoint's column list is an allowlist rather than `SELECT *` minus a few fields: `raw_payload_json` holds unredacted Saxo responses and `account_uid` identifies the account. A test asserts neither is served and that adding a column to the table cannot silently add it to the response — verified live across four orders with zero leaked fields.
- Events are ordered oldest-first because this reads as a timeline (placement to terminal state), unlike the newest-first flat list. An order with no events says so explicitly rather than rendering blank: never submitted to the broker is a real state, not a failure.
- Verified in a browser, not just by tests: 25 timeline cells bound; expanding order 285 renders `queued_by_trading_manager` 14:55:42 → `submitted_to_broker` 14:55:43 (broker id 5039464790) → `broker_final_fill` 14:55:59 (FinalFill · Confirmed · qty 6 · @ 193.12); no console errors; reopening does not re-fetch (one request, content retained), confirming the `loadState` guard.
- 541 tests pass; fmt and `-D warnings` clean; 0 smoke warnings.

## [2026-08-21] correctness | Benchmark returns rendered 100x too large; open the todo page

- The Performance tab's Benchmark Comparison showed the portfolio at **+50.7% for a single day** and the Dow at +87.8%. `return_pct` in `performance_benchmarks.rs` yields percentage *points* -- the live value was `0.5068`, meaning 0.51% for a 243,172 -> 244,405 day -- and the row formatted it with `format_signed_pct`, which multiplies by 100.
- A correct helper already existed and was documented for exactly this case (`format_signed_percentage_points`, "formats an already-percent-valued quantity"). The newer Tuning tab uses it correctly; only the older Performance panel did not. Test asserts precision-independently that a sub-1% day can never render as double digits.
- Also fixed earlier the same day: resting protective stops rendered "error: ConocoPhillips" because `execution_status_detail` searched diagnostics for anything error-shaped even on healthy orders, and the search matched Saxo's `Description` key -- display metadata, never error text. Quarantine was unaffected; it keys on the `error_text` column, correctly NULL throughout.
- Opened `wiki/todo.md` for work that is neither a defect nor roadmap-scale: **T1** the three review queues that nothing empties (14 of 20 holding theses due, Hermes proposals expiring unjudged, missed-trade shadows with no review step), and **T2** whether the strategy has an edge -- 3 wins in 20 closed trades, -21,372 DKK since the 07-16 reset, with the system's own forward-movement evidence rounding to zero at five sessions.
- Noted in T2 as the most actionable lever: `shadow_report_outcomes` and `shadow_report_outcome_quotes` are both at 0 rows despite the 25-commit tuning build. That infrastructure is the right answer to having more signal sources than closed trades, and it cannot help while empty.
- 603 tests pass; fmt and `-D warnings` clean.

## [2026-08-06] correctness | Mark the corrupt cost-basis window, fix performance range truncation

- **Range truncation was worse than "the selector does nothing".** `performance_history_for_range` applied `ORDER BY recorded_at ASC` with a row-count `LIMIT`, so it kept the *oldest* rows in each window. Every range beyond about a month returned the same early slice with today's point appended: 3M, YTD, 1Y and ALL rendered identically, and 1M reported a -46,562 DKK "change" measured against a row three weeks before the window it claimed to cover. Now downsamples on `id % stride` to span the whole window at bounded resolution (1,500 points), re-appending the newest row since sampling can drop it. Modulo rather than a window function, to stay portable across both backends `AnyPool` serves.
- **The 2026-06-03 → 2026-07-09 cost-basis corruption is marked, not repaired.** ~6,300 snapshots hold 5.4M-26.9M DKK against ~240k invested. Detection is data-driven (`cost_basis_is_plausible`, ratio > 3x) rather than a hardcoded date range, so it marks whatever is actually corrupt. The threshold deliberately separates "under water" from "arithmetically impossible" — a real book can carry cost above market value after a drawdown, and a test pins both sides.
- Repair is impossible, and that is the real finding: `portfolio_value_history` stores only aggregates, so there is no per-position detail to recompute from. The same limitation blocked the unrealised-P/L backfill after the 2026-08-06 broker-currency bug. **An aggregate-only snapshot converts any upstream arithmetic bug into permanent history** — both incidents were fixed at source within days and both left a scar.
- Filed a schema proposal on the roadmap (`portfolio_position_snapshots`) whose central property is that every stored DKK figure is reproducible from `quantity x price_local x fx_rate_to_dkk`. The argument is precedent, not theory: the 2026-08-06 FX-split repair was possible only because `trade_ledger` already stored both rates per row. Includes retention sizing (~1M rows/year) and a four-step sequence whose first slice — dual-write, change no reads — stops the bleeding on its own.
- 550 tests pass; fmt and `-D warnings` clean.

## [2026-08-04] ui | Server-side filters for the Markov signals table

- The table carries ~200 signals across pages since the U11 instrument fix restored the universe, so finding held positions, gate-clearing signals, or the one remaining failure meant paging through everything.
- Filters are plain links carrying their own query string: no JavaScript, and every filtered view has a shareable URL. Changing filter resets to page 1 by omitting the page param.
- The load-bearing design choice: the page query and the count query share one `markov_filter_sql` function. If they ever applied different predicates the pagination control would advertise pages that render empty — worse than shipping no filter. A test asserts every filter yields a single predicate that extends the shared `WHERE` rather than replacing it. The paging links carry the active filter for the same reason.
- "High conviction" compares against `strategy.swing.markov_gate.min_signed_signal` — the threshold the Trading Manager actually applies — so it means "would clear the gate", not an arbitrary display cutoff. A non-finite or non-positive configured threshold clamps to 0 rather than becoming a negative bound that would admit errored rows.
- **"Stale signals" from the roadmap was deliberately not implemented.** Every row in a run shares that run's `run_date`, so staleness is a property of the run as a whole, never of one signal against its siblings; a per-row stale filter would match everything or nothing, and presenting that as a choice would mislead.
- The filter value reaches SQL, so it is validated against an allowlist rather than sanitized — a test pins that an injection-shaped value falls back to `all`.
- Verified live against production: All 201, Portfolio 18 (exactly the 18 held positions), Watchlist 183, High conviction 117, Errors 1 (SPCX, with its full diagnostic). 18 + 183 = 201 confirms portfolio/watchlist partition the run with no gap or overlap. Paging checked on the conviction filter: page 2 of 3 reports "117 matching High conviction", and the prev/next hrefs carry `markov_filter=conviction`.
- 540 tests pass; fmt and `-D warnings` clean; 0 smoke warnings.

## [2026-08-04] risk | Widen the drawdown halt to 25% (U9), and fix two Hermes contract drifts it exposed

- Operator decision after a recap of the options: widen `strategy.capital.drawdown_halt_pct` 0.20 -> 0.25 in both shipped configs. The book was at 19.37% against the 20% floor (peak 297,463 DKK on 2026-06-30, current ~240,300), roughly a 1% day from suspending all BUYs. Halt threshold moves 237,970 -> 223,097 DKK.
- The soft band stays at 0.10 deliberately -- it is already active and halving the cycle BUY budget, so only the hard stop moved and caution is retained. `DEFAULT_HALT_PCT` in `src/drawdown_guard.rs` also stays 0.20: it is the fallback for missing config, and falling to the stricter value is the correct direction for a risk control.
- **The change exposed two genuine drifts in the Hermes goal contract.** `deploy/k8s/base/hermes.yaml` embeds `SELF_IMPROVEMENT_GOAL.yaml` in the `hermes-daytrader-context` ConfigMap, mounted into the Hermes pod at `/opt/daytrader-context` -- a *second, static copy* of the goal contract that the U3 guarantee ("the contract reads the same key the gate applies, so the two cannot drift") does not cover, because that guarantee only holds for the Rust `hermes_goal_contract_value`. It still carried `max_drawdown: 0.20`, which after this change would have told Hermes a limit the runtime no longer enforces -- exactly the U3 failure class, reappearing through a copy nobody had checked. It also still carried `gas_reserve: 0.05`, which U3 recorded as deleted but had only been removed from the Rust side. Both corrected, with a comment on the file naming the invariant.
- `docs/hermes-agent.md` had drifted further still: `max_drawdown: 0.20`, `gas_reserve: 0.05`, `min_cash_buffer_pct: 0.10` (the deployed value is 0.02), and prose describing "the 47% 30-day return target" -- the objective corrected to 0.0117 on 2026-07-25. All fixed.
- Verified no `0.20` drawdown value remains in any shipped config, manifest, or doc. `kubectl kustomize` builds clean; 536 tests pass; fmt and `-D warnings` clean.
- **Unchanged and still open: there is no re-entry rule.** Widening moves the cliff, it does not define how a halted book resumes. That should be settled before the 25% floor is ever reached.

## [2026-08-03] ui | Make protective stops legible in the Overview Execution Queue

- Direct evidence from this session: the operator saw orders 272-274 on the Overview tab and had to ask what they were. The table could not answer -- it rendered `SELL / broker_working / Limit: n/a / GoodTillCancel`, which reads as a mysterious resting sell rather than the automatic ATR protective stops they actually were.
- Two separate problems, both fixed. (1) The price column bound `limit_price_local` only, so every stop order showed `n/a` despite carrying a real `stop_price_local` -- the column was not merely uninformative, it asserted the order had no price. `execution_order_trigger_price` now resolves the governing price by order type, labels it `Stop` or `Limit`, and returns an empty kind for market orders so a blank value is never mislabelled. Header renamed `Limit` -> `Trigger`. (2) Nothing distinguished an automatic protective stop from a decided sell; a compact `protective` tag now marks them, keyed on `strategy_type` (set by the runtime at insert, never by the model, so it is provenance rather than a heuristic on price or action).
- Backend untouched: `execution_orders_page` already selected `order_type`, `stop_price_local`, and `strategy_type`. This was purely the UI failing to render data it already had.
- Note the Execution tab was already adequate here -- it has 15 columns including Strategy and Order Type. Only the 8-column Overview table, which is what the operator was actually looking at, lacked the distinction.
- Four tests covering the real order-272 shape, limit orders, market orders (must claim no price kind), and an unknown/missing `order_type` falling back to whichever price exists. 536 tests pass.

## [2026-08-03] correctness | Read position decisions from decision reports, not retired Python tables

- User asked why the Overview "Decision" column showed `n/a` for BAKKA, DANSKE, EQNR, DEMANT, and ALMB. Root cause was not those five symbols: `latest_symbol_decisions` read from `swing_sentiment_snapshots` and `swing_position_targets`, neither of which any Rust code writes -- no `INSERT` for either exists in `src/`. Both are frozen at `report_id = 12` (2026-05-08), written by the retired Python runtime.
- The query selected "the most recent report that has rows in those tables," which therefore always resolved to report 12 regardless of the 191 newer reports. Consequences: every position inside that one report's fixed ~82-symbol US large-cap universe rendered a chip whose age only ever grew ("Stale · HOLD, 87d old"), and every position outside it -- the Nordic holdings especially -- rendered `n/a` permanently. Same failure class as the `audit_log` table dropped earlier today: a Python-era table nothing in Rust populates, still presented as live.
- `report_json` already carries the live equivalents. `symbol_sentiment` matches the sentiment/confidence/rationale half exactly; `suggested_trades` supplies the action and the `strategy_metadata.technical` snapshot the trend sparkline reads via `source.technical`. Now scans the 40 most recent reports newest-first, keeping the first entry per symbol, so each symbol carries its own report's timestamp and the existing 7-day staleness horizon applies per symbol instead of uniformly to one frozen report.
- A suggested trade may only enrich an entry its own report created. Pairing a newer report's sentiment with an older report's action would present two different moments as one decision.
- Bounded to 40 reports (~4 weeks at two weekday pulses) because this runs on dashboard load; anything older should read as absent rather than as very stale advice. JSON is parsed in Rust rather than SQL to stay portable across the SQLite and PostgreSQL backends `AnyPool` serves -- `json_array_elements` is Postgres-only.
- Verified against production before deploying: all five reported symbols resolve (EQNR BUY today, BAKKA/DANSKE BUY and DEMANT HOLD 07-31, ALMB BUY 07-30). `V:xnys` is the clearer win -- it displayed a 3-month-old `HOLD` while its actual latest view is a `BUY` from 07-30, recent enough to render as current rather than stale.
- Three new tests: newest-report-wins with per-symbol timestamps, the same-report enrichment boundary, and malformed/missing payload tolerance. 532 tests pass.

## [2026-08-03] operations | Drop the two dead swing_* tables

- Operator confirmed dropping `swing_sentiment_snapshots` and `swing_position_targets` after the `latest_symbol_decisions` fix (earlier today) moved off them onto live `report_json` data.
- Same sequencing as the `audit_log` drop: found and removed a live reader first, deployed, verified, only then dropped. The reader was a second, older code path in the Watchlist builder that queried `swing_sentiment_snapshots` directly (separate from `latest_symbol_decisions`) -- fully redundant with the fixed path above it, and its only unique behaviour (a `legacy_archive_fallback` placeholder row) was already inert in production since the watchlist universe is configured (`legacy_archive_fallback = configured_universe.is_empty() = false`). Removing it also stopped a wasted round-trip and future log noise the drop would otherwise have caused on every Overview/Watchlist load, since `unwrap_or_default()` would have silently swallowed the resulting "relation does not exist" error rather than surfacing it.
- Dropped both tables on the primary (`daytrader-postgres-2`). Database 109 MB -> 108 MB (small; these were always tiny compared to the deleted `audit_log`). Verified: both `to_regclass` calls return null, scheduler cycle still `ok`, 0 pod restarts, no errors in logs, and the live `/api/portfolio/positions` endpoint still returns all 13 positions with a decision -- confirming the fix truly no longer depends on these tables at all.
- 532 tests pass throughout; no test referenced either table.

## [2026-08-03] operations | Fix the ENS activity backfill's 14-day boundary bug

- User asked for a diagnostics pass on the day's activity. `make diagnostics` and a direct check of `scheduler_cycle_history` surfaced `ens_activity_backfill` failing every cycle since 2026-08-03T09:09:25Z with `"Saxo GET failed: The request is invalid!"` -- unrelated to anything landed earlier today, so worth chasing rather than dismissing as noise.
- Reproduced the exact call live against SIM: `FromDateTime` computed as `now - 14 days` gets `400 InvalidRequest`, `ModelState.FromDateTime: ["Maximum 14-days old activities can be fetched."]`. The boundary is exclusive -- a request landing exactly on the 14-day line is rejected -- so the bug was latent since the feature landed 2026-07-26 and only started firing once the wall clock ticked past the original request's time-of-day on the 14th day.
- Confirmed a 13-day lookback returns 200 with real data before changing anything. Extracted the computation into `ens_activity_backfill_from_datetime`, taking `now` as a parameter for a deterministic test, and gave it a one-day margin. Read-only endpoint, sanitized aggregate storage only -- unaffected by execution-critical paths.
- One new test pins the gap strictly under 14 days; 529 total tests pass.
- Also verified today's other landed fixes against live data while diagnosing: U16 (FX cache fresh, `expires_at` in the future), U13 (`last_analyze` fresh from this morning's deploy), U12 (today's real decision-report prompts are 298-362 KB versus 479-545 KB on 07-31, a 34-38% reduction matching the isolated-field estimate). U11's fix is deployed but unproven against a live run -- the last Markov sweep predates it; tonight's 23:30 CET run is the first real test. U9 pulled back slightly to 18.98% (was 18.999%), still soft_reduce not halted.

## [2026-08-03] review | Investigated the 1-2 day round trips before building a churn guard

- Chose "Trading quality" as the next roadmap direction. Before implementing the "cooldown and churn guard" idea, investigated whether the three fast round trips flagged in the 2026-08-02 review (`AJG`, `JNJ`, `DSV`) were actually the pattern they looked like -- the roadmap's own note said to determine the cause before adding a rule.
- Checked `execution_orders.strategy_type` for each closing SELL rather than assuming holding period alone meant indecision. `AJG` and `JNJ` both closed via `protective_stop` -- the ATR stop firing correctly, exactly the behavior U1 was built for.
- Widened the query to every fast round trip in the trading history (correctly paired to each symbol's most recent preceding BUY, not a flawed lifetime-first-BUY join that undercounted multi-cycle symbols). Found 9 total: 3 were one coordinated portfolio-wide flatten from a single report two days after the 2026-05-05 launch import (a one-time bootstrap event), 3 were `strategy_type: manual` (operator-placed, not automated), 2 were the protective stops already found, and exactly **1** (`DSV`) was a genuine discretionary same-week reversal, with an explicit rationale rather than an unexplained flip.
- Conclusion: do not build the cooldown/churn guard on this evidence. n=1 is not a pattern, and two of the original three examples were the safety system working correctly, not failing. Updated both `urgent-todo.md` and `roadmap.md` with the finding and the concrete signal to watch for if it should be revisited (recurring discretionary reversals specifically, tracked by `strategy_type`, not raw holding period).
- No code changed; this is a diagnosis that prevented building the wrong thing, not a landed feature.

## [2026-08-03] operations | Drop the dead audit_log table (U14)

- Operator confirmed dropping `audit_log` (65 MB, 38% of the database, no writes since 2026-05-10, nothing in Rust reads it) -- a destructive production DB operation, so this was not executed on the standing "continue with the next item" instruction alone; explicitly asked first.
- Found and fixed a real prerequisite before touching production: `tune_append_heavy_table_autovacuum` (landed earlier today for U13) runs `ALTER TABLE audit_log SET (...)` on every pod startup. Dropping the table first would have crash-looped every future pod restart or rollout on a missing table. Removed `audit_log` from that list, deployed (`44a0945`), confirmed all four deployments rolled out clean with 0 smoke warnings, only then dropped the table.
- `DROP TABLE audit_log` against the primary (`daytrader-postgres-2`, confirmed via `pg_is_in_recovery()` before writing). Database size 172 MB -> 109 MB. Verified: `to_regclass('audit_log')` returns null, the scheduler's next cycle still completed `ok`, no pod restarts, no errors in either API or scheduler logs in the five minutes after.

## [2026-08-03] hygiene | Execute the Python removal plan

- Ran the removal plan drafted earlier in `wiki/urgent-todo.md`, one commit per step so any part is independently revertible: `e5394fb` (AGENTS.md), `bc679ae` (phase validators), `b27ae6f` (systemd/launchd + renderer), `dfc77e0` (main.py/web_main.py/run_scheduler.py), `b43f0b7` (src/saxo_daytrader_xai/), `790fa8a` (requirements.txt), `e7fc99e` (SQLite migration one-shot).
- Checked both caveats the plan named before deleting the package, rather than assuming they were fine. The FX-attribution formula was ported this session as part of U10. The tax-bracket calculation turned out to already be ported (`share_income_tax_due_dkk` in `state.rs`) -- the roadmap's "hardcoded to 0.0" note was itself stale, describing only the unavailable-status fallback.
- AGENTS.md needed more than the Python-removal plan anticipated: it still claimed broker sync/reconcile/cancel "use the legacy Python code" and carried an 8-step porting order where every step was already done, including order cancel (implemented as cancel-and-reissue via `saxo_delete_json`, confirmed before writing the correction).
- Found 11 more Python scripts under `scripts/` the original plan never enumerated (diagnostic/manual tools like `saxo_oauth_helper.py`, `reset_portfolio_baseline.py`). Left them alone -- unlike the phase validators, these look like they could still be run by hand, and deleting on a guess isn't reversible in the way that matters (git history doesn't restore an operator's confidence that a tool they relied on didn't just vanish silently).
- Found README.md is far more Python-era than the plan's single-line estimate: 848 lines, ~91 `.py` references, ~27 Rust mentions, a wrong config path, and a stale claim about `audit_log` still being written. Spawned a separate follow-up task for the full rewrite rather than folding an 848-line rewrite into a cleanup pass.
- `audit_log` drop (U14) is the one item left in the plan, deliberately not executed -- a destructive production DB operation needs explicit confirmation, not a "continue" instruction.
- No Rust code changed; `kubectl kustomize deploy/k8s/base` still builds clean; no deploy needed.

## [2026-08-03] review | U15's two most promising Saxo endpoints are unusable in SIM right now, for data reasons not access reasons

- Set out to build the `/port/v1/closedpositions` ledger cross-check U15 recommended first. Before writing code, checked what the endpoint actually returns: 6 rows total, all `stop-test:*` closures from manual protective-stop testing on 2026-07-30/31. None of the other 39 SELLs in `trade_ledger` (back to June) appear. `FromDate`/`ToDate` query parameters don't change the count -- this is the account's real recorded history, not a lookback-window default.
- Checked `hist/v4/performance/timeseries` the same way rather than trusting the earlier "verified reachable" note. Its 5 points run 2021-03-16 to 2021-03-22 -- stale SIM provisioning data, zero points after 2026-06-01.
- Building either cross-check now would compare our ledger against data Saxo never recorded for this account, producing false-positive divergence alerts rather than real findings. Declined to build it; updated U15 to record why and what would make it actionable (more SIM trading history for closedpositions; a LIVE-environment check, out of scope, for performance/timeseries).
- No code changed. This is worth recording precisely because the earlier "verified reachable, 200 OK" note in U15 was true but incomplete -- reachability isn't the same as usable data, and the gap between them would have produced a broken feature if built on the earlier note alone.

## [2026-08-03] performance | Stop duplicating the Markov run payload into the decision prompt (U12)

- The original diagnosis (recent_labels reaching the prompt via the per-symbol context) was wrong in its specifics, though right that Markov data was the bulk source. `compact_markov_context`'s own `signals` list was already properly trimmed. The actual source was a second field, `latest_run`, which embedded `markov_signal_runs.summary_json` verbatim -- and that row deliberately carries up to 20 full signal objects with `raw_payload_json.recent_labels` for operational debugging.
- Measured directly against the live production row rather than estimated: `summary_json` was 189,664 bytes, of which 189,053 bytes (99.7%) was the embedded `signals` array alone -- 35% of the entire 527 KB average prompt from one nested field.
- `trim_markov_run_for_prompt` strips only that array, keeping the run-level metadata (status, run_id, counts, config) that is genuinely useful for judging signal freshness. Lands for all three consumers of `compact_markov_context` at once: the decision prompt, the MCP tool, and the Hermes evidence pack.
- Checked both sibling modules for the same defect before assuming it was Markov-specific. `daily_indicators.rs` was already clean -- its `latest_run` query never selects `summary_json`. `quiver.rs` has the identical pattern but the cost is small (10,418 bytes, ~2% of the prompt); left alone and noted as a minor follow-up rather than bundled in.
- Four new tests reproduce the real production shape (a run row with 20 embedded signals × 62 recent_labels each) and assert an 80%+ size reduction plus that run-level metadata survives; plus null/malformed-input handling. 528 tests pass; `cargo fmt --check` and `RUSTFLAGS="-D warnings" cargo check --all-targets` clean.

## [2026-08-03] operations | Fix stale query-planner statistics (U13)

- Every `pg_stat_user_tables.last_autoanalyze` in production dated to 2026-06-30, 33 days stale at the time of the review. The planner believed `audit_log` held 0 rows where it held 67,578, `trade_ledger` 47 where it held 118, `decision_reports` 85 where it held 137.
- `AppState::tune_append_heavy_table_autovacuum` now runs an immediate `ANALYZE` and lowers `autovacuum_analyze_scale_factor` to 0.02 with `autovacuum_analyze_threshold = 50` on nine append-heavy tables, so a much smaller amount of row-count drift is enough to trigger a re-analyze than Postgres's 0.10 default.
- Deliberately per-table rather than a CNPG Cluster-level default: a cluster-wide autovacuum setting needs a CNPG reconcile and affects every table, not just the ones actually accumulating stale statistics; per-table `ALTER TABLE ... SET (...)` applies immediately via ordinary DDL.
- Guarded to Postgres only via a new `database_url_is_postgres` helper, since SQLite (local dev, every other test) has no autovacuum and does not accept the syntax. Runs on every pod startup as part of the existing schema-migration function, consistent with its idempotent neighbours.
- `audit_log` is included even though U14 plans to drop it — tuning costs nothing and covers the case where that deletion lands later than this fix.
- 525 tests pass; `cargo fmt --check` and `RUSTFLAGS="-D warnings" cargo check --all-targets` clean.
- Verified live post-deploy against `daytrader-postgres-2` (the actual primary — `daytrader-postgres-1`, used for every read-only query earlier in the session, is currently the standby; `pg_stat_user_tables.last_analyze`/`last_autoanalyze` are per-instance activity counters that do not replicate, unlike `pg_statistic` itself, so they must be read from the primary to mean anything). `n_live_tup` now reads correctly — `audit_log` 67,578 (was 0), `trade_ledger` 118 (was 47), `decision_reports` 137 (was 85) — `last_analyze` is fresh from the deploy, and `reloptions` carries the tightened settings on all nine tables.

## [2026-08-02] operations | Decouple FX rate refresh from market hours (U16)

- Found while closing U10: all six major currency pairs in `currency_fx_rates` had not refreshed since 2026-07-31T19:39:20Z against a 30-minute TTL — over two days stale in production, silently.
- Root cause: `refresh_best_effort_fx_rates` was only reachable through `refresh_portfolio_prices` (`src/price_monitor.rs`), which returns early whenever every watched exchange is closed or the Saxo session is unavailable — both before the FX refresh call. FX trades nearly continuously; the equity exchanges this runtime watches do not, so a plain weekend silently exhausted the cache. Every downstream conversion then fell through to `static_fx_rate_to_dkk`, a literal pinned to 2026-07-02 and roughly 8% off live USD/DKK, with no signal that it had happened.
- `crate::fx::run_fx_rate_refresh_cycle` now runs unconditionally from the main scheduler cycle (`src/scheduler.rs`), which fires every 10 minutes regardless of market hours or weekday, ahead of any broker or ledger read in the same cycle. The existing 30-minute cache TTL throttles the actual network call, so this keeps rates within about half an hour of live — tighter than the hourly cadence requested, for free, since it reuses a cadence that already exists rather than adding a second one.
- A missing or expired Saxo session now degrades straight to the ECB daily fallback instead of skipping the step, so a broken session no longer also stops FX from updating.
- The price-monitor call site is untouched; the two refreshes are redundant by design, both gated by the same cache TTL so neither over-calls Saxo.
- 524 tests pass (unchanged — this is a scheduling wiring fix, not new pure logic; the codebase's existing tests don't mock the network-touching refresh calls, consistent with `refresh_saxo_fx_rates`/`refresh_ecb_fx_rates` having none either). `cargo fmt --check` and `RUSTFLAGS="-D warnings" cargo check --all-targets` clean.

## [2026-08-02] strategy | Attribute realised gains to price and currency (U10)

- `crate::fx::split_realised_gain` replaces a `fx_gain_dkk` column that was a hardcoded `0` literal in the ledger insert while `price_gain_dkk` received the whole realised gain. Every sale therefore reported 100% price and 0% currency by construction, on a book that is 63% USD during a month when USD/DKK fell 7.66%.
- The decomposition is exact rather than approximate: price is `(net_local − cost_local) × sale_rate`, currency is `cost_local × (sale_rate − cost_rate)`, and the two sum to the realised gain. A helper asserts that identity on four real production rows covering gains, losses, and both FX directions.
- A second defect surfaced while fixing the first. `realised_gain_local` was computed as `realised_gain_dkk / sale_rate` — the DKK gain restated at the sale rate, not the gain in local currency — which made any price/FX split circular. It is now `net_local − cost_local`.
- A startup backfill recomputes historical rows from columns already on each row, so it performs no rate lookup and cannot drift with current FX. It derives the cost rate as `cost_basis_sold_dkk / cost_basis_sold_local` rather than reading `cost_basis_fx_rate_to_dkk`, because that column is 100x too small on rows written before 2026-07-09 by the legacy Python path; trusting it would have produced roughly +3,095 DKK of fictional currency gain on one 2,356 DKK profit. A test pins that discrimination.
- Both the split and the backfill degrade to the previous behaviour rather than to a wrong answer: with no usable local cost basis there is nothing to attribute currency against, so the whole gain stays classified as price and `fx_gain_dkk` stays zero.
- The first production backfill run exposed two things the initial implementation had wrong, both now fixed. Scoping to `fx_gain_dkk = 0` skipped exactly the worst rows — the retired Python wrote its own values against the same corrupt rate column — so the price/currency identity stayed broken on them; the backfill now recomputes every SELL row and is idempotent. And a corrupt cost basis produces a split that is internally exact while being entirely fictional, which is the real hazard: production holds derived cost rates of 128.2545 and 31.8992 against a ~7.02 sale rate, plus exact zeros. A cost rate outside a 2x band either side of the sale rate is now refused, since no currency here moves by half between purchase and sale. 41 of 45 sales attribute cleanly, 3 are refused, 1 lacks a local basis.
- Noted while reading the module: `static_fx_rate_to_dkk` hardcodes USD at 7.0215, about 8% stale against today's 6.4837, and is used silently on any cache miss. It is the likely source of the `7.0215` sale rates on older ledger rows.
- 523 tests pass; `cargo fmt --check` and `RUSTFLAGS="-D warnings" cargo check --all-targets` clean.

## [2026-08-02] strategy | Restore 27 unresolvable universe symbols (U11)

- Corrected every symbol the Markov and daily-indicator sweeps have been unable to resolve: 20 Stockholm names moved from the `:xsto` suffix to Saxo's `:xome`, five tickers corrected (`SAP`→`SAPG`, `DB1`→`DB1Gn`, `SCHP`→`SCHO`, `SHOP`→`SHOP_NEW:xnas`, `AKRBP`→`AKERBP`), `WMT` moved to Nasdaq, and `NZYM-B` replaced by its merger successor `NSIS-B` (Novozymes into Novonesis). Each replacement was verified individually against live SIM `/ref/v1/instruments`.
- `SPCX:xnas` was deliberately left as-is. It is a documented pending entry — SpaceX listed on Live 2026-06-12, Saxo SIM reference data has not synced — carrying an ISIN and activating automatically. It is the one member of the failing set that is not a defect.
- `exchange_id_for_suffix` returned ISO MICs where Saxo's `ExchangeId` is a proprietary code, so the exchange-scoped fallback in `lookup_instrument` had never matched anything in any of its fifteen cases; verified live that `ExchangeId=XSTO` returns an empty set where `SSE` returns Stockholm. Replaced with real codes and pinned by a test that also asserts no entry returns its own MIC.
- `base_lookup_variants` now emits both Saxo share-class spellings, the bare-letter (`ERICb`) and underscored (`ESSITY_B`) forms, since Saxo uses both and the symbol alone does not indicate which. Keeping this in code rather than configuration avoids per-symbol maintenance.
- No negative-cache purge was required: every corrected symbol is a new string, so none inherits a cached failure and all are looked up fresh on the next sweep.
- Market-open detection is unaffected and `analysis_pulses.exchange_codes` deliberately keeps `XSTO`: that path keys on the exchange `code`/`iso_mic`, both of which are still `XSTO`, and Stockholm has been reported open throughout. Only the instrument-lookup path needed Saxo's `ExchangeId`.
- Noted for the follow-up: `saxo_exchange_snapshots` has stored the correct `code=XSTO, exchange_id=SSE, mic=XOME` mapping since 2026-05-17, so the hardcoded table was duplicating — incorrectly — a fact the runtime already held. Resolving it from stored reference data is the durable fix, but its `XNYS` row resolves to `AMEX`/`XASE` (NYSE American), so a naive swap would break NYSE.
- 517 tests pass; `cargo fmt --check` and `RUSTFLAGS="-D warnings" cargo check --all-targets` clean.

## [2026-08-02] review | Live production, Saxo API, and SQL review

- Reviewed a month of live production data, the live Saxo SIM API, and the Saxo OpenAPI reference docs. Filed seven new items (U9-U15) in `wiki/urgent-todo.md` with backing reference sections, plus roadmap entries for the non-urgent findings.
- Read-only throughout: production Postgres was queried, and the Saxo probes were `GET` requests to `/ref/v1`, `/port/v1/closedpositions`, and `/hist/v4`. No order was placed, modified, or cancelled, and no configuration or table was changed.
- Two findings are live conditions rather than latent risks. The drawdown guardrail stands at **18.999% against a 20% halt** (peak 297,463 DKK on 2026-06-30, current 241,281) — one −1.4% day from suspending all BUYs, with no re-entry rule defined. And **28 of 201 universe symbols have never been analysable**, all of Stockholm among them, because Saxo's suffix is `xome` rather than `xsto`; `exchange_id_for_suffix` compounds it by returning ISO MICs where Saxo expects proprietary `ExchangeId` codes, making the exchange fallback dead code in all 15 cases.
- Also recorded: `trade_ledger.fx_gain_dkk` is a hardcoded `0` literal, so a 63%-USD book that lost 7.66% to currency in a month reports its FX attribution as zero; the decision prompt has doubled to 527 KB, mostly raw Markov `recent_labels`; planner statistics are 33 days stale; and `audit_log` is 65 MB of dead Python exhaust.
- Added a Python/Next.js removal plan. Next.js is already fully gone. Python is 91 tracked files / 28,353 lines, of which only the two Postgres backup CronJob scripts are live. `AGENTS.md` still directs agents to the retired package as the behavior reference, which is the part doing active harm.

## [2026-08-01] architecture | Typed Decision Gate Replay API envelope

- Replaced `/api/decision/gate-replay` compatibility JSON with typed `DecisionGateReplayPayload` fields for availability, run count, scenarios, safety, interpretation, and support-risk evidence.
- Kept nested historical target-gate and support-risk analysis details dynamic while preserving existing state/query behavior.
- Added a serialization regression; replay calculation, evidence collection, Decision Reports, Hermes, Trading Manager, configuration, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed performance API envelope

- Replaced `/api/performance` compatibility JSON with typed `PerformancePayload` fields for range selection, history, summary, benchmarks, and goal tracking.
- Kept nested history, benchmark, and goal-tracking read-model details dynamic while preserving existing state/query behavior.
- Added a serialization regression; performance collection, benchmark retrieval, Decision Reports, Hermes, Trading Manager, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed market-status API envelope

- Replaced `/api/market/status` compatibility JSON with typed `MarketStatusPayload` fields for exchange rows, summary, scheduler, and price-monitor state.
- Kept nested market, scheduler, and monitor read-model details dynamic while preserving existing state/query behavior.
- Added a serialization regression; exchange-calendar refreshes, market-window calculation, quote monitoring, Decision Reports, Hermes, Trading Manager, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed market-watchlists API envelope

- Replaced `/api/market/watchlists` compatibility JSON with typed `MarketWatchlistsPayload` fields for generation time, cache TTL, universe metadata, and categories.
- Kept quote and decision-derived category rows dynamic while preserving the existing degraded empty-category response.
- Added normal and degraded serialization regressions; quote collection, candidate membership, Decision Reports, Hermes, Trading Manager, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed Hermes experiments API envelope

- Replaced the protected `/api/hermes/experiments` compatibility-JSON envelope with typed `HermesExperimentsPayload`.
- Kept individual persisted experiment rows dynamic inside the explicit advisory read-only envelope, with the existing Hermes API-key boundary and proposal lifecycle intact.
- Added a serialization regression; experiment creation/transitions, Trading Manager, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed Hermes reflections API envelope

- Replaced the protected `/api/hermes/reflections` compatibility-JSON envelope with typed `HermesReflectionsPayload`.
- Kept individual persisted reflection rows dynamic inside the explicit advisory read-only envelope, with the existing Hermes API-key boundary intact.
- Added a serialization regression; reflection creation, experiment proposals, Trading Manager, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed Scheduler API envelope

- Replaced the public `/api/scheduler` compatibility-JSON envelope with typed `SchedulerPayload`.
- Kept the scheduler status snapshot and persisted cycle rows as compatibility JSON inside the explicit read-only envelope, preserving the existing `null` and empty-list degraded-read fallbacks.
- Added a serialization regression; scheduler cadence/jobs, Trading Manager, Hermes, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed portfolio trades response

- Replaced the public `/api/portfolio/trades` compatibility-JSON envelope with typed `PortfolioTradesPayload`.
- Kept individual persisted trade rows dynamic inside the bounded list envelope while the portfolio trade read-model port remains staged.
- Added a serialization regression; the trade ledger, Saxo portfolio reads, providers, Hermes, manager gates, stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed portfolio positions response

- Replaced the public `/api/portfolio/positions` compatibility-JSON envelope with typed `PortfolioPositionsPayload`.
- Kept individual position rows dynamic inside the bounded count/list envelope while the portfolio read-model port remains staged.
- Added a serialization regression; Saxo portfolio reads, providers, Hermes, manager gates, stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed runtime-health response

- Replaced the health endpoint's `serde_json::Value` response with a typed liveness/build-identity model while preserving its three public fields and serialized contract.
- This endpoint has no application-state, provider, Hermes, Trading Manager, protective-stop, or Saxo-execution effect.

## [2026-08-01] architecture | Typed Decision Report schema-health response

- Replaced the public schema-health `serde_json::Value` response with typed health and issue models. OpenRouter schema construction and strict-schema validation remain dynamic only inside the provider integration.
- The endpoint's serialized contract is regression-tested. This is diagnostics-only and does not alter provider calls, Hermes, Trading Manager gating, protective stops, or Saxo execution.

## [2026-08-01] architecture | Typed Decision Report debug response

- Replaced the debug endpoint's outer `serde_json::Value` compatibility response with typed, serializable models for the report metadata and its four bounded diagnostic payloads.
- The stored provider-shaped report remains internal, and the existing 4,000-character caps plus server-side secret redaction remain enforced and regression-tested. This changes no provider, Hermes, Trading Manager, protective-stop, or Saxo-execution behavior.

## [2026-07-31] architecture | Current system and execution-boundary synthesis

- Added a current-state architecture concept that connects Markov, Support Risk, Quiver, Decision Reports, Hermes, performance benchmarks, Trading Manager, protective-stop maintenance, the execution queue, and Saxo broker validation.
- Documented the four enforced boundaries between an advisory provider/Hermes output and a Saxo order: response validation, deterministic Trading Manager gating, execution-queue revalidation, and Saxo precheck/placement authority.
- The README now carries the same concise architecture map and security posture; the Hermes capability document now explicitly describes Support Risk, benchmarks, protective stops, and the non-bypassable execution boundary. These are documentation changes only.

## [2026-07-31] architecture | Scheduler dashboard pagination extraction

- Moved the deterministic Execution-tab Scheduler-history page clamp and offset from `src/state.rs` to `src/scheduler_state.rs`, retaining the existing 12-row page size and focused unit coverage.
- This is a persisted read-model boundary only. Scheduler cadence, jobs, history retention, notifications, Trading Manager, protective stops, and Saxo behavior are unchanged.

## [2026-07-31] architecture | Execution dashboard pagination extraction

- Moved the deterministic Execution dashboard page clamp, offsets, and existing overview/shared row limits from `src/state.rs` to `src/execution_state.rs`, with focused unit coverage.
- This remains a local read-model boundary only. It does not change SQL ordering, broker synchronization, fills, order states, Trading Manager decisions, protective stops, or Saxo mutation behavior.

## [2026-07-31] ui | On-demand sanitized Decision Report debug payloads

- Changed AI Prompts from server-rendering the full stored Decision Report prompt, request, provider response, and normalized report to loading those values only after an operator expands the debug section.
- The new read-only endpoint redacts secret-like fields and token-shaped strings server-side, caps each of the four payloads at 4,000 characters, and the UI inserts returned content as text with local copy controls. It has no provider, Hermes, manager, configuration, stop, or broker-execution effect.

## [2026-07-31] performance | Recorded SELL-route provenance

- Extended the read-only reconciled SELL-outcome panel with a bounded attribution by the recorded exit order's strategy type and role, joined through reconciled fills. The display explicitly counts single-order links, unlinked local ledger rows, and ambiguous multi-order links.
- This is exit provenance, not attribution to the original BUY strategy or a causal measure of a strategy's performance. It does not feed Hermes, Decision Reports, Trading Manager, sizing, stops, or broker execution. Sector attribution remains deferred until a durable source exists.

## [2026-07-31] performance | Realised outcome attribution

- Extended the read-only reconciled SELL-outcome view with bounded realised P/L attribution by symbol and instrument currency. Rows are ranked by absolute realised DKK P/L, while partial sales remain independently counted local ledger outcomes.
- Instrument currency is a grouping label only. Realised P/L is recorded in DKK and this view does not isolate FX impact, infer a sector, or attribute a result to a strategy role, Hermes, or a Decision Report.

## [2026-07-30] performance | Reconciled SELL outcome evidence

- Added a read-only Performance panel from local `trade_ledger` SELL rows with a recorded cost basis. It reports realised P/L, win/loss/breakeven counts, win rate, average win/loss, payoff ratio, commission, tax, and recent closed-sale rows.
- Partial sales are intentionally counted as individual ledger outcomes rather than being presented as complete round-trip trades. The panel is `collecting` until it reaches 20 rows and then remains explicitly `preliminary`; it is accounting evidence rather than a backtest or trading signal.
- Holding time and realised slippage stay unavailable: the ledger does not yet retain a durable FIFO lot-to-sale association or a broker quote-at-submission observation. This panel does not feed Hermes, Decision Reports, Trading Manager, sizing, stops, or execution.

## [2026-07-30] operations | Calendar-aware Quiver context

- Replaced the fixed 19:00 Europe/Copenhagen Quiver run with the Saxo calendar's shared XNAS/XNYS opening plus 45 minutes. The US Decision Report remains at opening plus 75 minutes, so the scheduled Quiver cycle has an approximately 30-minute completion window before it is consumed.
- Added compact Quiver freshness metadata for Decision Report and Hermes context: `fresh`, `partial`, `stale`, `missing`, `failed`, `not_due`, and `no_us_session`. Stored prior-day signals remain available as historical context but cannot be presented as current advisory evidence.
- The change remains read-only and advisory. It creates no order, changes no Trading Manager gate, and performs no Saxo mutation.

## [2026-07-27] trading-quality | Cash deployment BUY-gate diagnostics

- The Trading Manager now persists a compact count of skipped BUY candidates grouped by its stable gate code. The Overview Cash Deployment panel renders up to four ranked, human-readable blocks such as `cash budget: 3` or `market open: 1`.
- This is an audit projection only. It does not reconstruct unrecorded causes, change capital policy, modify Hermes advice, or create, amend, or place Saxo orders.

## [2026-07-27] trading-quality | Quiver held-position conflict review

- Strong bearish Quiver Congress-trading signals against positive-quantity holdings are now presented as bounded review flags to the Decision Report prompt, Hermes, and the Quiver dashboard. The default threshold is `-0.35`; only symbols held by the current local/broker-aware position view are included.
- The projection is advisory only. It cannot create an exit, approve or block an order, adjust a manager gate, or override technical, Markov, capital, or Saxo evidence.

## [2026-07-26] trading-quality | Durable BUY trade-thesis provenance

- The Trading Manager now records a compact, immutable thesis only when it admits a BUY to the execution queue. It includes the Decision Report/pulse reference, intended two-week holding window, rationale, catalyst or monitor, deterministic approval evidence, compact technical and Markov evidence, and a review-only invalidation condition.
- Execution attribution can retrieve the latest recorded BUY thesis for a later same-symbol order, without looking ahead to later entries. This lets a future trim or exit be interpreted against the original admission evidence rather than only the mechanics of the latest order.
- The thesis remains provenance, not policy: it cannot approve, size, place, amend, cancel, retain, or exit a broker order. Historical orders with no snapshot stay explicitly blank, and the next milestone is aggregate outcome measurement after enough mature observations exist.

## [2026-07-26] trading-quality | Read-only post-fill holding-period attribution

- Execution-order attribution now uses the weighted price of reconciled fills and the first and fifth subsequent persisted daily-indicator closes to show 1-session and 5-session market and directional returns. This adds no Saxo or quote call while rendering the Execution view.
- BUY direction treats a higher later close as positive; SELL direction reverses that sign. The UI explicitly labels the comparison as evidence only: it excludes FX, commission, tax, slippage, fill timing, and later position changes, and is not realised P/L. Weekends and holidays contribute no synthetic session: only distinct available trading-day closes count.

## [2026-07-26] testing | Decision-report provider regression corpus

- Added compact, source-controlled Decision Report fixtures that must conform to the OpenRouter strict schema before they are fed through the same parser and completion normalizer used at runtime. The corpus covers fenced/prefaced JSON, Nordic/EU exchange scope filtering, strategy-plan normalization, and dry-run execution safety without a provider, broker, database, or market-hours dependency.
- A deliberately narrow test-side schema instance checker rejects missing, mistyped, out-of-enum, or unexpected fields, so a changed provider contract cannot make the fixtures look valid by accident. Add an anonymized fixture whenever a future provider/parser incident is diagnosed.

## [2026-07-26] safety | Decision-report BUY selection cap becomes enforced (U2)

- `strategy.max_selected_assets` now limits the number of distinct approved BUY symbols per Decision Report. It runs only after Hermes and all deterministic trade gates, so it bounds cumulative new exposure without hiding a candidate from the audit trail.
- SELLs remain eligible and a repeat BUY for an already-selected symbol does not spend another slot. `0` is explicit unlimited mode; a negative configuration fails new BUYs closed. Policy and per-order safe metadata are retained in the manager run, Hermes preflight, and Candidate Scoring Waterfall.
- This removes one unused risk control from U2. The remaining risk-surface keys are now 14.

## [2026-07-26] resilience | Bounded scheduler advisory enrichment

- The scheduler now enforces explicit deadline budgets around the four slow advisory-enrichment steps: Markov and daily indicators default to four minutes; Quiver and editorial research default to 45 seconds. Each can be tuned by a bounded one-to-900-second environment override.
- A timeout persists a structured terminal result (`status: timeout`, retry on the next scheduler cycle) rather than failing the whole cycle, so operations can distinguish an unavailable enrichment source from a broker or decision-path failure.
- The boundary is intentional: Saxo session/order work, protective stops, decision submission, Trading Manager queueing, execution, and reconciliation are not cancellable through this helper. A late broker mutation must be reconciled, not treated as safely absent.

## [2026-07-26] safety | ATR-based risk-per-trade sizing becomes enforced (U2)

- `strategy.swing.risk_per_trade_pct` now caps each approved BUY's estimated initial stop loss instead of sitting unused in configuration. The cap uses a database-verified daily close and ATR14, the same configured stop multiple used by automatic protective-stop maintenance, and the already verified DKK share value.
- The gate downsizes an order where possible and rejects it when even one share would exceed the loss budget. Missing/invalid inputs, model-supplied indicator data, a missing verified DKK value, or disabled automatic protective stops fail the BUY closed. It rechecks the commission-efficiency floor after downsizing.
- This is a sizing guard, not a broker-fill guarantee: gaps can execute below a stop. Gate evaluation makes no Saxo request. The config-contract inventory moves to 28 enforced / 36 unused / 20 risk-surface settings.

## [2026-07-26] safety | Emergency risk exclusions become effective (U2)

- `risk.excluded_symbols_csv: ENV:RISK_EXCLUDED_SYMBOLS` is now resolved by the Rust Trading Manager, not merely described in the configuration. It merges with the versioned risk and never-trade lists, normalizes casing/whitespace, and applies before any candidate can queue.
- The value comes from the already-injected `daytrader-env` secret in API and scheduler pods. A missing or empty variable is a no-op, preserving the current default behaviour. The config-contract inventory moves to 25 enforced / 39 unused / 22 risk-surface settings.

## [2026-07-26] safety | Existing automation switches become enforced (U2)

- `strategy.enabled` now prevents new scheduled Decision Report submissions and Trading Manager queueing, while previously submitted provider work still reaches a terminal audit state. It intentionally does not stop read-only market analysis, broker reconciliation, or protective-stop maintenance.
- `strategy.swing.trading_manager.enabled` prevents new execution-order creation. The EU and US pulse switches suppress their own scheduled submissions, and Operations reports them as disabled rather than stale.
- Kubernetes now states the Trading Manager settings explicitly. The config-contract inventory moves from 20 enforced / 44 unused / 27 risk-surface to 24 enforced / 40 unused / 23 risk-surface settings. Production defaults remain enabled; this change establishes an operator pause path without changing normal trading behaviour.

## [2026-07-26] performance | Saxo request pacing per service group (U6)

- `src/saxo_rate_limit.rs` paces Saxo requests per service group (the first path segment: `chart`, `port`, `trade`, `ref`). Installed in the shared `markov_method::saxo_get_json`, which both the Markov and the daily-indicator sweeps already call, so the two share one budget instead of pacing independently against one limit. Also wired into the portfolio and order paths.
- **Even spacing rather than a token bucket.** A bucket of 100/min lets a sweep fire a hundred requests back to back and then stall for a minute — inside the window, but the burstiest possible way to spend the quota, and worse behaviour than the fixed 500 ms sleep it replaces. Spacing cannot burst: 100/min is one request every 600 ms, sustained, which is strictly more conservative than the sleep it supersedes. An idle group accumulates no burst allowance.
- **Driven by Saxo's own accounting.** `X-RateLimit-<dimension>-Remaining` over `-Reset` is the pace the server is telling us it will accept. It tightens automatically as quota depletes instead of waiting for the first 429 to learn something the headers already said, and the tightest reported dimension wins. Exhausted quota waits out the reset. A `Remaining` with no matching `Reset` is ignored rather than guessed at.
- The configured rate is a ceiling, not a floor: `saxo.requests_per_minute` (default 100) is clamped to Saxo's documented 120 so a config typo cannot raise the pace above what the broker accepts, and a header-driven hold always wins when it is longer.
- No single wait exceeds 30 s. `acquire` re-plans and sleeps again, so a pathological `Reset` slows a nightly job rather than stranding it.
- **Scope limit, stated rather than hidden:** state is per process. Saxo's limit is per *session*, and the API and scheduler pods share one session, so they cannot see each other's usage. Coordinating would mean putting the limiter in the database, in front of every request. Both nightly sweeps run in the scheduler pod and the API pod's calls are sporadic and operator-driven, so process-local pacing plus header adaptation covers the real exposure. Revisit if the API pod ever starts sweeping.
- 390 tests pass.

## [2026-07-26] safety | Portfolio drawdown guardrail makes max_drawdown real (U3)

- `src/drawdown_guard.rs` enforces the `max_drawdown: 0.20` the Hermes goal contract has advertised since it was written. A soft band (10%) reduces the cycle-wide BUY budget; the hard floor (20%) suspends new BUYs. SELLs are never blocked, matching the monthly-loss breaker's shape so the operator has one mental model for both.
- The contract now reads its limit from `strategy.capital.drawdown_halt_pct` — the same key the gate applies — in all three places it quotes one (`objective.max_drawdown`, `promote_only_if.drawdown_lte`, `rollback_if.drawdown_gt`). The advertised and enforced numbers can no longer drift.
- Every objective and constraint declares an `enforcement` status. `hermes_goal_contract_declares_enforcement_for_every_field` fails the build if a field is added without one, in either direction. `max_positions`, `slippage_tolerance`, and `require_backtest_before_activation` are now explicitly `not_enforced` instead of implicitly claimed. `gas_reserve` was deleted as a crypto-template leftover that never meant anything for a Saxo equities account.
- Two production data artifacts were found by checking what the rule would do against the live book *before* deploying, and both would have halted all buying on the first cycle:
  - **A bad snapshot became a false peak.** Five consecutive scheduler snapshots on 2026-06-10 recorded 485,094 DKK with negative cash — a mid-settlement double-count on a book worth about 264,000 that day. As a peak it implied a 47% drawdown. Fixed by measuring peak-to-current on **daily closes** rather than intraday snapshots, which is the conventional definition anyway; the day's close was clean. Any glitch that does not survive to a close cannot set the high-water mark.
  - **The peak reached back across a re-baselining.** Mid-May 2026 operator cash adjustments and a "Live export reset" moved the book from ~351,000 to ~265,000 DKK. Nothing was lost, but a peak spanning that boundary reads as a 27% drawdown. Fixed by starting the window after the most recent `DEPOSIT`/`WITHDRAWAL`/`ADJUSTMENT` row in `trade_ledger`. A peak from before a re-baselining describes a different portfolio.
- With both fixed the guardrail reads **14.0%** against the live book (peak 297,463 on the post-reset series, current 255,823) — a real drawdown, inside the soft band, no halt.
- Direction of failure is deliberately inverted from the rest of the risk code: thin or unusable history **disables** the guardrail loudly rather than tripping it. Failing closed here means halting all buying, and the inputs that go missing (an empty history after a restore, a position batch mid-load) are exactly the ones that occur when nothing has been lost.
- Overlapping soft bands take the **strictest** multiplier, not the product. A losing month and a drawdown are usually one decline seen from two angles; multiplying them double-counts it and lands on a deployed capacity nobody chose.
- The operator override is anchored to the peak it was granted against and lapses by itself once the book makes a new high, so a one-off exemption cannot become permanent. A grant with no recorded peak is refused rather than honoured forever.
- Slack alerts fire only on the edges of the suspension, and a run recorded before the guardrail shipped reads as inactive rather than cleared.
- 377 tests pass. No Saxo call and no order path is touched: this gate can only *withhold* BUYs.

## [2026-07-25] implementation | Public editorial research ingestion

- Added the initial bounded public-feed ingestion path for App Economy Insights. The Rust scheduler persists sanitized metadata and compact summaries, deduplicates by feed identity, and matches only explicit configured aliases before exposing the context to Decision Reports and Hermes.
- Paid content remains out of scope. This secondary editorial evidence cannot become a Trading Manager gate or Saxo action without separately measured evidence and an approved one-variable proposal.
- Recorded the next source-catalog milestone: port and validate the legacy Yahoo Finance, CNBC, Reuters, and macro RSS configuration through the same bounded Rust framework. Yahoo quote pages remain human-facing links, not an ingestion target.

## [2026-07-25] safety | Protective-stop coverage exceptions and reconciled SIM evidence

- Coverage now counts a SIM lifecycle test only when its read-only reconciliation recorded `broker_working` and a broker order identifier. Placement-submitted, cancelled, failed, ambiguous, and non-SIM records remain non-protective.
- The Execution view now presents explicit read-only exception rows for persisted broker positions with no complete broker-confirmed coverage, including the uncovered quantity, reason, and a non-mutating operator next step.
- This does not place, amend, cancel, or reconcile a Saxo order, enable parent/child order automation, alter Trading Manager or Hermes behavior, or make a stop guarantee its fill price through a gap.

## [2026-07-25] fix | Post-deploy smoke cleanup under strict shell mode

- Guard cleanup now handles an empty port-forward PID list before iterating under `set -u`, so an early smoke-test failure reports the original mismatch without an unrelated unbound-array error.

## [2026-07-23] fix | TradingView Novonesis alias and resilient modal close

- Mapped historical `NZYM-B:xcse`/`NZYMB:xcse` chart requests to current TradingView listing `OMXCOP:NSIS_B`; persisted Saxo symbols, historical keys, and trading behavior remain unchanged.
- Modal close now uses a capture-phase delegated handler plus an immediate dismissed state, with Escape support and a non-modal fallback fragment. This prevents a third-party frame or focus behavior from leaving the overlay stuck.

## [2026-07-23] fix | TradingView symbol aliases and modal close position

- Added TradingView-specific aliases for `NOVOB:xcse` to `OMXCOP:NOVO_B`, `SHELL:xlon` to `LSE:SHEL`, and `ARKK:xmil` to `AMEX:ARKK`, plus OMX Copenhagen/Stockholm share-class punctuation conversion (`-` to `_`) for symbols such as `MAERSK-B` and `HEXA-B`.
- Modal close controls now clear the URL fragment with `history.replaceState` and restore the prior scroll position, avoiding the page jump caused by a bare `#` close link.
- The Kubernetes deploy target now resolves a separate Git SHA at invocation time instead of accepting an inherited stale value, so the image build metadata and post-deploy guard identify the actual deployed commit.

## [2026-07-23] deploy | Rust Docker dependency cache

- Split the Rust image build into a manifest-only dependency layer and a final source build, using BuildKit Cargo registry and target caches. A tracked `build.rs` injects the Git SHA and reruns when it changes; the final Cargo command consumes the build argument directly so Docker invalidates only the metadata-bearing application step while dependencies remain cached across deployments.
- Excluded screenshot directories from the Docker build context. A repeated validation build transferred approximately 16 KB of source context and reused all layers.

## [2026-07-23] ux | Markov top pagination

- Added Previous/Next navigation at the top of the Markov signals table, preserving the existing bounded server-side page URLs and bottom navigation.

## [2026-07-23] fix | Watchlist quote provenance labels

- Replaced opaque Watchlist quote-status codes with compact labels and hover descriptions. A successful Saxo price-monitor quote is now distinct from a configured analysis-universe member that is awaiting enrichment.
- Decision-derived values no longer inherit the ambiguous `ok` quote state; they persist as `decision_snapshot`. Broker-only and current-source rows remain separately identified. This is a display/provenance change only and does not affect instrument membership, decision prompts, or trading.

## [2026-07-23] fix | Lazy TradingView chart embeds

- Watchlist sparklines previously rendered a live external TradingView iframe inside every hidden chart modal, which made a selected chart compete with the entire watchlist for third-party widget loading.
- The iframe now remains `about:blank` until the modal's URL fragment opens it. The selected chart receives an immediate local loading state and remains loaded for the rest of the page session; no Saxo or application data path changed.

## [2026-07-23] configuration | TSLA and NOVO watchlist eligibility

- Added `TSLA:xnas` and `NOVOb:xcse` to the versioned analysis universe used by Markov, daily indicators, Quiver, and decision reports.
- Removed both symbols from the active Rust `never_trade_symbols` and `risk.excluded_symbols` lists. They remain subject to all normal market, technical, risk, Hermes, and broker gates; this is not an order or position change.

## [2026-07-23] improvement | Hermes learning memory compression

- Added a bounded read-only projection that groups safe proposed reflection actions across distinct reflections into `emerging`, `stable`, and `stale` lessons.
- Emerging lessons expire after 7 days and stable lessons after 21 days. Stale lessons remain operator-visible for audit but are excluded from Hermes advisory context; the projection stores no second workflow and cannot change configuration, lifecycle, or broker behavior.

## [2026-07-23] improvement | Hermes baseline promotion evidence

- Added a bounded read-only evidence pack for the active promoted baseline. It links only the exact source-experiment overlay manager runs to compact report/order outcome counts and uses persisted portfolio snapshots for experiment-window and post-promotion return, drawdown, cash utilization, and sufficient-sample zero-risk-free Sharpe observations.
- The pack is explicitly observational rather than causal. It excludes raw experiment evidence, hypotheses, risk notes, provider payloads, broker payloads, and secrets; it cannot alter baseline/experiment lifecycle, configuration, or Saxo behavior.

## [2026-07-23] improvement | Hermes proposal quality review

- Added a deterministic, bounded read-only review projection for active and pending Hermes experiments. It scores only safe persisted metadata: variable-path clarity, evidence presence/source names, measurable expected effect, changed values with risk notes, and exact or related active-proposal risk.
- The projection emits a compact score, review status, counts, and review gaps. It excludes raw evidence, risk notes, hypotheses, provider payloads, broker data, and secrets; it cannot change a proposal lifecycle, strategy configuration, or Saxo behavior.

## [2026-07-23] improvement | Hermes one-variable audit

- Added a bounded read-only dashboard projection that distinguishes a promoted baseline audit artifact from the exact allowed strategy experiment overlay selected for a future paper/SIM Trading Manager cycle.
- The projection reuses the manager's runtime selection helper and records only safe variable, old/new value, lifecycle, sanitized hypothesis, scope, and latest-manager-observation metadata. It cannot transition experiments, modify configuration, or affect Saxo behavior.

## [2026-07-23] improvement | Hermes lessons pending review

- Added a bounded read-only Hermes dashboard queue derived from the most recent reflection `proposed_actions`.
- The queue collapses duplicate normalized action text to its newest reflection and displays only action text plus safe reflection context. It excludes raw Hermes/provider payloads, evidence blobs, Saxo data, and broker payloads; sensitive-looking action text is redacted.
- A queued lesson remains advisory context only. It cannot create or transition an experiment, change strategy configuration, or affect broker behavior; those remain behind the protected one-variable experiment lifecycle.

## [2026-07-21] fix | Versioned analysis-universe source

- Replaced the hidden dependency on 2026-05 archived sentiment rows for Markov, daily-indicator, and decision-report membership with a 197-symbol `market_data.watchlists.universe_symbols` catalog in both local and Kubernetes configuration.
- Current broker positions, fresh reports, and `extra_symbols` remain additive. Symbol deduplication is now case-normalized, so a live `AMD:xnas` position cannot coexist with an archived/configured `amd:xnas` entry.
- Historical price and sentiment rows remain a warning-level membership fallback only for installations whose configured universe is empty; stale content itself remains excluded from prompt evidence. The watchlist payload exposes its universe source and configured/additive counts for audit.

## [2026-07-21] improvement | Reconciled fill outcome attribution

- The Execution attribution disclosure now aggregates linked `execution_fills` and `trade_ledger` entries for an order. SELLs show realised P/L and recorded costs; BUYs show a position-book update rather than a fabricated P/L.
- The projection reports fill quantity, completion, and source. Multi-fill orders use the reconciled aggregate; legacy rows with only `execution_orders.ledger_id` are marked as such; unreconciled/partial states are not presented as final results.
- This is a local database read only. It does not query Saxo, calculate mark-to-market returns, expose broker payloads, or influence order placement.

## [2026-07-21] improvement | Report-time execution attribution

- Execution-order details now prefer immutable Trading Manager evidence: the final database-verified technical gate result, the candidate's stored Markov preflight, and the manager-time capital budget.
- For historical rows that predate these snapshots, the dashboard may still read a current signal, but it labels that value `latest fallback` rather than presenting it as report-time evidence.
- The compact attribution omits raw Hermes rationale, broker payloads, execution errors, and unrelated manager fields. It adds no broker or provider calls and cannot change order execution.

## [2026-07-21] fix | Fail closed on ambiguous Saxo order placement

- Placement errors containing `TradeNotCompleted`, `timed out`, or `timeout` now move the order to `broker_state_unknown` rather than `execution_failed`. The queue cannot claim or resubmit that order automatically.
- The stored audit payload retains the completed precheck, sanitized order payload, stable `ExternalReference`, and the `x-request-id` used for placement; account scope is excluded from the diagnostic payload.
- Unknown SELL placement keeps its local reservation, while the Execution view and Overview integrity check show it as a warning. A dedicated `broker_state_unknown` event records the uncertainty for later broker/ENS reconciliation.
- Follow-up remains broker-authored reconciliation by activity/open-order lookup or ENS replay before the hold is cleared. The runtime intentionally does not infer that an absent response means the broker did not receive the order.

## [2026-07-21] feature | Broker-audit reconciliation for ambiguous Saxo placements

- Each normal Saxo broker-sync cycle now separately scans a bounded set of `broker_state_unknown` orders. It makes a read-only `cs/v1/audit/orderactivities` request from the order creation time (with a 14-day fallback), and matches the locally retained `ExternalReference` exactly before doing anything to the local order.
- A confirmed activity attaches its Saxo `OrderId`, moves the local record to `submitted_to_broker`, and records a `broker_state_reconciled` event. Existing order-status sync can then continue normally; the runtime never replays the original placement.
- No exact match keeps the order blocked, preserves any SELL reservation, and records `broker_state_unknown_not_found` with the lookup context. Audit payloads stored for this workflow recursively remove account, client, user, and handler identity fields.
- The audit endpoint has no documented `ExternalReference` query parameter, so this is an exact local comparison against a bounded activity response. ENS replay or paginated audit history remains the later coverage improvement for unusually busy histories.

## [2026-07-21] feature | Normalized Saxo execution failure taxonomy

- Local Saxo execution failures now persist a safe `error_taxonomy` beside the raw diagnostic: stable code, short label, remediation, and retry policy. Categories cover ambiguous broker state, session expiry, rate limits, commission setup, cash, tick/price, quantity, holdings, instrument tradability, market closure, terminal broker outcomes, and an explicit unknown fallback.
- The Execution Queue now prefers the persisted label and adds category, next step, and retry policy to the existing status tooltip. Historical rows retain the prior text-based display behavior until they next receive a new execution result.
- Broker-sync terminal outcomes now carry the same taxonomy. Hermes's redacted preflight failure bundle selects the stored allow-listed code before its legacy string matcher, so raw broker error text remains excluded from Hermes context.

## [2026-07-17] fix | Manual decision report runs detached (10s connection drop)

- Operator report: Generate Report died after ~10s with ERR_CONNECTION_CLOSED on the ngrok URL. Root cause: the action handler ran the entire pipeline — prompt build, OpenRouter call (600s budget), Trading Manager, execution queue — synchronously inside one HTTP request through the OAuth-wrapped ngrok tunnel. It only ever "responded fast" before because the dead API key made OpenRouter 401 instantly; with a real key the multi-minute request outlived some hop's timeout, and the disconnect made axum cancel the whole pipeline mid-flight.
- The handler now claims a single manual-report slot (`manual_report_claim` runtime setting, 15-minute stale takeover; double-clicks refused), spawns the pipeline detached with `tokio::spawn`, and redirects to the decisions view immediately. A dropped browser connection can no longer cancel report generation.
- The decisions view shows the existing "running" banner whenever a claim is fresh (`manual_report_in_flight` on `DashboardView`), and the completion poll is baseline-aware: it records the newest report id+status at render and only navigates when the latest report differs — the old status-only poll would have reload-looped every 4s while a spawned run worked.
- Tests: claim exclusivity + release, stale-claim takeover. Full suite: 249 passed.

## [2026-07-17] feature | OpenRouter API key rotation via Settings

- Decision reports failed all day with OpenRouter HTTP 401 "User not found" after the operator rotated the API key: the running pods bake `ENV:OPENROUTER_API_KEY` from a deploy-time secret, so a rotation used to require re-running the deploy script with the new env value.
- Settings now has an "OpenRouter API key" password field posting to `/api/settings/ai-key`. The key is stored as a `runtime_settings` override (`ai_api_key`) and `effective_ai_api_key` makes both AI call sites (decision submit + deferred poll) prefer it over the config/env value — a rotated key takes effect immediately, no redeploy. Submitting an empty field clears the override back to config.
- The key is never echoed: status surfaces only `{configured, source, masked (first 6 + last 4), updated_at}`, the request struct derives no Debug, and the handler logs source/configured only. Validation rejects whitespace/non-printable input.
- Also fixed in passing: the AI model validation now accepts OpenRouter's `~` floating-alias prefix (e.g. `~openai/gpt-5`), which the character allowlist previously rejected.
- Tests: override-wins-over-config + never-echoed, reject-invalid + missing-status, mask behavior, `~` alias round-trip. Full suite: 247 passed.

## [2026-07-17] fix | Local-vs-broker quantity divergence alert

- `refresh_broker_snapshots` now runs `local_broker_quantity_divergences` on every scheduler cycle: the latest local `position_snapshots` quantity per symbol (excluded = 0, matching the basis reader's semantics) is compared against the broker positions just fetched, in both directions — broker positions the local book under/over-states AND local positions the broker no longer holds. Divergences are logged as a structured warning and returned in the refresh result (`quantity_divergences`).
- This is the watchdog for the fill-time book keeping landed earlier today: any missed fill, corporate action, or out-of-band broker change now surfaces within one 10-minute cycle instead of silently corrupting SELL accounting or the flat-position starter gate.
- Housekeeping: the host disk hit 100% (118 MiB free) from accumulated Docker build cache; pruned 113.9 GB of build cache plus 27 GB of unused images (all rebuildable), restoring 130 GiB free. Cluster pods were unaffected (their images live in the k8s node's containerd store).

## [2026-07-17] fix | Fills maintain the local position book

- Verified overnight: the 2026-07-16 23:47 indicator run covered 199 symbols (up from 20), confirming the widened universe landed correctly.
- Closed the root cause behind the recurring zero-basis class: `sync_final_fill` now calls `apply_fill_to_local_book` for every reconciled fill delta. BUY fills add quantity and commission-inclusive basis to the current `position_snapshots` row (creating it — batch-linked, FK-safe — when the position is new) and insert an idempotent `position_lots` row (`buy-fill:{order}:{ledger}`, `source_type = 'buy_fill'`). SELL fills remove quantity and prorated basis, so a full exit leaves quantity 0 and a later re-buy cannot inherit the dead position's basis.
- Ordering is deliberate: the trade ledger reads the pre-sale basis first, then the book is decremented — and the fill-delta guard makes replays no-ops, so the book never double-moves.
- New positions reuse the latest import batch (FK to `import_batches`), so `latest_position_quantity`'s latest-batch filter keeps seeing the whole book; a dedicated `fill-sync-*` batch is created only on an empty database.
- Tests: extended the SELL idempotency test with decrement assertions and added `buy_final_fill_writes_local_snapshot_and_lot_without_http` + `buy_final_fill_tops_up_existing_snapshot_in_place`. Full suite: 242 passed.

## [2026-07-16] fix | Age-gate stale sentiment out of decision prompts

- `watchlists_payload` merged four sources with no age gate: orphaned `portfolio_price_snapshots` rows (15 rows from 2026-05-07…06-26 — exactly the phantom former holdings NNIT, ORSTED, AMZN, PLTR, GOOGL, MSTR…), unbounded-age `latest_symbol_decisions` blobs, and `swing_sentiment_snapshots` whose 1,068 rows ALL date from 2026-05-05…08. This is what kept telling the model NNIT/ORSTED were "Existing portfolio holding" with 2026-06-24 quotes, producing five phantom SELL suggestions this week.
- Entries older than `strategy.swing.position_decision_stale_after_days` (7d) are reduced to bare universe members (`{symbol, quote_status: stale_history}`): stale prices, sentiment, and decision blobs are stripped, and the backfilled `decision` annotations use an age-filtered map. Live sources (current positions, fresh price rows, broker exposures) are untouched.
- Critical catch during live verification: the May sentiment archive IS the analysis universe — Markov's 199-asset nightly list is built from this payload, and a first version that dropped stale rows entirely shrank the live `all` category to 14 symbols. Membership is therefore preserved; only the stale data is removed. Follow-up: give the universe an explicit configured source instead of fossil sentiment rows.
- Full suite: 240 passed; live payload verified after deploy (universe restored, phantom data gone).

## [2026-07-16] fix | Daily-indicator universe widened to the full watchlist

- Verified the operator applied the broker-bootstrap SQL: 13 live positions now carry snapshot+lot rows (batch `broker-bootstrap-20260716T190000Z`), the 18 stale 2026-05-18 rows are excluded, and the max new unit cost (~11.8k DKK, ASML) stays under the 100k integrity threshold.
- Raised `strategy.swing.daily_indicators.max_symbols` 20 → 0 (unlimited) in both configs, aligning the indicator universe with the Markov run (~199 portfolio+watchlist symbols, 199/172/27 nightly). The 20-symbol cap meant candidate BUYs outside current holdings never had technicals, so the confluence gate and Hermes stood down every rotation (DSV reports 170/174).
- Prompt size stays bounded: `compact_indicator_context` already limits the decision prompt to the top 80 signals by confluence count; the Trading Manager gate reads per-symbol signals straight from the database.
- Tonight's 23:45 Copenhagen run is the first full-universe pass; follow-up is confirming coverage and chart-API pacing.

## [2026-07-16] fix | Broker-authoritative cost-basis fallback for SELL fills

- Found that `latest_position_cost_basis` only reads `position_snapshots`, whose latest rows date from the 2026-05-18 import — and BUY fills never write snapshots — so a SELL of ANY position acquired since (ARM, CSCO, AMAT, and even in-ledger DANSKE/CHEMM/AMGN buys) would book cost basis 0 and record the full sale proceeds as realised gain. With the flatten fix unblocking risk-off exits, this would have fired on the very next defensive SELL.
- Added a broker-authoritative fallback: when the local snapshot is missing or has no usable basis, the fill accounting derives basis from `broker_position_snapshots` open price (including costs when available) times quantity, FX-converted with the cached rate; if neither source exists it books zero but warns loudly. Stale zero-basis snapshot rows also defer to the live broker position.
- Added fixtures: a full SELL final-fill reconciliation with no local snapshot (asserts the realised loss reflects the broker basis, not phantom gains) and a stale zero-basis snapshot deferring to the broker row. Full suite: 240 passed.
- Prepared a broker-bootstrap SQL transaction (new import batch + snapshot/lot rows for the 13 live broker positions, stale May-18 rows marked excluded) — session permissions blocked applying it to the live database; it is staged for the operator.

## [2026-07-16] fix | Server-verified flatten-role SELL exits

- Removed the `technical_gate` escape hatch that approved SELLs on the exact strategy role `FLATTEN` — a string the pipeline never emits (`risk_reduction_flatten` in practice), and a model-claimed label the gate should not have trusted anyway.
- Added a server-verified risk-off fallback in `run_for_report`: when a flatten-family SELL is blocked on neutral technicals, it is approved only if this process independently confirms the broker position is under water (fresh daily-indicator close below the broker open price, local currency) or the latest Markov regime signal is negative; both checks enforce the 5-day freshness window.
- Documented the behavior in manager JSON execution notes; added pure-gate tests (flatten label alone never approves) and database-backed fixtures for the under-water, profitable-with-positive-regime, negative-regime-only, and stale-signal cases. Full suite: 238 passed.

## [2026-07-16] roadmap | Live-system week review: blocked risk-off exits, phantom holdings, stale lots

- Reviewed the other agent's 25 commits since 2026-07-11 (per-tab lazy loads, server pagination, DB-metadata redaction, deploy provenance, fail-closed scheduled reports, execution-queue admission guards, Hermes preflight/advice-delta/counterfactuals, candidate scoring waterfall, and ten workflow-test fixtures) and audited the live system (image `20260715215717`, pods healthy, all signal pipelines fresh).
- Week performance (2026-07-09 → 07-16): portfolio 283.7k → 259.3k DKK (−24.4k, −8.6%); month P/L −35.1k against the goal baseline; July realised +12,293 DKK (ADI +9,937, NVDA +2,356) on 20 trades, so the bleed is unrealised decay in the open book. Last executed trade was 2026-07-14; reports 171–175 approved zero orders.
- Root causes added as roadmap rows: (P0) `technical_gate`'s flatten escape hatch requires exactly `FLATTEN` while the model emits `risk_reduction_flatten`, so the ARM:xnas risk-off SELL (−4,221 DKK unrealised) was skipped twice on "HOLD with neutral trend"; (P0) prompts recycle stale decision-history sentiment, so NNIT/ORSTED still read "Existing portfolio holding" with 2026-06-24 quotes and produced 5 phantom SELL suggestions the broker-authoritative guard had to block; (P1) `position_lots` still hold the full May-18 book (18 symbols, ~740k basis incl. TSLA×163/NOVOb×235) while the broker holds a different 13-symbol portfolio — the next outside-ledger SELL re-creates the cost-basis corruption class repaired 2026-07-08; (P1) the nightly indicator universe is only 20 symbols, so candidate BUYs like DSV:xcse always fail confluence and Hermes stands them down; (P2) `monthly_loss_halt_dkk` was loosened −10k → −50k in e80621e without a documented rationale.
- Refreshed the Hermes "unstick the experiment review queue" row: five proposals now sit in `pending_review`, oldest 2026-06-16; aging alerts fire but nothing closes the loop.

## [2026-07-15] testing | Database-backed SELL final-fill cost-basis fixture

- Added isolated SQLite coverage for a confirmed four-share SELL final fill using the latest local `position_snapshots` cost basis.
- The fixture verifies pro-rated local/DKK basis, the XNAS minimum commission, net proceeds, realised P/L, instrument identity, execution-fill linkage, and replay idempotency.
- It starts after broker response data is available and never calls Saxo HTTP.

## [2026-07-15] testing | Database-backed partial-to-final fill delta fixture

- Added isolated SQLite coverage for a one-share partial fill already recorded locally before a later cumulative four-share `FinalFill`.
- The final-fill reconciliation records only the three-share delta, preserves both ledger prices, completes the order, and remains idempotent on replay.
- The fixture starts after broker response data is available and never calls Saxo HTTP.

## [2026-07-15] testing | Database-backed final-fill reconciliation fixture

- Added isolated SQLite coverage for a confirmed Saxo final fill after its broker response has already been received.
- The fixture verifies one local fill record, one trade-ledger row, executed order state, price backfill, and persisted broker context.
- Replaying the same cumulative fill records no additional ledger or execution-fill rows; the fixture never calls Saxo HTTP.

## [2026-07-15] testing | Database-backed broker-sync not-found fixture

- Added an isolated SQLite fixture for the case where Saxo returns neither an active open order nor an audit-activity record.
- The local order remains `broker_working` with no error, while the missing broker visibility, lookup sources, broker order id, and audit event are retained for later reconciliation.
- The test begins after the broker lookup result exists and makes no Saxo HTTP request.

## [2026-07-14] testing | Database-backed broker terminal-state fixture

- Added an isolated SQLite fixture for the local persistence half of terminal broker synchronization.
- A synthetic `Expired` Saxo response records a normalized event and transitions the order to `broker_expired`, retaining prior local result data plus broker-sync provenance and the terminal error summary.
- The test intentionally starts after a response exists: it does not create a Saxo session or make broker HTTP requests.

## [2026-07-14] testing | Database-backed execution-order claim race fixture

- Added an isolated SQLite fixture around the conditional execution-order claim update.
- Two concurrent local claim attempts for the same pending order produce exactly one winner, preserve the empty broker-order id, and clear a stale local error as the order moves to `submitting_to_broker`.
- The fixture makes no Saxo session request and no broker HTTP request; it verifies only the local idempotency boundary that precedes broker mutation.

## [2026-07-14] testing | Database-backed Trading Manager queue fixture

- Added an isolated one-connection SQLite fixture for the manager-owned execution-order, execution-event, and manager-run tables.
- The fixture proves a completed scheduled-report candidate creates exactly one local `pending_execution` order and one queue audit event, deduplicates a repeated strategy key, and persists a matching manager run without a Saxo session or broker HTTP.
- This covers the manager-to-local-queue boundary only. Saxo precheck, placement, and broker-status behavior stay in the separately tested and explicitly gated execution path.

## [2026-07-14] testing | Execution queue admission guard

- Saxo execution queue admission is now a pure, tested safety gate with fixed precedence: non-live/non-Saxo configuration, then `app.dry_run`, then `execution.require_approval_live`.
- Regression coverage proves that only explicit `execution.mode=live`, `execution.adapter=saxo`, `app.dry_run=false`, and `execution.require_approval_live=false` can proceed toward a Saxo session or broker call.
- API response shapes and gate reasons are preserved; this refactor does not change any order payload, queue claim, or broker mutation behavior.

## [2026-07-14] ux | Per-pulse scheduled report health

- The persistent Operations banner now exposes separate EU and US decision-report health chips, including the latest normalized status and the timestamp of the last successful report.
- `completed` and locally accepted `xai_fallback` reports both count as a last success, aligning dashboard health with the Trading Manager's scheduled-report eligibility rule.
- The shared dashboard payload contains compact report-status metadata only; prompt, provider-response, and normalized-report JSON remain scoped to report detail views.

## [2026-07-14] testing | Scheduled report hand-off guard

- Trading Manager now fails closed before queueing unless a report has a positive id, `completed` or `xai_fallback` status, a scheduled pulse key, and a parseable timestamp inside the configured freshness window.
- Regression tests cover accepted completed/fallback reports and rejected deferred, stale, malformed-timestamp, and non-scheduled reports.
- This is a workflow safety change only; it does not alter candidate gates, Hermes advice, or Saxo execution behavior for valid reports.

## [2026-07-14] ux | Schedule-aware Operations freshness

- Operations health now evaluates Markov, Quiver, and daily-indicator run age against the live configured timezone, due time, weekday policy, and enabled state.
- A completed prior-weekday run is neutral `idle (weekend)` during the weekend and `waiting` before its next local due time. Only jobs overdue for their latest expected date remain stale warnings; job failures and partial runs retain their higher-severity signals.
- The dashboard receives compact schedule metadata from active configuration rather than trusting historical run configuration, so configuration changes take effect immediately in health labels.

## [2026-07-14] ux | Age-aware decision labels

- Portfolio and watchlist decision badges now show a relative report age instead of presenting an absolute historical timestamp as active advice.
- The display threshold is `strategy.swing.position_decision_stale_after_days`, defaulting to seven days. Old recommendations render as `Stale`, and missing/invalid timestamps fail closed to an undated stale state.
- This is a rendering-only safeguard; decision-report generation, Hermes advice, Trading Manager queueing, and Saxo execution are unchanged.

## [2026-07-14] data-hygiene | Rust scheduler-history retention

- The Rust scheduler now applies the existing configured `history_retention_days` age cutoff followed by `history_max_rows` after recording every cycle.
- The prune path is best-effort: a database prune failure is logged but does not turn an otherwise successful scheduling cycle into a failed one.
- Existing PostgreSQL disk bloat is not implicitly vacuumed; physical reclaim remains a deliberately scheduled operator task.

## [2026-07-14] performance | Server-side Scheduler Cycle pagination

- The Execution tab now reads 12 scheduler-cycle rows at a time with a bounded offset, explicit total, and Previous/Next navigation.
- The dashboard query has an explicit UI projection rather than `SELECT *`; the standalone Scheduler API retains its existing limit-based response behavior.
- Page numbers clamp to the available history total, so arbitrary query parameters cannot force unbounded offsets.

## [2026-07-13] performance | Server-side Quiver signal pagination

- The Quiver tab now reads 40 signals at a time from only the latest Quiver run, with a run-scoped count and Previous/Next navigation.
- Page numbers clamp to the available total, preventing arbitrary query offsets, while the standalone Quiver signals API retains its existing limit-based response behavior.
- Header pills now reflect `quiver_signal_runs.success_count` and `error_count`, not merely the visible page.

## [2026-07-13] performance | Server-side Markov signal pagination

- The Markov tab now reads 40 signals at a time from only the latest completed Markov run, with a run-scoped count and Previous/Next navigation.
- Page numbers clamp to the available total, preventing arbitrary query offsets, while the standalone signals API retains its existing limit-based response behavior.
- Header pills now reflect `markov_signal_runs.success_count` and `error_count`, not merely the visible page.

## [2026-07-13] performance | Server-side execution-order pagination

- The Execution tab now requests a bounded 25-order page with an explicit total and Previous/Next links; out-of-range pages clamp to the last valid page.
- Overview renders only its 12 most recent queue entries, while other tabs retain 20 recent orders for the persistent execution-health indicator.
- This bounds the expensive per-row execution attribution lookups to the displayed page and avoids the previous fixed 80-row dashboard query.

## [2026-07-13] performance | Tab-exclusive dashboard collection gating

- Stopped fetching Hermes, execution detail, Markov, Quiver, Watchlist, End-of-Day, and decision-pulse collections for unrelated SSR tabs.
- Retained overview positions/orders plus shared market, quote, and latest-run inputs because they drive the persistent operational health strip.
- Added regression coverage for the tab-exclusive data gate; the next P0 step is pagination for long shared tables.

## [2026-07-13] performance | Performance-tab history gating

- Dashboard SSR no longer reads `portfolio_value_history` for views other than Performance, where no component consumes it.
- The Performance view continues to load its selected range using the existing range limits; the standalone performance API remains unchanged.
- Added regression coverage so a later dashboard change cannot silently reintroduce the cross-tab history query.

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

## [2026-08-19] improvement | Verified shadow reference quotes

- Replaced report/model price baselines for new Hermes counterfactuals and deterministic manager-gate shadows with captured Saxo read-only info-price references. The manager requests the existing quote refresh as soon as it persists a new shadow; the price monitor retries an `awaiting_reference` row when a market/session is temporarily unavailable.
- Each baseline now records a source and timestamp, while any report-provided price is displayed only as labelled context. Historical pre-provenance rows are marked `legacy_unverified_reference` and excluded from aggregate missed-trade learning evidence so old model-derived prices cannot contaminate future tuning.

## [2026-08-19] safety | Server-owned Decision Pulse authority

- Added a typed `ExecutionEligible` versus `Shadow` pulse mode and persisted `pulse_mode` plus `queue_eligible` on every Decision Report. Existing dry runs are backfilled as shadow/non-queueable; historical non-dry-run reports preserve their prior execution eligibility for audit continuity.
- The scheduled Trading Manager selector and the manual immediate pipeline both require `execution_eligible` plus `queue_eligible=true`, so a completed shadow report cannot write an execution queue row or reach a Saxo mutation path. The Decision Reports dashboard displays the recorded authority instead of inferring it from a label.

## [2026-07-10] improvement | Broker exposure integrity reconciliation

- Continued the accounting-integrity roadmap by comparing dashboard unrealised P/L against the latest Saxo instrument exposure aggregate.
- Added a warning-level quantity drift check between `broker_instrument_exposures` and `broker_position_snapshots`.
- New broker exposure integrity warnings receive stable issue keys, so the acknowledgement lifecycle can track them without hiding the underlying drift.

## [2026-07-21] improvement | Independent broker cash-book guard

- Verified that the configured Saxo SIM account carries an independent EUR capital balance, while the dashboard and buy limits use the bounded DKK strategy ledger configured under `portfolio.initial_cash_dkk` and `portfolio.virtual_cap_dkk`.
- Added the explicit `portfolio.broker_cash_reconciliation_enabled` opt-in. It defaults to `false`; broker balance snapshots remain available for execution and audit, but the Overview only compares absolute broker cash when both books are intentionally the same.
- This removes the false multi-million-DKK `broker_cash_drift` warning without weakening position, fill, exposure, or strategy-ledger integrity checks.

## [2026-07-21] testing | SELL partial-to-final fill reconciliation

- Added a database-backed, no-HTTP regression fixture for a SELL that first reconciles one partial share and later reaches a four-share final cumulative fill.
- The fixture verifies each delta consumes the correct fraction of the then-current basis, the remaining local book retains the correct six-share basis, and a replay of the final fill creates no extra fill or sale.

## [2026-07-21] improvement | Tiered monthly-loss guardrail

- Added a configured soft-loss tier at `strategy.capital.monthly_loss_soft_reduce_dkk` (-25,000 DKK) with `monthly_loss_soft_buy_multiplier` (0.50); it reduces the entire Trading Manager BUY budget only between that floor and the existing hard halt at -50,000 DKK.
- The manager persists the effective and unreduced budgets plus tier state, the Cash Deployment panel explains the active reduction, and decision prompts/Hermes preflight receive the same constrained BUY budget.
- Invalid, disabled, or reversed loss floors fail closed by leaving the soft tier inactive; hard-halt semantics, operator hard-halt override behavior, and SELL eligibility are unchanged.

## [2026-07-21] improvement | Candidate waterfall final technical evidence

- Fixed a decision-report audit ambiguity where the Candidate Scoring Waterfall rendered only the preflight technical snapshot even after the Trading Manager replaced it with a fresh database-verified daily indicator result.
- Manager outcomes now persist a compact final technical snapshot without raw model rationale, broker payloads, or error text. The waterfall renders the final signal, retains the preflight value as context, and explains the deterministic BUY or SELL condition behind a technical-gate block.

## [2026-07-21] fix | Decision report dry-run scheduler safety

- Hardened the existing provider/schema dry-run action so its persisted lifecycle is explicitly non-actionable: `dry_run_xai_deferred`, `dry_run_completed`, and `dry_run_error` are distinct from live report statuses for both OpenRouter and deferred xAI flows.
- The normalized dry-run artifact records its local safety boundary, the Decisions UI keeps a dry deferred run pending until its real terminal state, and the Trading Manager status gate rejects dry-run completion even if the scheduler sees it later.
- Added pure regression coverage for dry-run normalized output and Trading Manager rejection alongside the existing immediate-pipeline guard. No Saxo or Hermes mutation path is invoked by this mode.

## [2026-07-21] improvement | Staged execution failure diagnostics

- Execution failures now persist a small, sanitized `failure_stage` alongside the existing taxonomy: local validation, request build, local precheck guard, Saxo precheck, Saxo placement, or queue execution fallback.
- The Execution Queue and Order Events surface the stage as a compact label and in their existing tooltips and safe diagnostics, so `execution_failed` no longer hides whether Saxo was contacted.
- Non-ambiguous Saxo placement errors now persist their precheck context and placement stage directly rather than relying on the queue's generic error catch.

## [2026-07-22] data hygiene | Retire legacy cash-buffer runtime override

- The active Rust runtime does not read `strategy.capital.cash_buffer`; it was a Python scheduler compatibility setting that could preserve a zero-cash-buffer value from 2026-05-05.
- Startup now deletes that exact retired key idempotently after ensuring the runtime settings table. Active AI key/model settings and the short-lived manual-report claim remain untouched.
- Added a database-backed regression test that seeds both a retired cash-buffer setting and an active model setting, then verifies only the retired key is removed.

## [2026-07-22] safety | Sanitized Saxo failure alerts

- Routed the persisted Saxo execution error taxonomy into individual Slack failure alerts and execution-failure burst alerts.
- Alerts now send only the allow-listed category, label, remediation, and retry policy; raw broker diagnostics and local error text remain in protected execution records and are not included in Slack payloads.
- Added regression coverage proving taxonomy extraction excludes arbitrary raw fields and secret-like error text.

## [2026-07-22] Hermes governance | Expire stale pending proposals

- Added the terminal `expired_stale` lifecycle status for proposals that remain in `pending_review` beyond the configured 30-day review window.
- Each scheduler transition records its actor, reason, and threshold in `approval_json`; only still-pending rows are eligible, so approved, active, SIM, promoted, and broker paths remain unchanged.
- The existing 14-day Slack/dashboard warning remains the earlier operator signal, while expiry prevents indefinite duplicate-blocking proposal backlog.
## [2026-07-22] Hermes governance | Weekly proposal review digest

- Added a once-per-local-ISO-week Monday 09:00 sanitized Slack digest for current
  `pending_review` experiment proposals.
- The digest shows only safe audit metadata, review-age counts, and the configured
  stale-closure window. It is a notification only and cannot change a proposal,
  strategy configuration, baseline, or broker action.

## [2026-07-22] Hermes governance | Shared proposal pre-insert review

- Unified exact duplicate inspection across the protected Hermes HTTP adapter and the MCP `create_experiment_proposal` tool; active or pending proposals with the same normalized variable path are rejected consistently before insert.
- Added a deliberately narrow `cash_buffer_policy` related-family signal for the two supported cash-buffer paths. It returns safe metadata for active/pending sibling proposals but is advisory only: it cannot merge, reject, approve, activate, or otherwise change a different experiment.
- The returned review context excludes raw evidence, provider payloads, strategy configuration, and broker data. Hermes must put an exact duplicate candidate in reflection proposed actions instead of retrying proposal creation.

## [2026-07-23] Hermes evidence | Offline gate replay

- Added a bounded Decision Reports gate-replay projection over sanitized persisted Trading Manager snapshots.
- The initial one-variable comparisons test a Markov starter signed-signal threshold of `0.25` and a BUY technical-confluence threshold of `4`; results distinguish a target-gate flip from a full approval.
- Replay is read-only: it cannot call a provider or Saxo, create orders, alter experiments, or mutate runtime configuration.
- Added the safe replay summary to Hermes `get_context` and listed the Markov threshold as an allowed one-variable overlay proposal, so Hermes can use replay evidence without treating it as approval.

## [2026-07-25] roadmap | Protective stops and multi-horizon technical risk

- Recorded the current Rust technical-analysis boundary: daily Saxo OHLC analysis uses 260 daily samples with SMA20/50/200, RSI14, MACD, ATR14, and a 60-day resistance high for reward/risk. It does not calculate support zones or 5-year structural levels.
- Added a SIM-first broker-hosted protective-stop lifecycle item: after a confirmed BUY fill, precheck/place and reconcile a GTC SELL Stop where Saxo supports related orders, with explicit unprotected-position visibility. Stop-market behavior remains subject to gaps and execution slippage.
- Added a read-only, backtest-gated multi-horizon support/resistance roadmap item. It must demonstrate out-of-sample value as a risk overlay before Hermes or the Trading Manager can propose a one-variable activation.

## [2026-07-25] improvement | Read-only support-risk context

- Implemented the first multi-horizon support-risk slice from Saxo daily OHLC data. It clusters repeatable pivot lows from up to 1,260 daily bars, returns the nearest and next lower support, downside to each level, a bounded break-risk assessment, pivot-touch count, confidence, and actual returned-history coverage.
- Persisted the compact projection beside each daily indicator signal with additive runtime schema migration. Watchlists show a concise risk label and an explanatory tooltip; Decision Report manager snapshots and the OpenRouter prompt receive the same sanitized context; Hermes receives it through its bounded daily-indicator context.
- This is observation only. It does not change a technical gate, candidate quantity, Hermes lifecycle, broker order, or Saxo payload. The roadmap retains held-out backtesting and a one-variable SIM proposal as prerequisites for any future strategy effect.

## [2026-07-25] improvement | Support-risk outcome collection

- Added a bounded 180-day, read-only evidence projection over persisted daily indicator closes. It groups recorded low/moderate/high support-break labels and compares each to its next available one- and five-run closes for the same symbol, reporting sample counts, average return, negative-return rate, confidence, and indicator-history coverage.
- Decision Reports render the projection as `collecting` until at least 30 completed five-run observations exist; the same compact aggregate is included in the existing bounded gate-replay payload that Hermes reads. It is descriptive rather than causal and excludes costs, slippage, gap risk, provider calls, Saxo calls, configuration changes, lifecycle transitions, and order effects.

## [2026-07-25] safety | Read-only protective-stop coverage audit

- Added a bounded local reconciliation of persisted positive `broker_position_snapshots` against local SELL `Stop` and `StopLimit` execution records. Execution now shows protected, partial, planned, and unprotected coverage per snapshot position; Hermes receives the same sanitized, bounded context.
- Only `submitted_to_broker`, `broker_working`, `broker_amended`, `broker_partially_filled`, and `broker_replace_requested` stop statuses count as broker-confirmed coverage. Queued, uncertain, cancelled, and failed rows remain non-protective by design.
- The audit makes no Saxo HTTP call and cannot place, replace, cancel, or activate stops. It is a prerequisite visibility slice before any small-SIM broker-hosted `GoodTillCancel` Stop lifecycle experiment.

## [2026-07-25] safety | SIM protective-stop precheck probe

- Added an explicit operator-confirmed SIM-only probe for a broker-hosted `GoodTillCancel` SELL `Stop`. It reads the current Saxo position, validates sellable quantity, applies the existing Saxo tick normalizer, and calls only `/trade/v2/orders/precheck`.
- The probe cannot call Saxo order placement, create an execution order, or reserve a holding. Its local audit keeps only the requested safe fields, sanitized payload metadata, and result classification; tokens, account keys, and raw broker responses are excluded.
- The next lifecycle step remains a separately approved small-SIM placement/cancel/reconciliation test before any automation or filled-BUY child-order behavior is considered.

## [2026-07-25] safety | Manual SIM protective-stop lifecycle test workflow

- Added a separate lifecycle-test record for one operator-confirmed SIM `GoodTillCancel` SELL `Stop` placement after a successful local precheck. It is intentionally outside `execution_orders`, the scheduler, Trading Manager, and Hermes, so it cannot reserve inventory or affect routine decisions.
- Placement, cancellation, and reconciliation are distinct UI actions. Both mutations require their own SIM confirmation; reconciliation reads Saxo's open-order endpoint with audit-activity fallback and writes only sanitized broker state.
- Transport ambiguity or timeout becomes an operator-visible unknown/pending state with no automatic retry. The system does not create a broker order until an operator explicitly uses the placement form; no placement was made by this implementation.

## [2026-07-25] roadmap | Urgent todo, config contract, CI

- Added [urgent-todo](urgent-todo.md): a short ranked page for verified gaps between what the runtime claims or is configured to do and what it actually enforces. Six items: finish broker-hosted protective stops, config-contract audit, reconcile the Hermes goal contract with enforced reality, prompt-injection screen for editorial research, CI on every push, and Saxo rate-limit pacing for the now-unlimited nightly runs.
- Implemented the config-contract audit in `src/config_contract.rs` and wired it into startup logging and the Overview integrity payload. Against `config.yaml` it reports 20 enforced, 30 advisory, and 44 unused keys, 27 of them risk-surface: `strategy.enabled`, `trading_manager.enabled`, and both pulse `enabled` switches do nothing; position sizing is not risk-based; no concentration gate exists; five separate position-weight caps are unenforced; no protective stop, trailing stop, bracket, or session flatten is ever placed; `RISK_EXCLUDED_SYMBOLS` has no effect; and after-tax P/L equals pre-tax P/L. Also surfaced two config divergences: `trading_manager.max_report_age_hours` is read but supplied by neither shipped config, and `strategy.quiver.*` exists only in the Kubernetes config.
- Method note for future passes: leaf-name grep over-counts badly (`cash_buffer_pct` appears to have 54 hits because it is a substring of `min_cash_buffer_pct`). Statuses were established by extracting full config access paths — both `&["a", "b", "c"]` slices and chained `.get("a")` — from `src/*.rs`.
- Added continuous integration in `.github/workflows/ci.yml`: fmt, check, and test with warnings-as-errors on push, pull request, and manual dispatch. Hermetic — no secrets, no broker or provider access, no deploy step.
- Recorded that the Hermes goal contract declares `max_drawdown: 0.20` and `min_sharpe: 1.0` as constraints while only the monthly-loss DKK floors are enforced; drawdown is computed for display only.
- Recorded that editorial-research feed text is the first attacker-influenceable free text to reach the decision prompt and Hermes context, and must be screened and delimited before the feed catalog expands.
- Added roadmap rows for risk-configuration integrity, a drawdown guardrail reusing the existing tiered-budget mechanism, editorial-text screening, continuous integration, concentration-gate data prerequisites, maximum holding period, weekend/off-pulse gap exposure, and a concrete first `state.rs` extraction.
- The config-contract audit reads configuration and reports; it cannot change a gate, a size, or an order. CI adds no runtime code.

## [2026-07-25] strategy | Return goal realigned to +15% per year

- The operator's actual target is +10-20% per year. The configured goals stated three different and much larger things: `xai.performance_goals.monthly_target_dkk` 20,000 (~+115%/yr on a ~304,000 DKK book), `weekly_target_dkk` 5,000 (~+137%/yr), and a Hermes objective of `target_return_30d: 0.47` noted as "10x in 6 months" (~70x the real target).
- The Hermes figure was not cosmetic. `experiment_policy.promote_only_if.return_30d_gte` used the same 0.47, so no one-variable experiment could clear the promotion bar on merit, and every reflection was measured against a return only reachable by taking far more risk than the loss floors permit.
- Set to +15% per year (range midpoint) in all five copies: `config.yaml`, `deploy/k8s/base/config.k8s.yaml`, `deploy/k8s/base/hermes.yaml`, `docs/hermes-agent.md`, and `AppState::hermes_goal_contract_value`. Now `target_return_30d: 0.0117`, 880 DKK/week, 3,800 DKK/month, 1,200 DKK stretch week, `goal_version: 2`, `failure_below_30d_return: -0.02`, and matching `promote_only_if`/`rollback_if` thresholds. The Kubernetes watchdog reflection payloads were moved to `goal_version: 2` so reflections attribute to the active goal.
- Rescaled the monthly loss floors with the goal: -25,000/-50,000 was -8.2%/-16.4% of the portfolio in a single month, letting one bad month erase roughly a year of target gains before the hard halt fired. Now -9,000 soft (-3%) and -18,000 hard (-6%), preserving the 2:1 ratio and leaving SELLs unblocked.
- Recorded two follow-ups in [urgent-todo](urgent-todo.md): the targets are stored as DKK against a ~300,000 DKK book and drift silently as the portfolio changes, and `max_drawdown: 0.20` is now loose relative to a 15%/year target while still being unenforced.
- New finding U7: `deploy/k8s/base/hermes.yaml` lists `strategy.swing.cash_buffer_pct` as a supported experiment variable, and the config contract proves nothing reads it. Hermes can propose, run, observe, and promote an experiment whose variable has no effect.

## [2026-07-25] integrity | Adopted positions are not unreconciled orders

- Investigated the 15 orders the Overview integrity check reported as stale or unreconciled. All 15 were `status = 'executed'` with `ledger_id IS NULL` and a single shared `created_at` of `2026-05-05T05:20:09+00:00` — a bulk insert, not 15 separate events.
- They are the original portfolio adoption: `strategy_type = 'portfolio_sync'`, 19 rows in total (ids 1-19), of which 4 are `execution_failed` for instruments that never resolved (ARKI:xlon, ARKK:xmil, FIGR:xnas, QOMP:xetr). Every order from id 20 onward carries a real strategy key and a ledger id.
- Adopted rows record holdings that already existed at the broker when this system took over the book. No trade happened under this system, so a trade-ledger row is not merely missing — it would be wrong. The check was asserting an invariant that cannot hold for this row class.
- Fixed by excluding `portfolio_sync` from the ledger-less arm of the check and reporting `adopted_orders_without_ledger` as a separate count in the integrity payload. The count stays visible; it no longer alerts.
- This had held overview `healthy` false continuously since 2026-05-05, roughly three months. The operational cost is desensitization: a genuine `executed`-without-ledger fill — the exact failure class repaired on 2026-07-08 — would have arrived as an increment to a warning already treated as normal.
- The unreconciled-orders SQL and its adoption exclusion are now shared constants (`unreconciled_orders_sql`, `ADOPTED_ORDER_EXCLUSION`, `ADOPTED_ORDERS_WITHOUT_LEDGER_SQL`) so the regression test exercises the production query instead of a copy. The test was confirmed to fail against the previous behavior.
- No order, gate, ledger row, or broker call was changed. Only the integrity classification of existing rows.

## [2026-07-25] roadmap | Orphaned strategy_type on Trading Manager orders

- Recorded a new item (urgent-todo U8) after the unreconciled-orders investigation surfaced that `strategy_type` is NULL on 101 of 156 `execution_orders`.
- Not legacy residue: the newest NULL row is 2026-07-23. Legacy Python rows carry `swing` through 2026-05-07, the NULLs start 2026-05-12, and the stored timestamp format differs across the boundary (`+00:00` legacy, `Z` Rust). Every order the Rust Trading Manager has queued is affected.
- Root cause: `CandidateOrder::from_json` reads `strategy_type` from the model's suggested-trade JSON, and the field is not in the decision-report schema at all. The neighbouring `strategy_key` avoided this because `unique_strategy_key` constructs it locally.
- Operator-visible today: the Execution table renders `fallback_text(row, "strategy_type", "manual")`, so every automated order displays as `manual`; Slack `execution_source_label` falls through to `Execution` instead of `Trading Manager`.
- Also blocks the roadmap's per-pulse attribution work, and the column is now load-bearing for the 2026-07-25 adoption exclusion in the integrity check — correct today only because adopted rows are among the populated ones.
- Durable lesson for the wiki: record provenance in the component that knows it; do not ask the model to classify its own orders.
- Documentation only. No code, schema, gate, or broker behavior was changed by this entry.

## [2026-07-25] execution | Trading Manager orders record their own provenance

- Implemented urgent-todo U8. `CandidateOrder::from_json` no longer reads `strategy_type` from the model's suggested-trade JSON; the Trading Manager sets `TRADING_MANAGER_STRATEGY_TYPE` (`swing`) itself and ignores any value a model supplies. The value matches what the legacy Python runtime wrote through 2026-05-07 and what `execution_source_label` already maps to "Trading Manager", so backfilled and new rows read identically.
- Backfilled the 101 historical rows at startup, scoped to `strategy_type IS NULL AND report_id IS NOT NULL`. That predicate is exact rather than convenient: every unset row carried a report id, and every row with another strategy type (`portfolio_sync`, `clean_reconciliation`, `manual`) carried none, because those originate in adoption and manual paths rather than a decision report. The update is idempotent and cannot overwrite a value another path set.
- Changed the Execution table fallback from `manual` to `unknown`. The old fallback was what made the defect silent: an absent value was displayed as a concrete, wrong provenance instead of as missing. A display fallback should never assert a fact.
- Two tests: candidate orders carry runtime provenance even when the model claims a different `strategy_type`, and the backfill leaves adoption, reconciliation, manual, and report-less rows untouched across repeated runs.
- The pulse (scheduled EU/US or manual) remains in `strategy_session` and `strategy_key`; `strategy_type` answers "which subsystem queued this", not "which pulse".
- No gate, sizing, order, or broker behavior changed. This corrects a persisted classification and its display.

## [2026-07-25] safety | Protective stop levels computed from ATR

- Direction set with the operator: move from read-only observation toward enforcement, protective stops first. Recorded here because it reframes the next several slices — the two weeks before today produced audits and projections, not gates.
- Established a blocking prerequisite by inspection: `protective_stop_prechecks` and `protective_stop_lifecycle_tests` both hold zero rows. The whole protective-stop broker path — payload construction, tick normalization, precheck, placement, cancellation, reconciliation — has never executed against Saxo. Automating placement on top of an unexecuted broker path would bypass the manual-test gate the 2026-07-25 lifecycle work deliberately established.
- Split U1 into three slices. Slice 1 (this entry) needs no broker call. Slice 2 is the operator's manual SIM precheck plus one placement/cancel/reconcile. Slice 3 is automatic placement after a confirmed BUY fill, together with the SELL-reservation conflict: a stop reserving the full position would block discretionary exits, which is the parent/child linkage the roadmap flagged as separate design work.
- Slice 1 makes `strategy.ladder.stop_loss_atr_multiple` live. For every position without full broker-confirmed coverage, the audit computes `close - (ATR14 x multiple)` from the latest stored daily indicator row and reports the level, reference close, ATR14, multiple, absolute distance, and distance percentage. The proposal is sized to the uncovered quantity only, so a partially covered position proposes a stop for the remainder rather than the whole holding.
- Fails closed rather than guessing: no proposal when close or ATR14 is missing or non-positive, or when the computed level would be at or below zero. The payload states `tick_normalized: false` — rounding to Saxo's tick scheme requires instrument details and belongs to the precheck/placement path.
- The config contract entry moved from `unused_risk` to `advisory`, and becomes `enforced` when slice 3 lands. The contract's own drift check is what forced this update.
- Read-only. No Saxo call, no order, no reservation, no scheduler or Hermes involvement.

## [2026-07-25] incident | SQLite-only placeholders broke two production writers

- Surfaced when the operator ran the SIM protective-stop precheck and saw no feedback. The Saxo call had in fact succeeded — the API logged `SIM protective-stop precheck completed without placing an order` for both `V:xnys` and `LMND:xnys` — but the audit insert failed with `syntax error at or near ","` and `protective_stop_prechecks` stayed empty.
- Root cause: `?` bind placeholders are SQLite-only. The runtime opens an `sqlx::AnyPool`, so the same statement runs against local SQLite and Kubernetes PostgreSQL, and PostgreSQL rejects `?` at execution time. Nothing catches it at compile time, and nothing catches it in a SQLite-backed test.
- Two production writers were affected. `src/state.rs`: `record_protective_stop_precheck`, `prepare_protective_stop_lifecycle_test`, and `update_protective_stop_lifecycle_test`. `src/editorial_research.rs`: `store_item`, `source_due`, `record_run`, and `prune_old_records`.
- `update_protective_stop_lifecycle_test` was the dangerous one. It records placement, cancellation, and reconciliation of a real broker order; had the operator continued past the precheck to the lifecycle test, a live SIM stop would have existed at Saxo with no local record of it at all.
- Editorial research had never worked in production. Deployed 2026-07-25 with `editorial_research_items` and `editorial_research_runs` both at zero rows and the scheduler failing every cycle. Its four tests pass because they run on in-memory SQLite — passing tests were the misleading signal that argued for shipping it.
- `src/portfolio_reset.rs` was checked and already uses `$1` correctly.
- Fixed by renumbering to `$1`-style placeholders, verified to work on both backends, rather than converting to `format!` plus `sql_escape`. Parameter binding is kept deliberately: `editorial_research` stores untrusted third-party RSS text, which is exactly where string interpolation is worst.
- Added `production_sql_uses_backend_portable_placeholders` in `src/db.rs`. It scans each source file up to its first test module for placeholder-shaped SQL and fails with file and line. Confirmed to catch the original defect at `state.rs:4418`. Test modules are exempt because they only ever run on SQLite.
- Durable lesson: a green SQLite test suite does not evidence a PostgreSQL code path. Any future backend-specific behavior needs either a portability guard like this one or a PostgreSQL-backed test.

## [2026-07-25] safety | Bulk SIM stop placement, orphan recovery, and the fill-detection gap

- Diagnosed the stuck `placement_preparing` lifecycle test. No order reached Saxo: zero placement calls in the log, `placement_result_json` was `{"placement":"not_sent"}`, and `broker_order_id` was NULL. A double-clicked submit had committed the prepared row, then axum dropped the handler future when the browser cancelled the duplicate. The second request hit the duplicate guard correctly — against an orphan its own predecessor left behind.
- The orphan blocked its precheck permanently, because `placement_preparing` counts as active. Fixed by reconciling stale preparations against Saxo before reuse: only rows the broker does not know about are marked `placement_abandoned`. A row is never expired on a timer alone, because the same interruption could equally have happened after a successful placement. Rows carrying a broker order id are never abandoned.
- Added double-submit prevention in the batch form, so the cause is treated as well as the symptom.
- Added operator-confirmed bulk SIM placement from the Protection Exceptions table: a checkbox per eligible row plus one confirm. Symbols, quantities, and stop levels come only from the read-only coverage audit; operator-supplied prices are not accepted. Placement is strictly sequential, spaced 1.1s for Saxo's 1 order/second limit, and halts the entire batch on the first rejection, error, or ambiguous broker response. An ambiguous response is never retried and never followed by another placement.
- Recorded the fill-detection gap the operator raised. `sync_saxo_broker_orders` already runs twice per scheduler cycle — every 10 minutes, dropping to 1 minute while `outstanding_order_count > 0` — but reads `execution_orders` only, and `protective_stop_lifecycle_tests` is referenced zero times in that path. A stop filling at Saxo today would produce no ledger row, no position update, and no Trading Manager awareness.
- Design consequence: automated protective stops must be created as `execution_orders` rows rather than lifecycle-test rows. Broker sync, fast polling, fill reconciliation, and the coverage audit then cover them without new plumbing. The manual lifecycle-test table stays what it was built for — a one-off validation harness outside the queue.
- Open risk to handle with that change: a resting GTC stop keeps `outstanding_order_count` above zero indefinitely, which would pin the scheduler at 1-minute polling. The fast-poll trigger must exclude resting protective stops.

## [2026-07-25] fix | Repeated form fields in bulk stop placement

- The bulk placement button returned `Failed to deserialize form body: symbols: invalid type: string "LMND:xnys", expected a sequence`. A checkbox column submits one repeated `symbols` field per checked row, and `serde_urlencoded` — which axum's `Form` extractor uses — cannot map repeated keys onto a `Vec`. It rejects the whole request rather than collecting them.
- Parsed the body directly with `form_urlencoded` instead. The crate was already in the dependency tree via `url`, so nothing new is downloaded. The parse is a pure function: it decodes, trims, upper-cases, de-duplicates, and drops blank symbols, and treats confirmation as opt-in so a missing or non-`true` value places nothing.
- Two tests cover it, one reproducing the exact submitted body from the failure.
- Note for future form work in this codebase: any multi-select or checkbox column has this problem. `Form<T>` with a `Vec` field will compile and then fail at runtime on the first real submission.

## [2026-07-25] security | Editorial research injection screen (U4)

- Landed the prompt-injection screen while the exposure was live rather than theoretical: editorial ingestion started working in production that morning and is now feeding real items into the decision prompt.
- A deliberately narrow marker list detects text addressed at a model rather than a reader — "ignore previous", "system prompt", "you are now", role markers, chat-template delimiters. Ordinary financial language is explicitly out of scope: "buy", "sell", "upgrade", and "target price" must keep flowing or the screen would gut the feature and train the operator to ignore the flag. A test pins both directions, including "Fed signals it will disregard one month of noisy inflation data" as a non-match.
- Screening happens at the context boundary, not only at ingest. That covers items stored before the screen existed — including the twelve already in production — and means widening the marker list applies retroactively with no backfill. Flagged items stay in the database for review and are reported as `screened_out`; they simply never reach a prompt.
- The decision prompt now carries an explicit security-boundary instruction: every string in the section is untrusted third-party text, is data to read rather than instructions to follow, and cannot alter the instructions, schema, market scope, or any gate.
- Blast radius was already bounded by the deterministic Trading Manager gates — injected text cannot bypass the technical, Markov, or budget checks — but it could bias which candidates the model proposes and consume the limited suggestion slots.

## [2026-07-25] governance | Hermes cannot experiment on a dead variable (U7)

- Removed `strategy.swing.cash_buffer_pct` from the supported one-variable overlay list in both the Rust capabilities payload and the Kubernetes ConfigMap. The config-contract audit had proved nothing reads it, so an experiment on it could have been proposed, run in SIM, observed, and promoted while changing nothing at all — and whatever the portfolio did in that window would have been attributed to it.
- The list is now a single `SUPPORTED_EXPERIMENT_VARIABLES` constant, published from one place and cross-checked against the config contract by test. Any variable the contract classifies `unused` fails the build. Confirmed the guard fails when the dead path is re-added.
- Paths outside the audited roots (`execution.min_trade_value_dkk`) are not described by the contract and are skipped rather than assumed dead.

## [2026-07-25] safety | Protective stops need StopIfTraded, not Stop

- The first real placement attempts all failed with Saxo `OrderTypeNotSupported: The chosen order type is not supported for this instrument type`, across LMND, ASML, DANSKE, and AAPL. The batch halted on the first failure each time, exactly as designed, so five attempts produced five failed records and no partial exposure.
- Root cause: the payload sent `OrderType: "Stop"` with `AssetType: "Stock"`. In Saxo's OpenAPI `Stop` is the FX form, triggered on bid/offer; equities use `StopIfTraded`, triggered by the traded price, which is what a protective stop on a share position means.
- The important operational lesson is separate from the fix: **Saxo's precheck accepted every one of these orders before placement rejected them.** `/trade/v2/orders/precheck` does not validate order-type support for the instrument. Precheck acceptance is therefore not evidence that an order can be placed, and no workflow should treat it as a green light.
- Fixed by reading `SupportedOrderTypes` from `/ref/v1/instruments/details/{uic}/{assetType}` and selecting from a preference list (`StopIfTraded`, then `Stop`) rather than hardcoding. This adapts per instrument and per asset type instead of encoding an assumption, and fails closed naming what the instrument does support. When reference data is silent it uses the equity form.
- Also validated by this episode: the orphan recovery worked. Lifecycle test 1 (`V:xnys`) was reconciled against Saxo, confirmed absent, and marked `placement_abandoned` automatically, unblocking its precheck without operator action.
- Remaining UI gap: `placement_failed` rows show "No action available". A failed placement should offer a retry once the cause is fixed, rather than leaving a dead row.

## [2026-07-25] safety | First protective stops placed; batch moved off the request path

- The `StopIfTraded` fix worked. Nine SIM protective stops were placed with real Saxo order identifiers (AAPL, ADS, AMAT, AMD, AMGN, ASML, BAC, CHEMM, DANSKE), which is the first time this broker path has completed end to end.
- No duplicates were created despite the operator running overlapping batches — all twelve, then several subsets, then all twelve again. The per-precheck active-test guard held every time: each symbol has exactly one active record and one broker order id. This is the evidence that the duplicate guard works under real concurrent operator behaviour.
- A twelve-symbol batch timed out after 15-30 seconds behind the public proxy. Each order costs several Saxo round trips (instrument lookup, tick details, order-type details, precheck, placement) plus 1.1s spacing, so a full batch exceeds any reasonable request timeout.
- That is not a cosmetic problem. When the client disconnects, axum drops the handler future, and mid-batch that can place an order at Saxo and lose the local record — the same class as the earlier double-click orphan, but during a mutation sequence rather than before it. One `placement_abandoned` row came from exactly this.
- Fixed by running the batch detached with `tokio::spawn`. The request validates, resolves stale preparations, builds targets, and returns immediately; placement proceeds independently of the client. The operator watches the lifecycle table rather than a held-open request.
- Also added automatic reconciliation after each successful placement. `placement_submitted` is not coverage: the audit counts a stop only once Saxo reports it working, so a batch previously left the operator a table of unverified placements. Reconciliation failure leaves the row submitted rather than guessing, because the order exists either way.
- Remaining: DDOG, LMND, and V are still unprotected. `placement_failed` rows still offer no retry action.

## [2026-07-25] safety | All twelve positions carry a protective stop

- Every position is now covered. Nine stops were placed at 20:08 and three more (DDOG, LMND, V) at 20:21; the three placed after auto-reconciliation landed went straight to `broker_working`, confirming the reconcile-after-placement change works.
- Repeat batches then began failing precheck with Saxo `SellOrdersAlreadyExistForOwnedContracts: A sell order already exists for this instrument`. This is the broker enforcing the one-resting-sell-per-holding rule — the same conflict recorded as slice 3's hardest open item, surfacing at the broker rather than locally.
- Root cause of the wasted attempts was local, not broker-side: the nine stops sat at `placement_submitted`, and the coverage audit counts only `broker_working`. Their positions therefore still rendered as unprotected exceptions, so each new batch dutifully retried them against stops that were already resting.
- Three fixes. A sweep now reconciles `placement_submitted` rows older than 15 seconds so they reach the state the audit counts. The batch independently excludes any symbol with a non-terminal lifecycle test, so it cannot attempt a second sell even while coverage lags. Both new broker errors gained taxonomy entries — `sell_order_already_exists` (reconcile before retry) and `order_type_not_supported` (manual review) — replacing "Unclassified Saxo failure".
- Worth recording as a general lesson: local state lagging the broker does not merely mislead a dashboard. Here it caused the system to repeatedly attempt orders the broker had already accepted, and only Saxo's own guard prevented double protection.

## [2026-07-25] safety | Scheduler confirms placed protective stops

- Added a read-only protective-stop confirmation step to the scheduler cycle. It asks Saxo what state each already-placed stop is in and records the answer. It cannot place, amend, or cancel anything, so the manual-only boundary around stop *mutation* is unchanged.
- Needed because confirmation previously ran only inside a placement request. A stop therefore stayed at `placement_submitted` until an operator happened to trigger another placement, the coverage audit kept reporting its position as unprotected, and the next batch retried an order Saxo already held.
- This is the first scheduler involvement in the protective-stop lifecycle. The distinction that keeps it safe: the scheduler may *observe* stop state, and only an explicitly confirmed operator action may change it.

## [2026-07-25] safety | Protective stops become real orders and yield to decided exits

- U1 slice 3a. Protective stops are now adopted into `execution_orders` each scheduler cycle, which is the load-bearing half of the slice-3 design. `sync_saxo_broker_orders` reads that table and nothing else, so until now a stop could fill at Saxo overnight and produce no ledger row, no position update, and no Trading Manager awareness. Adoption inherits broker sync, fill reconciliation, the trade ledger, position updates, and the Slack alert with no new plumbing.
- Adoption writes local rows only — the broker order already exists — and is idempotent on `broker_order_id`, with a unique `strategy_key` as a second guard against two scheduler pods racing during a rollout.
- Adding a new row type to `execution_orders` turned out to be the interesting part: it inherits every query ever written against that table, including the ones that assumed the old population. Four consumers needed to learn what a protective stop is, and each was a live defect waiting for the first adopted row.
  - The stale-order integrity check flags anything `broker_working` for over 24 hours. A GoodTillCancel stop is supposed to rest for the life of the position, so all twelve would have become permanent warnings — the same false positive adopted positions produced earlier the same day. Only the age branch is scoped; `broker_state_unknown` and executed-without-a-ledger-row still apply.
  - `outstanding_order_count` drives the scheduler's fast poll. Twelve resting stops would have held it above zero forever, silently converting a 10-minute cadence into a permanent 1-minute one.
  - `active_sell_reservations` would have counted each stop as reserving the whole holding, making every discretionary exit look impossible.
  - The instrument-quarantine scan reads any row with `error_text` as an instrument fault, and `update_order_broker_status` writes `error_text` for every `broker_cancelled` row. Since releasing a stop before a sell is routine, the runtime would have accrued quarantine strikes against precisely the symbols it was trading successfully, and eventually refused to trade them.
- The SELL-reservation conflict, filed as slice 3's hardest open item, is resolved. Yesterday's `SellOrdersAlreadyExistForOwnedContracts` rejection proved the broker permits exactly one resting sell per holding, so layering a stop and a discretionary sell was never possible. `cancel_protective_stops_before_sell` now clears the stop at the single chokepoint in `execute_order`, before the sell payload is built.
- That cancellation is the first automatic broker mutation in the protective-stop machinery, so it is deliberately narrow: scoped to rows this runtime marked `protective_stop` on exactly the symbol being sold; it does not trust Saxo's acceptance of the DELETE but reads the order back and refuses to proceed while the stop is still working; and it releases the lifecycle-test row so the position can be re-protected afterwards. Standing protection yields to a decision — it is never cancelled on a timer.
- Remaining in slice 3b: fill-triggered placement, amendment on quantity change, and the trailing ratchet. Until those land, a partial exit leaves the residual position unprotected until the next operator batch.

## [2026-07-26] safety | Protective stops maintain themselves

- U1 slice 3b, and the last of the automation the operator asked for: stops now appear, re-size, and ratchet without anyone clicking. `src/protective_stops.rs` runs one sweep per scheduler cycle.
- The design brief called for three separate triggers — place after a fill, amend on a quantity change, trail as the price rises — and they collapsed into a single declarative reconciliation, which is a better shape. The sweep compares each position's desired protective state against its actual one and closes the gap, so one path covers a new BUY fill, a partial exit leaving a residual holding, a stop released for a discretionary sell, and a placement that failed earlier. Nothing hooks a fill event, so no event can be missed while the process restarts. A missed event is silent; a missed reconciliation is corrected on the next cycle.
- `decide_stop_action` is pure and testable, and holds two invariants at the point where the price is computed rather than at the call site. A stop never moves down, which makes the resting order its own high-water mark and the ratchet monotonic without a stored peak. A stop always sits below the last close, because a stop at or above the market fires on acceptance and turns protection into an unplanned market sell.
- It fails closed. Missing or non-finite close/ATR, a sub-one-share position, or a computed level not below the close all yield `Hold`. An unprotected position is visible in the coverage audit; a stop at a fabricated level is not. Writing the tests surfaced a state I had not considered — a resting stop already above the close, which is self-contradictory and means stale data — and holding there is the only outcome that cannot make things worse.
- This is the first path in the runtime that places a broker order with nobody confirming it, so the guards carry the weight: gated on `strategy.ladder.submit_stop_loss_after_fill`, SIM-only, skips exchanges not currently accepting orders, capped at five actions per cycle, halts on the first failure. Cancelling to replace leaves a genuine unprotected window, so it is verified at the broker before the replacement is requested and logged at warn level while it lasts.
- Three dead config keys became enforced: `submit_stop_loss_after_fill`, `stop_loss_atr_multiple`, `trail_stop_atr_multiple`. `min_ratchet_atr_fraction` is new — without hysteresis, ATR drift would rewrite twelve broker orders a day for no protective gain, each costing an unprotected window.
- Worth flagging rather than burying: the configured trail multiple (1.25) is tighter than the initial one (2.0), so every existing stop becomes ratchet-eligible immediately, before any position has appreciated. The twelve stops placed yesterday at 2.0 ATR will be rewritten to 1.25 ATR over the first few sweeps. That follows from the configured pair, but it is a tightening of risk posture caused by a config relationship rather than by price, and 1.25 ATR is close to daily noise on a swing horizon.
- Follow-up the same morning: the first live sweep reported `considered: 12, skipped_market_closed: 12` with an empty `held` list — every position was queued for a rewrite and only the closed exchanges prevented it. Confirmed against the data: all twelve stops sat at exactly 2.00 ATR. Setting `trail_stop_atr_multiple` to 2.0 makes the ratchet respond to price rather than to a config relationship, and a test now pins that semantics. The lesson is about observability rather than about stops: the sweep's own output made a mistimed risk change visible before it could execute, because it reports what it *would* do and not only what it did.
## 2026-07-26 - Hermes experiment contract reconciliation

- Reconciled the Trading Manager overlay loader to `SUPPORTED_EXPERIMENT_VARIABLES`, the same checked list published in Hermes capabilities.
- Removed the retired `strategy.swing.cash_buffer_pct` alias and two manager-only unpublished paths from overlay acceptance; revised daily/weekly reflection prompts and operator documentation to match.
- Added regression coverage proving published variables are loadable while retired or unpublished paths cannot affect queue creation.
## 2026-07-26 - After-tax estimate becomes real

- Ported the deterministic Danish share-income estimate from the legacy portfolio path into the Rust overview: progressive configured brackets now estimate the incremental tax on current-year realised SELL gains plus current unrealised P/L.
- The dashboard labels the value as an estimate and shows the provisional tax. Invalid brackets, a non-DKK setting, or an unavailable ledger leave the gross value unchanged and surface an unavailable status.
- The estimate is read-only: it does not rewrite ledger tax, affect performance accounting, change sizing, or make a Saxo request.

## 2026-07-26 - Observed position-lifecycle attribution

- Added read-only cross-order lifecycle evidence to Execution attribution from local reconciled fills ordered by timestamp and fill id. It labels a current order as `entry`, `add`, `reduce`, or `exit` only when that local sequence supports the claim.
- A sequence beginning with a SELL or crossing below zero is explicitly `partial_history`; the UI states that local fill history excludes imported inventory, later broker adjustments, and broker position truth.
- Added pure and database-backed regression coverage plus Execution detail rendering coverage. This path makes no Saxo, provider, or broker mutation call.

## 2026-07-26 - BUY cost guard becomes real

- Wired `strategy.estimated_slippage_bps` and `strategy.cost_guard_multiple` into the Rust Trading Manager after price verification, technical evaluation, and ATR risk sizing. A BUY now needs database-verified indicator reward to exceed a deterministic lower-bound hurdle: the exchange minimum round-trip commission multiplied by the configured guard plus configured one-way slippage.
- The manager records expected reward, commission floor, slippage, required reward, configuration, basis, and pass/fail outcome in sanitized manager JSON. The Decision Report waterfall renders the calculation for a blocked candidate, and Hermes receives the active policy in its preflight context.
- The lower-bound estimate intentionally does not promise actual broker commission, FX cost, spread, or fill price. Missing/model-supplied indicator data and invalid negative configuration fail closed.

## 2026-07-26 - Decision Report candidate ceiling becomes real

- Wired `strategy.swing.trading_manager.max_symbols` into the Rust Trading Manager as an early, per-report distinct-symbol limit. The provider's raw report remains intact, while the first configured symbols in report order proceed to Hermes and deterministic gates; excess distinct symbols are stored as skipped audit rows rather than silently dropped.
- Repeated actions for an already-admitted symbol remain eligible. `0` is explicit unlimited mode; a negative value records every candidate as skipped and creates no queueable order. Hermes receives the active cap and only eligible candidates.

## 2026-07-26 - Per-symbol BUY exposure cap becomes real

- Wired `strategy.ladder.max_position_weight` into the Rust Trading Manager as a deterministic total per-symbol BUY-exposure ceiling. It combines persisted DKK position value with BUYs approved earlier in the same scheduler cycle, sizes a new BUY only into remaining headroom, and blocks it if less than one share fits.
- Missing/invalid portfolio or exposure evidence fails closed. The sanitized basis, cap, existing value, headroom, quantity reduction, and final value are stored in the manager run, rendered in the Candidate Scoring Waterfall, and passed to Hermes preflight. Hermes remains advisory and cannot relax the manager gate.

## 2026-07-26 - Maximum holdings becomes real

- Wired `strategy.swing.max_holdings` into the Rust Trading Manager as a deterministic cap for new-symbol BUYs. It counts persisted positive-quantity holdings and reserves a slot for every new-symbol BUY approved earlier in the same scheduler cycle; adds to existing symbols do not consume a second slot.
- An unavailable position snapshot fails closed for a new symbol. The active policy and sanitized holding count are stored in manager JSON and Hermes preflight, while the Hermes goal contract now correctly declares `constraints.max_positions` as runtime-enforced.
## [2026-07-26] safety | Retired duplicate swing cash-buffer configuration (U2/U7)

- Removed `strategy.swing.cash_buffer_pct` from both shipped configs and the config contract. The active and only supported cash reserve is `strategy.capital.min_cash_buffer_pct`.
- Startup now purges both retired database runtime-setting keys (`strategy.capital.cash_buffer` and `strategy.swing.cash_buffer_pct`), so a historical override cannot resurrect an inert path.
- Updated the Python reference's runtime settings, swing sizing, and decision prompt to consume the active capital reserve rather than a separate swing setting. Hermes already rejects the removed experiment variable.
## [2026-07-26] safety | Trading Manager report freshness becomes explicit configuration

- Added `strategy.swing.trading_manager.max_report_age_hours: 6` to both shipped configs. The Trading Manager already used this value as its Rust fallback; scheduled reports older than the window cannot create execution orders.
- Updated the config contract, README, urgent todo, and roadmap so report freshness is now an operator-visible advisory policy rather than a contracted-but-absent default.

## [2026-07-26] operations | Local Quiver cadence matches production

- Added the production `strategy.quiver` policy to `config.yaml`: enabled on weekdays at 23:10 Europe/Copenhagen, with a 120-day lookback and a 60-symbol cap. The `QUIVERQUANT_API_KEY` remains environment-backed and is not stored in configuration or documentation.
- Added a config-contract regression test that compares the full local and Kubernetes Quiver policy blocks. Local scheduler runs no longer silently use Rust defaults when validating advisory-data behavior.

## [2026-07-26] safety | Retired inert minimum-selection floor

- Removed `strategy.min_selected_assets` from both shipped configs and the Rust config contract. The active Rust Trading Manager never read it, so leaving it configured suggested a minimum portfolio breadth that did not exist.
- Removed the same fallback from the legacy selector. It now preserves its sector-screened candidate list instead of re-expanding it to reach a minimum. The actual `strategy.max_selected_assets` cap remains unchanged.

## [2026-07-26] operations | Content-addressed deployment images

- Default API and backup image tags now equal the validated full Git SHA of the clean worktree. The deploy metadata, Kubernetes image references, and binary build revision identify the same source without relying on a timestamp.
- `IMAGE_TAG`, `API_IMAGE`, and `BACKUP_IMAGE` remain deliberate operator overrides. CI syntax-checks the deploy and post-deploy scripts alongside Rust formatting, checks, and tests.

## [2026-07-26] operations | Read-only Saxo ENS activity backfill

- Added a daily, bounded `ens/v1/activities` read over the preceding 14 days for Order and Position activity. The scheduler records only aggregate counts, local broker-order match counts, page-capping state, and latest activity time; it never persists raw broker activity, account or client identity, symbols, order identifiers, fills, or ledger changes.
- The daily completion cursor survives rollouts in `runtime_settings`, preventing the 10-minute scheduler from repeatedly requesting the same lookback window. A capped response is explicitly marked `partial`; pagination and deterministic event-level reconciliation remain follow-up work.

## [2026-07-26] trading-quality | Read-only trade-thesis outcome evidence

- Execution now aggregates only the latest 50 persisted, `recorded` BUY thesis snapshots against their reconciled local fills and later stored daily indicator closes. It reports directional return and positive-rate evidence at one and five sessions, and remains explicitly `collecting` until 20 mature five-session observations exist.
- This is forward-only observational context, not a backtest or a causal measurement. It excludes blocked candidates, FX, commission, tax, slippage, later position changes, broker adjustments, and outside-ledger inventory; it performs no Saxo, provider, Hermes, configuration, gate, or order mutation.

## [2026-07-27] trading-quality | Deterministic missed-trade shadow book

- Added a separate, bounded quote-to-quote observation ledger for selected deterministic Trading Manager blocks: candidate limit, market timing, monthly-loss/drawdown guardrails, cash budget, risk/cost floors, and holding or selection capacity. The Hermes counterfactual ledger remains limited to quantity Hermes blocked or reduced.

## [2026-07-27] trading-quality | Missed-trade shadow evidence aggregation

- Added a bounded read-only aggregate for missed-trade shadows in the Hermes view. It shows equal-weighted directional quote outcomes overall and by recorded manager gate, remains `collecting` until 20 observed rows, and does not combine currency-denominated P/L.
- The aggregate is diagnostic only: it neither reopens a deterministic gate nor changes Hermes advice, configuration, Saxo/provider calls, or order behavior. It excludes fees, FX, slippage, tax, broker execution, later position changes, and causal claims.
- The new shadow book excludes technical/Markov validity failures, risk exclusions, and instrument quarantines. It records only compact gate provenance, local candidate quantity/price/currency, and later price observations; it is not a backtest, causal result, realised P/L, broker execution, or input to any gate, Hermes recommendation, or order.

## [2026-07-27] trading-quality | Decision pulse outcome evidence

- Added a bounded, read-only Execution aggregate grouped by report origin: EU open follow-up, US open follow-up, manual/manual dry-run, portfolio sync, and other/legacy. The source is only local execution orders, reconciled fills, local ledger rows, and stored daily indicator closes.
- BUY outcomes retain the existing equal-weighted one- and five-session directional-return method; SELL outcomes separately sum reconciled local-ledger DKK gain, commission, and tax. This is not aggregate unrealised P/L, broker position truth, a backtest, or a causal claim.
- Hermes effect coverage now comes from the matching candidate in the persisted Trading Manager `hermes_advice_delta` snapshot, keyed by the stored execution order's strategy key, symbol, and action. The panel reports effects such as `allowed` and `reduced`, never raw advice rationale or a reconstruction from live data.
- The classification remains non-causal. It does not compare Hermes-effect subgroups, and separately recorded blocked/reduced counterfactuals remain in the Hermes view. No Saxo, provider, Hermes, configuration, manager gate, or order mutation is performed.

## [2026-07-27] trading-quality | Decision pulse order-state coverage

- Added bounded per-pulse counts of the local `execution_orders.status` values, including completed, working, expired, and failed states where those exact records exist. The panel does not query Saxo to fill missing data or infer a broker outcome from time or price.
- Status coverage remains descriptive and non-causal. It does not classify root causes, modify local queue state, requeue an order, or alter Hermes, Trading Manager, configuration, or broker behavior.

## [2026-07-27] risk | Exchange and currency concentration gates

- Added `strategy.concentration.max_assets_per_exchange` and `strategy.concentration.max_assets_per_currency` to both shipped configs and the Rust config contract. Both defaults are `0`, an explicit unlimited policy, so the rollout exposes and audits the gate without silently changing live allocation policy.
- A positive cap counts distinct positive-quantity persisted holdings plus BUYs approved earlier in the same Trading Manager cycle. It uses only the canonical exchange suffix and the local exchange-to-currency mapping, lets an add to an existing symbol retain its slot, and fails closed when snapshot or bucket evidence is unavailable. Negative configuration is invalid and blocks BUYs.
- The policy, bucket counts, and outcome are included in the Trading Manager snapshot, Hermes preflight, and Candidate Scoring Waterfall. It never alters SELLs, queries a provider, or exposes broker/session data. Sector concentration remains deferred pending a durable sector-data source.

## [2026-07-27] trading-quality | Read-only holding thesis reviews

- Added a bounded Holding Thesis Reviews queue in Execution and sanitized it into Hermes context. It compares only the latest persisted broker-position snapshot with a recorded BUY thesis and reconciled fill timestamp when present; a missing thesis or ambiguous timestamp is omitted rather than inferred.
- A row becomes review due when its decision evidence has aged beyond the configured seven-day horizon or its recorded two-week thesis window elapsed. The displayed next step is a fresh decision comparison against current verified technical and Markov evidence.
- This is deliberately not a maximum-holding-period exit rule. It performs no Saxo/provider call, does not alter Hermes advice or Trading Manager gates, and cannot place, amend, cancel, size, approve, or retain a broker order.

## [2026-07-27] risk | Retired unsupported sector concentration cap

- Removed `strategy.max_assets_per_sector` from the local and Kubernetes configurations, config contract, documentation, and legacy reference selector. The active Rust Trading Manager never read this setting, so the change does not modify deployed BUY/SELL behavior.
- Exchange and currency concentration remain the supported, audited diversification controls. A sector cap can return only with a durable sector-data source and explicitly defined held/planned exposure semantics; it must not rely on model text or inferred labels.

## [2026-07-27] risk | Retired duplicate position-weight controls

- Removed the three unused strategy weight knobs and the unused `risk.max_position_weight` override from both shipped configurations and the config contract. The Rust manager's existing `strategy.ladder.max_position_weight` remains the sole enforced 4% default cap on total held/planned BUY exposure for one symbol.
- Legacy reference prompt, planning, and execution now derive their ceiling from that same supported setting. This changes no active Rust/Saxo decision or broker behavior, but prevents a future reference-only run from applying a contradictory 25% cap or a 2% minimum target weight.

## [2026-07-27] risk | Retired inactive session-flatten controls

- Removed `strategy.ladder.session_flatten_enabled` and `flatten_minutes_before_tradable_close` from both shipped configurations and the config contract. The Rust runtime has no session-flatten path and the swing strategy is explicitly designed to hold across market closes.
- Protective stops remain the active intraday/overnight downside control. The legacy reference defaults remain disabled when these settings are absent, so the cleanup changes no active decision, scheduled exit, or broker behavior.

## [2026-07-27] trading-quality | Retired legacy benchmark ticker configuration

- Removed the legacy Yahoo ticker map at `strategy.swing.journal.benchmark_indices` from both shipped configurations and the Rust config contract. The Rust journal did not query or calculate from it, so leaving it configured implied a benchmark comparison that does not exist.
- The legacy reference retains its own default map when run separately. A future Rust benchmark must use verified canonical Saxo symbols with daily-indicator close coverage, an explicit fixed comparison baseline, and coverage/freshness reporting. It will remain read-only and outside decision, Hermes, sizing, and broker paths until separately implemented and tested.

## [2026-07-27] operations | Shared Saxo HTTP transport

- Auth, Markov, daily-indicator, portfolio, and order calls now share a process-wide 30-second `reqwest::Client`, which permits connection-pool and HTTP/2 reuse instead of constructing a client for every request.
- The transport refactor does not centralize policy: rate pacing, OAuth/session handling, request ids, retries, response parsing, and broker-mutation decisions remain at the prior call sites. Provider, Slack, editorial, and public-data clients keep separate transports and timeouts.

## [2026-07-27] risk | Retired inactive bracket and take-profit switches

- Removed `strategy.ladder.submit_bracket_with_entry` from both shipped configurations and `submit_take_profit_after_fill` from local configuration plus the Rust config contract. The Rust runtime never submitted entry brackets or automatic take-profit orders, so these settings only implied protection that did not exist.
- `strategy.ladder.submit_stop_loss_after_fill` remains the explicit, enforced switch for automatic protective-stop coverage. Legacy Python reference paths default the retired settings to false when absent, so this changes neither the active Rust strategy nor a reference-only run.
- Any future bracket/target implementation must use Saxo's bundled related-order request shape and prove SIM parent/child lifecycle, cancellation, replacement, reconciliation, and unprotected-position behavior before exposing a new operator control.

## [2026-07-27] operations | Shared Saxo gateway mapping

- Centralized the SIM/LIVE OpenAPI gateway selection in `src/saxo_http.rs`; OAuth client-context, Markov, portfolio, and order paths now share the same fail-closed mapping.
- Pure unit tests pin the two valid gateways and ensure unknown environments remain rejected. Request pacing, session handling, retries, ids, parsing, and broker behavior were deliberately left at their existing call sites.

## [2026-07-27] operations | Earlier Quiver advisory cycle

- Moved the weekday Quiver signal cycle from 23:10 to 19:00 Europe/Copenhagen in the Rust fallback and both shipped configurations. The scheduler remains date-idempotent and skips the cycle until the configured local time.
- Quiver's Congress-trading source is date-based advisory data, not official-close market data. The earlier run makes the latest completed signal available to evening Decision Report and Hermes context before end-of-day journaling, without altering Saxo, order creation, or execution behavior.

## [2026-07-27] operations | Daily-indicator support-risk backfill

- Investigated an all-row `No support data` Watchlist state. The newest successful daily-indicator run predated the support-risk fields and Sunday scheduling correctly skipped under the weekday-only policy, so its otherwise successful rows had null support columns.
- Ran the explicit read-only manual indicator refresh: 173 of 201 assets completed with persisted support-risk fields; 28 cached unresolved Saxo mappings remain visible as partial coverage. The operation did not create, amend, or execute an order.
- Aligned local and Kubernetes daily-indicator policy to the five-year 1,260-bar target and added a config-contract regression test that requires the two policy blocks to match.

## [2026-07-27] market-data | Watchlist daily-change source precedence

- Corrected held Watchlist rows that showed `0.0%` when Saxo's broker exposure supplied an approximated zero while the fresh price-monitor snapshot had a non-zero move from `LastClose` to the current infoprice. The monitored quote's `change_pct` now takes precedence, including a verified flat zero; broker exposure remains the fallback only when a quote snapshot is absent.
- This is display and read-model correctness only. It does not change quotes, decision logic, Hermes context, order sizing, or broker behavior.
## [2026-07-30] performance | Saxo-backed benchmark comparison

- Added the read-only Saxo-backed ETF-proxy performance benchmark capability and documented its account-value, price-return, FX, dividend, cash-flow, and SIM-data limitations in [docs/performance-benchmarks](/Users/lindau/codex/rust_daytrader/docs/performance-benchmarks.md).
- Recorded the safety boundary and verification follow-up in [roadmap](roadmap.md). No benchmark data enters decision, Hermes, sizing, or broker-mutation paths.
- Initial SIM backfill resolved `QQQ:xnas` and `EUNL:xetr` with 1,200 daily closes each. The S&P proxy was corrected from the NYSE suffix to `SPY:arcx`, and the shared Saxo resolver now recognizes NYSE Arca. The comparison remains partial until that corrected read-only backfill succeeds.
- The completed backfill exposed a PostgreSQL compatibility defect: `sqlx::AnyPool` returns the benchmark `REAL` close as `f32`, while the initial comparison reader requested only `f64`. The reader now uses the shared cross-database row adapter and can align the stored closes with portfolio timestamps without making a new market or broker call.
- The Performance range picker is now explicitly the benchmark horizon selector: `1D` presents the latest daily comparison and `1W` presents the weekly comparison. The panel labels the selected window so operators do not mistake a month-to-date number for a daily figure.
- Saxo SIM validated `DIA:arcx` as the Dow Jones Industrial Average ETF proxy and backfilled 1,200 daily closes; daily and weekly comparisons are ready across S&P 500, Nasdaq-100, Dow Jones, and MSCI World. Euronext and LSE still need separately named regional index trackers because they are exchange venues, not directly comparable index series.

## [2026-07-30] performance | Named Europe and UK benchmark proxies

- Added named broad-Europe and UK tracker candidates to the read-only comparison path. SIM accepted the FTSE 100 `ISF:xlon` and backfilled 1,200 daily closes. It rejected the initially selected STOXX Europe 600 `EXSA:xetr` as non-tradable, so the Europe candidate is corrected to the older Xetra-listed MSCI Europe `EUNK:xetr` before being declared available. These references remain outside Watchlists, Decision Reports, Hermes, Trading Manager, sizing, and broker inputs.
- Added a config-contract regression test that requires every configured reference to have a non-empty unique key, unique symbol, and a label that clearly discloses its proxy role. The first deployed refresh must resolve and backfill the corrected Europe tracker through Saxo SIM before treating it as available in the dashboard.

## [2026-07-30] performance | End-of-day benchmark readthrough

- The Rust daily strategy journal now records the aligned day-boundary comparison used by the Performance view, including its explicit native-currency price-return and DKK account-value caveat. Missing account or benchmark history remains `pending_*`; it is never turned into a zero return or inferred conclusion.
- Hermes sees this only through its existing end-of-day journal evidence and the payload carries the `read_only_end_of_day_context_only` scope. It remains excluded from Decision Report prompts, Trading Manager gates, sizing, protective stops, and broker execution.

## [2026-07-30] performance | Benchmark refresh precedes EOD journal

- Corrected the benchmark schedule from 23:55 to 22:15 Europe/Copenhagen. The daily journal is due at 22:30, so the prior ordering could have produced a formally valid comparison using only the preceding trading day's proxy close.
- The scheduler already runs the benchmark step before the journal step. A config-contract test now asserts that the configured benchmark time stays earlier than the daily journal time in local configuration; the shipped-config parity test keeps the Kubernetes policy identical. This remains a read-only data-timing fix, not a trading-policy change.

## [2026-07-30] performance | End-of-day benchmark visibility

- Added a compact End-of-Day benchmark table sourced exclusively from the persisted daily journal snapshot. It renders proxy coverage, account return, proxy return, and excess return with the same caveat Hermes receives, instead of requiring operators to inspect raw diary JSON.
- The view does not refresh Saxo data, derive a new signal, alter Hermes advice, or affect Decision Reports, Trading Manager gates, sizing, protective stops, or broker execution.
## [2026-07-30] performance | Benchmark timestamp alignment

Added read-only benchmark freshness metadata derived from the persisted account
and Saxo proxy dates. Performance and End-of-Day now label aligned, prior, and
stale closes so an older market close cannot look like an intraday comparison.
No provider request, Hermes context change, decision gate, sizing rule, or
broker mutation was added.

## [2026-07-30] performance | Weekly and monthly target progress

- Exposed the existing same-batch portfolio-value goal baselines on the Performance tab as weekly and monthly read-only target-progress cards.
- Corrected missing history semantics at the payload boundary: a period without a valid baseline is now `pending_baseline` with null P/L and progress, rather than a misleading `0 DKK` and `0%` result.
- This does not change the configured target, Hermes context, Decision Reports, Trading Manager gates, sizing, or broker execution. Daily, since-reset, and drawdown cards remain separate follow-up work because each needs an explicit baseline contract.

## [2026-07-30] performance | Since-reset and range-drawdown context

- Added a read-only since-reset card that uses the earliest persisted account-value snapshot from the currently active import batch. A missing or unusable same-batch row remains `pending_baseline`; the runtime does not reuse a pre-reset balance or invent a zero return.
- Added a selected-range maximum drawdown card calculated from valid account-value snapshots only. It is labelled as display evidence and intentionally remains distinct from the trailing drawdown guardrail that can reduce or halt BUYs.
- Neither metric enters Hermes, Decision Reports, Trading Manager selection, sizing, protective stops, or broker execution.
## [2026-07-30] performance | Account-value confidence

- Added a compact Performance confidence label that names whether the displayed account value is a live runtime aggregate, partial history, recent stored history, stale stored history, or unavailable.
- The tooltip preserves the selected range's valid-point count and aggregate source. It deliberately scopes the label to account-value evidence: quote, benchmark, and broker-order freshness retain their own existing status indicators.
- No provider calls, Hermes context, Decision Report input, Trading Manager gate, sizing rule, protective stop, or broker mutation changed.
## [2026-07-30] performance | Stored exposure P/L attribution

- Added a bounded, read-only Performance projection of the current stored Saxo exposure snapshot: largest per-symbol unrealised contributors plus instrument-currency groups.
- The source P/L is Saxo account-currency P/L converted using the stored account DKK FX basis. Instrument currency remains a display grouping, not an FX P/L decomposition.
- The projection excludes realised P/L, costs, tax, sector, strategy role, Hermes, Decision Reports, Trading Manager, protective stops, and broker execution.

## [2026-07-30] performance | Unrealised P/L sources

- Added a read-only Performance table that exposes the same dashboard-versus-Saxo exposure comparison already used by Overview integrity, even when the values remain inside the existing tolerance.
- The table keeps response-time aggregate, stored history, and stored Saxo exposure timestamps distinct. Saxo exposure P/L is shown only after conversion from the recorded broker account currency with the recorded DKK FX rate.
- It remains diagnostic context, not an accounting assertion or a trading input. Hermes, Decision Reports, Trading Manager, sizing, stops, and broker execution are unchanged.

## [2026-07-31] architecture | First Hermes read-model extraction

- Moved deterministic Hermes reflection lessons and expiring learning-memory projections from `src/state.rs` into `src/hermes_state.rs`.
- The extraction preserves bounded inputs, redaction, duplicate handling, cadence/status/TTL semantics, and existing dashboard/API output. It exposes no runtime writes.
- Database reads, advice and experiment transitions, provider calls, Trading Manager gates, protective stops, and Saxo execution remain in their existing modules and are unchanged.

## [2026-07-31] architecture | Hermes evidence read-model extraction

- Moved the deterministic one-variable baseline/overlay audit, proposal-quality rubric, duplicate-family vocabulary, and baseline-evidence calculations into `src/hermes_state.rs`.
- `AppState` still performs every database query and owns experiment/baseline lifecycle operations; the new module transforms only persisted snapshots into existing dashboard/API payloads.
- The evidence pack remains read-only and explicitly non-causal. Provider calls, Decision Reports, Trading Manager gates, protective stops, and Saxo execution are unchanged.

## [2026-08-01] architecture | Typed SSO session response

- Replaced the public `/auth/session` and `/api/auth/session` compatibility-JSON wrapper with the existing typed `SsoSession` contract.
- Focused coverage verifies the anonymous response and the only authenticated fields derived from trusted ngrok-injected headers.
- The change exposes no Saxo session state or credentials and does not alter ngrok authentication, provider calls, Hermes, Trading Manager gates, protective stops, or Saxo execution.

## [2026-08-01] architecture | Typed cash-buffer settings response

- Replaced the public cash-buffer settings compatibility-JSON wrapper with a typed model that preserves the deployed reserve, deployment ceiling, reinvestment threshold, source, null update time, and configuration baseline.
- The request endpoint remains a preview only. Its regression pins the requested reserve as distinct from the active configuration baseline.
- This does not persist a setting, activate an experiment, change capital policy, or alter provider calls, Hermes, Trading Manager gates, protective stops, or Saxo execution.

## [2026-07-31] architecture | Performance read-model extraction

- Moved pure account-value summary, selected-range return/drawdown, and confidence projections from `src/state.rs` into `src/performance_state.rs`.
- `AppState` still owns account-history/current aggregate reads, benchmark and goal-tracking queries; the extracted helpers cannot make provider calls or mutate trading state.
- Performance remains display-only evidence. Hermes, Decision Reports, Trading Manager gates, sizing, protective stops, and Saxo execution are unchanged.

## [2026-07-31] architecture | Markov read-model extraction

- Moved deterministic Markov dashboard pagination from `src/state.rs` into `src/markov_state.rs`, with the existing bounds preserved and covered in the new module.
- Moved the persisted latest-signal summary query used by read-only execution attribution into `src/markov_method.rs`, where the Markov run and signal tables are already read.
- Markov calculation, scheduler cadence, advisory context, Trading Manager gates, and Saxo execution are unchanged.

## [2026-07-31] architecture | Quiver read-model extraction

- Moved deterministic Quiver dashboard pagination from `src/state.rs` into `src/quiver_state.rs`, preserving the existing page size, bounds, and offset calculation with module coverage.
- The Quiver provider, persisted signal queries, subscription behavior, scheduler cadence, and downstream advisory/trading paths remain unchanged.
## [2026-08-01] architecture | Typed localization response

- Replaced `GET /api/localization`'s compatibility JSON with the existing typed `LocalizationPrefs` response contract.
- Header/config defaults and authenticated per-operator stored preferences retain the same locale, time zone, hour-cycle, week-start, separator, and measurement-system behavior.
- Added a serialization regression; this does not alter settings persistence, dashboard formatting, Saxo sessions, provider access, Hermes, Trading Manager gates, protective stops, or Saxo execution.
## [2026-08-01] architecture | Typed Saxo authentication status

- Replaced `GET /api/saxo/auth/status`'s compatibility JSON with the typed, sanitized `SaxoAuthStatus` response contract.
- It preserves the existing connection, environment, expiry, re-authentication, status, session-path, and optional-error fields while OAuth tokens, client keys, and account keys are excluded by construction.
- The dashboard and extended Session API retain their compatibility JSON adapters. Saxo OAuth, refresh leases, durable-session persistence, and every decision or execution behavior are unchanged.
## 2026-08-01 - Typed AI Prompts API Envelope

- Replaced the public `/api/prompts` compatibility-JSON envelope with typed `AiPromptsPayload` and `AiPromptItem` contracts.
- Kept provider-shaped latest Decision Report/Trading Manager content as optional compatibility JSON inside the bounded operator-facing envelope.
- Added a serialization regression; no prompt-building, provider, Hermes, manager, stop, or Saxo-execution behavior changed.
## 2026-08-01 - Typed Latest Decision Report API Envelope

- Replaced the public `/api/decision/latest` compatibility-JSON envelope with typed `DecisionLatestPayload`.
- Kept persisted Decision Report content dynamic inside the bounded polling envelope.
- Added a serialization regression; report generation, providers, Hermes, manager gates, stops, and Saxo execution are unchanged.
## 2026-08-01 - Typed Decision Report List API Envelope

- Replaced the public `/api/decision/reports` compatibility-JSON envelope with typed `DecisionReportListPayload`.
- Kept persisted report rows dynamic inside the bounded list envelope while report-pipeline porting remains staged.
- Added a serialization regression; report generation, providers, Hermes, manager gates, stops, and Saxo execution are unchanged.

## 2026-08-01 - Typed Markov Signals API Envelope

- Replaced the public `/api/markov/signals` compatibility-JSON envelope with typed `MarkovSignalsPayload`.
- Kept the latest persisted run summary and individual signal rows as compatibility JSON inside the explicit read-only envelope.
- Added a serialization regression; Markov calculation, scheduler timing, Decision Reports, Hermes, manager gates, stops, and Saxo execution are unchanged.

## 2026-08-01 - Typed Quiver Signals API Envelope

- Replaced the public `/api/quiver/signals` compatibility-JSON envelope with typed `QuiverSignalsPayload`.
- Kept the latest persisted run summary and individual signal rows as compatibility JSON inside the explicit read-only envelope.
- Added a serialization regression; Quiver collection, scheduler timing, Decision Reports, Hermes, manager gates, stops, and Saxo execution are unchanged.

## 2026-08-01 - Typed Strategy Journal API Envelope

- Replaced the public `/api/strategy-journal` compatibility-JSON envelope with typed `StrategyJournalPayload`.
- Kept individual persisted strategy-journal rows as compatibility JSON inside the explicit read-only list envelope.
- Added a serialization regression; journal collection, Hermes reflections and proposals, Decision Reports, manager gates, stops, and Saxo execution are unchanged.

## 2026-08-01 - Typed Execution API Envelope

- Replaced the public `/api/execution` compatibility-JSON envelope with typed `ExecutionPayload`.
- Kept persisted execution order, fill, and event rows as compatibility JSON inside the explicit read-only envelope, including its existing per-list degraded-read behavior.
- Added a serialization regression; broker synchronization, queue processing, Trading Manager, Hermes, protective stops, and Saxo execution are unchanged.

## 2026-08-10 - Saxo execution environment and reconciliation integrity

- Recorded and fixed the running-system findings: execution admission now verifies configured and cached Saxo LIVE environments; local/broker holding reconciliation uses ISIN-first and case-normalized identity; stored Saxo exposure P/L uses instrument-currency FX rates.
- Added focused regressions for the SIM rejection, symbol-casing identity, and corrected attribution. No credentials, broker payloads, account identifiers, or session data were added to the wiki.

## 2026-08-17 - Correct Saxo SIM execution admission

- Corrected the prior environment gate: SIM is Saxo's simulated broker venue and is allowed when both `saxo.environment` and the durable session report SIM. LIVE remains allowed only when both sides report LIVE.
- Unknown, missing, or mismatched environments fail closed; the overview distinguishes `simulated_broker`, `live_broker`, and disabled mismatch states so configured intent cannot be confused with broker venue.
- Added focused queue-gate and status regressions. No credentials, broker payloads, account identifiers, or session data were added to the wiki.

## [2026-08-19] planning | Shadow mid-session Decision Reports and tuning evidence

- Recorded the operator-selected weekday shadow schedules: 14:15 Europe/Copenhagen for the configured European scope and 14:15 America/New_York for XNAS/XNYS, gated by Saxo exchange calendars/current regular-session state. The US pulse normally displays as 20:15 Copenhagen and automatically shifts to 19:15 during the short US/EU DST mismatch. The existing execution-eligible open +75-minute reports remain unchanged.
- Defined shadow mode as server-owned and permanently queue-ineligible: it may persist a normalized report, pure deterministic evaluation, record-only Hermes advice, and priced outcomes, but cannot insert `execution_orders` or reach Saxo order precheck, placement, replacement, cancellation, or approval.
- Sequenced authority/measurement prerequisites before cadence work: eliminate pending-experiment influence on Hermes, expire or revalidate discretionary queued orders, reserve pending BUY exposure across cycles, and make candidate reference pricing reliable.
- Added a phased tuning-dashboard plan covering portfolio outcomes, pulse novelty, candidate funnels, signal calibration, Hermes policy provenance/counterfactuals, execution latency/cost/slippage, risk, and one-variable experiment maturity. Every metric must identify environment, sample/window, maturity, gross/net, executed/shadow, and timestamp.
- Required at least 20 eligible days and 20 mature five-session candidate observations per pulse before a separate operator promotion decision. No runtime configuration, provider schedule, Hermes mode, experiment state, Trading Manager gate, or Saxo behavior changed in this documentation pass.
- Added Nasdaq 23/5 compatibility: the SEC-approved venue structure currently targets 6 December 2026 subject to SIP/activation readiness, but the 14:15 America/New_York shadow pulse remains regular-session-only. The implementation must persist explicit US session identity and independently verify Saxo client, instrument, and order-session eligibility before any future extended-hours experiment.

## [2026-08-19] implementation | Cross-cycle active BUY reservations

- Landed the first revised-roadmap Phase 0 prerequisite in `src/trading_manager.rs`: active current-mode/current-adapter BUY orders now reserve cash, deployment capacity, symbol exposure, holding slots, exchange/currency concentration, and position-weight headroom before each new manager cycle.
- Partial fills reserve only their unfilled fraction using recorded `execution_fills`; ambiguous submission/cancellation states stay reserved until terminal reconciliation. A reservation with missing DKK valuation blocks new BUYs rather than being treated as zero exposure. Terminal or different-environment orders are excluded.
- Added hermetic SQLite regressions for pending, broker-working, partial, terminal, cross-environment, and unvalued reservations. No Saxo request, order placement, cancellation, approval, or Hermes behavior was added or changed.

## [2026-08-19] implementation | Stable Decision Pulse provenance

- Completed Phase 1's third shadow-pulse contract prerequisite: scheduled pulse keys now use the explicitly configured local trading date, while every report retains local and UTC due times, schedule time zone, deterministic sorted market scope, and Saxo exchange-calendar provenance.
- Scheduler-cycle history now records an explicit terminal scheduling result for each configured calendar pulse (`not_due`, `due`, `missed_due_window`, or `invalid_schedule`). These are read-only operational evidence, never Decision Reports, queue entries, or Saxo actions.
- Added deterministic DST/provenance and scheduler-result regressions. The existing two execution-eligible open-follow-up pulses are unchanged; the new EU/US shadow schedules remain disabled and unconfigured.

## [2026-08-19] implementation | Decision Pulse calendar and restart safety

- Completed Phase 1's fourth prerequisite with hermetic calendar regressions: empty holiday calendars and sessions that close before the requested offset never create a due pulse, while the US pulse remains anchored to America/New_York during the Europe/US DST mismatch.
- Added a SQLite persistence regression proving that a terminal report consumes its date-local pulse key across a scheduler restart, so a retry cannot duplicate the provider request. Existing manager/API tests retain the explicit shadow no-queue/no-Saxo boundary.
- This is scheduler safety coverage only: no shadow schedule was enabled, no Decision Report was generated, and no Hermes, queue, or Saxo behavior changed.

## [2026-08-19] implementation | Regular-session-only US Decision Pulses

- Completed Phase 1's explicit US-session prerequisite. XNAS/XNYS pulse targets are now classified in America/New_York as regular, pre-market, post-market, Night Session, pause, or closed; only regular targets can be scheduled.
- A calendar-provided continuous Night Session is rejected rather than changing the existing open-follow-up behavior. Pulse provenance persists the regular-session target and states that extended-hours execution is not assessed.
- Tests cover all US session boundaries, weekend closure, and the requirement that future extended-hours capability needs both Saxo-client and instrument evidence. No extended-hours order path, queue authority, or Saxo mutation was enabled.

## [2026-08-19] implementation | Scheduled EU and US shadow Decision Reports

- Enabled `europe_mid_session_shadow` at 14:15 Europe/Copenhagen and `us_mid_session_shadow` at 14:15 America/New_York. The latter derives its UTC/Copenhagen time from the time-zone database, including the short EU/US DST mismatch.
- Each fixed-time pulse is server-owned `shadow` with `queue_eligible=false`. It reaches the provider only when at least one configured exchange is currently regular and tradable; a closed scope records `market_closed` scheduler evidence instead.
- Added deterministic EU/US anchoring and closed-scope regressions. No scheduled shadow report can enter the Trading Manager or Saxo order path.

## [2026-08-19] safety | Enrich shadow reports with bounded operational context

- Completed Phase 2 context prerequisite 3. Shadow prompts now include a read-only, allowlisted summary of non-terminal local execution orders and persisted protective-stop coverage alongside the existing portfolio, cash, scoped positions, approved strategy baseline, and signal context.
- Each EU/US shadow pulse also receives only its same-local-date opening report, projected to bounded normalized market/capital/candidate fields. Historical report text is explicitly untrusted analytical data, never an instruction source; raw provider responses and original prompts remain excluded.
- This is prompt-context work only. It performs no Saxo read or mutation, cannot alter pulse mode or queue eligibility, and adds no Trading Manager or Hermes authority.

## [2026-08-19] safety | Normalize shadow-report material-change outcomes

- Completed Phase 2 prerequisite 4. Mid-session shadow reports with an available same-date opening report must now report either concrete material changes or `no_new_information`; missing evidence is recorded as `not_available`, and non-midpoint reports as `not_applicable`.
- The Rust completion boundary independently normalizes the outcome. `no_new_information` and malformed comparison evidence clear selected assets, sentiment, suggested trades, and strategy-plan candidates, so a report cannot invent duplicate candidates while claiming nothing changed.
- The normalized assessment is observation-only metadata. It leaves report persistence and scheduler completion intact, retains the server-owned shadow queue block, and performs no Saxo request or mutation.

## [2026-08-19] operations | Monitor shadow Decision Report schedules

- Completed Phase 2 prerequisite 5. The Decisions view and operations health strip now expose separate Nordic/EU and US 14:15 shadow-pulse rows, in addition to the existing opening and manual cadence rows.
- An eligible shadow pulse that passes its due window without a report now creates a once-per-pulse/local-date medium operational alert. Existing reports consume the condition; closed/non-eligible scopes do not alert.
- The alert is explicitly observational: it never retries a provider request, invokes Hermes, reaches the Trading Manager, inserts an execution order, or calls Saxo.

## [2026-08-19] implementation | Capture shadow Decision Report outcome baselines

- Landed Phase 3's initial outcome-ledger slice. Each valid BUY/SELL from a persisted server-owned shadow report receives an idempotent observation row with pulse/report provenance, candidate rank, same-date opening-pulse presence, proposed quantity/currency, compact technical/Markov/cash context, and any report-supplied price labelled as context only.
- New observations enter the existing read-only Saxo info-price monitor as `awaiting_reference`; the first returned info price establishes the local reference. The baseline path has no Trading Manager gate, Hermes request, queue insertion, Saxo precheck, or Saxo order mutation.
- Gate/Hermes/policy provenance, FX, Quiver, Support Risk, after-cost, and excursions remain explicitly uncollected in the capture slice rather than being inferred from local-currency quote movement.

## [2026-08-19] implementation | Mature shadow outcomes from daily closes

- Landed the next Phase 3 slice: after each daily-indicator run, referenced shadow candidates derive and persist 1-, 5-, and 20-session directional observations from later distinct stored trading-session closes. Maturity remains explicit as `collecting`, `preliminary`, or `mature`.
- The comparison is case-normalized by symbol and excludes the reference day, weekends, holidays, and missing-session coverage. BUY and SELL observations invert direction appropriately, but neither is reported as a broker fill, realised P/L, execution simulation, or causal result.
- The maturation pass is local database evidence only: no Saxo quote/request, provider request, Hermes request, manager gate, queue insertion, precheck, or order mutation is reachable.

## [2026-08-19] trading-quality | Capture shadow FX and estimated-cost provenance

- New shadow references now store their fresh local FX-cache source, observed/expiry timestamps, rate-to-DKK, DKK reference notional, native exchange-minimum commission, and configured per-side slippage. A missing/expired cache stays explicitly unavailable; the static-FX fallback is prohibited for persisted shadow valuation.
- Later 1/5/20-session directional observations derive a fixed-reference estimated after-cost result only when that baseline is available. The estimate is labelled separately from the original local-price observation and excludes actual fills/fees, tax, post-reference FX movement, and position changes, so it cannot be read as realised P/L or an execution simulation.
- All work remains inside the local shadow ledger after the ordinary read-only quote refresh: no broker precheck/order mutation, queue insertion, manager gate, Hermes request, or provider request is added.

## [2026-08-19] trading-quality | Preserve shadow decision-time signal context

- New shadow candidates now receive an allowlisted projection of their exact persisted report-time prompt: symbol-matched daily technical and Support-Risk data, Markov and Quiver signal context, cash plan, market-scope concentration counts, and active approved-baseline identity.
- The projection never re-reads current data, copies the raw prompt, or turns prompt context into a manager gate. Its concentration counts are explicitly scoped decision context rather than the full portfolio gate, and an execution-entry thesis stays `not_available_pre_trade` rather than being inferred.
- This is local database provenance only. It adds no provider, Hermes, Saxo, queue, precheck, or order path.

## [2026-08-19] trading-quality | Observe shadow intraday excursions

- The price monitor now retains a time-bounded per-candidate trail from its existing read-only Saxo infoprice refresh while a shadow outcome is collecting. On each sample it recomputes the BUY/SELL-direction maximum favourable and adverse observed movement from the durable reference quote.
- The record explicitly reports sample count and first/last observation time and calls its coverage sampled rather than continuous. It never claims the venue's high/low, a fill path, broker execution, realised P/L, or execution quality; collection stops once the 20-session result matures.
- No Saxo call, provider request, Hermes request, gate, queue, precheck, or order mutation was added: the feature only persists and reads the quote already returned by the established monitor.

## [2026-08-20] trading-quality | Project shadow decision-time signal gates

- Shadow outcome rows now preserve a compact technical/Markov signal-gate projection from the server-generated prompt snapshots plus the server-owned policy values supplied at report creation. The projection records a stable technical, Markov, or explicit not-evaluated code alongside its limited result.
- The projection is intentionally narrower than the Trading Manager. It names every omitted current-state gate, including market, cash, risk, holdings, cost, sellability, concentration, selection, and Hermes; a signal clear is not queue approval or execution authority.
- The implementation reads only the durable report/prompt data while creating the outcome row. It adds no Saxo request, provider request, Hermes request, manager run, queue row, precheck, or broker mutation.

## [2026-08-20] trading-quality | Audit shadow Hermes advice without authority

- New shadow reports with candidates now use a distinct server-owned Hermes session and a compact read-only report/candidate projection. The per-candidate ledger retains only a record-only action/effect match, self-check completion, and approved strategy-policy provenance; raw prompts, rationale, broker data, sessions, and secrets stay outside this view.
- Shadow Hermes advice is hard-coded record-only and never flows to a Trading Manager gate, execution queue, Saxo precheck, or broker mutation. Disabled, unavailable, failed, and timed-out advice is persisted as missing evidence rather than silently treated as no-op advice.

## [2026-08-20] observability | Expose execution-pulse lifecycle coverage

- Added a typed Tuning read model for bounded, persisted local execution-order status coverage for the EU/US execution-eligible pulses. Queued, broker-active, broker-state-unknown, terminal, and unclassified counts remain distinct.
- The view reuses existing local execution evidence only. It neither polls Saxo nor replays the manager, and it makes no latency, fill-quality, or current broker-state claim.

## [2026-08-20] observability | Expose current protective-stop coverage in Tuning

- Added a typed Tuning projection of the existing local protective-stop audit: position states, confirmed quantity coverage, and exceptions remain separately labelled from 30-day decision-pulse evidence.
- The snapshot only counts broker-confirmed stop evidence. It neither polls Saxo nor makes a stop-placement, cancellation, queue, manager, or Hermes call.

## [2026-08-21] observability | Expose execution-pulse candidate funnel in Tuning

- Added a typed, bounded report-to-manager funnel for EU/US execution-eligible pulses: report candidates, eligible candidates, Hermes matches, deterministic outcomes, local execution rows, and missing manager snapshots remain distinct.
- The final count records locally persisted execution rows only. The view neither replays decisioning nor makes a provider, Hermes, queue, precheck, or Saxo call.

## [2026-08-21] observability | Expose shadow Markov context in Tuning

- Added a typed, bounded shadow-candidate summary of persisted decision-time Markov snapshot coverage, direction buckets, missing/legacy records, and complete-signal average.
- The view never reruns Markov or consults current data. It remains observational only and cannot affect a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-21] observability | Expose shadow Quiver context in Tuning

- Added a typed, bounded shadow-candidate summary of persisted decision-time Quiver snapshot coverage, source freshness, direction buckets, missing/legacy records, and complete signal/confidence averages.
- Source freshness remains distinct from a symbol-matched candidate snapshot. The view never refreshes Quiver or consults current data, and cannot affect a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-21] observability | Expose recorded BUY thesis outcomes in Tuning

- Added a typed Tuning projection of the existing newest-recorded BUY-thesis outcome aggregate, preserving reconciled-fill counts, 1/5-session directional observations, scan limit, and maturity threshold.
- Its newest-thesis scope, gross-only return label, and exclusion of realised P/L remain explicit and separate from the tab's 30-day pulse window. It uses only local execution/fill/daily-close evidence and cannot affect Hermes, a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-21] observability | Expose experiment governance in Tuning

- Added a typed retained-lifecycle inventory for strategy experiments: pending review, approved/active paper and SIM, ready, promoted, terminal, and legacy-unknown counts remain separate.
- The inventory contains no proposal values, rationale, raw Hermes material, or performance claim. It cannot activate an experiment or affect Hermes, a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-21] observability | Expose one-month benchmark comparison in Tuning

- Added a typed Tuning projection of the existing one-month local account-value comparison against stored native-currency ETF proxy price returns, retaining per-reference status, alignment, freshness, proxy return, and portfolio excess.
- The view keeps cash inclusion and every comparison limit explicit: it is not time-weighted or total return and does not normalize FX, dividends, fees, tax, or external cash flows. It reads local history and stored closes only, without refreshing a benchmark, calling Saxo, or affecting Hermes, a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-28] architecture | Type Watchlist universe and category envelopes

- Changed the Watchlists API and dashboard boundary to use typed universe provenance/counts and typed category identity, labels, caps, and totals.
- Individual quote, decision, support-risk, and trend rows remain staged compatibility JSON while their display model evolves; unrecognized category and universe fields cannot cross the typed outer boundary.
- This changes no quote collection, candidate membership, Decision Report context, Hermes, manager gate, queue, precheck, or Saxo execution behavior.

## [2026-08-28] architecture | Type and narrow Market Status active pulses

- Changed Market Status active-pulse rows to a typed, allowlisted scheduling projection: identity, label, timing window, due state, market scope, and exchange scope.
- Manager-only decision-pulse linkage and retained detail remain internal. Missing or evolving fields degrade to safe empty/default display values rather than exposing the raw manager document.
- This changes no scheduler cadence, report generation, Hermes, manager gate, queue, precheck, or Saxo execution behavior.

## [2026-08-28] architecture | Reuse typed scheduler status in Market Status

- Changed Market Status to reuse the existing typed scheduler timing and lifecycle contract rather than forwarding the retained scheduler row.
- Detailed last-cycle/provider documents and the scheduler process PID remain internal; the panel retains its start and last-cycle timestamps.
- This changes no scheduler cadence or jobs, Decision Report, Hermes, manager gate, queue, precheck, or Saxo execution behavior.

## [2026-08-28] architecture | Type and narrow public price-monitor status

- Changed the Market Status API and dashboard boundary to use typed retained quote-monitor lifecycle data: status, timestamp, pass counters, known-closed symbols, reason, and a bounded error count.
- Free-form per-symbol quote errors plus Saxo calendar and FX refresh documents remain internal. The monitor continues to collect and persist the same data; the new count preserves partial-run observability without exposing provider details.
- This changes no Saxo session, quote-refresh, Decision Report, Hermes, manager gate, queue, precheck, or order behavior.

## [2026-08-28] architecture | Type and narrow the public Market Status summary

- Changed the Market Status API and dashboard boundary to use a typed, allowlisted summary for analysis windows, active market sets, scheduler pulse labels, and quote-monitor lifecycle metadata.
- The read-only Saxo exchange-calendar refresh now exposes only lifecycle status, source, checked time, and exchange count; free-form refresh errors remain internal. Active-pulse, scheduler, and price-monitor documents remain staged compatibility JSON.
- This changes no calendar refresh/session mechanics, Decision Report, Hermes, manager gate, queue, precheck, or Saxo execution behavior.

## [2026-08-21] observability | Expose one-month account-value outcome in Tuning

- Added a typed local account-value outcome projection for the same one-month history used by the benchmark comparison: snapshot confidence/freshness, latest DKK value including cash, simple change/return, maximum drawdown, and cost-basis warning count remain explicit.
- This is account-value snapshot movement only, not realised P/L, time-weighted return, total return, or a normalized cash-flow/FX/dividend/fee/tax attribution. It reuses local evidence without refreshing a benchmark, calling Saxo, or affecting Hermes, a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-21] observability | Expose benchmark collector provenance in Tuning

- Added typed benchmark-source audit fields beside the one-month proxy comparison: last run status, created-at/run-date, configured reference count, and successful/error refresh counts.
- Collector provenance remains distinct from portfolio/proxy close alignment and is local stored evidence only. The panel neither refreshes a benchmark nor affects Hermes, a manager gate, queue, broker precheck, execution simulation, or Saxo action.

## [2026-08-21] observability | Expose calendar-month goal context in Tuning

- Added a typed projection of the existing configured monthly DKK planning target against the active import batch's calendar-month account-value baseline, preserving pending-baseline status, target, value change, target ratio, and period start.
- This is local planning context rather than realised P/L, time-weighted/total return, or cash-flow/FX/dividend/fee/tax-normalized attribution. It does not create a risk, sizing, manager, queue, broker-precheck, Hermes, or Saxo-execution input.

## [2026-08-23] trading-quality | Complete the shadow-outcome path for synchronous providers

- OpenRouter's synchronous completion path now uses the same record-only shadow-outcome finalizer as xAI's deferred completion path. Every newly completed eligible shadow report therefore persists its candidate context, captures its immediate read-only Saxo reference quote, and records the constrained Hermes observation without acquiring manager, queue, precheck, or order authority.
- The scheduler also performs a bounded, idempotent repair pass for older completed shadow reports that were missing a ledger row. A historical report is never assigned a later quote as if it were its report-time baseline: repaired candidates are explicitly marked `reference_not_captured_retroactively`, excluded from outcome maturation, and shown separately from captured references in Tuning. This preserves auditability instead of manufacturing forward-return evidence.
## [2026-08-26] architecture | Type dashboard trade-thesis outcome evidence

- Changed the Execution-tab Trade Thesis Outcome Evidence card to consume typed aggregate status, recorded/fill coverage, and one/five-session directional-return summaries.
- Raw thesis, fill, close, scan, and safety-marker data remain outside SSR; malformed aggregate evidence degrades to the existing unavailable state.
- The aggregation remains observational only, excludes blocked candidates, FX, commission, tax, slippage, later position changes, broker adjustments, and causal claims, and cannot mutate Hermes, configuration, manager gates, or Saxo orders.
## [2026-08-26] architecture | Type dashboard holding-thesis reviews

- Changed the Execution-tab Holding Thesis Reviews queue to consume typed review status, held/review coverage, staleness policy, and bounded display rows.
- Raw position snapshots, execution/fill records, thesis documents, and safety metadata remain outside SSR; malformed review evidence degrades to the existing unavailable state.
- The queue remains a read-only fresh-decision comparison prompt, not an exit signal, sizing instruction, manager gate, or broker action.
## [2026-08-26] architecture | Type dashboard decision-pulse outcome evidence

- Changed the Execution-tab Decision Pulse Outcome Evidence panel to consume typed pulse labels, outcome counts, directional-return summaries, reconciled SELL totals, and normalized lifecycle/Hermes-effect counts.
- Raw manager documents, broker payloads, fills, ledger rows, scan metadata, and safety markers remain outside SSR; malformed aggregate evidence degrades to the existing unavailable state.
- BUY forward movement and reconciled SELL accounting remain distinct observational fields; Hermes effects classify recorded advice application only and cannot change Hermes, configuration, gates, queues, prechecks, or Saxo orders.
## [2026-08-26] architecture | Type dashboard end-of-day journal envelope

- Changed the End-of-Day dashboard journal list to consume typed stable metadata for retained local entries.
- Detailed metrics, learnings, and diary documents remain staged compatibility JSON for the existing EOD detail view and read-only benchmark context; malformed outer rows degrade to an empty local journal list.
- This is a local read-model boundary only and does not invoke Hermes, change scheduler cadence, alter strategy configuration, or mutate a Saxo order.
## 2026-08-26 — Typed selected Decision Report dashboard envelope

- Replaced the selected Decision Report's generic dashboard JSON envelope with a typed read-only Rust payload for identity, lifecycle, pulse, and queue-authority fields.
- Kept the normalized report, provider diagnostic documents, and candidate scoring waterfall as explicit compatibility JSON for the existing Decisions detail view; malformed data degrades locally and cannot trigger report generation, Hermes, queueing, or Saxo execution.
## 2026-08-26 — Typed Decision Report completion polling

- Changed `/api/decision/latest` to use the compact Decision Report summary query and a typed lifecycle-only payload (id, creation time, status).
- Full prompt, request, response, and normalized report documents remain on the existing detailed-report and sanitized debug paths; a malformed summary is treated as unavailable with no report, queue, Hermes, or broker side effect.
## 2026-08-26 — Typed Decision Report list API

- Changed `/api/decision/reports` to use the compact summary query and typed `DashboardDecisionReportSummaryPayload` rows while retaining the `{ items }` response envelope.
- Full prompt, request, response, normalized report, and error documents no longer cross that list endpoint; malformed rows fail closed at the read-only API boundary with no report, Hermes, queue, or broker side effect.
## 2026-08-26 — Typed portfolio positions API

- Changed `/api/portfolio/positions` to reuse the dashboard's stable typed position projection while retaining the count/list envelope.
- Broker/provider payloads and unbounded advisory detail remain outside this API boundary; malformed data fails closed with no quote refresh, Decision Report, Hermes, queue, or broker side effect.
## 2026-08-26 — Typed portfolio trades API

- Replaced `/api/portfolio/trades`' `SELECT *` response with an explicit stable trade-ledger allowlist and typed display rows.
- Notes plus retained before/after portfolio, decision-context, and broker/provider documents remain outside the endpoint; malformed rows fail closed with no fill reconciliation, accounting mutation, Hermes, queue, or Saxo side effect.

## [2026-08-28] architecture | Type Cash Deployment blocked-BUY gate rows

- Changed Cash Deployment's ranked blocked-BUY gate display to consume typed, allowlisted gate codes and aggregate counts.
- Candidate, broker, and rule-evaluation documents remain staged in the Trading Manager diagnostics; malformed or blank rows drop individually without hiding valid diagnostic rows.
- This remains read-only historical manager evidence and cannot change cash policy, report generation, Hermes, manager gates, queues, broker prechecks, or Saxo execution.

## [2026-08-28] architecture | Type Cash Deployment monthly-loss breaker evidence

- Changed Cash Deployment's monthly-loss circuit-breaker display to consume a typed, allowlisted restriction, threshold, soft-reduction, and local override projection.
- Rule-trace and broker documents remain staged in the Trading Manager diagnostics; the projection has no ability to alter circuit-breaker policy or an operator override.
- This remains read-only completed-run evidence and cannot change report generation, Hermes, manager gates, queues, broker prechecks, or Saxo execution.

## [2026-08-28] architecture | Type Cash Deployment reinvestment and budget evidence

- Changed Cash Deployment's reinvestment diagnostic and capital-budget display to consume typed, allowlisted status, explanation, aggregate candidate counts, and available-budget values.
- Candidate and policy documents remain staged in the Trading Manager diagnostic record; ranked blocked-BUY reasons keep their separately resilient typed projection.
- This remains read-only completed-run evidence and cannot change capital policy, report generation, Hermes, manager gates, queues, broker prechecks, or Saxo execution.

## [2026-08-28] architecture | Type Instrument Quarantine summary metadata

- Changed the Overview Instrument Quarantine summary to consume typed, allowlisted enabled state, aggregate counts, retention window, and failure threshold.
- Active rows retain their separately resilient typed projection, so malformed rows cannot hide valid blocks; rule-trace documents remain staged in the manager diagnostic record.
- This remains read-only completed-run evidence and cannot change quarantine policy, an operator override, report generation, Hermes, manager gates, queues, broker prechecks, or Saxo execution.

## [2026-08-28] architecture | Type Overview Integrity finding rows

- Changed the Overview Integrity panel to consume typed, allowlisted visible finding, acknowledgement, and expiry-pending DayOrder lifecycle fields.
- Broker, ledger, and configuration detail remains internal; the panel retains only what is required to surface an issue and preserve the existing acknowledgement action.
- This remains read-only operational evidence and cannot change integrity checks, acknowledgements, scheduler work, reconciliation, Hermes, manager gates, queues, broker prechecks, or Saxo execution.

## [2026-08-28] architecture | Type Protective Stop Coverage summary

- Changed the Execution Protective Stop Coverage panel to consume typed, allowlisted aggregate position, coverage, quantity, and exception counts.
- Per-position, exception, and SIM-test lifecycle evidence remains staged because it depends on broker and lifecycle state; broker documents cannot cross the typed summary boundary.
- This remains read-only coverage observation and cannot change stop computation, SIM precheck, placement, cancellation, reconciliation, Hermes, manager gates, queues, or Saxo execution.

## [2026-08-28] architecture | Type Protective Stop Coverage position rows

- Changed the Execution Protective Stop Coverage table to consume typed, allowlisted stored position, coverage, stop-price, status, currency, and timestamp evidence.
- Detailed broker/order evidence and proposed-stop documents remain staged for the separate manual SIM exception workflow; those documents cannot cross the typed table row boundary.
- This remains read-only coverage observation and cannot change stop computation, SIM precheck, placement, cancellation, reconciliation, Hermes, manager gates, queues, or Saxo execution.

## [2026-08-28] architecture | Type Cash Deployment drawdown-guardrail evidence

- Changed Cash Deployment's portfolio-drawdown guardrail display to consume a typed, allowlisted restriction, peak, threshold, soft-reduction, and local override projection.
- Rule-trace and broker documents remain staged in the Trading Manager diagnostics; the projection has no ability to alter the guardrail or an operator override.
- This remains read-only completed-run evidence and cannot change report generation, Hermes, manager gates, queues, broker prechecks, or Saxo execution.
## [2026-08-29] architecture | Type Candidate Scoring Waterfall

- Changed the selected Decision Report's deterministic Candidate Scoring Waterfall to consume typed run identity, lifecycle, aggregate outcome-count, and per-candidate display evidence.
- Raw manager documents and unallowlisted candidate detail are dropped at the typed boundary; the existing market, technical, Markov, Hermes, cost, holding, concentration, and position-weight explanations remain available from compiler-checked fields.
- This remains read-only historical evidence and cannot generate a report, alter manager gates or configuration, invoke Hermes, queue work, precheck, or mutate a Saxo order.

## [2026-08-29] architecture | Extract Decision Report schema construction

- Moved the canonical Decision Report JSON Schema builders into the pure `decision_schema` module and added a direct contract test for the required suggested-trade metadata.
- OpenRouter strict-output enforcement and the public schema-health check remain at the existing provider boundary; the report request continues to use the unchanged canonical schema.
- This changes no report schedule, model-provider behavior, Trading Manager gate or configuration, Hermes role, queue, precheck, or Saxo execution behavior.

## [2026-08-29] architecture | Extract Decision Report provider HTTP boundary

- Moved Decision Report endpoint construction, timeout-bound chat and deferred-completion HTTP calls, provider key naming, and the shared HTTP response envelope into `decision_provider`.
- Scheduling, request construction, strict response normalization, persistence, and the public schema-health API remain at their existing boundaries in `xai_decision`.
- This changes no report schedule, model-provider behavior, Trading Manager gate or configuration, Hermes role, queue, precheck, or Saxo execution behavior.

## [2026-08-29] architecture | Extract Decision Report provider request assembly

- Moved provider-specific Decision Report chat request assembly into `decision_provider`, including OpenRouter plugin selection and response-format and reasoning-effort placement.
- At this stage, report prompting, schema construction and validation, strict completion normalization, scheduling, persistence, and the public schema-health API remained at their existing boundaries in `xai_decision`; the subsequent provider-boundary entry records the strict-schema move.
- This changes no report schedule, model-provider behavior, Trading Manager gate or configuration, Hermes role, queue, precheck, or Saxo execution behavior.

## [2026-08-29] architecture | Extract OpenRouter strict schema provider boundary

- Moved OpenRouter strict-schema shaping, recursive validation, response-format construction, and schema-validation issue data into `decision_provider` alongside the existing provider transport and request assembly.
- The public schema-health API remains a thin mapping in `xai_decision`; report prompting, canonical schema construction, completion normalization, scheduling, persistence, and all execution boundaries remain unchanged.
- This changes no report schedule, model-provider behavior, Trading Manager gate or configuration, Hermes role, queue, precheck, or Saxo execution behavior.
