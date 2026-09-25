# Fresh-v1 blind labeller notes

## Provenance and scope

Independent fresh-context Codex AI reviewer, not human ground truth. All 51 whole notes and all their evidence were read manually, in original order. The only substantive input was `docs/jev-fresh-v1.json`, including its embedded rubric. The empty `docs/jev-fresh-v1-labels.json` was consulted solely as the output template; `/Users/lindau/.codex/RTK.md` solely for shell conventions. An initially truncated instrument display was followed by complete, untruncated reads of cases 1–13, 14–26, 27–39 and 40–51 and the rubric. No keys, scorer/checker source, tests, repository history, wiki, other evaluation material, memory, external source, provider or other reviewer was consulted. No delegation occurred.

Claim selection, mappings and judgments were manual. Code serialized the manually authored labels and checked structure; it did not generate judgments. The accompanying timestamp is an actual UTC time obtained during annotation. These are AI annotations, not a claim of accuracy or agreement.

## Rubric interpretations

- Numeric assertions about indicators remain in scope even when surrounded by holding, execution, or recommendation prose. Qualitative Bull/Sideways labels, rankings such as highest/top, sector descriptions, and favorable/low/strong adjectives are not separately judged.
- Ordinary decimal and percentage claims use rounding to the precision actually written. Probability fields are multiplied by 100 for percentages. Whole-currency support values use nearest-unit rounding, not truncation. For example, 502.89 supports 503, not 502; 428.11 supports 428.
- A literal conviction claim maps to `markov.conviction` (magnitude). A numeric signal/regime/state claim maps to `markov.signed_signal`, not a probability or the qualitative state string. The explicitly directional phrase “negative Markov conviction” is read as signed conviction and maps to `markov.signed_signal`. This contextual reading is a judgment call, documented below.
- Missing support or break-risk evidence is `cannot_tell` with field `none`, never a contradiction. A price exceeding a claimed support price does not by itself establish that technical support exists.
- A generic quoted consolidation price with no explicit alternate timing is compared with `daily_indicators.close` when present; the alternative Markov close is considered too. Explicit intraday/mid-session prices and returns need contemporaneous evidence: differing closes are not automatically contradictions of a separately timed intraday observation. This temporal distinction is an interpretation, not externally verified information.
- “near” is not treated as an exact equality. The approximate trading-price claim below is unresolved because its tolerance is not specified. “near-zero” is retained as a separate number-word comparison: the supplied conviction magnitude 0.0063687 rounds to zero at the integer precision of that phrase and is small on the supplied probability-derived scale. The reference zero is a threshold/comparison, so its field is `none`.
- Each written number is its own claim; overlapping contextual quotes are permitted but the same underlying figure span is never labelled twice. No count/minimum slash notation occurs in these notes. Signs and percent symbols are preserved in figures; the number word `zero` is preserved rather than rewritten as a digit.
- Excluded numeric intentions: fresh-02712374e4 “awaiting Markov conviction above 0.20” and fresh-1fa84ecf3c “awaiting Markov signal expansion above 0.20 threshold” describe desired future gates, not a present evidence claim.
- Excluded holdings/execution/stop/cost/profit figures follow the rubric even if a price field happens to be numerically close. In particular, “Freshly executed ... starter holding at” prices in fresh-4c86903ff6 (413.70 NOK), fresh-978ff05310 (24.18 EUR), and “Executed ... starter holding at” fresh-f05dd4b576 (801 DKK) are execution/holding figures. “trading near 6.71 EUR” in fresh-312af6c9ae is instead an explicit market-price assertion and remains in scope.
- Excluded stops throughout; filled prices 435.06 USD in fresh-79a8be9e87 and 227.23 USD in fresh-a39dbec5d9; shares/commission floor 25 and 6,240 DKK in fresh-b15ea011c2; 40 shares in fresh-b196192ef8; unrealised gains +8% in fresh-505293bb65, +6.1% in fresh-8372feaaeb, and +10.1% in fresh-f513b5a0d5. Intraday market returns are distinct from portfolio unrealised gains and remain in scope.

## Every inconsistent or cannot_tell claim

### fresh-11253ba36c

- “502 DKK support” — `cannot_tell`, field `none`. Daily indicators are null, so there is no support-level evidence. The Markov close 515.5 being above 502 does not prove support at 502.

### fresh-12ed988968

- “technical cushion above 502 DKK” — `cannot_tell`, field `none`. A bare price-above-threshold reading is supported by close 515.5, but an established technical-support/cushion reading is also defensible and needs the missing technical evidence.

### fresh-296fde72d8

- “0.236 break risk” — `cannot_tell`, field `none`. Daily indicators are null; neither Markov nor Quiver supplies support break risk.

### fresh-312af6c9ae

- “trading near 6.71 EUR” — `cannot_tell`, field `markov.close`. The available close is 6.771999835968018, which rounds to 6.77 rather than 6.71. But “near” permits an approximate reading, and its tolerance is not specified. I did not silently convert this into an exact price claim or exclude it as an execution.

### fresh-3a4fe3c8f2

- “break risk (0.343)” — `cannot_tell`, field `none`. Daily indicators and the risk value are absent.
- “776 DKK support” — `cannot_tell`, field `none`. No support level is supplied. The Markov close 797 is not evidence of where technical support lies.

