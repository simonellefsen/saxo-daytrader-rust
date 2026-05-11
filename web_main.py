from __future__ import annotations

import argparse
import contextlib
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from types import FrameType

ROOT = Path(__file__).resolve().parent
SRC = ROOT / "src"
FRONTEND_DIR = ROOT / "frontend"
RUNTIME_DIR = ROOT / ".run"
WEB_LAUNCHER_PID_PATH = RUNTIME_DIR / "web-launcher.pid"
API_PID_PATH = RUNTIME_DIR / "api.pid"
FRONTEND_PID_PATH = RUNTIME_DIR / "frontend.pid"
SCHEDULER_PID_PATH = RUNTIME_DIR / "scheduler.pid"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.importer import sync_portfolio


def _write_pid(path: Path, pid: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"{pid}\n", encoding="utf-8")


def _remove_pid(path: Path) -> None:
    with contextlib.suppress(FileNotFoundError):
        path.unlink()


def _clear_runtime_state(*, api: bool = True, frontend: bool = True, scheduler: bool = True, launcher: bool = False) -> None:
    if api:
        _remove_pid(API_PID_PATH)
    if frontend:
        _remove_pid(FRONTEND_PID_PATH)
    if scheduler:
        _remove_pid(SCHEDULER_PID_PATH)
    if launcher:
        _remove_pid(WEB_LAUNCHER_PID_PATH)


def _terminate_process_group(process: subprocess.Popen[bytes] | None, *, sig: int) -> None:
    if process is None:
        return
    with contextlib.suppress(ProcessLookupError):
        try:
            os.killpg(process.pid, sig)
            return
        except PermissionError:
            pass
    with contextlib.suppress(ProcessLookupError, PermissionError):
        process.send_signal(sig)


def _wait_process(process: subprocess.Popen[bytes] | None, *, timeout: float) -> int | None:
    if process is None:
        return None
    try:
        return process.wait(timeout=timeout)
    except (subprocess.TimeoutExpired, KeyboardInterrupt):
        return None


def _spawn_process(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        cmd,
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        start_new_session=True,
    )


def _scheduler_restart_enabled(config: dict) -> bool:
    return bool(config.get("app", {}).get("scheduler_restart_on_failure", True))


def _scheduler_max_restarts(config: dict) -> int:
    return int(config.get("app", {}).get("scheduler_max_restarts", 3))


def _scheduler_restart_delay_seconds(config: dict) -> float:
    return float(config.get("app", {}).get("scheduler_restart_delay_seconds", 2.0))


def _shutdown_signal_handler(signum: int, _frame: FrameType | None) -> None:
    raise KeyboardInterrupt(f"Received signal {signal.Signals(signum).name}")


def _install_signal_handlers() -> dict[int, signal.Handlers]:
    previous: dict[int, signal.Handlers] = {}
    for sig in (signal.SIGINT, signal.SIGTERM):
        previous[sig] = signal.getsignal(sig)
        signal.signal(sig, _shutdown_signal_handler)
    return previous


def _restore_signal_handlers(previous: dict[int, signal.Handlers]) -> None:
    for sig, handler in previous.items():
        signal.signal(sig, handler)


def _should_launch_scheduler(config: dict, *, with_scheduler: bool, no_scheduler: bool) -> bool:
    app_config = config.get("app", {})
    default_enabled = app_config.get(
        "launch_scheduler_with_ui",
        app_config.get("launch_scheduler_with_dashboard", False),
    )
    return bool(with_scheduler or default_enabled) and not no_scheduler


