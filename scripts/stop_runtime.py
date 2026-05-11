from __future__ import annotations

import contextlib
import os
import signal
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUNTIME_DIR = ROOT / ".run"
RUNTIME_MARKERS = (
    str(ROOT / "main.py"),
    str(ROOT / "web_main.py"),
    str(ROOT / "scripts" / "run_scheduler.py"),
    str(ROOT / "src" / "saxo_daytrader_xai" / "ui" / "app.py"),
    "saxo_daytrader_xai.api.app:create_app",
    "streamlit run",
    "next dev",
    str(ROOT / "frontend"),
)
PID_FILES = {
    "scheduler": RUNTIME_DIR / "scheduler.pid",
    "api": RUNTIME_DIR / "api.pid",
    "frontend": RUNTIME_DIR / "frontend.pid",
    "web-launcher": RUNTIME_DIR / "web-launcher.pid",
}
LEGACY_PID_FILES = {
    "legacy-dashboard": RUNTIME_DIR / "dashboard.pid",
    "legacy-launcher": RUNTIME_DIR / "launcher.pid",
}


def _read_pid(path: Path) -> int | None:
    if not path.exists():
        return None
    try:
        return int(path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None


def _process_exists(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _command_for_pid(pid: int) -> str | None:
    try:
        output = subprocess.check_output(
            ["ps", "-p", str(pid), "-o", "command="],
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    command = output.strip()
    return command or None


def _pid_matches_runtime(pid: int) -> bool:
    command = _command_for_pid(pid)
    if not command:
        return False
    return any(marker in command for marker in RUNTIME_MARKERS)


def _terminate_pid(pid: int, *, name: str, sig: int) -> bool:
    try:
        os.killpg(pid, sig)
        print(f"Sent signal {sig} to {name} process group {pid}.")
        return True
    except ProcessLookupError:
        print(f"{name.capitalize()} process group {pid} no longer exists.")
        return False
    except PermissionError:
        try:
            os.kill(pid, sig)
            print(f"Sent signal {sig} to {name} process {pid}.")
            return True
        except ProcessLookupError:
            print(f"{name.capitalize()} process {pid} no longer exists.")
            return False
        except PermissionError:
            print(f"Permission denied while stopping {name} process {pid}.")
            return False


def _remove_pid_file(path: Path) -> None:
    with contextlib.suppress(FileNotFoundError):
        path.unlink()


def _fallback_candidate_pids() -> list[tuple[int, str]]:
    try:
        output = subprocess.check_output(
            ["ps", "-ax", "-o", "pid=,command="],
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return []

    candidates: list[tuple[int, str]] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        try:
            pid_text, command = stripped.split(None, 1)
            pid = int(pid_text)
        except ValueError:
            continue
        if pid == os.getpid():
            continue
        if any(marker in command for marker in RUNTIME_MARKERS) and str(ROOT) in command:
            candidates.append((pid, command))
    return candidates


def main() -> int:
    tracked_pids = {
        pid
        for path in (*PID_FILES.values(), *LEGACY_PID_FILES.values())
        if (pid := _read_pid(path)) is not None
    }
    stopped_any = False
    for name, path in {**PID_FILES, **LEGACY_PID_FILES}.items():
        pid = _read_pid(path)
        if pid is None:
            continue
        if not _process_exists(pid):
            _remove_pid_file(path)
            continue
        if not _pid_matches_runtime(pid):
            _remove_pid_file(path)
            continue
        if _terminate_pid(pid, name=name, sig=signal.SIGTERM):
            stopped_any = True
        _remove_pid_file(path)

    for pid, command in _fallback_candidate_pids():
        if pid in tracked_pids:
            continue
        if _terminate_pid(pid, name="fallback", sig=signal.SIGTERM):
            print(f"Matched fallback process {pid}: {command}")
            stopped_any = True

    if not stopped_any:
        print("No tracked runtime processes were running.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
