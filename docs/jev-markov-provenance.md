# Where the missing Markov figures came from

Measured 2026-09-24 at `c80c6d9`, clean tree, over the 233-report dump the
controls were drawn from. Output unedited in
`jev-markov-provenance-results.json`; code in `src/jev_provenance.rs`.

## The question

The controls reconciliation left one question open. Twelve sampled claims
quote a Markov figure while the candidate's grading evidence has `markov` null.
From the instrument alone, nobody could tell whether those figures came from
somewhere in the prompt or were invented.

## The answer

**They were not invented.** Across the whole frame there are 54 claims the
checker attributes to a `markov.*` field while the evidence has no Markov
block, spread over 34 reports. Every one of them is in the prompt the model was
given, for the same symbol and under the same field:

| where found | claims |
|---|---|
| `markov_method.latest_run.summary_json.signals[]`, the embedded run rows | **53** |
| the model's own earlier report (`earlier_same_scope_report`) | 1 |
| anywhere else, or nowhere | 0 |

- **All 54 round to the value found.** None needs the checker's allowance for
  truncation.
- **All 53 embedded rows are current.** Each comes from the same run date as
  the evidence list, and each has status `ok` with no error. Report #199's JPM
  row, for example, rests on 520 samples.

The figures were faithful quotes of valid, current signals. The checker called
them `not_in_evidence` because the **grading evidence reads only one of the two
Markov lists the prompt carried.**

## How sure

- **Control.** All 340 Markov claims that the checker *matched* are found by
  the same search in the evidence list for the same symbol. The search finds
  what it should.
- **Coincidence.** Each target figure was moved 5, 7, 11 and 13 units in its
  last place and searched again. Of 428 perturbed figures, **3 (0.7%)** still
  landed on the same symbol and the same field; the real figures did so 54
  times out of 54. A looser test — any field in the same symbol's run rows —
  catches 23 of 428 (5.4%).

## Why the evidence list left them out

The prompt's `markov_method` block had two lists:
- **`signals`**, the one the grading evidence is built from. Until 2026-09-03,
  it was **alphabetical and cut off** at about 64 to 70 symbols, ending around
  GS or GSK.
- **`latest_run.summary_json.signals`**, up to 20 full signal rows embedded
  for debugging. They were removed from the prompt on 2026-08-03 (`5a19a4a`),
  to cut its size.

**All 54 symbols sort after the end of an alphabetical evidence list:** V (15
claims), ORSTED (14), NNIT (11), LMND (7), NVDA (5), JPM (1) and NESTE (1).
Of the 53 embedded-row claims, 37 are about positions the portfolio held.
Across the 84 reports that carried embedded rows, 594 of 1,680 valid rows
(35%) were for symbols missing from the evidence list. In the 33 of those
reports where a claim resulted, the figure was 225 of 660.

| reports | dates | Markov in the prompt |
|---|---|---|
| #71–#201 | 2026-06-04 to 07-31 | an alphabetical list cut around G, plus 20 embedded rows |
| #202–#270 | 2026-08-03 to 09-03 | the alphabetical list only. **Symbols after the cut had no Markov data in the prompt at all.** |
| #271 onward | from 2026-09-04 | ranked by conviction, holdings first (`47c9731`) |

## The one exception: a figure that belonged to another symbol

Report #260 (2026-09-01 12:15 UTC) quotes NESTE at "+0.2331 Markov". NESTE had
no Markov data in that prompt, because N falls after the alphabetical cut. The
figure appears only in the earlier same-scope report, #259 (08:45 UTC). There
it sits in the model's own `strategy_metadata.markov` for its NESTE trade, and
in its sentiment rationale.

Traced by hand, and checkable from the values:
- #259's prompt gave NESTE no Markov signal either.
- Its evidence list holds **DTE:xetr** at `signed_signal`
  0.23313425481319427, Sideways, run 2026-09-01. That is the exact value #259
  attached to NESTE, identical to all 17 digits.
- The two are different instruments: DTE closed at 28.28 and NESTE at 31.88.

So #259 attached another symbol's signal to NESTE, and #260 read it back as
NESTE's. **This one is wrong, and it was carried forward through the model's
own output.** The grading rule that a report may not supply its own evidence is
why the checker could not have matched it. Both Markov gate paths in the
trading manager look up their own signal by the order's symbol
(`latest_markov_signal(state, &order.symbol)`), not the model's metadata. The
wrong figure reached report text, but not the gate.

