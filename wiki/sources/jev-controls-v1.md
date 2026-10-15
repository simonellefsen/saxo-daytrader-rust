---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-controls
  path: docs/jev-controls.md
---

# Jev numeric checker: clean controls v1

Raw source: [jev-controls.md](/Users/lindau/codex/rust_daytrader/docs/jev-controls.md),
with the instrument, key, labels, provenance, `results-v4` and reconciliation
files beside it.

## What it is

**A blind, drawn sample of natural claims, labelled by someone other than the
checker's author.** It was built to reach what the
[adjudications](jev-grading-adjudications-2026-09-21.md) and the
[challenge set](jev-challenge-set.md) cannot: claims the checker accepted or
never compared. 283 of the 1,445 distinct claims in the history were never
compared at all.

- **Sample:** `controls-v1-2026-09-22`, 121 cases from the complete history,
  keyed at `n10-2026-09-22`.
- **The draw:** ordered by a seeded hash within each stratum. Abstention
  classes are over-sampled.
- **Hidden flags:** the nine `differs` are in it, unmarked.
- **No answers in the instrument:** a test searches it for any verdict name.

| stratum | population | drawn |
|---|---|---|
| `matches` | 1,153 | 60 |
| `unattributed` | 151 | 18 |
| `uncertain_attribution` | 48 | 12 |
| `not_in_evidence` | 56 | 8 |
| `not_a_field_value` | 22 | 8 |
| `implausible_attribution` | 6 | 6 |
| `differs` | 9 | 9 |

**The analysis plan was fixed before any label existed,** as
`controls-protocol-v4-2026-09-23`. Its population figures are exact
hypergeometric bounds per stratum, Bonferroni-combined. The coverage of at
least 95% is checked by enumeration in a test, not asserted. Every
`cannot_tell` is counted both ways. Protocol versions 2 and 3 were corrected
by review before any label arrived; v3's Wilson bounds did not cover.

## Results under v4

Labelled by a fresh-context AI reviewer, a Codex subagent per its provenance.
It was given the instrument, the rubric, and one disclosed clarification:
missing evidence is `cannot_tell`. Scored once at `9304c1d`.
- **The labels:** 77 consistent, 7 inconsistent, 37 `cannot_tell`.
- **Error proportion among accepted claims:** 1 of the 60 sampled was
  labelled wrong, a population range of **1 to 132 of 1,153** (0.09% to
  11.4%).
- **All claims labelled wrong:** 7 to 393 of 1,445 (0.5% to 27.2%).
- **Wrong and not flagged, end to end:** 1 to 384 of 1,445 (0.07% to 26.6%).
- **No point estimate.** Nothing in `not_in_evidence` was settled.
- **No false positive among settled flags.** Of the nine flags, six were
  labelled wrong and three `cannot_tell`.
- **Attribution:** all 60 accepted claims name the same field on both sides.
  The 13 differences are all in claims the checker did not compare.

**The width is the `cannot_tell` cases,** 34 of the 37 where the checker
abstained. Under the frozen protocol they count both ways.

## Reconciliation, 2026-09-24

Done by the checker's author, so not independent. **No label changes, and the
reconciled figures equal v4's.** A test pins the labels' hash and recomputes
every figure.
- **The one false negative is a convention question.** LMND's "RSI 60"
  against 60.66: the checker accepted truncation, and the labeller rounded to
  61.
- **FLS 593 is not revised.** The label calls it wrong; the 2026-09-22
  adjudication left it ambiguous. The checker's author should not raise the
  confirmed count of its own checker on one agreeing label.
- **In all 13 attribution differences, the labeller's field holds.** Six are
  coverage gaps: "R/R" (2) and a "5-day" horizon (4).
- **What the 37 `cannot_tell` are:**
  - 12 Markov fields null in the evidence;
  - 14 out of scope;
  - 4 order state;
  - 2 holding periods;
  - 5 genuinely two readings.

## Since

- **Rounding was settled on 2026-09-26.** LMND's label is right under it, and
  from `n17` the checker flags it. The v4 figures stand as scored.
- **The coverage gaps were fixed in `n12`** (2026-09-24). All six sampled
  cases now match.
- **The missing Markov evidence was traced:**
  [Markov provenance](jev-markov-provenance.md).
- **Because these controls shaped `n11` and `n12`,** from `n11` on they are
  development and regression evidence, not validation.

The measurement procedure is validated, not the checker's accuracy. It covers
sampling error only; label error and extraction omissions are outside it.
Omissions are what the [whole-note audit](jev-whole-notes-v1.md) measures.
