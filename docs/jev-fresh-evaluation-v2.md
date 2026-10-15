# Fresh evaluation v2: fixed before its frame exists

`fresh-v2-2026-09-26`, protocol **`fresh-protocol-v2-2026-09-26-r3`**. Code:
`src/jev_fresh/v2.rs`, test-only.

**Status: scored once and reconciled, 2026-10-15.** It was preregistered as
revision 3 on 2026-09-26, before any report was in the frame. Every
preregistered section is unchanged since; the results are inserted after "The
order of work", as they came out. The amendments at the end record what each
revision changed, and why.

**This set measures `n20`.** With the results recorded, the test that pinned
the checker's source stands down. A later checker that these notes shaped
cannot be validated by them.

| file | |
|---|---|
| `jev-fresh-v2-frame-manifest.json` | every report in the window, the eligible ids and the one exclusion's reason |
| `jev-fresh-v2.json` | the instrument: 143 whole notes and their evidence, with the rubric and conventions. **No verdicts, no spans, no strata.** |
| `jev-fresh-v2-key.json` | the frame, and every figure `n20` extracts from each note, with its verdict |
| `jev-fresh-v2-labels.json` | 143 labelled notes from a fresh-context AI reviewer, as delivered. Never edited. |
| `jev-fresh-v2-labeller-notes.md` | the labeller's judgement calls, and its reason for every uncertain or contradicting claim |
| `jev-fresh-v2-labelling-provenance.json` | who labelled, from what, with fingerprints and the one deviation |
| `jev-fresh-v2-seeded.json` | 2,245 seeded cases generated from the committed labels, with the refusals counted |
| `jev-fresh-v2-results.json` | the single scoring run, unedited, with fingerprints |
| `jev-fresh-v2-reconciliation.json` | the disagreements, read beside the labels, and the seeded totals traced figure by figure |

## What is frozen

**Numeric method `n20-2026-09-26`, frozen at `07cf157`, pinned by source
hash.** `src/jev_numeric.rs` must hash to
`7816fd23fee51c7e7ddd103a3ed28f1ee92da25f462ab747191d484eac27893b`. A version
string alone cannot catch an implementation change made without a version
bump.
- A test fails every build on which the checker's source differs, until this
  set is scored.
- The builder and the scorer refuse to run on any other source or version.
- A defect found meanwhile is recorded against `n20`. A fix would be `n21`,
  which must wait, or amend this page with a dated reason.

## The frame

**Every completed report created after `2026-09-26T09:39:03Z`, when `n20` was
frozen, and before `2026-10-10T00:00:00Z`, taken whole.** That is every
distinct note those reports attach to a selected symbol. None of these reports
exists yet, so none can have shaped the method. At about four reports a
trading day, expect roughly 40 reports and 200 notes.

**The window is fixed.** Moving it after seeing any report would let the
frame be chosen by its contents. An amendment must be dated and justified
without reference to anything in the frame.

**The frame is checked against a manifest, so a partial export cannot become
the frozen evaluation.** Both queries are fixed in the code:
- `MANIFEST_QUERY` lists every report in the window, whatever its status.
- `DUMP_QUERY` exports the eligible ones.

The manifest is committed with the time it was taken, its eligible ids, and a
reason for every exclusion. The builder then refuses to run if any of these
hold:
- the manifest was taken before the close;
- a query differs from the recorded one;
- an exclusion has no reason;
- no report is eligible;
- the dump lacks an eligible report, or holds one that isn't;
- any report is not completed, or its creation time disagrees with the
  manifest;
- any dumped report's contents are unusable.

Unusable contents means any of these:
- no report object, or no `selected_assets` list;
- a selected asset without a symbol, or with notes that are not text;
- a stored prompt that does not decode;
- an evidence block that is present but has no `signals` list.

**These are told apart from legitimate gaps.** A report that selected nothing
is kept: 13 stored reports did. A prompt without one of its evidence blocks
is kept, and the key records which were absent. In every stored report, each
block present is an object holding a `signals` list.

It also refuses to run before the close, and to run twice. A test checks the
built frame against the manifest on every build.

## The conventions, stated in the instrument

- **Rounding.** A figure written short is consistent only if it is the stored
  value rounded to the precision written, either way at an exact half. A
  truncation rounding does not produce is inconsistent. Simon settled this on
  2026-09-26.
