from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions, fetch_portfolio_summary


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    summary = fetch_portfolio_summary(connection, batch_id=result.batch_id)
    positions = fetch_portfolio_positions(connection, batch_id=result.batch_id)
    symbols = {row["symbol"] for row in positions}
    excluded_symbols = set(config["risk"]["excluded_symbols"])

    assert result.source_positions == 20, f"Expected 20 source positions, got {result.source_positions}"
    assert result.excluded_positions == 2, f"Expected 2 excluded positions, got {result.excluded_positions}"
    assert result.imported_positions == 18, f"Expected 18 imported positions, got {result.imported_positions}"
    assert summary["position_count"] == 18, f"Expected 18 active DB positions, got {summary['position_count']}"
    assert symbols.isdisjoint(excluded_symbols), "Excluded symbols leaked into the active portfolio"

    print("Phase 1 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Active positions in DB: {summary['position_count']}")
    print(f"Excluded symbols: {', '.join(sorted(excluded_symbols))}")
    print(f"Active portfolio market value DKK: {summary['total_market_value_dkk']:.2f}")
    print("Top 5 holdings by market value:")
    for row in positions[:5]:
        print(f"- {row['symbol']}: {row['market_value_dkk']:.2f} DKK")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
