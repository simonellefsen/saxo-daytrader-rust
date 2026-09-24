# Whole-note audit: what the checker never extracts

`whole-notes-v1-2026-09-24`: 75 whole notes drawn from the 233-report dump the
controls used. The key is at numeric method `n12-2026-09-24`. Seed
`jev-whole-notes-v1`, protocol `whole-notes-protocol-v1-2026-09-24`.

| file | |
|---|---|
| `jev-whole-notes-v1.json` | the instrument: whole notes and their evidence. **No verdicts, no spans, no strata.** |
| `jev-whole-notes-v1-key.json` | each note's stratum and every figure the checker extracted, with its verdict. Do not read first. |
| `jev-whole-notes-v1-labels.json` | 75 labelled notes from a fresh-context AI reviewer, as delivered. Never edited. |
| `jev-whole-notes-v1-labeller-notes.md` | the labeller's reasoning for every uncertain or contradicting claim |
| `jev-whole-notes-v1-labelling-provenance.json` | who labelled, from what, with fingerprints |
| `jev-whole-notes-v1-results.json` | the single scoring run, unedited, with fingerprints |
| `jev-whole-notes-v1-reconciliation.json` | every disagreement, reconciled against the evidence, beside the labels |

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

## Results, as they came out

Scored once, at `71fd860` with a clean tree. The labels were committed and
pushed before the run. **The labels are one AI review, not human ground
truth.** The labeller was given the instrument and its rubric only; its own
reasoning file records the calls it had to make.

- **The file was accepted.** All 75 notes are labelled, and none was rejected.
- **264 claims were found:**
  - **90 numeric:** 77 consistent, 11 `cannot_tell`, 2 inconsistent.
  - **174 qualitative:** 133 consistent, 41 `cannot_tell`, 0 inconsistent.
- **2 notes make no in-scope claim.**

### Per note, by stratum

| stratum | sampled | `omission` | `unflagged_numeric_error` | `qualitative_error` |
|---|---|---|---|---|
| `reached_with_figures` | 40 of 608 | 1 yes, 39 no | 1 yes, 35 no, 4 unresolved | 0 yes, 30 no, 10 unresolved |
| `reached_without_figures` | 15 of 229 | 2 yes, 13 no | 0 yes, 14 no, 1 unresolved | 0 yes, 7 no, 8 unresolved |
| `never_reached` | 20 of 279 | 10 yes, 10 no | 0 yes, 16 no, 4 unresolved | 0 yes, 3 no, 17 unresolved |

### Population ranges, at 95% or more

| outcome | notes of 1,116 | share |
|---|---|---|
| `omission` | 72 to 408 | 6.5% to 36.6% |
| `unflagged_numeric_error` | 1 to 395 | 0.1% to 35.4% |
| `qualitative_error` | 0 to 726 | 0% to 65.1% |

These are wide, and it is clear what makes them so. `omission` is driven by
`never_reached`, where the checker never ran: 68 to 211 of those 279 notes.
The other two outcomes are driven by unresolved notes, which the protocol
counts both ways. In the notes the checker read, `omission` is 1 to 94 of 608,
and 3 to 103 of 229.

### Claim by claim

| numeric claims | consistent | `cannot_tell` | inconsistent |
|---|---|---|---|
| found and compared, all agreed (`accepted`) | 56 | 3 | **1** |
| found and compared, one disagreed (`flagged`) | 0 | 1 | 1 |
| found, not compared (`abstained`) | 9 | 2 | 0 |
| **never found (`omitted`)** | 12 | 5 | 0 |

**What the checker never found.** There are 17 omitted claims:
- **11 are in notes that never reached the checker.** Six of them are Markov
  figures whose evidence *was* supplied, and all six are consistent.
  Candidates are excluded from grading when they lack an indicator snapshot,
  so their Markov claims go unchecked even though they could be checked.
- **The other 6 are in 3 of the 55 notes the checker read, and every one is a
  number written as a word:** "five-day", "five-confluence", "Five bullish
  technical confluences". The scanner reads digits only. None of the six is
  wrong.

**The two wrong numeric claims.**
- **FLS, report #286, "593 DKK support"**, was flagged by the checker. The
  labeller agrees it is wrong, reading 593 as the close, where the support is
  551.5.
- **ASML, report #284, "break risk (0.400)"**, against 0.4006, was accepted.
  The checker accepts truncation, which gives 0.400; the labeller rounds, which
  gives 0.401. It is the same convention disagreement as LMND in the controls,
  not an unambiguous arithmetic error, and it is the only unflagged numeric
  error in the sample.

**Found, not compared.** 11 claims, all consistent or `cannot_tell`. Six were
refused by the grammar: "positive at", "near", "above 75", "Bull-state".
Five could not be attributed:
- "3.7% support-break risk", where the percentage is not admitted for a field
  stored as a fraction;
- "4.1% distance to support", which is not one of the field's names;
- a unit price in DKK;
- a monitored quote and an intraday move, which have no field.

