# Jev adoption: reviewed implementation and release gates

Reviewed 2026-09-20 against `24c11c4` and TypeSafe documentation. This supersedes
readiness claims in the external `fyi-you-are-sharing-steady-river.md` plan;
the original is retained unchanged.

## Architecture and verification

Use OpenRouter model **`~typesafe/jev-latest`** with `JEV_OPENROUTER_API_KEY`.
Jev returns typed judgments, not narrative reports. It is not a drop-in report
author replacement. A deterministically rendered report could incorporate its
typed outputs, but that is a separate pipeline design, not a model-name swap.

Keep Jev observational. `symbol_rankings` is a diagnostic view, not a candidate
pre-screen in the trading pipeline. It does not change marker screening,
candidate admission, trading gates, quantities or broker actions.

A real synthetic-only OpenRouter `/api/v1/systemone` smoke test succeeded:
the alias resolved to `typesafe/jev-1.13-20260917`; the original probe returned
467 input / 75 output tokens in 505 ms. Transport success is not financial
calibration or verification of every provider error shape.

The amended-parser probe also passed (632 ms), with provider-reported billed
cost **USD 0.000019614**, not merely a rate-card estimate. `make validate` passed:
944 tests passed, one live smoke test excluded from the hermetic suite; formatting,
shell-script checks and `cargo check --all-targets` passed with warnings denied.
The ignored smoke test was run explicitly and passed separately.

The running API and scheduler were each ready at revision `24c11c4`;
`/api/ai/jev` returned `disabled` with no signals. Review amendments are local
until released; both config defaults remain disabled.

## Review amendments

- Remove awaited Jev calls from report and editorial cycles. A separate sequential
  worker in the singleton scheduler prevents an outage from holding the Trading
  Manager behind ten grading and ten classification requests. Each batch stops
  on its first transport failure rather than fanning failure across the backlog.
- Validate answer type, option set, probability mass, confidence, rubric labels,
  score bounds and weighted-score consistency. Invalid measurements remain absent,
  never clamped into strong signals. Unicode error excerpts cannot panic, and
  config debug output redacts the API key.
- Preserve provider billing through transport and storage. Previously callers
  passed an empty response to the cost helper, always discarding billed cost.
  Missing costs remain NULL. Malformed JSON answer envelopes retain usage/cost
  when those fields are readable.
- Retain validated answer distributions, question definitions, request fingerprint
  and measurement-schema version in `result_json.measurement`. Partial answers
  have status `partial`; automatic re-spending on these subjects is suppressed.
  This is not historical replay: a hash does not replace retained source inputs.
- Replace delete/insert signal replacement with an atomic upsert. Do not persist
  an editorial signal if its accounting record could not be saved.
- Preserve readable ledger sources when another source fails; name unavailable
  sources and unpriced calls. Cost totals are known subtotals, not invoices.
  Daily rows describe only the bounded retained request sample.
- Label report grading as **self-consistency**, not independent source checking:
  its evidence is the report's own metadata.

## Revised roadmap

### A. Readiness before observational activation

1. Review and release the isolation/accounting fixes. A passed smoke test does not
   automatically authorize activation; keep defaults off until an operator
   approves a bounded observation pilot.
2. Finish the Jev operator panel: worker last-success/failure/backlog, partial
   answers, cost coverage, marker disagreement and resolved-model changes.
   The JSON endpoint does not meet the original UI exit criterion; the shared
   cost panel alone is not Phase 2 completion.

   *Status, 2026-10-16, beside the item above.* The panel is on the AI
   Prompts view, in two cards beside the Jev Model card. `/api/ai/jev`
   carries the same data under `operator`.
   - **Worker.** Each cycle now writes `jev_worker_status`: one row per step,
     plus one for the cycle. A row records the step's status, its last clean
     run and last failed run, and what its own selection found waiting and
     left. A step that judged nine and lost the tenth counts as failed, not
     clean. An enabled worker with no finished cycle in three intervals
     (30 minutes) shows as silent. Each backlog is bounded by its step's
     selection:
     - grading: the newest `jev.grading_candidate_window` reports;
     - failure classification: the newest 60 failed reports;
     - editorial items: at most 60 pairs, marked as a lower bound when full;
     - numeric recompute: 200.
   - **Calls, per purpose.** Answered, partial and failed calls; the
     questions left unanswered in partial calls; failures since the last
     answer; and the latest failure. A failure is shown as a bounded label,
     such as `HTTP 429` or `transport error`, never the provider's text.
   - **Cost coverage, per purpose.** Answered calls are counted by source:
     billed, rate-card, or no cost at all. An answer with no cost is shown as
     unknown, not free. The known cost sums only priced calls.
   - **Marker disagreement.** The marker screen and Jev's
     `instruction_shaped` are counted in both directions, beside the newest
     20 disagreements. Each shows the title Jev judged, because the source
     row is pruned. Neither screen is ground truth; adjudicating examples
     is section B's work.
   - **Resolved-model changes** were done under item 5.
   - **What it does not show:** rows are logical calls, not provider
     attempts. A call billed and then lost to a failed database write is not
     in the ledger. Both belong to item 3, which stays open, as does
     circuit-breaker state. The panel raises no alert.
