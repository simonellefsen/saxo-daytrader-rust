---
type: source-note
tags:
  - daytrader/wiki
  - markov-method
  - maintained-by-llm
updated: 2026-05-23
source:
  title: markov-hedge-fund-method
  url: https://github.com/jackson-video-resources/markov-hedge-fund-method
---

# Markov Hedge Fund Method

Source repository: [jackson-video-resources/markov-hedge-fund-method](https://github.com/jackson-video-resources/markov-hedge-fund-method)

Primary method document: [markov-hedge-fund-method.md](https://github.com/jackson-video-resources/markov-hedge-fund-method/blob/main/markov-hedge-fund-method.md)

The method is an observable three-state Markov regime model:

- Fetch daily price history.
- Label each day as Bull, Bear, or Sideways from a rolling-return threshold.
- Estimate the 3x3 transition matrix by counting state-to-state transitions and normalizing rows.
- Forecast multiple steps by raising the transition matrix to powers.
- Compute a stationary distribution as the long-run regime mix.
- Emit `bull_prob - bear_prob` as signed direction and conviction.

Project adaptation:

- Implemented in Rust as a scheduler-owned advisory skill.
- Uses Saxo chart samples instead of yfinance.
- Runs over portfolio and watchlist assets.
- Stores audited rows in `markov_signal_runs` and `markov_asset_signals`.
- Feeds the dashboard, API, Hermes context/MCP, and AI decision prompt context.
- Does not mutate orders or bypass existing approval gates.