- **Signed conviction.** Beside "conviction", a figure written with a sign
  states `markov.signed_signal`, and an unsigned one states
  `markov.conviction`. This is a labelling convention from the 2026-09-26
  adjudication, not a claim about what past authors meant.
- **Missing evidence** is `cannot_tell`, never `inconsistent`.

The key's own arithmetic follows the rounding convention, in whole units of
the last written place. A figure a double cannot settle seeds nothing.

## The seeded changes

**`valid_context` changes leave evidence the runtime could produce.**
- The evidence is changed only for the two groups whose every dependent this
  generator reproduces exactly:
  - **Markov:** the signed signal, conviction, bull and bear probabilities and
    the direction move together; the sideways probability is held.
  - **Quiver:** the signal, its direction by the ±0.15 threshold, and its
    confidence, which carries 0.35 of the signal's magnitude.
- Every other field is computed by the runtime from inputs this generator
  cannot reproduce. The RSI, for example, feeds the confluences, the count,
  the sentiment and the break risk. None of them is moved as a valid context.
- **Every `valid_context` case**, whatever made it and including changes to
  the note alone, **must pass `evidence_problems`**, and so must the note's
  original evidence. The check covers:
  - the Markov links and bounds;
  - the Quiver bound, direction and confidence;
  - the break-risk label's band and the support's distance from the close;
  - whole-number counts.

  Each of these relations holds in the evidence of every stored note: 1,002
  with Markov rows, 489 with indicators and 372 with Quiver.

**`corruption` moves one field alone, and is reported apart, never pooled.**
Its only question is whether the checker reads the field it names.

| family | change | direction |
|---|---|---|
| `valid_context` | `note_value_up`, `note_value_down` | true → false |
| `valid_context` | `sign_flipped`, on a signed field | true → false |
| `valid_context` | `other_symbol_value`: another symbol's value for the field, copied in | true → false |
| `valid_context` | `evidence_moved_up`, `evidence_moved_down`: a Markov or Quiver group moved, the note unchanged | true → false |
| `valid_context` | `both_moved`: note and a Markov or Quiver group moved together | stays true |
| `valid_context` | `repaired`: a claim labelled and computed wrong, rewritten to the stored value | false → true |
| `valid_context` | `removed_with_its_links`: the field and everything that determines it | unsettleable |
| `valid_context` | `out_of_scope_stop_inserted` | out of scope |
| `corruption` | `single_field_up`, `single_field_down`: one field moved alone | true → false |
| `corruption` | `single_field_both_moved`: note and one field moved alone | stays true |

**Every case attempted and not generated is counted, by change and reason.**
The counts are recorded in the seeded file and the results:

| reason | meaning |
|---|---|
| `original_evidence_incoherent` | the note's own evidence fails the check, so nothing seeded from it is a controlled change |
| `dependents_not_reproduced` | a valid-context change to a field outside the two groups |
| `outside_bounds` | the move would take a field past its bounds |
| `evidence_problem` | a valid-context case whose final evidence fails the check |
| `key_not_borne_out` | the key's arithmetic does not bear out the change, cannot settle a figure, or finds a linked figure left stale |

Anchors come from the labeller, and generation never calls the checker, as in
fresh-v1.

## The analysis plan

As fresh-v1:
- the input is valid or nothing is scored;
- nothing is scored while any note is unlabelled;
- note-level outcomes, over the notes the checker read and over every note;
- counts over the frame taken whole, with no rate extrapolated.

The seeded results are reported per family and per change, with the changed
figure apart from the untouched ones. An abstention is neither correct nor
wrong. **`valid_context` is the headline; `corruption` sits beside it.**

**Two qualifications travel with any summary:**
1. A result on a subset chosen after scoring locates a disagreement. It does
   not validate anything.
2. "Valid context" means passing the checks in `evidence_problems`. It does
   not prove every relation in a synthetic context is valid.

## The order of work

1. After the close, record the manifest with `MANIFEST_QUERY`, including
   `taken_at`, the eligible ids and every exclusion's reason. Take the dump
   with `DUMP_QUERY`.
2. `build_the_fresh_v2_instrument` validates the frame and writes the
   instrument, the key and an empty labels file. Commit the manifest with
   them.
