---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-wording-measurement
  path: docs/jev-wording-measurement.md
---

# Jev wording grader: seeded measurement

Raw source: [jev-wording-measurement.md](/Users/lindau/codex/rust_daytrader/docs/jev-wording-measurement.md),
set `wording-v1-2026-09-22`, with each run's results file beside it.

## What it is

**The seeded measurement of the model-backed wording grader.** The
[adjudications](jev-grading-adjudications-2026-09-21.md) measured how often
it agreed with one reader. Every other Jev instrument measures the
deterministic numeric checker. The grader judges qualitative wording, such as
whether "bullish" fits a field that reads `Bear`.

Three runs over the same 85 cases and the same resolved model,
`typesafe/jev-1.13-20260917`:

| run | grading version | difference |
|---|---|---|
| A | `v15` | the instruction as it stood |
| B | `v16` | the categorical fields named one by one |
| C | `v16` again | unchanged, for variance |

**It asks only what the evidence answers.** A categorical flip is false
whatever field it names, because the cases are drawn only from candidates
whose categorical fields all read positive. **There is no arm for
`overstated`:** building one needs a threshold, and a threshold chosen by the
set's builder would be a judgement posing as an answer key.

## Results

| arm | cases | A (`v15`) | B (`v16`) | C (`v16`) |
|---|---|---|---|---|
| `category_flip` | 30 | 21 detected | **28** | **27** |
| `stripped` | 12 | 12 | 12 | 12 |
| `repeat` | 12 | 12 held | 12 held | 11 held, 1 moved |
| `paraphrase` | 7 | 6 held, 1 moved | 7 held | 7 held |
| `invented_evidence` | 12 | 8 flagged, 4 inconclusive | 8, 4 | 9, 3 |
| `control` | 12 | 11 `fair`, 1 `overstated` | 12 `fair` | 12 `fair` |

- **The headline is 27 to 28 of 30 flips detected at `v16`, over two runs.**
  Between B and C, 3 of 85 verdicts differ. One reversed confidently, at a
  margin of 0.34.
- **Most `v15` misses were two Markov phrasings,** "bull markov" and
  "bull-state markov". Naming the fields took them from 2 of 10 to 8 of 10,
  with nothing else in the arm going backwards.
- **A control changed verdict.** These runs cannot tell ordinary variation
  from an instruction-induced change.

Corrections to the first write-up are recorded on the page. Among them: a
scorer loophole, sufficiency scored without its control, and the wrong source
commit recorded for two runs.

## What it cannot establish

- **`v16` was measured on the set that motivated it.** That makes it a
  development baseline, not an evaluation.
- **Two runs of `v16` give a range, not a distribution,** and the single
  `v15` run has no variance estimate at all.
- **The controls have no ground truth,** and the set was not independently
  labelled.

## Since: the model rolled over

The deployed config uses the floating alias `~typesafe/jev-latest`, and
`jev_requests` records the version that answers each call.
- **`typesafe/jev-1.13-20260917`** answered every request from 2026-09-20 to
  2026-10-14 18:21Z.
- **`typesafe/jev-1.14-20261014`** answers from 2026-10-15 08:55Z.

The figures in the tables above are 1.13's. **Re-measured on 1.14 the
same day**, two runs on the same cases and question text:
- every flip 1.14 answered it detected, 29 of 30 in all, against 27 to 28 on
  1.13; the remaining case came back unanswered in both runs;
- invented evidence crossed for 7, against 8 to 9, which two runs on twelve
  cases cannot separate from variance;
- run-to-run variance stayed at 3 of 85 verdicts;
- the movers were again the 009 family.

The raw doc has the full table. The
deterministic numeric checker does not call the model, so its evaluations are
unaffected.

**Policy since 2026-10-15:** Jev always runs the latest version. That makes
every rollover a dated measurement epoch: judgements keep the version that
made them, nothing is regraded, and the baseline has now been re-measured on
1.14, as above. It is stated as
`jev::ALIAS_ROLLOVER_POLICY` and in the
[adoption review](/Users/lindau/codex/rust_daytrader/docs/jev-adoption-review.md)
under item A.5. Rollovers are announced once in Slack and shown on the AI
Prompts view.
