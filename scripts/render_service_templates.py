from __future__ import annotations

import argparse
import getpass
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


TEMPLATES = {
    "systemd/saxo-daytrader-scheduler.service": ROOT / "deploy" / "systemd" / "saxo-daytrader-scheduler.service.tmpl",
    "systemd/saxo-daytrader-dashboard.service": ROOT / "deploy" / "systemd" / "saxo-daytrader-dashboard.service.tmpl",
    "launchd/com.saxo-daytrader.scheduler.plist": ROOT / "deploy" / "launchd" / "com.saxo-daytrader.scheduler.plist.tmpl",
    "launchd/com.saxo-daytrader.dashboard.plist": ROOT / "deploy" / "launchd" / "com.saxo-daytrader.dashboard.plist.tmpl",
}


def render_template(text: str, values: dict[str, str]) -> str:
    rendered = text
    for key, value in values.items():
        rendered = rendered.replace(f"{{{{{key}}}}}", value)
    return rendered


def main() -> int:
    parser = argparse.ArgumentParser(description="Render systemd and launchd service templates for this workspace.")
    parser.add_argument("--output-dir", default=str(ROOT / "deploy" / "rendered"), help="Output directory for rendered files")
    parser.add_argument("--project-dir", default=str(ROOT), help="Absolute project directory to embed")
    parser.add_argument("--python-bin", default=str(ROOT / ".venv" / "bin" / "python"), help="Python executable to embed")
    parser.add_argument("--user", default=getpass.getuser(), help="User to embed in systemd units")
    parser.add_argument("--frontend-port", default="3000", help="Frontend port")
    parser.add_argument("--api-port", default="8000", help="API port")
    args = parser.parse_args()

    output_dir = Path(args.output_dir).expanduser().resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    (Path(args.project_dir).expanduser().resolve() / "logs").mkdir(parents=True, exist_ok=True)

    values = {
        "PROJECT_DIR": str(Path(args.project_dir).expanduser().resolve()),
        "PYTHON_BIN": str(Path(args.python_bin).expanduser().resolve()),
        "USER": args.user,
        "FRONTEND_PORT": str(args.frontend_port),
        "API_PORT": str(args.api_port),
    }

    rendered_files: dict[str, str] = {}
    for relative_target, template_path in TEMPLATES.items():
        target_path = output_dir / relative_target
        target_path.parent.mkdir(parents=True, exist_ok=True)
        rendered = render_template(template_path.read_text(encoding="utf-8"), values)
        target_path.write_text(rendered, encoding="utf-8")
        rendered_files[relative_target] = str(target_path)

    print(json.dumps({"status": "ok", "output_dir": str(output_dir), "files": rendered_files}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