3. The instrument only goes to a fresh-context labeller. I do not label: I
   wrote the checker.
4. Labels are committed as delivered.
5. Run `generate_the_v2_seeded_cases`, and commit.
6. Run `score_the_fresh_v2_set`, once.
7. Any reconciliation goes in a separate file, beside the labels.

## Results, as they came out

Scored once, on 2026-10-15, at `f299df0` with a clean tree and the checker's
source matching its pin. The order held:
1. the manifest and the frame, after the close (`1707945`);
2. labels committed as delivered (`fa6b75b`);
3. seeded cases generated from them (`f299df0`);
4. one scoring run (`575d6a1`).

**The frame is 36 reports and 143 notes.** The manifest, taken at
2026-10-15T17:22:11Z, lists 37 reports in the window, #364 to #400. One is
excluded: #376, the 2026-10-01 US mid-session shadow pulse, ended `xai_error`
because the provider's completion carried no message content, so no report
exists. The dump is not committed. The key records its hash, so a rerun of
`DUMP_QUERY` can be checked against it. That is fewer notes than the
preregistration's guess of about 200; it is the frame, taken whole.

**The labels are one AI review, not human ground truth.** The labeller was a
fresh-context Claude subagent given the instrument only, in a directory
outside the repository. **One deviation is recorded in its provenance:** the
session harness put the project's standing instructions and a memory index
into its starting context. It reports neither opening nor using them, and
neither holds labels, keys or note text. The blinding rests on context and
instructions, not on operating-system access controls.

### Natural notes

All 143 notes were labelled, and the file was accepted. The labeller found
**319 numeric claims**: 271 consistent, 41 `cannot_tell` and 7 inconsistent.
One note makes no numeric claim.

| per note | the 100 the checker read | all 143, as the method reads them |
|---|---|---|
| `false_alarm` | 0 | 0 |
| `accepted_error` | 0 | 0 |
| `unflagged_error` | 3 to 13 | 5 to 32 |
| `omission` | 1 | 1 |
| `wrong_field` | 21 | 43 |

`unflagged_error` reads "5 to 32": 5 notes hold an inconsistent claim the
checker did not flag, and 32 would, if every unflagged `cannot_tell` claim
were an error.

**No consistent claim was flagged: 0 of the 207 the checker compared in
production**, and 0 of 248 across every note. Every figure the method compared
lies inside a labelled claim. The 38 extracted figures that no claim covers
were all left uncompared.

**The checker decided 250 claims, and every decision agrees with the label's
verdict.** It accepted 248, all labelled consistent, and flagged 2, both
labelled inconsistent. It left 68 undecided and missed 1.

**The seven inconsistent claims:**
- **Flagged, on the labeller's field:**
  - **#373 SALM, "above 566 NOK support"** against a nearest support of
    566.81. Under the rounding convention, 566 is a truncation. It is the
    kind of figure fresh-v1's checker accepted, CHEMM's "502"; `n20` rounds,
    and flags it.
  - **#385 DEMANT, "above 310.65 DKK nearest support"** against 318.075.
- **Not flagged: five bare price levels.** The checker finds each figure but
  attributes it to no field, so it does not compare it. The labeller compares
  each with the close.
  - **#393 and #394 DB1Gn, "consolidating near 281 EUR"**, against a close of
    292.2. It is one sentence, in two consecutive reports, and the frame,
    taken whole, counts both.
  - **#382 DEMANT, "holding above 317.00 DKK"**, against a close of 314.0.
  - **#390 CHEMM, "consolidating at 570 DKK"** and **#390 DEMANT, "trading
    stably at 323.8 DKK"**, against Markov closes of 576.0 and 325.2. These
    two are in notes production never showed the checker.

  Fresh-v1 recorded the same kind of miss, CHEMM's "consolidation at 515.5
  DKK".

**The rest:**
- **22 consistent claims were found but not compared:** 11 with an uncertain
  attribution, 9 unattributed and 2 implausibly attributed.
- **The 41 `cannot_tell` claims were all left uncompared.** For 17 the key
  records the figure as not in the evidence.
- **One claim was omitted:** #389 ASML's "single-share price (~12.2k DKK)".
  `n20` does not read a figure with a `k` suffix.
