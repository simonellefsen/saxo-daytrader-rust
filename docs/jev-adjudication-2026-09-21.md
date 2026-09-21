# Adjudication run — 2026-09-21

Against `docs/jev-adjudication-rubric.md`, frozen 2026-09-21 before any example
was read under it. Grades are v8.

## Method

111 unique (symbol, note) pairs had been graded. Every fourth was taken, giving
28. Each was read against the technical, Markov and Quiver evidence from the
report's own stored prompt, and a category recorded, **before** the grader's
verdict was looked at.

**Blindness, stated exactly.** Four verdicts had been seen earlier in the
session while debugging (#304 CHEMM, #308 ALV, #309 ALV, #311 TSM). Of those,
two fall in this sample: case 24 (#309 ALV) and case 27 (#311 TSM). Both are
marked below. The remaining 26 were genuinely blind. A future run should be
done by someone who has not been debugging the grader.

## Result

| | |
|---|---|
| Exact category agreement | **24 / 28** |
| Both flagged, category differs | 2 (cases 24, 28) |
| Grader flagged, adjudicator did not | 2 (cases 8, 19) |
| Adjudicator flagged, grader did not | **0** |

## The margin separates agreement from disagreement

`margin` is the selected option's probability minus the runner-up's. It was
added because `confidence` measures how peaked the distribution is, not how
likely the selected option is.

| margin | cases | disagreements |
|---|---|---|
| ≤ 0.25 | 5 | **4** |
| > 0.25 | 23 | **0** |

Median margin: **0.66** where we agreed, **0.17** where we did not.

Every disagreement in this sample sat at or below 0.25. That is 28 cases, one
sample, one adjudicator — it is a pattern worth acting on as a review trigger,
not a calibrated threshold. It should be re-derived on the next run rather than
assumed to hold.

## The four disagreements

**Case 8 — #283 TSM, grader `misdescribes_category` (p=0.52, margin 0.20).**
*"strong Markov Sideways-to-Bull conviction (+0.3506)"* with `state=Sideways`,
signal +0.3506, direction long. The note **names Sideways explicitly** and
describes a move toward Bull, which is the situation. Adjudicated `fair`; the
grader's flag reads as a false positive, and it is the most precisely worded
note in the sample.

**Case 19 — #299 CRM, grader `overstated` (p=0.49, margin 0.07).**
*"advancing with +0.559 Bull Markov confirmation, positive Congressional
trading flow"*. Both figures match. The break risk is 0.441, labelled
`moderate`, and the note does not mention it — an omission, not a
misstatement. Adjudicated `fair`. Margin 0.07 is very nearly a tie.

**Case 24 — #309 ALV, grader `misdescribes_category` (p=0.43, margin 0.14).**
*Not blind.* *"consolidating comfortably above 446 EUR support ... and positive
Markov regime"* with `break_risk=0.392/moderate` and `state=Sideways`,
signal +0.2467. Adjudicated **ambiguous, overstated-leaning**: "comfortably"
against a moderate break risk overstates, and "positive Markov regime" may name
the positive signal rather than assert the categorical state. Both of us flagged
it; we disagree on which category, and the rubric says an ambiguous case stays
ambiguous.

**Case 28 — #313 CHEMM, grader `overstated` (p=0.60, margin 0.25).**
*"Top held conviction holding trading above 525 DKK"* with `close=510.0`. The
stock was not trading above 525. Adjudicated a **numerical contradiction**, not
an overstatement. The grader flagged the note; the deterministic checker did
not, and that is the more interesting half.

## What the deterministic layer missed

Case 28 is a false negative in `jev_numeric`. There is no keyword mapping
"trading above" to `daily_indicators.close`, so 525 was never compared to 510.

The obvious fix is wrong. "above X" is a **relational** claim — that the price
exceeds X — and the checker compares for equality. Mapping "trading above" to
`close` and testing equality would report a disagreement for every note saying
"trading above 500" when the close is 510, which is true. Closing this needs a
relational comparison, and the number must be read as a threshold only when no
field keyword already names it: in case 24 the 446 *is* the support level and an
equality check against `nearest_support` is the correct reading.

Left unfixed rather than half-fixed. Rushing exactly this kind of rule is what
produced six false disagreements earlier in the week.

## What this establishes, and what it does not

It establishes that on 28 notes, one adjudicator and the grader agreed on 24,
that the two clear grader false positives both sat at low margin, and that the
grader did not miss anything the adjudicator caught.

It does not establish accuracy on unreviewed notes, that the categories predict
anything about returns, or that any grade should influence a trading decision.
The sample contains one `misdescribes_category` I would accept and one I would
not, which is too few of either to say anything about that category.

No trading parameter changed.
