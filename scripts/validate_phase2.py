from __future__ import annotations

import sys
from datetime import UTC, datetime
from pathlib import Path

import pytz

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.market_data import fetch_live_prices
from saxo_daytrader_xai.market_news import fetch_market_intelligence
from saxo_daytrader_xai.market_schedule import get_market_status, summarize_analysis_window
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions, fetch_portfolio_symbols, fetch_portfolio_summary
from saxo_daytrader_xai.watchlists import build_watchlists


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    summary = fetch_portfolio_summary(connection, batch_id=result.batch_id)
    positions = fetch_portfolio_positions(connection, batch_id=result.batch_id)
    symbols = fetch_portfolio_symbols(connection, batch_id=result.batch_id)
    excluded_symbols = set(config["risk"]["excluded_symbols"])

    if config["portfolio"].get("source_csv"):
        assert result.source_positions == 20, f"Expected 20 source positions, got {result.source_positions}"
        assert result.excluded_positions == 2, f"Expected 2 excluded positions, got {result.excluded_positions}"
        assert summary["position_count"] == 18, f"Expected 18 active DB positions, got {summary['position_count']}"
    else:
        assert result.source_positions == 0, f"Expected empty source import, got {result.source_positions}"
        assert summary["position_count"] == 0, f"Expected empty post-reset portfolio, got {summary['position_count']}"
    assert set(symbols).isdisjoint(excluded_symbols), "Excluded symbols leaked into the active portfolio"

    watchlists = build_watchlists(config)
    category_by_key = {row["key"]: row for row in watchlists["categories"]}
    assert "uk" in category_by_key, "Missing UK watchlist category"
    assert "us" in category_by_key, "Missing US watchlist category"
    assert "eu" in category_by_key, "Missing EU watchlist category"
    assert len(watchlists["nordic"]) == min(
        config["market_data"]["watchlists"]["nordic_limit"],
        category_by_key["nordic"]["total_universe"],
    ), f"Unexpected Nordic watchlist size: {len(watchlists['nordic'])}"
    assert 0 < len(watchlists["global"]) <= config["market_data"]["watchlists"]["global_limit"], (
        f"Unexpected global watchlist size: {len(watchlists['global'])}"
    )
    assert all(
        row["symbol"] not in excluded_symbols
        for category in watchlists["categories"]
        for row in category["items"]
    )

    sample_quotes = fetch_live_prices(symbols[:3], timeout_seconds=config["market_data"]["request_timeout_seconds"])
    assert len(sample_quotes) == min(3, len(symbols))

    holiday_reference = pytz.timezone("Europe/Copenhagen").localize(datetime(2026, 4, 6, 10, 5))
    holiday_rows = get_market_status(config, reference_time=holiday_reference.astimezone(UTC))
    holiday_lookup = {row["code"]: row for row in holiday_rows}
    assert holiday_lookup["XCSE"]["is_open"] is False, "Expected Copenhagen to be closed on Easter Monday"
    assert holiday_lookup["XCSE"]["holiday_name"] == "Easter Monday"
    assert holiday_lookup["XOSL"]["is_open"] is False, "Expected Oslo to be closed on Easter Monday"
    assert holiday_lookup["XOSL"]["holiday_name"] == "Easter Monday"
    assert holiday_lookup["XCSE"]["calendar_source"] == "exchange_calendars"
    assert holiday_lookup["XOSL"]["calendar_source"] == "exchange_calendars"

    copenhagen_open_day = pytz.timezone("Europe/Copenhagen").localize(datetime(2026, 4, 7, 10, 5))
    market_status_rows = get_market_status(config, reference_time=copenhagen_open_day.astimezone(UTC))
    analysis_summary = summarize_analysis_window(market_status_rows)
    assert analysis_summary["analysis_window_active"], "Expected an active analysis window on a normal trading day"
    assert "Copenhagen" in analysis_summary["active_markets"], "Expected Copenhagen to be active on 2026-04-07"

    pre_dst = pytz.timezone("Europe/Copenhagen").localize(datetime(2026, 3, 27, 10, 5))
    post_dst = pytz.timezone("Europe/Copenhagen").localize(datetime(2026, 3, 30, 10, 5))
    pre_dst_lookup = {row["code"]: row for row in get_market_status(config, reference_time=pre_dst.astimezone(UTC))}
    post_dst_lookup = {row["code"]: row for row in get_market_status(config, reference_time=post_dst.astimezone(UTC))}
    assert pre_dst_lookup["XCSE"]["session_open_local"].endswith("09:00")
    assert post_dst_lookup["XCSE"]["session_open_local"].endswith("09:00")
    assert pre_dst_lookup["XCSE"]["session_open_utc"].endswith("08:00")
    assert post_dst_lookup["XCSE"]["session_open_utc"].endswith("07:00")

    intelligence = fetch_market_intelligence(
        config,
        portfolio_symbols=symbols[:8],
        watchlist_symbols=[row["symbol"] for row in watchlists["global"][:6]],
    )

    print("Phase 2 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Active positions in DB: {summary['position_count']}")
    print(f"Excluded symbols: {', '.join(sorted(excluded_symbols))}")
    print(f"Nordic watchlist size: {len(watchlists['nordic'])}")
    print(f"Global watchlist size: {len(watchlists['global'])}")
    print(f"Analysis window active exchanges: {', '.join(analysis_summary['active_markets'])}")
    print("Sample quotes:")
    for row in sample_quotes:
        print(f"- {row['symbol']}: status={row['status']} price={row['current_price']} change={row['change_pct']}")
    print(f"Market news items: {len(intelligence['market_news'])}")
    print(f"Macro event items: {len(intelligence['macro_events'])}")
    print(f"Earnings events: {len(intelligence['earnings_calendar'])}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
