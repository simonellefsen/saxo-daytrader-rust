# Held-out blinded pass — pre-registration

Written and committed **before** any note text was read and **before** the
grader was run on any of these reports. Fixing the design in advance is what
stops the sample or the threshold being chosen to suit the result.

## Population

Completed decision reports that are **not graded**, carry a persisted daily
indicator snapshot, and have at least one selected asset with a matching
snapshot entry.

- Report ids 83–271, 141 reports, **718 checkable candidates**.
- None of these reports was read while building or fixing the numeric checker.
  The tuning fixtures came from reports 273–313, which are excluded entirely.

## Sample

Every 24th candidate, ordered by report id then symbol: 30 cases. Ids are
listed in `jev-adjudication-2026-09-21b-cases.json` and were fixed before any
note was displayed.

## Order of operations

1. Select and record the case ids. *(this file)*
2. Read note and evidence, record a label per case. **The grader has not run
   at this point**, so blindness is structural rather than a matter of my
   looking away — there is no verdict in existence to see.
3. Commit the labels.
4. Grade the sampled reports.
5. Compare.

Two cases in the previous run carried labels I had already seen verdicts for.
Grading after labelling removes that failure mode entirely.

## Pre-registered hypothesis

From the 2026-09-21 run: **flag-level disagreements sit at margin ≤ 0.25.**

Applied unchanged. It is not re-fitted to this sample. It either holds or it
does not, and a threshold re-picked per sample would validate nothing.

## Stated limits, in advance

- The adjudicator is **Claude Opus 5 — the agent that wrote the grader.** This
  is not independent ground truth, whatever the agreement rate turns out to be.
- Numeric checks run under `n3-2026-09-21`. Agreement on them measures whether
  the attribution is right on notes never used to tune it; it is still not a
  correctness rate against an external source.
- Wording and numeric findings are scored separately. v8 is told to judge
  wording and not to re-check arithmetic, so mixing them is not like-for-like.
- No trading parameter changes on any result here.