3. Add durable attempt-level accounting and circuit-breaker/backoff state,
   including billed responses followed by database-write failures. Current
   failure rows are logical-call records, not separate retry attempts. Verify
   long `Retry-After` handling across worker cycles, not only within a call.

   *Status, 2026-10-16, beside the item above.*
   - **Attempts.** `jev::ask` logs every HTTP attempt: status, latency,
     Retry-After, and outcome (`answered`, `retried`, `deferred`, `failed`
     or `unparseable`). The log is stored with its call as
     `jev_requests.attempts_json`, in the same write, so a call and its
     attempts cannot disagree. A failed call now records the attempts it
     made; before, every failure row said `attempts = 0`.
   - **Circuit breaker.** One durable gate covers the provider, in
     `jev_provider_state`. A failure that reached the provider shuts it until
     whichever is later: the provider's own Retry-After, capped at a day, or
     a backoff of one cycle doubling to four hours. The next answer reopens
     it. While it is shut, no worker step calls Jev; each still reports its
     backlog. A call refused locally, such as an oversized state, never
     reached the provider and leaves the gate alone. An unreadable gate
     fails closed.
   - **Retry-After across cycles.** A wait longer than one call may sleep
     ends the call at once, as before, and is now persisted. The next cycle
     is deferred rather than retried ten minutes later. A test drives this
     end to end against a loopback provider that answers 429 with an hour's
     wait: one request, a shut gate, and the next cycle `deferred`.
   - **A billed answer whose write fails** stops its step, so nothing more is
     spent while accounting is failing. Before, grading and editorial
     scoring carried on to the next subject. The row is held by the worker
     and written at the start of a later cycle. The panel shows held rows
     with their cost, which is in no total until written.
   - **What it does not do.** The hold is in memory, so a scheduler restart
     loses what it holds; the warning logged with each held row, cost
     included, is then the only trace. It holds at most 200 rows, dropping
     and counting the oldest after that.
   - **Belongs to item 4, not fixed here.** A subject refused locally still
     stops its step's batch. The worker selects newest first, so such a
     subject would stop that step on every cycle. No refusal has been
     recorded in production. *(Fixed the same day under item 4.)*
4. Bound/redact outbound failure text. Test report grading on representative
   sanitized reports: one tiny fixture does not prove all reports fit the byte
   budget. Reject rather than silently truncate oversize input; budget state
   **plus questions** against provider token limits.

   *Status, 2026-10-16, beside the item above.*
   - **The limits, from TypeSafe.** Jev 1.13 and 1.14 allow 64k tokens for
     the state plus every question, and 32k for the state plus any one
     question. Over either, the provider answers 400 `max_tokens_exceeded`.
     The config had said "OpenRouter lists a 32k context" and budgeted only
     the state, at 24,000 characters; questions were never counted. That
     state cap stays, as a cost bound. `jev.max_request_tokens` and
     `jev.max_question_tokens` now carry the two documented limits.
   - **Budgeting state plus questions.** Before sending, `jev::ask`
     estimates the whole request and the state plus its largest question
     against both limits. The estimate assumes 1.5 bytes per token, which
     overcounts every measured case: an editorial request of 2,310 bytes was
     billed about 850 tokens, and the smallest failure classification, of
     at least 960 bytes, 527. Each call stores its request size and
     estimate beside the provider's own count. The panel shows the largest
     call per purpose and warns if any call shows the estimate undercounting.
     A logged warning does the same per call.
   - **Real reports instead of sanitized fixtures.** The provider has counted
     every report graded. Its counts for the 2,061 grading calls to
     2026-10-16:

     | | input tokens |
     |---|---|
     | median | 6,930 |
     | 95th percentile | 12,996 |
     | largest | 18,384, 29% of the per-request limit |

     None was refused or failed. The state plus one question is part of the
     whole request, so every call was also under the 32k per-question
     limit. A test adds the bound for reports not yet written: a full
     10-candidate grading request, with a state at the 24,000-character cap,
     fits both limits. So only input above that cap can be refused. The
     grading questions alone come to about 34,000 bytes at 10 candidates,
     more than the state cap, so questions, not reports, dominate.
   - **Refused, never truncated.** An oversized state or request is still
     refused before sending. The refused subject is now recorded as
     `refused` at its question version and is not selected again until the
     questions change, and its step goes on to the next subject. A refusal
     is not a call: it is outside the usage ledger, the cost figures and the
     failure counts.
   - **Outbound failure text.** Failure classification sent a decision
     report's stored error text to Jev unbounded and unredacted. It now
     masks credential-shaped words before sending: long or prefixed tokens,
     the credential after `Bearer`, `Basic` or an `Authorization:` header,
     and any `key=`, `token=`, `secret=` or `password=` value. It then sends
     at most 2,000 characters. The state gives the full length and says when
     the text was cut, so the classifier knows it saw an opening, not the
     whole: a stated bound rather than a silent one. Provider error bodies
     stored in the ledger are masked the same way.