### fresh-74282c98e4

- “advancing +2.08% intraday” — `cannot_tell`, field `none`. The supplied evidence lacks an intraday return and matching reference/current quotes.
- “intraday to 842.40 DKK” — `cannot_tell`, field `none`. Markov close is 829.2000122070312, but the explicitly intraday quote is a potentially differently timed observation absent from the evidence. A different historical close does not refute it.

### fresh-7e5e21362e

- “consolidation at 515.5 DKK” — `inconsistent`, field `daily_indicators.close`. Interpreted as a factual market-price level against the supplied snapshot, with no explicit alternate intraday time. Daily close is 515.0 and Markov close is 516.0; neither yields 515.5 at one decimal. I did not average the two differently sourced closes to manufacture support. Reading this as an unprovided later quote would change the evidential question; that timing is not stated.
- “502 DKK support” — `inconsistent`, field `daily_indicators.support.nearest_support`. Nearest support is 502.89 and next support 473.84. Nearest-unit rounding gives 503, not 502. Truncating 502.89 to 502 is not the rounding interpretation adopted.

### fresh-9e662171fc

- “mid-session pullback to 34.23 EUR” — `cannot_tell`, field `none`. Daily close 35.82 and Markov close 35.220001220703125 do not supply the explicitly mid-session quote or prove that it did not occur. Stop 33.59 EUR is separately out of scope.

### fresh-b15ea011c2

- “support above 391 NOK” — `cannot_tell`, field `none`. This is a threshold comparison of a technical support level. Daily indicators are absent; the close 413.7999877929687 does not locate that support.

### fresh-bce6065f20

- “+2.30% intraday advance” — `cannot_tell`, field `none`. No intraday return or time-matched start/end quotes are supplied. The two evidence close fields are not labelled as the appropriate intraday endpoints.

### fresh-e2f6a33d7a

- “6 confluences” — `inconsistent`, field `daily_indicators.confluence_count`. The supplied count is 5. The distinct support touch count 6 does not establish six confluences.

### fresh-f513b5a0d5

- “gaining +2.05% intraday” — `cannot_tell`, field `none`. No intraday return or appropriate matched reference/current prices are supplied.
- “intraday to 35.80 EUR” — `cannot_tell`, field `none`. Daily close 34.23 and Markov close 35.130001068115234 are not the stated intraday quote. A different-time price is missing evidence, not a contradiction.

### fresh-f6c714e6bb

- “break risk (0.638)” — `cannot_tell`, field `none`. Daily indicators are null, so support break risk cannot be established.

### fresh-f7fd7b58b5

- “support break risk (0.638)” — `cannot_tell`, field `none`. Daily indicators are null, so support break risk cannot be established.

## Other explicit ambiguity judgments

- fresh-e2f6a33d7a, “negative Markov conviction (-0.124)” — `consistent` as `markov.signed_signal`, which is -0.1242300420999527. The literal magnitude field `markov.conviction` is +0.1242300420999527, but “negative” makes a directional reading the intended one in my judgment. I did not count the same figure again as a magnitude error.
- fresh-e950b8c436, “Markov conviction (+0.006)” — `consistent` as the explicitly named magnitude field, 0.006368707865476608. The negative signed signal is a different quantity; the note does not call this positive/long/bullish conviction.
- fresh-e950b8c436, “near-zero Markov conviction” — `consistent`, figure `zero`, field `none`. This is included because the rubric explicitly includes number words. It is a coarse comparison to zero, not an assertion that the stored magnitude equals zero exactly.
- fresh-10688ff154, “+0.745 Markov Bull regime”, and analogous Bull wording beside numerical Markov claims: only the number is labelled. A qualitative Bull/Sideways disagreement does not change a supported numeric value under the numeric-only rubric.

## Every zero-claim note

- fresh-47fe52e6dd: “High Quiver interest and constructive price momentum; awaiting lower break risk before entry.” Entire note reviewed. It contains no numeric value or number word; qualitative interest/momentum and entry intention are outside this numeric task.
- fresh-52fa6618fd: “Positive congressional inflows and constructive momentum; awaiting support break-risk compression before proposing starter capital.” Entire note reviewed. It contains no numeric value or number word; qualitative inflows/momentum and capital intention are outside this numeric task.

## Structural validation

Validation checks parseable JSON, exact schema/types/enums, version, reviewer provenance and UTC timestamp; all 51 original IDs exactly once in original order; status labelled for every note including empty claim lists; exact unique quote occurrence in the source note; exact figure occurrence once as a complete number/number word within its quote; no duplicated or overlapping underlying figure spans; and every non-none field resolving to a finite numeric value in that note's evidence. Validation does not score label correctness.

Validation result: PASS. All listed checks passed for 51 notes and 90 claims; 75 non-none field paths resolve to finite numbers. Every inconsistent/cannot_tell claim and both zero-claim notes have an exact-quote rationale above. No scorer or repository test was run.

Final totals: 51 reviewed notes, 90 claims, 72 consistent, 3 inconsistent, 15 cannot_tell, and 2 zero-claim notes. These are annotation counts only, not accuracy or agreement estimates.
