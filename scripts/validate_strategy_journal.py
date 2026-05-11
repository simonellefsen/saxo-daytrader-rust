from __future__ import annotations

import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.strategy_journal import fetch_recent_journal_learnings, generate_due_strategy_journals


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    connection = connect(":memory:")
    init_db(connection)
    result = generate_due_strategy_journals(
        connection,
        config,
        reference_time=datetime.fromisoformat("2026-05-04T22:45:00+02:00"),
    )
    assert result["status"] == "ok", result
    assert len(result["entries"]) == 1, result
    assert result["entries"][0]["status"] == "completed", result
    duplicate = generate_due_strategy_journals(
        connection,
        config,
        reference_time=datetime.fromisoformat("2026-05-04T22:50:00+02:00"),
    )
    assert duplicate["entries"][0]["status"] == "skipped", duplicate
    learnings = fetch_recent_journal_learnings(connection)
    assert len(learnings) == 1, learnings
    assert learnings[0]["learnings_json"], learnings
    assert learnings[0]["diary_json"], learnings
    assert learnings[0]["diary_json"]["diary"]["executive_summary"], learnings
    print("Strategy journal validation passed.")
    print(f"Journal entries: {len(learnings)}")
    print(f"First cadence: {learnings[0]['cadence']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
