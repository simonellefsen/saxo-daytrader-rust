# Adjudication: what a signed "conviction" figure names

2026-09-26. It settles the question the fresh evaluation left open
(`jev-fresh-v1-reconciliation.json`, claim `fresh-e2f6a33d7a#1`):

> #360 NOKIA: "Solid technical base with 6 confluences, currently gated due to
> negative Markov conviction (-0.124)."

The labeller mapped the figure to `markov.signed_signal`. The checker, `n15`
and `n16` alike, compared it with `markov.conviction`. The evidence holds
-0.1242300420999527 and 0.1242300420999527 respectively. Both readings accept
the note as written, so the question is which field the figure names, not
whether it is true.

**Who decided, and what that is worth.** Claude, the author of the checker, at
Simon's request. That is not independent. The ruling goes *against* the
checker's current rule, so it cannot flatter the checker's scores, and every
step below cites a primary source. It sits beside the labels, the
reconciliation and the recorded score, and edits none of them.

## The evidence

1. **The model defines conviction as a magnitude.** `markov_method.rs:547`
   computes `signed_signal` as the bull probability less the bear, and
   `conviction` as its absolute value. Conviction is never negative. A negative
   figure can only be the signed signal.
2. **The gate the note cites reads the signed signal.** "Currently gated"
   refers to the Markov gate, `trading_manager.rs:4907`. It passes a candidate
   only if `direction == "long"` and `signed_signal ≥ min_signed_signal`, which
   is 0.20 in both config files. NOKIA's signal is -0.124 and short, so it is
   gated. The conviction field plays no part in that gate.
3. **The report model is told about the signed signal, and never about
   conviction.** Report #360's system prompt says a starter BUY needs "a fresh
   signal with direction long and signed_signal at or above 0.20". It also
   says that a positive bull-minus-bear signal supports a long bias and a
   negative one supports reducing risk. It never mentions conviction. The
   payload passes both as raw fields in each Markov row: `"conviction":
   0.1242…` beside `"signed_signal": -0.1242…`. So "conviction" in a note is
   the report model's own word, not a defined term. The quantity it was told
   gates a trade is the signed signal.
4. **The report model's usage is the signed signal.** The same prompt carries
   its earlier rationales, which write "+0.216 Markov conviction", "+0.429
   Markov conviction" and "+0.599 Markov conviction". Across the development
   dump and the fresh frame, the checker gives 30 figures to
   `markov.conviction`.
   - Three are not conviction values at all: two prices (#312 and #313 CHEMM,
     "525 dkk") and a threshold (#360 BAVA, "awaiting markov conviction above
     0.20", the gate's 0.20).
   - Of the other 27, **26 are written with an explicit sign**. A magnitude
     never needs one.
   - **25 of those 26 signs agree with the signed signal's direction**: 24 plus
     signs on positive signals, and NOKIA's minus on a negative one.
   - The one that does not is #358 UBER, below.
   - One figure is unsigned: #172 ADS, "weak markov conviction at 0.055", on a
     positive signal.
5. **The labeller's rule sits between the two.** Its notes read "conviction" as
   the magnitude unless a directional word qualifies it. So "*negative* Markov
   conviction (-0.124)" is the signed signal to it, and "Markov conviction
   (+0.006)" is the magnitude. That is a defensible reading, recorded in its own
   words in `jev-fresh-v1-labeller-notes.md`.

## The ruling

**In these notes, "Markov conviction" is the report model's name for the Markov
signal. A figure written with a sign states `markov.signed_signal`. An unsigned
figure states the magnitude, which `markov.conviction` holds.**

The written sign decides, because the sign is exactly the information that
tells the two fields apart. A reader does not have to judge which adjectives
count as directional. And the rule never reads more into a figure than it
writes: an unsigned figure is compared as the magnitude, where both fields
agree.

**NOKIA's "-0.124" states `markov.signed_signal`, and is consistent.** The
labeller's field is right. The checker compared the right value through the
wrong field.

## What follows for the fresh evaluation

Nothing is rescored, and no label changes. As description:

- **The natural `wrong_field` is the checker's error.** It does not change a
  verdict.
- **The nine seeded outcomes on the NOKIA anchor are keyed to the right field.**
  They remain one anchor, not nine independent findings.
  - The three evidence-side ones, `both_moved`, `evidence_value_up` and
    `evidence_value_down`, hold evidence the model cannot produce. They say
    nothing about a coherent market context.
  - `note_value_up`, `note_value_down` and `other_symbol_value` were flagged,
    but against conviction: detected, on the wrong field.
  - **`sign_flipped`, "(+0.124)" for a -0.124 signal, was accepted. Under this
    ruling that is a genuine miss:** the checker compares conviction by
    absolute value, so it discards a written sign it attributes there.
  - In `evidence_removed` the signed signal was removed, but it is still
    derivable as bull less bear. The checker matched conviction's equal
    magnitude: the right value, through the wrong field.
  - The ninth outcome is the same figure, untouched, in the note's `repaired`
    case. It was accepted through conviction, again the right verdict on the
    wrong field.
- **#358 UBER, "near-zero Markov conviction (+0.006)", now reads as a sign
  error.** The signed signal is -0.0064, the direction short, the state Bear.
  Under this ruling the figure states the signed signal, and its sign is wrong.
  - It is negligible in size.
  - It is the only written sign across both frames that contradicts the
    signal.
  - The labeller read it as the magnitude, consistent, and the checker
    accepted it, so neither the score nor the reconciliation could surface it.
  - It is recorded here beside the label, which stands as delivered.

As review cautioned: this locates the disagreement and settles a term. It does
not validate the checker, and "no wrong decision without NOKIA" remains a
post-hoc subset result.

## What it implies, not done

**The checker's rule for conviction discards a written sign.** Under this
ruling, a signed figure beside "conviction" should be compared with the signed
signal, so that a wrong sign is flagged. That would be a new method version.
Review's advice is to stop tuning here, so it is not built. Whether to build
it is Simon's decision. If built, only a later fresh set could evaluate it.

**The next fresh evaluation's rubric should state this rule**, so a labeller
applies it rather than choosing one. The truncation convention, which has now
decided a case in each of three instruments, is the other term to settle
before then. It is a policy choice, and it is Simon's to make.
