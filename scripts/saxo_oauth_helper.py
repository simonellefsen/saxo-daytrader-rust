from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import secrets
import sys
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, quote, urlencode, urlparse, urlunparse

import requests

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_dotenv
from saxo_daytrader_xai.saxo_openapi import default_session_path, save_session


ENVIRONMENT_SETTINGS = {
    "sim": {
        "auth_base_url": "https://sim.logonvalidation.net",
        "openapi_base_url": "https://gateway.saxobank.com/sim/openapi",
    },
    "live": {
        "auth_base_url": "https://live.logonvalidation.net",
        "openapi_base_url": "https://gateway.saxobank.com/openapi",
    },
}


def normalize_redirect_uri(redirect_uri: str, default_port: int = 8765) -> str:
    parsed = urlparse(redirect_uri)
    if parsed.hostname not in {"127.0.0.1", "localhost"}:
        return redirect_uri
    if parsed.port is not None:
        return redirect_uri

    netloc = parsed.hostname or "localhost"
    if parsed.username or parsed.password:
        auth = parsed.username or ""
        if parsed.password:
            auth = f"{auth}:{parsed.password}"
        netloc = f"{auth}@{netloc}"
    netloc = f"{netloc}:{default_port}"
    return urlunparse((parsed.scheme or "http", netloc, parsed.path or "/", parsed.params, parsed.query, parsed.fragment))


class CallbackHandler(BaseHTTPRequestHandler):
    server_version = "SaxoOAuthHelper/1.0"

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        params = parse_qs(parsed.query)
        expected_path = getattr(self.server, "expected_callback_path", None)
        if expected_path and parsed.path != expected_path:
            self.send_response(404)
            self.end_headers()
            return

        auth_params = {key: values[0] for key, values in params.items()}
        if not any(key in auth_params for key in ("code", "error", "state")):
            self.send_response(400)
            self.end_headers()
            return

        self.server.auth_params = auth_params
        body = (
            "<html><body><h1>Saxo authorization complete</h1>"
            "<p>You can close this window and return to the terminal.</p></body></html>"
        )
        encoded = body.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A003
        return


def build_pkce_pair() -> tuple[str, str]:
    verifier = secrets.token_urlsafe(64)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode("ascii")).digest()).decode("ascii").rstrip("=")
    return verifier, challenge


def wait_for_callback(redirect_uri: str, timeout_seconds: int) -> dict[str, str]:
    parsed = urlparse(redirect_uri)
    if parsed.hostname not in {"127.0.0.1", "localhost"}:
        raise ValueError("redirect_uri must use localhost or 127.0.0.1 for the helper script")
    if parsed.scheme != "http":
        raise ValueError("redirect_uri must use http")
    if parsed.port is None:
        raise ValueError("redirect_uri must include an explicit localhost port, e.g. http://localhost:8765/callback")

    server = ThreadingHTTPServer((parsed.hostname, parsed.port), CallbackHandler)
    server.timeout = 1
    server.auth_params = None
    server.expected_callback_path = parsed.path or "/"
    deadline = time.time() + timeout_seconds
    try:
        while time.time() < deadline and server.auth_params is None:
            server.handle_request()
    finally:
        server.server_close()

    if server.auth_params is None:
        raise TimeoutError("Timed out waiting for Saxo redirect callback")
    return server.auth_params


def exchange_code_for_token(
    *,
    environment: str,
    auth_mode: str,
    client_id: str,
    client_secret: str,
    redirect_uri: str,
    code: str,
    code_verifier: str | None,
    timeout_seconds: int,
) -> dict[str, Any]:
    token_url = f"{ENVIRONMENT_SETTINGS[environment]['auth_base_url']}/token"
    headers = {"Content-Type": "application/x-www-form-urlencoded"}
    data = {
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
    }
    auth = None
    if auth_mode == "secret":
        auth = (client_id, client_secret)
    else:
        data["client_id"] = client_id
        data["code_verifier"] = code_verifier or ""

    response = requests.post(token_url, data=data, headers=headers, auth=auth, timeout=timeout_seconds)
    response.raise_for_status()
    return response.json()


def fetch_json(url: str, access_token: str, params: dict[str, Any] | None = None, timeout_seconds: int = 20) -> dict[str, Any]:
    response = requests.get(
        url,
        params=params,
        headers={"Authorization": f"Bearer {access_token}", "Accept": "application/json"},
        timeout=timeout_seconds,
    )
    response.raise_for_status()
    return response.json()


