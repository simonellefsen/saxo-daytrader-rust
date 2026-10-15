---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-fresh-evaluation
  path: docs/jev-fresh-evaluation.md
---

# Jev numeric checker: fresh evaluation v1

Raw sources:
- [jev-fresh-evaluation.md](/Users/lindau/codex/rust_daytrader/docs/jev-fresh-evaluation.md),
  with its instrument, key, labels, labeller notes, provenance, seeded,
  results and reconciliation files;
- [the signed-conviction adjudication](/Users/lindau/codex/rust_daytrader/docs/jev-adjudication-2026-09-26-signed-conviction.md),
  which settled the question it left open.

## What it is

**The first held-out evaluation: the frozen checker on notes nobody tuned
against.** Every earlier instrument drew on the 233-report development dump,
which shaped every fix from `n11` to `n15`. The
[controls](jev-controls-v1.md) and the [whole-note audit](jev-whole-notes-v1.md)
had become development evidence.

- **Method:** `n15-2026-09-25`, frozen at `cc7b0a4`. The scorer refused any
  other.
- **Frame:** every completed report after #319, the last in the dump,
  through #362, the last before the freeze. That is 11 reports from
  2026-09-23 to 09-25, taken whole as 51 notes:
  - 28 shown to the checker in production;
  - 23 never shown to it.
- **Part 1, the natural notes:** numeric claims labelled blind.
- **Part 2, seeded cases:** generated from the labels, not the checker.
  Anchors are labelled-consistent claims the key's own arithmetic confirms,
  which removes the [challenge set](jev-challenge-set.md)'s limit of
  anchoring on what the checker already reads.

## Results

Labelled by a fresh-context AI reviewer, a Codex subagent per its provenance.
The sequence:
- labels `afcb71f`;
- seeded cases `3843366`;
- one process fault fixed in `057006d`, with no case changed;
- scored once at `ddc7781`.

**Natural notes: 90 claims.** 72 consistent, 15 `cannot_tell`, 3
inconsistent.

| per note | the 28 the checker read | all 51 |
|---|---|---|
| `false_alarm` | 0 | 0 |
| `accepted_error` | 1 | 1 |
| `unflagged_error` | 1 to 4 | 1 to 13 |
| `omission` | 1 | 1 |
| `wrong_field` | 1 | 1 |

- **No consistent claim was flagged:** 0 of the 46 compared in production.
- **The three inconsistent claims:**
  - NOKIA "6 confluences" against 5, flagged;
  - CHEMM "502 DKK support" against 502.89, accepted. It was the third
    instrument where truncation against rounding decided a case.
  - CHEMM "consolidation at 515.5 DKK", not compared.
- **The one defect:** "supported" contested as "support", a keyword matched
  inside a longer word. It was fixed in `n16`, which this set therefore
  cannot validate.

**Seeded: 490 cases from 69 anchors.**
- 329 of 349 changed false figures were flagged on the right field.
- No untouched neighbour was flagged.
- **All nine wrong or wrong-field outcomes are one anchor,** NOKIA's
  "negative Markov conviction (-0.124)". The labeller reads the signed
  signal; `n15` reads conviction.

## Reconciliation and the adjudication

**Review found the seeded contexts often incoherent.** Each change moved one
field alone, though the model links conviction to the signed signal. So 37 of
69 `both_moved` cases and 72 of 138 evidence-side cases hold evidence the
model could not produce. They are single-field arithmetic tests, not market
contexts.

The reconciliation, by the checker's author, changed no label. Its findings:
- Without the NOKIA anchor, no seeded decision is wrong.
- With coherent evidence only, the one false alarm goes.
- 35 of 69 removals are still derivable from linked fields.

Two qualifications travel with these: they are post-hoc subsets, and
"coherent" means passing the listed checks.

**The adjudication on 2026-09-26** was also by the checker's author, and
ruled against the checker's rule. It was amended after review.
- **NOKIA's "-0.124" is the signed signal.** The ruling rests on the note's
  own "negative", its minus sign and the gate it cites. The labeller's field
  is right.
- **For future labelling, a convention:** a signed figure beside "conviction"
  states `markov.signed_signal`.
- **It is not a finding about past authors.** Where the signal is positive,
  the two fields hold the same value and cannot be told apart.
- **Under the ruling, the seeded sign-flip acceptance is a genuine miss.**
  Whether to change the checker is Simon's decision. It is now
  [todo T4](../todo.md).

**Kept as historical evidence,** never regenerated for cleaner results. Its
successor is [fresh-v2](jev-fresh-evaluation-v2.md). That one states the
signed-conviction and rounding conventions in its rubric, and separates
valid-context changes from corruption.
