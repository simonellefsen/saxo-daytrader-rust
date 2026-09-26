# Fresh evaluation v2: fixed before its frame exists

`fresh-v2-2026-09-26`, protocol `fresh-protocol-v2-2026-09-26`. Code:
`src/jev_fresh/v2.rs`, test-only.

**Status: preregistered; the frame is open, and no report is in it yet.**

## What is frozen

**The method under evaluation is numeric method `n20-2026-09-26`, frozen at
`07cf157`.** Review closed its last findings and advised stopping the tuning
here. Until this set is scored:
- the checker does not change;
- a defect found meanwhile is recorded against `n20`, and a fix would be
  `n21`, which this set could not validate;
- the scorer refuses to run under any method but `n20`.

## The frame

**Every completed report created after `2026-09-26T09:39:03Z`, when `n20` was
frozen, and before `2026-10-10T00:00:00Z`: two full trading weeks.** Every
distinct note those reports attach to a selected symbol, taken whole. It is
prospective: none of these reports exists yet, so none can have shaped the
method.

The close is a default. Simon may move it, but only before the frame is built.
The builder refuses to run before the close, and refuses to run twice. At about
four reports a trading day, expect roughly 40 reports and 200 notes, four times
fresh-v1.

## What changes from fresh-v1, and why

Each change answers a finding from review of fresh-v1.

- **The conventions are stated in the instrument.** fresh-v1's labeller had to
  choose them. Its choice of rounding decided CHEMM's "502", and its reading of
  "conviction" was a disputed attribution. The instrument now carries:
  - **rounding**: a figure written short is consistent only if it is the
    stored value rounded to the precision written, either way at an exact
    half. A truncation rounding does not produce is inconsistent. Simon
    settled this on 2026-09-26.
  - **signed conviction**: beside "conviction", a figure written with a sign
    states `markov.signed_signal`, and an unsigned one states
    `markov.conviction`. This is a labelling convention from the 2026-09-26
    adjudication, not a claim about what past authors meant.
  - **missing evidence**: it is `cannot_tell`, never `inconsistent`.
- **The key's arithmetic follows the rounding convention.** It decides in whole
  units of the last written place. A figure a double cannot settle seeds
  nothing, rather than being keyed by an old convention.
- **Valid-context changes are kept apart from evidence corruption.** fresh-v1
  moved one field at a time, and 37 of its 69 "kept true" cases held evidence
  the Markov model could not produce.
  - **`valid_context`.** A change to the evidence moves every field the model
    links to the one it changes:
    - the signed signal, conviction, bull and bear probabilities and the
      direction together, with the sideways probability held;
    - the support's distance with the support, where the note's evidence
      derives it from the close.

    Every field stays inside its bounds, and any case `context_problems`
    rejects is not generated.
  - **`corruption`.** The single-field change is kept as its own family, and
    only where it breaks the model's links. Its only question is whether the
    checker reads the field it names.
- **A removal takes the field and everything that determines it.** In fresh-v1,
  35 of 69 "unsettleable" removals could still be settled through a linked
  field. Now removing a Markov field removes the whole linked group, and
  removing a linked support removes its distance too.

## The seeded changes

| family | change | direction |
|---|---|---|
| `valid_context` | `note_value_up`, `note_value_down` | true → false |
| `valid_context` | `sign_flipped`, on a signed field | true → false |
| `valid_context` | `other_symbol_value`: another symbol's value for the field, copied in | true → false |
| `valid_context` | `evidence_moved_up`, `evidence_moved_down`: the evidence moved coherently, the note unchanged | true → false |
| `valid_context` | `both_moved`: note and evidence moved together, coherently | stays true |
| `valid_context` | `repaired`: a claim labelled and computed wrong, rewritten to the stored value | false → true |
| `valid_context` | `removed_with_its_links` | unsettleable |
| `valid_context` | `out_of_scope_stop_inserted` | out of scope |
| `corruption` | `single_field_up`, `single_field_down`: one linked field moved alone | true → false |

Anchors come from the labeller, and generation never calls the checker, as in
fresh-v1. Every key is computed by the key's own arithmetic, against the final
note and evidence. A change whose result the arithmetic does not bear out is
not generated.

Some cases are dropped rather than keyed:
- a `both_moved` case whose note also states a linked field, because that
  figure would go stale;
- any change that would leave a field outside its bounds.

## The analysis plan

As fresh-v1:
- the input is valid or nothing is scored;
- nothing is scored while any note is unlabelled;
- note-level outcomes, over the notes the checker read and over every note;
- counts over the frame taken whole, with no rate extrapolated.

The seeded results are reported per family and per change, with the changed
figure apart from the untouched ones. An abstention is neither correct nor
wrong. **The `valid_context` family is the headline. `corruption` is reported
beside it, never pooled with it.**

**Two qualifications travel with any summary**, as review asked of fresh-v1:
1. A result on a subset chosen after scoring locates a disagreement. It does
   not validate anything.
2. "Coherent" means passing the checks in `context_problems`. It does not
   prove every relation in a synthetic context is valid.

## The order of work

1. The frame closes. The dump is taken, and `build_the_fresh_v2_instrument`
   writes the instrument, the key and an empty labels file.
2. The instrument only, with its rubric and conventions, goes to a
   fresh-context labeller. I do not label: I wrote the checker.
3. Labels are committed as delivered.
4. `generate_the_v2_seeded_cases`, then commit.
5. `score_the_fresh_v2_set`, once.
6. Any reconciliation goes in a separate file, beside the labels.

## What this cannot establish

- **It will be two weeks of one system's reports.** Counts describe them, and
  are not a rate for later reports.
- **The labels will be one AI review, not ground truth.** The conventions
  remove two choices from the labeller, but not judgement.
- **It measures the numeric checker only.** The wording grader is not
  evaluated.
- **Fine precision is untested on real notes.** No stored note writes a
  figure between 7 and 15 decimals. The boundary there is correct by
  arithmetic, and this set will exercise it only if such notes appear.
