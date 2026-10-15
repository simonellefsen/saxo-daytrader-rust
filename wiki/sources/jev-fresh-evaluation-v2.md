---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-fresh-evaluation-v2
  path: docs/jev-fresh-evaluation-v2.md
---

# Jev numeric checker: fresh evaluation v2

Raw sources, unchanged by this note:
- [jev-fresh-evaluation-v2.md](/Users/lindau/codex/rust_daytrader/docs/jev-fresh-evaluation-v2.md):
  the preregistration and its write-up;
- [jev-fresh-v2-results.json](/Users/lindau/codex/rust_daytrader/docs/jev-fresh-v2-results.json):
  the single scoring run;
- [jev-fresh-v2-reconciliation.json](/Users/lindau/codex/rust_daytrader/docs/jev-fresh-v2-reconciliation.json):
  the disagreements, read beside the labels.

## What was measured

**The numeric checker, `src/jev_numeric.rs`, at method `n20-2026-09-26`.** It
is a pure function. It checks the figures a decision-report candidate note
quotes against the evidence that report was given. It runs only as an
observational sidecar in [jev_review.rs](/Users/lindau/codex/rust_daytrader/src/jev_review.rs),
after a report is stored, and cannot change a gate, a candidate, a queue or a
broker call.

**One rule shapes these results:** a figure the checker cannot confidently
attribute to an evidence field is recorded as unattributed and never
compared.

**Fresh-v2 is the second held-out evaluation.** Fresh-v1 measured `n15` on 51
notes from 11 reports
([jev-fresh-evaluation.md](/Users/lindau/codex/rust_daytrader/docs/jev-fresh-evaluation.md)).
Fresh-v2 was preregistered on 2026-09-26, before any report in its window
existed. It froze the method by source hash, fixed the window, and stated
three conventions in the rubric:
- rounding, not truncation;
- a signed figure beside "conviction" states the signed signal;
- missing evidence is `cannot_tell`.

## How it ran

All on 2026-10-15, each step committed before the next:

| step | commit | what |
|---|---|---|
| frame | `1707945` | 36 completed reports created 2026-09-26 to 2026-10-10, #364 to #400, and 143 notes. #376 excluded: it ended `xai_error` with no report. |
| labels | `fa6b75b` | a fresh-context AI reviewer, given the instrument only; 319 claims |
| seeded | `f299df0` | 2,245 cases from 270 anchors in 140 notes |
| score | `575d6a1` | once only; the scorer refuses to run again |
| reconcile | `87dbfbc` | a test recomputes every figure |
| write-up | `7dc3348` | added to the doc after "The order of work"; preregistered sections unchanged |

**The labels are one AI review, not ground truth.** Blinding rests on context
and instructions, not on access controls. The provenance records one
deviation: the harness put the project's standing instructions and a memory
index into the labeller's context. The labeller reports it did not use them.

## Results

**On natural notes:**
- **No false alarm,** on the 207 consistent claims the checker compared in
  production and on the 248 it compared across all 143 notes.
- **Every one of its 250 decisions agrees with the label's verdict.**
- **Detection on natural errors is 2 of 7.**
  - Flagged: SALM "566" against support 566.81, a truncation under the
    rounding convention, and DEMANT "310.65" against 318.075.
  - Not flagged: five **bare price levels**, such as "consolidating near 281
    EUR" and "holding above 317.00 DKK". They follow from the
    unattributed-never-compared rule, and fresh-v1 recorded the same kind of
    miss. Two of the five are one DB1Gn sentence in consecutive reports.
- **One omission:** ASML's "~12.2k". `n20` does not read a `k`-suffixed figure.
- **43 claims were compared on another field, and they are one reading.**
  Each is a positive signed figure beside "conviction". The labeller reads
  `markov.signed_signal`, `n20` reads `markov.conviction`. For a positive
  signal the two fields hold the same number, so these notes cannot tell the
  readings apart, and the verdicts agree.

**On seeded cases.** `valid_context` is the headline: realistic evidence,
1,435 cases. `corruption` is reported apart: one field moved alone, 810 cases.
- **False figures flagged on the right field:** 752 of 1,077 in
  `valid_context`.
- **No untouched neighbour was flagged** in either family.
- **Every wrong or wrong-field seeded outcome is the signed-conviction
  reading,** traced figure by figure:
  - in `valid_context`, 43 accepted sign flips and 180 flags on another field;
  - in `corruption`, 86 accepted errors and 43 false alarms.
- **On every other anchor, every seeded decision is correct.** The abstentions
  are concentrated: 118 changed `valid_context` figures from 27 anchors.

**Qualifications that travel with any summary:**
- The anchor split was chosen after scoring. It locates the disagreement and
  validates nothing.
- "Valid context" means passing `evidence_problems`, not proven validity.
- The 2,245 cases are not independent trials.
- The counts describe these 36 reports only. 7 natural errors are too few
  for a rate.

## What follows from it

- **The signed-conviction reading is the operator's call:**
  [todo T4](../todo.md). A checker changed to the convention would be `n21`,
  and this set could not validate it.
- **Two `n20` gaps are recorded, not fixed:** bare price levels and
  `k`-suffixed figures.
- **With the results recorded, the test pinning the checker's source has
  stood down.** Any fix is a new method version and needs fresh notes to
  validate it.
