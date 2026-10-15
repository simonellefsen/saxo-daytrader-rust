---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-grading-adjudications
  path: docs/jev-adjudication-2026-09-21.md
---

# Jev grading adjudications, 2026-09-21 to 09-22

The first measurements of Jev report grading. Each is one author adjudicating
cases against a rubric frozen beforehand. Raw sources:
- [rubric](/Users/lindau/codex/rust_daytrader/docs/jev-adjudication-rubric.md):
  frozen 2026-09-21, before any case was read;
- [first run](/Users/lindau/codex/rust_daytrader/docs/jev-adjudication-2026-09-21.md);
- [held-out pass, preregistration](/Users/lindau/codex/rust_daytrader/docs/jev-adjudication-2026-09-21b-preregistration.md)
  and [result](/Users/lindau/codex/rust_daytrader/docs/jev-adjudication-2026-09-21b.md);
- [the nine numeric findings](/Users/lindau/codex/rust_daytrader/docs/jev-adjudication-2026-09-22-numeric.md).

**The adjudicator wrote the grader in every one of them**, so none is
independent. Each was revised after review, and the corrections are kept on
the page rather than edited away.

## The rubric

Four categories:
1. numerical contradiction, which the deterministic `jev_numeric` settles,
   not the model;
2. unsupported assertion;
3. ambiguous or overstated wording;
4. no checkable claim.

**Ambiguity is a result, not a failure to decide.** Portfolio capital,
holdings, profit and costs are out of scope. Review samples must come from
every predicted category, because reading only flagged cases can never
measure what the grader misses.

## First run, 2026-09-21

28 of 111 graded `v8` notes, every fourth; 26 of the 28 blind.
- **Wording:** on the 26 blind cases, 23 agree on the exact category and 24
  on flag versus no flag. The grader flagged 2 the adjudicator did not; the
  adjudicator flagged none the grader missed.
- **Numeric correctness could not be measured.** The stored grades mixed
  three numeric methods under one label. The fix: the method now carries
  `NUMERIC_METHOD_VERSION` and is recomputed from stored evidence when it
  changes.
- **A margin hypothesis came out of this sample:** flag-level disagreements
  sit at margin ≤ 0.25.
- **Retracted:** CHEMM's "trading above 525 DKK" against a close of 510 was
  first called a contradiction. The note quoted a live price of 525.0, taken
  16 seconds before the report.

## Held-out blinded pass, 2026-09-21

Preregistered (`c4e7726`), then labelled **before the grader ran**, so
blindness was structural. 30 candidates, every 24th from reports 83 to 271,
none read while building the checker.
- **The margin hypothesis failed:** one disagreement at margin 0.299, where
  it predicted none above 0.25.
- **Wording:** 26 of 29 agree. Every frozen label was `fair`, so this
  **cannot measure sensitivity**, only concurrence on notes that look sound.
- **Numeric:** 28 matched and 2 were flagged differs, and both flags were
  attribution failures:
  - "RSI is above 70" is a relational claim the checker compared for
    equality;
  - in "5-day … signal", the 5 is a horizon, not the signal.
- **Also found:** the wording taxonomy has no slot for an unsupported
  assertion.

## The nine numeric findings, 2026-09-22

Every `differs` in the history, recomputed at `n10`. It was amended the same
day after review.
- **Five confirmed contradictions,** among them AMAT's `reward_risk only 0.65`
  against 0.5423 and BAC's `RSI 58` against 67.7608.
- **Four unresolved:**
  - three "N/N" confluence denominators, all from report 101;
  - FLS's "near 593 DKK support", where support is 551.5 but the close is
    exactly 593.0.
- **Withdrawn:** a claim of 9-of-9 precision and a 0.07% "accuracy" figure.
  - The comparable flag figures are 69 in 12,716 check rows, or 9 findings
    in 1,445 distinct claims.
  - A flag rate is not an error rate.
  - 283 of the 1,445 claims (19.6%) were never compared.
- **These nine are triage and regression material,** never a held-out set.

## What came next

The two needs these named led to two instruments:
- independently labelled clean controls, built as
  [controls-v1](jev-controls-v1.md);
- measured sensitivity, built as the
  [seeded challenge set](jev-challenge-set.md).

No trading parameter changed on any of these.
