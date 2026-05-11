from __future__ import annotations

import os
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

import main as launcher_main
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.scheduler_service import assess_scheduler_worker_health


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    now = datetime.now(UTC)

    healthy = assess_scheduler_worker_health(
        {
            "last_heartbeat_at": now.isoformat(timespec="seconds"),
            "scheduler_pid": os.getpid(),
        },
        poll_interval_minutes=int(config["scheduler"]["poll_interval_minutes"]),
        reference_time=now,
    )
    stale = assess_scheduler_worker_health(
        {
            "last_heartbeat_at": (now - timedelta(minutes=45)).isoformat(timespec="seconds"),
            "scheduler_pid": os.getpid(),
        },
        poll_interval_minutes=int(config["scheduler"]["poll_interval_minutes"]),
        reference_time=now,
    )
    dead = assess_scheduler_worker_health(
        {
            "last_heartbeat_at": now.isoformat(timespec="seconds"),
            "scheduler_pid": 999999,
        },
        poll_interval_minutes=int(config["scheduler"]["poll_interval_minutes"]),
        reference_time=now,
    )

    restart_enabled = launcher_main._scheduler_restart_enabled(config)
    max_restarts = launcher_main._scheduler_max_restarts(config)
    restart_delay = launcher_main._scheduler_restart_delay_seconds(config)

    assert healthy["status"] == "healthy", healthy
    assert healthy["restart_recommended"] is False, healthy
    assert stale["status"] == "stale", stale
    assert stale["restart_recommended"] is True, stale
    assert dead["status"] == "dead", dead
    assert dead["restart_recommended"] is True, dead
    assert restart_enabled is True, restart_enabled
    assert max_restarts == 3, max_restarts
    assert restart_delay == 2.0, restart_delay

    print("Phase 26 validation passed.")
    print(f"Healthy status: {healthy['status']}")
    print(f"Stale status: {stale['status']}")
    print(f"Dead status: {dead['status']}")
    print(f"Restart budget enabled: {restart_enabled}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