def _wait_for_children(
    api_process: subprocess.Popen[bytes],
    frontend_process: subprocess.Popen[bytes],
    scheduler_process: subprocess.Popen[bytes] | None,
    *,
    scheduler_cmd: list[str] | None = None,
    scheduler_restart_enabled: bool = False,
    scheduler_max_restarts: int = 0,
    scheduler_restart_delay_seconds: float = 2.0,
    scheduler_env: dict[str, str] | None = None,
) -> int:
    scheduler_restart_count = 0
    while True:
        api_code = api_process.poll()
        frontend_code = frontend_process.poll()
        scheduler_code = scheduler_process.poll() if scheduler_process is not None else None

        if api_code is not None:
            _clear_runtime_state(api=True, frontend=False, scheduler=False)
            _terminate_process_group(frontend_process, sig=signal.SIGTERM)
            _wait_process(frontend_process, timeout=5)
            if scheduler_process is not None:
                _terminate_process_group(scheduler_process, sig=signal.SIGTERM)
                _wait_process(scheduler_process, timeout=5)
            _clear_runtime_state(api=True, frontend=True, scheduler=True)
            return int(api_code)

        if frontend_code is not None:
            _clear_runtime_state(api=False, frontend=True, scheduler=False)
            _terminate_process_group(api_process, sig=signal.SIGTERM)
            _wait_process(api_process, timeout=5)
            if scheduler_process is not None:
                _terminate_process_group(scheduler_process, sig=signal.SIGTERM)
                _wait_process(scheduler_process, timeout=5)
            _clear_runtime_state(api=True, frontend=True, scheduler=True)
            return int(frontend_code)

        if scheduler_process is not None and scheduler_code not in (None, 0):
            _clear_runtime_state(scheduler=True, api=False, frontend=False)
            if (
                scheduler_cmd is not None
                and scheduler_restart_enabled
                and scheduler_restart_count < scheduler_max_restarts
            ):
                scheduler_restart_count += 1
                print(
                    f"Scheduler exited with code {scheduler_code}; restarting "
                    f"({scheduler_restart_count}/{scheduler_max_restarts})...",
                    file=sys.stderr,
                )
                time.sleep(scheduler_restart_delay_seconds)
                scheduler_process = _spawn_process(scheduler_cmd, env=scheduler_env)
                _write_pid(SCHEDULER_PID_PATH, scheduler_process.pid)
                continue
            print("Scheduler exited unexpectedly; stopping API and frontend...", file=sys.stderr)
            _terminate_process_group(api_process, sig=signal.SIGTERM)
            _terminate_process_group(frontend_process, sig=signal.SIGTERM)
            _wait_process(api_process, timeout=5)
            _wait_process(frontend_process, timeout=5)
            _clear_runtime_state(api=True, frontend=True, scheduler=True)
            return int(scheduler_code)

        time.sleep(0.2)


