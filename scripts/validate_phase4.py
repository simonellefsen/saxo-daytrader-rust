from __future__ import annotations

import argparse
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.xai_decision import fetch_latest_decision_report, generate_decision_report


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate Phase 4 xAI decision report generation.")
    parser.add_argument("--live", action="store_true", help="Use the real xAI API instead of mock mode.")
    args = parser.parse_args()

    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase4_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    decision = generate_decision_report(
        config=config,
        connection=connection,
        force_mock=not args.live,
    )
    stored_report = fetch_latest_decision_report(connection)
    report_json = stored_report["report_json"] if stored_report else None

    assert stored_report is not None, "Decision report was not stored"
    assert report_json is not None, "Stored decision report payload is missing"
    assert "reasoning_steps" in report_json and report_json["reasoning_steps"], "Missing reasoning steps"
    assert "suggested_trades" in report_json, "Missing suggested trades"
    assert stored_report["status"] in {"completed", "failed", "xai_fallback"}, f"Unexpected status {stored_report['status']}"

    print("Phase 4 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Decision report status: {stored_report['status']}")
    print(f"Decision report id: {stored_report['id']}")
    print(f"Model: {stored_report['model']}")
    print(f"Reasoning steps: {len(report_json['reasoning_steps'])}")
    print(f"Suggested trades: {len(report_json['suggested_trades'])}")
    print(f"Used live API: {args.live}")
    if stored_report.get("error_text"):
        print(f"Error text: {stored_report['error_text']}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
