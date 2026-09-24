# Whole-note audit: what the checker never extracts

`whole-notes-v1-2026-09-24`: 75 whole notes drawn from the 233-report dump the
controls used. The key is at numeric method `n12-2026-09-24`. Seed
`jev-whole-notes-v1`, protocol `whole-notes-protocol-v1-2026-09-24`.

| file | |
|---|---|
| `jev-whole-notes-v1.json` | the instrument: whole notes and their evidence. **No verdicts, no spans, no strata.** |
| `jev-whole-notes-v1-key.json` | each note's stratum and every figure the checker extracted, with its verdict. Do not read first. |
| `jev-whole-notes-v1-labels.json` | a template, one empty entry per note, waiting for a labeller who is not me |

## Why

The controls show a labeller one figure the scanner already found. A claim the
scanner never recognised — a number written in words, a form it does not read,
a note that never reached the checker — cannot enter that sample, and cannot
be labelled wrong. The controls page named the gap: extraction omissions need a
whole-note audit.

Here the labeller reads each **whole note** beside the evidence the report was
given, and lists every checkable claim in it. Each labelled numeric claim is
then matched to the figures the checker extracted inside its quote:

| handling | meaning |
|---|---|
| `omitted` | no extracted figure lies inside the quote |
| `abstained` | found, but not every figure was compared |
| `accepted` | found and compared, every figure agreed |
| `flagged` | found and compared, at least one figure disagreed |

## The frame and the draw

Every distinct note that a report attached to a selected symbol, counted rather
than asserted. The counts reproduce the controls' frame audit exactly:

| stratum | notes | drawn |
|---|---|---|
| `reached_with_figures`: the checker read it and found figures | 608 | 40 |
| `reached_without_figures`: read, no figure found | 229 | 15 |
| `never_reached`: no indicator snapshot, or past the question budget | 279 | 20 |
| | **1,116** | **75** |

The draw is ordered by `sha256(seed + report + symbol)` within each stratum.
Seven of the 20 `never_reached` notes come from early reports that carried no
evidence at all, so their claims can only be `cannot_tell`. That is an honest
result, not a defect: nothing was ever checkable there.

Seven of the 75 notes also hold a controls case. A labeller who has seen the
controls is not blind to those notes, so this instrument needs a fresh one.

## Labelling

Give a labeller `jev-whole-notes-v1.json`, which carries its own rubric, and
nothing else. For each note:

- **`claims`**: every assertion about the supplied evidence. That means every
  figure, and every qualitative statement about an indicator, a trend, a
  support level, a Markov state or signal, or a Quiver signal. For each one:
  - `quote`: the exact words, as short as still makes the claim. It must occur
    exactly once in the note.
  - `kind`: `numeric` if the claim states a number, in digits or in words;
    `qualitative` otherwise.
  - `field`: the dotted evidence field it concerns, or `none`.
  - `verdict`: `consistent`, `inconsistent` or `cannot_tell`. **Missing
    evidence is `cannot_tell`**, written into the rubric this time rather than
    supplied beside it.
- **`status`**: set to `labelled` once the whole note has been read, including
  when it makes no checkable claim. A note without it counts as unread.

Out of scope: portfolio capital, holdings, allocations, unrealised profit,
trading costs, orders, stops, executions, and recommendations. Their
supporting material is not in the evidence.

## The analysis plan, fixed before any label exists

**The input is valid or nothing is scored.** The file is rejected, as sets so
the order does not matter, for any of these: a duplicate or unknown id; a quote
that is not in its note or occurs twice; a kind or verdict the rubric does not
define; a status other than `labelled`; or claims on a note not marked as read.

**The unit is the note.** Claims within a note are not independent, so every
population figure is the share of notes in which something happened. Claim
counts are reported as description. The note-level outcomes are:

| outcome | yes when | unresolved when |
|---|---|---|
| `omission` | a numeric claim has no extracted figure inside it | never, because it does not depend on the verdict |
| `unflagged_numeric_error` | a numeric claim read as `inconsistent` was not flagged | none is, but a `cannot_tell` numeric claim went unflagged |
| `qualitative_error` | a qualitative claim is read as `inconsistent` | none is, but one is `cannot_tell` |

`qualitative_error` describes the notes, not any checker: the wording grader
judges whole notes, not spans, so nothing here is matched to it.

**The population figure is an interval.**
- Each unresolved note is counted once as no and once as yes.
- Each sampled stratum gets an exact hypergeometric bound at 1 − 0.05/3.
- The strata are weighted by their note counts, and every stratum stays in.
- It holds at 95% or more, under two assumptions: the hash-ordered draw
  behaves as a simple random sample within each stratum, and the labels are
  right. It covers sampling error only.
- A test enumerates every count each stratum could hold, to confirm the
  coverage.
- **Nothing is estimated while any note is unlabelled.**

The report also lists **scanned figures no labelled claim covers**. These are
quantities the rubric puts out of scope, or claims the labeller did not see.

### What a perfect result would report

Pinned by a test and checked with exact integer arithmetic. If every note came
back read, with nothing omitted and nothing wrong:

| | notes, at most |
|---|---|
| `reached_with_figures` | 66 of 608 (10.9%) |
| `reached_without_figures` | 60 of 229 (26.2%) |
| `never_reached` | 57 of 279 (20.4%) |
| **population** | **183 of 1,116 (16.4%)** |

That is what this method would print, not a limit on what the sample could
show. A clean result bounds each share; it cannot show that the share is small.

## After the labels

The same order as the controls:
1. **The instrument and its rubric only.** Not the key, not the controls, not
   the provenance findings.
2. **Labels are committed as delivered**, with `labelled_by` and
   `labelled_at`, before the key is opened.
3. **Scored once**, and reported as it comes out.
4. **Reconciled afterwards, in a separate file.** The original labels are
   never edited, and reconciled figures are reported beside the originals.

## What it will not establish

- **It measures the numeric checker's extraction at `n12`**, on the same frame
  as the controls. That frame is the history, not fresh reports: the fixes in
  `n11` and `n12` were shaped by it.
- **One labeller is one judgement.**
- **Qualitative claims are counted, not matched.** Whether the wording grader
  caught them is a separate question.
- **It says what is missed, not why it matters.** An omitted claim is not a
  wrong one. The second outcome, unflagged errors, is the one that bears on
  correctness.
