---
type: source-note
tags:
  - daytrader/wiki
  - research-source
  - hermes
updated: 2026-07-25
sources:
  - https://www.appeconomyinsights.com/
  - https://www.appeconomyinsights.com/p/6-charts-before-you-buy
---

# App Economy Insights Source Note

App Economy Insights is a Substack publication focused on business and earnings breakdowns. Its public material emphasizes revenue growth, margin trends, cash flow and capital expenditure, debt and leverage, price versus fundamentals, valuation expectations, and peer comparison. Some articles are public while premium content exposes only a preview, so any integration must retain the source URL, publication time, access state, and extraction boundary.

## Intended Use

- Use only public, attributable content as read-only external editorial research for matching holdings or configured-watchlist symbols.
- Extract only compact, dated factual claims or clearly marked editorial observations: revenue/growth, margin, free-cash-flow, capital-expenditure, leverage, guidance, catalysts, and risks.
- Supply bounded evidence cards to Decision Reports and Hermes beside existing Markov, technical, Quiver, and broker-safe context.
- Keep author disclosures, publication time, and premium-preview status visible where available.

## Safety Boundary

- This is a secondary editorial source, not broker data, a valuation authority, or an order signal.
- It must not directly open, size, block, amend, cancel, or place a Saxo order or become a Trading Manager gate without a separately measured one-variable proposal and SIM evidence.
- Do not scrape or bypass paywalls, retain full premium text, persist raw provider markup, or ingest credentials.
- Treat extracted claims as unverified until independently corroborated; only source attribution and bounded summaries enter Hermes context.

## Initial Ingestion Shape

1. Register a source through a versioned source catalog or its public RSS/Atom feed where available. The initial Rust implementation uses the public App Economy Insights RSS feed only.
2. Poll on a low-frequency, cached schedule with timeout and publication-date deduplication. The initial implementation refreshes each source no more often than every four hours and prunes persisted source runs/items after 90 days.
3. Match candidates only to configured universe symbols and current broker positions; preserve unmatched posts for operator search rather than guessing ticker mappings.
4. Persist a sanitized evidence card with source, URL, published time, matched symbol, access level, extract type, compact summary, and confidence.
5. Expose the cards as advisory-only research in the Decision Report and Hermes context. Add Watchlist cards and report-consumption tracking only after the persisted evidence is observed in production.
