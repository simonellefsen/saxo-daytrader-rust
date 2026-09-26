# Fresh evaluation v2: fixed before its frame exists

`fresh-v2-2026-09-26`, protocol **`fresh-protocol-v2-2026-09-26-r2`**. Code:
`src/jev_fresh/v2.rs`, test-only.

**Status: preregistered, revision 2. The frame is open, and no report is in it
yet.** The amendment at the end records what revision 2 changed, and why.

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
  manifest.

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