def main() -> int:
    load_dotenv(ROOT / ".env")

    parser = argparse.ArgumentParser(description="Authorize with Saxo OpenAPI and discover ClientKey / AccountKey.")
    parser.add_argument("--environment", choices=["sim", "live"], default=os.getenv("SAXO_ENVIRONMENT", "sim").lower())
    parser.add_argument("--auth-mode", choices=["pkce", "secret"], default="pkce")
    parser.add_argument("--client-id", default=os.getenv("SAXO_CLIENT_ID", ""))
    parser.add_argument("--client-secret", default=os.getenv("SAXO_CLIENT_SECRET", ""))
    parser.add_argument("--redirect-uri", default=os.getenv("SAXO_REDIRECT_URI", "http://localhost:8765/callback"))
    parser.add_argument("--timeout-seconds", type=int, default=180)
    parser.add_argument("--no-browser", action="store_true", help="Print the authorize URL instead of opening it automatically.")
    parser.add_argument("--write-env", action="store_true", help="Write SAXO_ACCOUNT_KEY and SAXO_CLIENT_KEY back into .env if found.")
    parser.add_argument("--write-session", action="store_true", help="Write a refreshable Saxo session cache to .secrets/saxo_session.json.")
    args = parser.parse_args()
    normalized_redirect_uri = normalize_redirect_uri(args.redirect_uri)
    if normalized_redirect_uri != args.redirect_uri:
        print(f"Normalized redirect URI to {normalized_redirect_uri} for local callback handling.")
        args.redirect_uri = normalized_redirect_uri

    if not args.client_id:
        raise SystemExit("Missing SAXO_CLIENT_ID / --client-id")
    if args.auth_mode == "secret" and not args.client_secret:
        raise SystemExit("Missing SAXO_CLIENT_SECRET / --client-secret for auth-mode=secret")

    state = secrets.token_urlsafe(24)
    code_verifier = None
    query = {
        "response_type": "code",
        "client_id": args.client_id,
        "state": state,
        "redirect_uri": args.redirect_uri,
    }
    if args.auth_mode == "pkce":
        code_verifier, code_challenge = build_pkce_pair()
        query["code_challenge"] = code_challenge
        query["code_challenge_method"] = "S256"

    authorize_url = f"{ENVIRONMENT_SETTINGS[args.environment]['auth_base_url']}/authorize?{urlencode(query, quote_via=quote)}"
    print(f"Environment: {args.environment.upper()}")
    print(f"Auth mode: {args.auth_mode}")
    print(f"Authorize URL: {authorize_url}")

    if args.no_browser:
        print("Open the authorize URL manually in your browser.")
    else:
        webbrowser.open(authorize_url)

    callback_params = wait_for_callback(args.redirect_uri, args.timeout_seconds)
    if callback_params.get("state") != state:
        raise SystemExit("OAuth state mismatch. Aborting.")
    if "error" in callback_params:
        raise SystemExit(f"Saxo returned an error: {callback_params}")
    code = callback_params.get("code")
    if not code:
        raise SystemExit(f"No authorization code returned: {callback_params}")

    token_response = exchange_code_for_token(
        environment=args.environment,
        auth_mode=args.auth_mode,
        client_id=args.client_id,
        client_secret=args.client_secret,
        redirect_uri=args.redirect_uri,
        code=code,
        code_verifier=code_verifier,
        timeout_seconds=args.timeout_seconds,
    )
    access_token = token_response["access_token"]

    openapi_base = ENVIRONMENT_SETTINGS[args.environment]["openapi_base_url"]
    me = fetch_json(f"{openapi_base}/port/v1/clients/me", access_token, timeout_seconds=args.timeout_seconds)
    client_key = me.get("ClientKey")
    default_account_key = me.get("DefaultAccountKey")
    accounts = fetch_json(
        f"{openapi_base}/port/v1/accounts",
        access_token,
        params={"ClientKey": client_key},
        timeout_seconds=args.timeout_seconds,
    ).get("Data", [])

    output = {
        "environment": args.environment,
        "openapi_base_url": openapi_base,
        "client_key": client_key,
        "default_account_key": default_account_key,
        "default_account_id": me.get("DefaultAccountId"),
        "client_id_display": me.get("ClientId"),
        "accounts": [
            {
                "AccountId": account.get("AccountId"),
                "AccountKey": account.get("AccountKey"),
                "AccountType": account.get("AccountType"),
                "Currency": account.get("Currency"),
                "Active": account.get("Active"),
            }
            for account in accounts
        ],
        "token_info": {
            "expires_in": token_response.get("expires_in"),
            "refresh_token_expires_in": token_response.get("refresh_token_expires_in"),
            "token_type": token_response.get("token_type"),
        },
    }
    print(json.dumps(output, indent=2))

    if args.write_env:
        env_path = ROOT / ".env"
        write_env_values(
            env_path,
            {
                "SAXO_ENVIRONMENT": args.environment,
                "SAXO_CLIENT_KEY": client_key or "",
                "SAXO_ACCOUNT_KEY": default_account_key or "",
            },
        )
        print(f"Updated {env_path} with SAXO_ENVIRONMENT, SAXO_CLIENT_KEY, and SAXO_ACCOUNT_KEY.")

    if args.write_session:
        session_payload = {
            "environment": args.environment,
            "auth_mode": args.auth_mode,
            "client_id": args.client_id,
            "redirect_uri": args.redirect_uri,
            "code_verifier": code_verifier,
            "client_key": client_key,
            "account_key": default_account_key,
            "access_token": token_response.get("access_token"),
            "refresh_token": token_response.get("refresh_token"),
            "token_type": token_response.get("token_type", "Bearer"),
            "access_token_expires_at": _expires_at(token_response.get("expires_in")),
            "refresh_token_expires_at": _expires_at(token_response.get("refresh_token_expires_in")),
            "created_at": _now_iso(),
        }
        session_path = default_session_path(load_config_for_session())
        save_session(session_path, session_payload)
        print(f"Updated {session_path} with Saxo access and refresh tokens.")

    return 0


def _expires_at(expires_in: Any) -> str:
    seconds = int(expires_in or 0)
    return time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime(time.time() + seconds))


def _now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime())


def load_config_for_session() -> dict[str, Any]:
    from saxo_daytrader_xai.config import load_config

    return load_config(ROOT / "config.yaml")


def write_env_values(env_path: Path, values: dict[str, str]) -> None:
    existing = {}
    lines = []
    if env_path.exists():
        lines = env_path.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            stripped = line.strip()
            if not stripped or stripped.startswith("#") or "=" not in line:
                continue
            key, _ = line.split("=", 1)
            existing[key.strip()] = index

    for key, value in values.items():
        rendered = f"{key}={value}"
        if key in existing:
            lines[existing[key]] = rendered
        else:
            lines.append(rendered)

    env_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
