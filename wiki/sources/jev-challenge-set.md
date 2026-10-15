---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-challenge-set
  path: docs/jev-challenge-set.md
---

# Jev numeric checker: the seeded challenge set

Raw source: [jev-challenge-set.md](/Users/lindau/codex/rust_daytrader/docs/jev-challenge-set.md),
with the frozen sets `jev-challenge-v1.json` to `-v3.json`.

## What it is

**A regression set of seeded errors, scored against the current checker on
every build.** It exists because every adjudicated label so far had been
`fair`
([adjudications](jev-grading-adjudications-2026-09-21.md)), so sensitivity had
never been measured.

Each case takes a real note and its real evidence and changes one thing. The
right answer then follows from the change: `holds`, `fails` or
`unsettleable`. Two rules keep it honest:
- the key records the truth of the claim, not the verdict the checker ought
  to give;
- an abstention is neither a hit nor a miss.

| set | generated against | cases |
|---|---|---|
| v1 | `n8-2026-09-22` | 240 |
| v2 | `n9-2026-09-22` | 242 |
| v3 | `n10-2026-09-22` | 242 |

**v1 stays frozen and is never regenerated.** A set rebuilt on every method
change has never seen the method it would need to catch.

## Results, v3 at `n10`

| | cases | decided correctly | unresolved | decided wrongly |
|---|---|---|---|---|
| false claims | 135 | **98** | 37 | **0** |
| true claims | 77 | 62 | 15 | 0 |
| unsettleable | 30 | 30 | — | 0 |

The headline is **72.6% of seeded false claims detected, none wrongly
accepted, 27.4% unresolved**. Both halves belong together.

**The abstentions are three findings:**
- a unit error off by a factor of 100 is never flagged, 0 of 15;
- negated claims are never checked in either direction, 0 of 15 each;
- about half of transposed figures go uncaught, 8 of 15.

**It found one silent defect:** `markov.bull_prob` and `markov.bear_prob` had
never once been checked in production. The keyword was spelled "bull prob".
`n9` fixed it.

## Since

**Rounding, not truncation, from `n17` (2026-09-26).** The frozen files are
not edited. Their truncation cases are re-read under the convention, making 8
false in v1, 7 in v2 and 6 in v3. The checker detects all of them.

The key's own rounding test was corrected at `n19`. From `n20`, a case it
cannot settle is `key_unresolved`, counted as neither true nor false.

## What it cannot establish

- **It is regression evidence, not held-out evidence.** The method has seen
  it.
- **The anchors are figures the checker already reads,** picked by running
  the checker. So the set cannot detect an attribution that was wrong from
  the start, only one that a later change breaks.
- **Unattributed figures are under-represented by construction.** At `n8`
  they were the largest abstention class in production.
- **It measures the numeric checker, not the wording grader.**

Successors that address the anchor limit: [whole-note audit](jev-whole-notes-v1.md),
[fresh-v1](jev-fresh-evaluation-v1.md).
