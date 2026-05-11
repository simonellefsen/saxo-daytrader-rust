from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config


def _should_launch_scheduler(config: dict, *, with_scheduler: bool, no_scheduler: bool) -> bool:
    app_config = config.get("app", {})
    default_enabled = app_config.get(
        "launch_scheduler_with_ui",
        app_config.get("launch_scheduler_with_dashboard", False),
    )
    return bool(with_scheduler or default_enabled) and not no_scheduler


def main() -> int:
    config = load_config(ROOT / "config.yaml")

    default_launch = _should_launch_scheduler(config, with_scheduler=False, no_scheduler=False)
    disabled_launch = _should_launch_scheduler(config, with_scheduler=False, no_scheduler=True)
    explicit_launch = _should_launch_scheduler(
        {**config, "app": {**config.get("app", {}), "launch_scheduler_with_ui": False}},
        with_scheduler=True,
        no_scheduler=False,
    )

    assert default_launch is True, default_launch
    assert disabled_launch is False, disabled_launch
    assert explicit_launch is True, explicit_launch

    print("Phase 21 validation passed.")
    print(f"Scheduler launched by default: {default_launch}")
    print(f"Scheduler disabled explicitly: {not disabled_launch}")
    print(f"Explicit with-scheduler flag: {explicit_launch}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