def main() -> int:
    parser = argparse.ArgumentParser(description="Launch the web API, Next.js frontend, and scheduler.")
    parser.add_argument("--config", default="config.yaml", help="Path to the YAML config file.")
    parser.add_argument("--api-port", type=int, default=8000, help="FastAPI/uvicorn port.")
    parser.add_argument("--frontend-port", type=int, default=3000, help="Next.js frontend port.")
    parser.add_argument("--with-scheduler", action="store_true", help="Launch the background scheduler alongside the web stack.")
    parser.add_argument("--no-scheduler", action="store_true", help="Do not launch the background scheduler.")
    parser.add_argument("--sync-only", action="store_true", help="Import the configured holdings baseline and exit.")
    args = parser.parse_args()
    previous_handlers = _install_signal_handlers()

    config = load_config(args.config)
    sync_result = sync_portfolio(config)
    print(
        f"Imported batch {sync_result.batch_id} from {sync_result.source_csv} "
        f"with {sync_result.imported_positions} active positions and "
        f"{sync_result.excluded_positions} exclusions."
    )
    if args.sync_only:
        return 0

    _write_pid(WEB_LAUNCHER_PID_PATH, os.getpid())

    base_env = os.environ.copy()
    existing_pythonpath = base_env.get("PYTHONPATH", "")
    base_env["PYTHONPATH"] = (
        str(SRC) if not existing_pythonpath else f"{SRC}{os.pathsep}{existing_pythonpath}"
    )
    frontend_env = base_env.copy()
    frontend_env.setdefault("NEXT_PUBLIC_API_BASE_URL", f"http://127.0.0.1:{args.api_port}")
    frontend_env.setdefault("WATCHPACK_POLLING", "true")
    frontend_env.setdefault("CHOKIDAR_USEPOLLING", "true")

    api_cmd = [
        sys.executable,
        "-m",
        "uvicorn",
        "saxo_daytrader_xai.api.app:create_app",
        "--factory",
        "--host",
        "127.0.0.1",
        "--port",
        str(args.api_port),
    ]
    frontend_cmd = [
        "pnpm",
        "exec",
        "next",
        "dev",
        "--port",
        str(args.frontend_port),
        "--hostname",
        "127.0.0.1",
    ]

    launch_scheduler = _should_launch_scheduler(
        config,
        with_scheduler=args.with_scheduler,
        no_scheduler=args.no_scheduler,
    )
    scheduler_process = None
    scheduler_cmd = None
    if launch_scheduler:
        scheduler_cmd = [
            sys.executable,
            str(ROOT / "scripts" / "run_scheduler.py"),
            "--config",
            str(Path(args.config).resolve()),
        ]
        print("Launching background scheduler alongside web stack...")
        scheduler_process = _spawn_process(scheduler_cmd, env=base_env)
        _write_pid(SCHEDULER_PID_PATH, scheduler_process.pid)

    print(f"Starting API on http://127.0.0.1:{args.api_port}")
    api_process = _spawn_process(api_cmd, env=base_env)
    _write_pid(API_PID_PATH, api_process.pid)

    print(f"Starting frontend on http://127.0.0.1:{args.frontend_port}")
    frontend_process = _spawn_process(frontend_cmd, cwd=FRONTEND_DIR, env=frontend_env)
    _write_pid(FRONTEND_PID_PATH, frontend_process.pid)

    try:
        return _wait_for_children(
            api_process,
            frontend_process,
            scheduler_process,
            scheduler_cmd=scheduler_cmd,
            scheduler_restart_enabled=_scheduler_restart_enabled(config),
            scheduler_max_restarts=_scheduler_max_restarts(config),
            scheduler_restart_delay_seconds=_scheduler_restart_delay_seconds(config),
            scheduler_env=base_env,
        )
    except KeyboardInterrupt:
        print("\nStopping frontend...", file=sys.stderr)
        _terminate_process_group(frontend_process, sig=signal.SIGTERM)
        print("Stopping API...", file=sys.stderr)
        _terminate_process_group(api_process, sig=signal.SIGTERM)
        if scheduler_process is not None:
            print("Stopping scheduler...", file=sys.stderr)
            _terminate_process_group(scheduler_process, sig=signal.SIGTERM)
        _clear_runtime_state(api=True, frontend=True, scheduler=True)
        frontend_stopped = _wait_process(frontend_process, timeout=5) is not None
        api_stopped = _wait_process(api_process, timeout=5) is not None
        scheduler_stopped = (
            _wait_process(scheduler_process, timeout=5) is not None
            if scheduler_process is not None
            else True
        )
        if not frontend_stopped:
            _terminate_process_group(frontend_process, sig=signal.SIGKILL)
            _wait_process(frontend_process, timeout=2)
        if not api_stopped:
            _terminate_process_group(api_process, sig=signal.SIGKILL)
            _wait_process(api_process, timeout=2)
        if scheduler_process is not None and not scheduler_stopped:
            _terminate_process_group(scheduler_process, sig=signal.SIGKILL)
            _wait_process(scheduler_process, timeout=2)
        return 0
    finally:
        _restore_signal_handlers(previous_handlers)
        _clear_runtime_state(api=True, frontend=True, scheduler=True, launcher=True)


if __name__ == "__main__":
    raise SystemExit(main())
