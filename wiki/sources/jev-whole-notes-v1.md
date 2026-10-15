---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-whole-notes
  path: docs/jev-whole-notes.md
---

# Jev numeric checker: whole-note audit v1

Raw source: [jev-whole-notes.md](/Users/lindau/codex/rust_daytrader/docs/jev-whole-notes.md),
with the instrument, key, labels, labeller notes, provenance, results and
reconciliation files beside it.

## What it is

**An audit of what the checker never extracts.** The
[controls](jev-controls-v1.md) only show a labeller a figure the scanner
already found. Here a labeller reads each whole note beside its evidence and
lists every claim in it: numeric, and qualitative too. Each numeric claim is
then matched to what the checker extracted inside its quote: `omitted`,
`abstained`, `accepted` or `flagged`.

`whole-notes-v1-2026-09-24`: 75 notes drawn by seeded hash from the same
233-report dump as the controls, keyed at `n12-2026-09-24`.

| stratum | notes | drawn |
|---|---|---|
| `reached_with_figures` | 608 | 40 |
| `reached_without_figures` | 229 | 15 |
| `never_reached`: no indicator snapshot, or past the question budget | 279 | 20 |
| | **1,116** | **75** |

**The note is the unit.** Population figures are exact per-stratum bounds,
holding at 95% or more for sampling error only. Unresolved notes count both
ways.

## Results

Labelled by a fresh-context AI reviewer, a Codex subagent per its provenance.
It had to be a new one, because 7 of the 75 notes also hold a controls case.
Scored once at `71fd860`.
- **264 claims.** 90 numeric: 77 consistent, 11 `cannot_tell`, 2
  inconsistent. 174 qualitative: 133 consistent, 41 `cannot_tell`, none
  inconsistent.
- **Population ranges, notes of 1,116:**
  - `omission`: 72 to 408 (6.5% to 36.6%);
  - `unflagged_numeric_error`: 1 to 395 (0.1% to 35.4%);
  - `qualitative_error`: 0 to 726 (0% to 65.1%).
- **17 omitted claims.**
  - 11 are in notes that never reached the checker. Six of those are Markov
    figures whose evidence was supplied, all consistent.
  - The other 6 are in 3 of the 55 notes the checker read, and every one is
    **a number written as a word**: "five-day", "five-confluence". The
    scanner read digits only.
- **The two wrong numeric claims:**
  - FLS "593 DKK support", flagged by the checker, which the labeller agrees
    with;
  - ASML "break risk (0.400)" against 0.4006, accepted. That is truncation
    against rounding again, and the only unflagged numeric error.

## Reconciliation, 2026-09-25

Done by the checker's author, so not independent. It has 35 entries. **No
label changes, and the reconciled figures equal the scored ones**; a test
checks both.
- **Every label stands.**
- **What the 52 `cannot_tell` are:**
  - 30 strength words with no threshold ("strong", "mild") or "fresh" with no
    report timestamp;
  - 11 candidates with no indicator snapshot;
  - the rest, claims with no field, no reference, or ambiguous wording.

**Corrected after review:** an earlier summary said none of the gaps hid a
wrong claim. The evidence supports only that no confirmed wrong claim was
found among them, with some still `cannot_tell`.

## Since

- **Number words have been read since `n13`** (2026-09-25), and all six
  omissions now match. This audit shaped that fix, so for `n13` it is
  regression evidence, not validation.
- **Rounding was settled on 2026-09-26.** ASML is flagged from `n17`. The
  recorded figures stand as scored.
- **Still open:** candidates without an indicator snapshot are not graded,
  though their Markov figures could be checked.

The audit measures extraction at `n12` on the development history, not fresh
reports. Next came [fresh-v1](jev-fresh-evaluation-v1.md).