5. Persist immutable decision-time feature/source references and collection
   timestamps. Old headlines scored today are not historical out-of-sample
   features. Define partial-answer regrading and alias-rollover policies.

   *Status, 2026-10-15, beside the item above.* Both policies are defined.
   - **Partial answers** were settled in code before this: a partial grade
     counts as judged, so its subject is not re-asked, and its numeric half is
     recomputed from the stored prompt with no provider call.
   - **Alias rollover.** Simon decided that Jev always runs the latest
     version, so `jev.model` stays the floating `~typesafe/jev-latest`. That
     day the alias moved from `typesafe/jev-1.13-20260917`, last answering
     2026-10-14T18:21:53Z, to `typesafe/jev-1.14-20261014`, first answering
     2026-10-15T08:55:12Z. The policy is `jev::ALIAS_ROLLOVER_POLICY`:
     - each resolved version is a measurement epoch;
     - a stored judgement keeps the version that made it, and none is
       regraded on a rollover, because a subject is re-asked only when its
       question set changes;
     - a figure that pools epochs says so;
     - a rollover is announced once;
     - model-backed baselines describe the version they were measured on
       until they are re-measured.
   - **Where it shows:**
     - `/api/ai/jev` carries `models`: the configured alias, the current
       version and every epoch;
     - the AI Prompts view has a Jev Model card;
     - a once-per-version operational Slack alert,
       `notifications.alerts.jev_model_rollover_enabled`, defaults on.
   - **Tests pin** the epoch fold and that a rollover selects nothing for
     regrading.
   - **Still open from item 2** on 2026-10-15: the rest of the operator
     panel (worker backlog, cost coverage, marker disagreement). It landed
     on 2026-10-16; see the note under item 2. Observation itself has run
     since 2026-09-20, with `jev.enabled: true`.

### B. Measurement before strategy influence

- `P(bullish)` currently measures the headline's implied direction, not
  `P(net return > 0 in five sessions)`. Test semantic correctness on independently
  labelled held-out items; test return association separately on predeclared
  future outcomes. A return comparison by confidence bucket is not a probability
  calibration curve for the original question.
- Measure relevance, injection-shaped content, rubric accuracy and abstention.
  Marker/Jev disagreement does not prove either correct: adjudicate examples in
  both directions. Typed output does not make prompt injection impossible.
- `restates_known` remains unverified without historical context supplied in
  state. Materiality from a headline is an uncalibrated judgment, not a forecast.
- Snapshot only information available before entry; retain candidate selection,
  avoid duplicate-story leakage, split by time/resolved model/question version,
  and report denominators and uncertainty. Preserve evaluator session-calendar
  and benchmark-freshness checks; forward movement is not realized P/L.
- Observational feature collection does **not** create a new strategy epoch.
  Version the measurement; create an epoch when actual decision behavior changes.
  Fix evaluation rules before a held-out interval and do not repeatedly tune on it.

### C. Optional rank-only experiment

Requires operator approval and evidence from B. First compare candidate ordering
in shadow against the unchanged baseline. Reordering or widening candidates can
change trades despite unchanged gates, so this is a strategy change. Jev must
never bypass gates, size orders or call Saxo. Agree activation, rollback criteria
and evaluation window before the experiment begins.

## Cost and interpretation corrections

Published input price is USD 0.042 per million tokens, output free. Annual cost
depends on traffic, question tokens, backfills, grading and retries; the original
"under USD 1/year" was not established. Sixty requests of 400 input tokens cost
about USD 0.001008 **per batch**, not 0.001 US cents. Prefer billed cost and label
rate-card estimates and unknown costs separately.

Provider calibration claims do not establish calibration on this portfolio or
these prompts. Engineering test results are not evidence of profitable trading.

## Sources

- [System One](https://docs.typesafe.ai/concepts/system-one)
- [Primitives and answer semantics](https://docs.typesafe.ai/primitives)
- [API contract](https://docs.typesafe.ai/api)
- [Models, pricing and alias drift](https://docs.typesafe.ai/models)
- [OpenRouter TypeSafe models](https://openrouter.ai/typesafe)
