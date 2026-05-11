from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai import scheduler_service


def main() -> int:
    calls: list[dict] = []
    original_run_scheduler_cycle = scheduler_service.run_scheduler_cycle

    def fake_run_scheduler_cycle(**kwargs):
        calls.append(kwargs)
        return {"status": "ok", "generated_decision": True, "queue": {"status": "ok"}}

    scheduler_service.run_scheduler_cycle = fake_run_scheduler_cycle
    try:
        live = scheduler_service.run_manual_scheduler_cycle(config={}, connection=object(), mock=False)
        mock = scheduler_service.run_manual_scheduler_cycle(config={}, connection=object(), mock=True)
    finally:
        scheduler_service.run_scheduler_cycle = original_run_scheduler_cycle

    assert live["status"] == "ok", live
    assert mock["status"] == "ok", mock
    assert len(calls) == 2, calls
    assert calls[0]["force_mock"] is False and calls[0]["force_decision"] is True, calls
    assert calls[1]["force_mock"] is True and calls[1]["force_decision"] is True, calls

    print("Phase 23 validation passed.")
    print(f"Manual live cycle status: {live['status']}")
    print(f"Manual mock cycle status: {mock['status']}")
    print(f"Mock generate flag observed: {calls[1]['force_mock']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