- **43 claims were compared against another field, and they are one
  reading.** Each is a signed figure beside "conviction". By the convention,
  the labeller reads `markov.signed_signal`, and `n20` reads the magnitude,
  `markov.conviction`. All 43 figures are positive, and in each the stored
  signed signal equals the conviction. So both readings accept every one,
  and these notes cannot tell the fields apart.

The 43 notes production never showed the checker hold 75 claims. The method
accepts 41 labelled consistent and decides none of the other 34. In
production none of them is checked at all.

### Seeded cases

2,245 cases, from 270 anchors in 140 notes. **Every consistent claim that
could anchor was confirmed by the key's own arithmetic.** The one that could
not was ASML's "12.2k", which the generator cannot rewrite.

**Every anchor's own evidence passed `evidence_problems`.** No change was
refused as `original_evidence_incoherent`. The changes not generated were
evidence moves:
- outside the Markov and Quiver groups: 138 for each of `evidence_moved_up`,
  `evidence_moved_down` and `both_moved`;
- past a bound: 60 for each of `evidence_moved_up` and `both_moved`.

**`valid_context`, the headline, 1,435 cases:**

| keyed figure | figures | correct | abstained | decided wrongly | other field |
|---|---|---|---|---|---|
| changed, made false | 1,077 | **752** | 102 | 43 accepted | 180 flagged |
| changed, kept or made true | 79 | **54** | 16 | 0 | 9 right verdict |
| changed, unsettleable | 270 | **270** | — | 0 | — |
| changed, out of scope | 9 | 9 | — | 0 | — |
| untouched neighbours | 1,763 | **1,509** | 155 | **0** | 99 right verdict |

**`corruption`, apart and never pooled, 810 cases.** Each moves one field alone,
and asks only whether the checker reads the field it names.

| keyed figure | figures | correct | abstained | decided wrongly | other field |
|---|---|---|---|---|---|
| changed, made false | 540 | **410** | 44 | 86 accepted | 0 |
| changed, kept true | 270 | **205** | 22 | 43 false alarms | 0 |
| untouched neighbours | 1,062 | **894** | 93 | **0** | 75 right verdict |

**The 43 accepted errors in `valid_context` are all sign flips**, 43 of the
132. **The seven repairs** turned the inconsistent claims true. Two were
accepted. The five price levels were abstained on, because the checker
compares none of them.

### What it shows, at its real strength

- **No false alarm was found on natural notes.** None on the 207 consistent
  claims the checker compared in production, and none on 248 across every
  note. **No untouched neighbour was flagged** in either seeded family. That is
  these 36 reports.
- **No natural claim was decided against its label.** The checker decided 2
  of the 7 inconsistent claims, and flagged both. The other 5 it did not
  compare, so **detection on natural errors is 2 of 7**. That is too few to
  measure a rate.
- **Seeded false figures were flagged on the right field 752 of 1,077 times.**
  The 2,245 cases come from 270 anchors, so they are far from 2,245
  independent trials. They measure sensitivity to one changed figure in claims
  the labeller found, not a natural error rate.
- **Every wrong or wrong-field seeded outcome is the signed-conviction
  reading.** The reconciliation traces this figure by figure.
- **Two gaps in `n20`, recorded and not fixed:**
  - a bare price level is attributed to no field, which accounts for all five
    unflagged natural errors;
  - a figure with a `k` suffix is not read.

  A fix would be a new version, which this set could not then validate.

## Reconciliation

`jev-fresh-v2-reconciliation.json`. **No label, seeded case or recorded score
is edited.** A test:
- asserts the hashes of all seven inputs;
- checks every entry against the label, the key and the evidence;
- recomputes every figure;
- shows that the per-figure seeded reading reproduces the recorded totals for
  both families.

I wrote the checker, so nothing is settled in its favour, and every reading
cites the evidence for someone independent.

**The signed-conviction reading.** The 43 claims decided on another field are
exactly the signed figures beside "conviction". Traced figure by figure:

