from __future__ import annotations

import json
import sys
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.analysis_pulses import analysis_pulse_status
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.trading_manager import run_trading_manager_cycle


def _market_rows(*, tradable: bool = True) -> list[dict[str, object]]:
    return [
        {
            "code": "XCSE",
            "market": "Copenhagen",
            "is_tradable": tradable,
            "session_open_at_utc": "2026-05-04T07:00:00+00:00",
            "tradable_close_at_utc": "2026-05-04T14:55:00+00:00",
        },
        {
            "code": "XETR",
            "market": "Frankfurt / Xetra",
            "is_tradable": tradable,
            "session_open_at_utc": "2026-05-04T07:00:00+00:00",
            "tradable_close_at_utc": "2026-05-04T15:20:00+00:00",
        },
        {
            "code": "XNAS",
            "market": "Nasdaq US",
            "is_tradable": tradable,
            "session_open_at_utc": "2026-05-04T13:30:00+00:00",
            "tradable_close_at_utc": "2026-05-04T20:00:00+00:00",
        },
    ]


def _pulse_by_kind(summary: dict[str, object], kind: str) -> dict[str, object]:
    for pulse in summary["pulses"]:  # type: ignore[index]
        if pulse["kind"] == kind:
            return pulse
    raise AssertionError(f"Missing pulse {kind}: {summary}")


def _insert_completed_report(connection, pulse: dict[str, object]) -> None:
    connection.execute(
        """
        INSERT INTO decision_reports (
            created_at, report_date, batch_id, model, status, analysis_window_active,
            response_id, prompt_text, request_json, response_json, report_json, error_text,
            analysis_pulse_key, analysis_pulse_label
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            "2026-05-04T08:16:00+00:00",
            "2026-05-04",
            None,
            "mock",
            "completed",
            1,
            None,
            "{}",
            "{}",
            "{}",
            json.dumps(
                {
                    "analysis_pulse": pulse,
                    "strategy_plan": {"mode": "swing", "swing_orders": []},
                    "suggested_trades": [],
                },
                sort_keys=True,
            ),
            None,
            str(pulse["key"]),
            str(pulse["label"]),
        ),
    )
    connection.commit()


def main() -> int:
    config = load_config("config.yaml")
    rows = _market_rows()

    eu_due = analysis_pulse_status(
        config,
        rows,
        reference_time=datetime(2026, 5, 4, 8, 15, tzinfo=UTC),
    )
    assert len(eu_due["pulses"]) == 2, eu_due
    assert eu_due["active_pulses"][0]["kind"] == "europe_open_followup", eu_due
    assert eu_due["active_pulses"][0]["target_at_utc"] == "2026-05-04T08:15:00+00:00", eu_due
    assert _pulse_by_kind(eu_due, "us_open_followup")["target_at_utc"] == "2026-05-04T14:45:00+00:00", eu_due

    us_due = analysis_pulse_status(
        config,
        rows,
        reference_time=datetime(2026, 5, 4, 14, 45, tzinfo=UTC),
    )
    assert len(us_due["pulses"]) == 2, us_due
    assert us_due["active_pulses"][0]["kind"] == "us_open_followup", us_due

    connection = connect(":memory:")
    init_db(connection)
    try:
        no_report = run_trading_manager_cycle(
            config=config,
            connection=connection,
            market_status_rows=rows,
            reference_time=datetime(2026, 5, 4, 8, 15, tzinfo=UTC),
        )
        assert no_report["status"] == "skipped_no_completed_report", no_report
        assert no_report["skipped_pulses"][0]["kind"] == "europe_open_followup", no_report

        _insert_completed_report(connection, eu_due["active_pulses"][0])
        matched_report = run_trading_manager_cycle(
            config=config,
            connection=connection,
            market_status_rows=_market_rows(tradable=False),
            reference_time=datetime(2026, 5, 4, 8, 16, tzinfo=UTC),
        )
        assert matched_report["status"] == "ok", matched_report
        assert matched_report["runs"][0]["status"] == "skipped_market_closed", matched_report
        assert matched_report["runs"][0]["pulse"]["decision_pulse_key"] == eu_due["active_pulses"][0]["key"], matched_report
    finally:
        connection.close()

    print("Phase 43 validation passed.")
    print("Decision reports are limited to Nordic/EU +1h15 and US +1h15 pulses.")
    print("Trading Manager waits for the completed report matching the pulse.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
