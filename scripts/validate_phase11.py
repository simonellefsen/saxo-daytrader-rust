from __future__ import annotations

import json
import importlib.util
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RENDER_PATH = ROOT / "scripts" / "render_service_templates.py"


def _load_render_main():
    spec = importlib.util.spec_from_file_location("render_service_templates", RENDER_PATH)
    assert spec and spec.loader, f"Unable to load {RENDER_PATH}"
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.main


def main() -> int:
    output_dir = Path("/tmp") / f"saxo_daytrader_phase11_{uuid.uuid4().hex}"
    argv_backup = sys.argv[:]
    sys.argv = [
        "render_service_templates.py",
        "--output-dir",
        str(output_dir),
        "--project-dir",
        str(ROOT),
        "--python-bin",
        str(ROOT / ".venv" / "bin" / "python"),
        "--frontend-port",
        "3000",
        "--api-port",
        "8000",
    ]
    try:
        render_main = _load_render_main()
        result_code = render_main()
    finally:
        sys.argv = argv_backup

    assert result_code == 0, result_code
    scheduler_unit = (output_dir / "systemd" / "saxo-daytrader-scheduler.service").read_text(encoding="utf-8")
    dashboard_unit = (output_dir / "systemd" / "saxo-daytrader-dashboard.service").read_text(encoding="utf-8")
    scheduler_plist = (output_dir / "launchd" / "com.saxo-daytrader.scheduler.plist").read_text(encoding="utf-8")
    dashboard_plist = (output_dir / "launchd" / "com.saxo-daytrader.dashboard.plist").read_text(encoding="utf-8")

    assert "scripts/run_scheduler.py --config" in scheduler_unit
    assert "main.py --no-scheduler --api-port 8000 --frontend-port 3000" in dashboard_unit
    assert "<string>com.saxo-daytrader.scheduler</string>" in scheduler_plist
    assert "<string>com.saxo-daytrader.dashboard</string>" in dashboard_plist
    assert "<string>--frontend-port</string>" in dashboard_plist
    assert "<string>3000</string>" in dashboard_plist

    print("Phase 11 validation passed.")
    print(f"Rendered output dir: {output_dir}")
    print("Rendered files:")
    for path in sorted(output_dir.rglob("*")):
        if path.is_file():
            print(f"- {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
