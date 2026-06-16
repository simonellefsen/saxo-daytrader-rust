# Markov Method

The Markov method is an advisory regime skill for the Rust daytrader. It runs daily over the current portfolio and watchlist universe, stores one signal row per asset, and exposes the results to the dashboard, API, Hermes context, and AI decision prompts.

Source inspiration:

- [jackson-video-resources/markov-hedge-fund-method](https://github.com/jackson-video-resources/markov-hedge-fund-method)
- [markov-hedge-fund-method.md](https://github.com/jackson-video-resources/markov-hedge-fund-method/blob/main/markov-hedge-fund-method.md)

The linked method describes a module that fetches daily OHLCV, labels each day Bull/Bear/Sideways from rolling returns, estimates a 3x3 transition matrix by maximum-likelihood counts, forecasts with matrix powers, solves a stationary distribution, and reports a signed signal.

## Runtime Contract

- The scheduler runs the skill once per configured local trading day after `strategy.markov.daily_time`.
- The default label rule is a 20-trading-day rolling return:
  - `Bull` when return is `>= +5%`
  - `Bear` when return is `<= -5%`
  - `Sideways` otherwise
- The transition matrix is ordered `Bull`, `Sideways`, `Bear`.
- Forecasts use Chapman-Kolmogorov matrix powers for configured horizons.
- The signed signal is `bull_prob - bear_prob` at `strategy.markov.signal_horizon_days`.
- The signal is advisory only. It does not approve, place, replace, or cancel orders.

## Data Flow

```mermaid
flowchart LR
  S["Scheduler"] --> U["Portfolio + watchlist universe"]
  U --> L["Saxo instrument lookup"]
  L --> C["Daily chart samples"]
  C --> R["Rolling-return regime labels"]
  R --> M["3x3 transition matrix"]
  M --> F["n-step forecasts"]
  M --> D["Stationary distribution"]
  F --> G["Signed signal"]
  G --> DB["markov_asset_signals"]
  DB --> UI["Dashboard/API"]
  DB --> XAI["AI decision prompt context"]
  DB --> H["Hermes context/MCP"]
```

## Storage

The runtime creates:

- `markov_signal_runs`: one daily run summary.
- `markov_asset_signals`: one asset-level result per run.

Each signal stores the current regime, rolling return, transition counts, transition probabilities, forecast distributions, stationary distribution, signed signal, direction, conviction, and any per-asset error.

## Configuration

```yaml
strategy:
  markov:
    enabled: true
    timezone: Europe/Copenhagen
    daily_time: "23:30"
    run_weekdays_only: true
    window_days: 20
    threshold: 0.05
    horizon_minutes: 1440
    sample_count: 520
    min_labeled_days: 60
    signal_horizon_days: 5
    forecast_steps: [1, 2, 3, 5, 10]
    max_symbols: 0
```

`max_symbols: 0` means no local cap. If Saxo rate limits become an issue, set a cap while we add batching or throttling.

## Operator Surfaces

- Dashboard: `/?view=markov`
- API: `/api/markov/signals`
- Hermes MCP tool: `get_markov_signals`
- AI decision prompt context: `markov_method`

## Safety

The Markov skill reads Saxo chart data and writes local analytics rows. It has no broker mutation path and no access to order approval endpoints. A negative signal should be treated as risk-reduction or stand-down context unless a separately reviewed strategy change permits short exposure.