## What this changes

- **For the census.** 53 of the 56 `not_in_evidence` claims in the frame are
  these Markov figures. Fifty-two were faithful quotes of what the model was
  shown, and one was a wrong-symbol copy.
- **For the controls.** The 9 sampled `evidence_absent` cases that quote a
  Markov value all trace to the embedded rows. The labeller's `cannot_tell` was
  correct for the evidence supplied. The v4 score is unchanged, and this is
  reported beside it, not in place of it.
- **For the checker.** Nothing yet. The evidence builder could read the
  embedded rows for prompts #71–#201, which would turn these abstentions into
  comparisons. That is a grading-method change, and the method is frozen at
  `n10` / `v16`, so it is not made here.

## What it does not establish

- **Only claims the checker attributes to `markov.*`.** Markov mentions it
  cannot attribute, such as the "5-day" horizon, are outside this measurement.
- **The coincidence rate is from perturbation**, not from an independent model
  of what a report might invent.
- **The ranked-list period is short.** 49 reports have run under it, with no
  such claim so far; that is too few to call the gap closed.
- **Whether the model should have leaned on debugging rows** is a separate
  question from where the figures came from.

## Since: the checker reads the embedded rows (`n11`)

2026-09-24, `0cb1565`, at the operator's request. The grammar is unchanged;
the evidence is wider. For a symbol missing from the compact list, the Markov
evidence now comes from the embedded rows. The list still wins wherever both
carry a symbol. Only rows with status `ok` are read, and a model's earlier
report never is. `markov_source` records which list was used.

**Like for like, exact.** An affected candidate had, under `n10`, the same
evidence with `markov` null, so both sides are rebuilt from the same prompt.
Every other candidate's evidence is byte-identical, and its verdicts cannot
move. Output in `jev-markov-n11-changes.json`, measured from a clean tree.

93 candidates in 50 reports now read the embedded rows. In them:

| `n10` → `n11` | claims |
|---|---|
| `not_in_evidence` → **`matches`** | **51** |
| `not_in_evidence` → `uncertain_attribution` | 1 |
| `not_in_evidence` → `not_in_evidence` | 1 |
| unchanged (`matches` 39, `unattributed` 14, `not_a_field_value` 3, `uncertain_attribution` 1) | 57 |
| → `differs` | **0** |

- **#180 V, "Markov 5-day signal is 0.560"**, becomes
  `uncertain_attribution`. Under `n10` the missing evidence ended the check
  before the grammar ran. With the evidence present, the frozen grammar
  rejects "5-day" between the field and the figure. The stored value 0.5597
  would have matched.
- **#185 V, "0.5651"**, stays `not_in_evidence`. The checker attributes it to
  `nearest_support`, not Markov, so it is an attribution miss, untouched here.
- **#260 NESTE** is not among the 93: its prompt carried no embedded rows. It
  stays `not_in_evidence`, as it should, because the figure was DTE's.

**The frame, before and after:**

| | `n10` | `n11` |
|---|---|---|
| compared (`matches` + `differs`) | 1,162 (80.4%) | **1,213 (83.9%)** |
| `matches` | 1,153 | 1,204 |
| `differs` | 9 | 9 |
| `not_in_evidence` | 56 | **4** |
| `uncertain_attribution` | 48 | 49 |
| never compared | 283 (19.6%) | 232 (16.1%) |

**What this does not change:**
- **The controls.** The instrument and key are frozen at `n10`, and the v4
  result stands as scored. Of the 8 sampled `not_in_evidence` claims, `n11`
  would call 7 `matches` and 1 `uncertain_attribution`. The labeller marked
  all 8 `cannot_tell`, correctly for the evidence it was given. That is
  reported here, beside v4, not folded into it.
- **The wording verdicts.** For those 93 candidates, the stored v16 wording and
  sufficiency answers were given without the Markov block. They are not
  re-asked: grading stays at v16, and a regrade would move the measured
  baseline.
- **Coverage is not correctness.** 51 more comparisons, all agreeing, say the
  quotes were faithful to what the model was shown. They say nothing about
  whether the reports that used them were right.