| keyed figure | as scored | signed-conviction anchors | other anchors |
|---|---|---|---|
| **`valid_context`** | | | |
| changed, made false | 752 of 1,077 (223) | 0 of 223 (223) | 752 of 854 (0) |
| changed, kept or made true | 54 of 79 (9) | 0 of 9 (9) | 54 of 70 (0) |
| changed, unsettleable | 270 of 270 (0) | 43 of 43 (0) | 227 of 227 (0) |
| changed, out of scope | 9 of 9 (0) | — | 9 of 9 (0) |
| untouched neighbours | 1,509 of 1,763 (99) | 0 of 99 (99) | 1,509 of 1,664 (0) |
| **`corruption`** | | | |
| changed, made false | 410 of 540 (86) | 0 of 86 (86) | 410 of 454 (0) |
| changed, kept true | 205 of 270 (43) | 0 of 43 (43) | 205 of 227 (0) |
| untouched neighbours | 894 of 1,062 (75) | 0 of 75 (75) | 894 of 987 (0) |

Each cell is correct of all. The brackets are figures decided wrongly or on
another field. Everything else is an abstention.
- **On every other anchor, every seeded decision is correct.** That holds in
  both families.
- **On the signed-conviction anchors, no seeded decision is correct.** That
  is what reading conviction gives, as `n20` does for all 43 natural figures.
  A flipped sign keeps the magnitude, so it is accepted. A signed signal moved
  alone leaves the conviction unchanged, so the unmoved note is accepted and
  the moved one flagged.
- **The abstentions are concentrated.** The 118 changed `valid_context`
  figures left undecided come from 27 note-and-field anchors. The 66 in
  `corruption` come from 22.

Whether the checker should read the convention is Simon's call. The 26 Sep
adjudication made it a labelling convention, and `n20` keeps its own rule. On
these natural notes the choice changes no verdict. A checker changed to read
it would be `n21`, which this set cannot validate.

**Open, and recorded beside the labels:**
- whether a bare price level should be compared with a close, and with what
  tolerance for "near";
- the `k`-suffixed figure.

No label is overruled. The five unflagged errors stand as the labeller judged
them.

**Two qualifications travel with any summary of these figures:**
1. "Every wrong seeded outcome is the signed-conviction reading" is a split
   chosen after scoring. It locates the disagreement. It does not validate
   anything, or establish which reading is right.
2. "Valid context" means passing the checks in `evidence_problems`. It does
   not prove every relation in a synthetic context is valid.

## What this cannot establish

- **It will be two weeks of one system's reports.** Counts describe them, and
  are not a rate for later reports.
- **The labels will be one AI review, not ground truth.** The conventions
  remove two choices from the labeller, but not judgement.
- **It measures the numeric checker only.** The wording grader is not
  evaluated.
- **Fine precision is untested on real notes.** No stored note writes a
  figure between 7 and 15 decimals.

## Amendment: revision 2, 2026-09-26

Made after review of revision 1 (`a145b02`), before any report existed in the
frame. It rests on the reviewer's reproductions and on the runtime's code,
not on anything in the frame.
- **Invalid valid-context evidence.** Revision 1 moved a Quiver signal of 0.8
  to 1.096 in `evidence_moved_up` and `both_moved`, past the runtime's bound
  of ±1. It also let note-only changes into `valid_context` when the note's
  own evidence was incoherent. Now:
  - every valid-context case, and its original evidence, must pass
    `evidence_problems`;
  - evidence changes are limited to the two groups whose dependents are
    reproduced;
  - the RSI, count, support and break risk are no longer moved as a valid
    context, because the runtime derives other fields from them;
  - rejections are counted with reasons.
- **An incomplete frame could be frozen.** The builder accepted whatever dump
  it was given, including an empty one. It now requires the recorded manifest
  and validates the frame against it.
- **The window could move.** Revision 1 allowed moving the close until the
  frame was built. It is now fixed.
- **The checker was pinned by version string only.** It is now pinned by
  source hash too.

Revision 1's protocol id, `fresh-protocol-v2-2026-09-26`, generated nothing:
no frame, labels or cases exist under it.

## Amendment: revision 3, 2026-09-26

Made after review of revision 2 (`7f7885f`), before any report existed in the
frame. Review reproduced that a dump row matching the manifest in id, time and
status, but with a null report and prompt, was accepted. Frame extraction then
counted that report and found no notes in it.

Now each dumped report's contents are validated, telling missing or malformed
payloads apart from an empty candidate list or an absent evidence source. A
test drives this through the same validation and extraction the builder uses.
The absent sources are recorded in the key.

Review asked that the machinery then stop changing. It does: the next step is
to let the window run.
