from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.scheduler_service import run_scheduler_cycle, run_scheduler_forever


def main() -> int:
    parser = argparse.ArgumentParser(description="Run the Phase 5 scheduler for saxo-daytrader-xai.")
    parser.add_argument("--config", default=str(ROOT / "config.yaml"), help="Path to config.yaml")
    parser.add_argument("--once", action="store_true", help="Run exactly one scheduler cycle and exit")
    parser.add_argument("--mock-decisions", action="store_true", help="Force mock xAI decision generation")
    parser.add_argument("--force-decision", action="store_true", help="Generate a decision report even outside analysis windows")
    args = parser.parse_args()

    if args.once:
        result = run_scheduler_cycle(
            config_path=args.config,
            force_mock=args.mock_decisions,
            force_decision=args.force_decision,
        )
        print(json.dumps(result, indent=2, ensure_ascii=False, sort_keys=True))
        return 0 if result["status"] == "ok" else 1

    run_scheduler_forever(
        config_path=args.config,
        force_mock=args.mock_decisions,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