**Figures the scanner found that no claim covers.** All 11 are out of scope:
allocations, a limit reference, a position's profit, a share count, a total
return, a stop and a budget. The labeller was right not to claim them, and the
checker right not to compare them.

**Qualitative claims.** None was read as wrong. The 41 `cannot_tell` are
mostly strength words with no defined threshold ("strong", "weak", "mild"),
freshness with no report timestamp, and missing evidence. The labeller's notes
record each one.

### For reconciliation — listed, not resolved

1. **ASML "break risk (0.400)"** is truncation against rounding, as with LMND.
2. **Strength and freshness words.** The rubric gives no threshold for
   "strong" and no time for "fresh", so 41 qualitative claims are
   `cannot_tell`. That is most of the width of `qualitative_error`.
3. **Claim boundaries.** The labeller sometimes quoted overlapping spans for
   independently checkable parts of one phrase. The note-level outcomes count
   each note once, so this does not move them.

### What it suggests, not yet acted on

Three gaps, each measured here and none fixed. The parser and the grader are
unchanged by this audit.
- **Numbers written as words are never read.** Every omission in a note the
  checker read was one.
- **Candidates without an indicator snapshot are not graded at all,** though
  their Markov figures could be checked.
- **Eleven figures were found but not compared,** for grammar and attribution
  reasons, and none of them was wrong.

## Reconciliation

`jev-whole-notes-v1-reconciliation.json`, 2026-09-25, **by the checker's
author**: not independent. Every reading cites the evidence in its case. The
rule is the controls' rule: the evidence settles a disagreement and the rubric
settles how to read it. Where neither can, the reading stays as labelled.

**No label changes, and the reconciled figures equal the scored ones.** Every
value the labeller judged was checked against its evidence, and each reading
holds. A test checks, on every build:
- the delivered labels are byte-identical to the hashed file;
- every entry quotes the label, and the checker's handling and figures,
  exactly;
- each of the 52 `cannot_tell` is classified exactly once;
- the figures are what the frozen scorer gives.

It is checked by breaking it: a misquoted label, a wrong handling, a wrong
figure and a dropped classification each fail it.

It covers 35 numeric claims — every one the checker did not simply accept
against the same field:

| point | claims | reconciled |
|---|---|---|
| never found (`omission`) | 17 | **the label stands in every case.** The 12 consistent ones agree with their evidence. Every omission in a note the checker read is a number written as a word. |
| found, not compared | 11 | **the label stands.** The 9 consistent ones agree with their evidence. |
| checker matched the figure; the label judges a broader claim `cannot_tell` | 3 | **no conflict.** The number agrees; what is undecidable is "strong", "fresh" or "favorable". |
| ASML "break risk (0.400)" against 0.4006 | 1 | **the convention is unresolved; the label is kept.** Truncation gives 0.400 and rounding gives 0.401, as with LMND. |
| FLS "593 DKK support", flagged | 1 | **label and checker agree.** The controls' blind labeller read it the same way. The 2026-09-22 adjudication is still not revised by the checker's author. |
| FLS "consolidating near 593 DKK support" | 1 | **no conflict.** The flagged figure is judged in its own claim; this one is about consolidating. |
| field focus: "low 0.122 modeled support-break risk" | 1 | **both agree with the evidence.** The label judges the "low" label; the checker compared the figure. |

**Figures no claim covers.** All 11 are out of scope, and the labeller and the
checker agree.

**What the 52 `cannot_tell` are:**

| reason | claims |
|---|---|
| a strength word with no threshold: strong, weak, mild, modest, solid, favorable, high-conviction | 21 |
| missing evidence: the candidate had no indicator snapshot | 11 |
| "fresh", with no report timestamp | 9 |
| a claim the evidence has no field for: an intraday move, a broker or monitored quote, sector strength | 5 |
| a comparison or pattern with no reference: "weaker", "extended", "consolidating" | 3 |
| ambiguous wording: "OVERWEIGHT/BUY", "Overweight" | 3 |

30 of the 52 are strength and freshness words. Defining them would be a rubric
change for a future instrument, not a reading of this one.

**Truncation, as a scenario, not a reconciled reading.** If the rubric accepted
truncation as the checker does, only ASML moves. Unflagged numeric errors would
be **0 to 376 of 1,116 notes** instead of 1 to 395. In the notes the checker
read figures in, the count goes from 1 to 0.

**Open, and not settled here:**
- **Truncation**, as above.
- **Numbers written as words**, the only omission in notes the checker read.
- **Notes excluded for lacking an indicator snapshot.** Their Markov figures
  could be checked; six were, by the labeller, and all six are consistent.
- **Found but not compared.** Six claims were refused by the grammar or the
  attribution margin. Five had no field: a percentage for a fraction-stored
  risk, "distance to support", a DKK unit price, a monitored quote and an
  intraday move. None of the eleven is wrong.

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
