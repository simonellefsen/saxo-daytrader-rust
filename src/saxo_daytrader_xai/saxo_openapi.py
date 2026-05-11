from __future__ import annotations

import json
import logging
import base64
import hashlib
import math
import secrets
import threading
import time
import uuid
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlencode

import requests


logger = logging.getLogger(__name__)

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

TRADABLE_ASSET_TYPES = ("Stock", "Etf", "Etn", "Etc")
TOKEN_SAFETY_MARGIN_SECONDS = 300
EXCHANGE_ID_MAP = {
    "xnas": "XNAS",
    "xnys": "XNYS",
    "xcse": "XCSE",
    "xsto": "XSTO",
    "xosl": "XOSL",
    "xhel": "XHEL",
    "xlon": "XLON",
    "xetr": "XETR",
    "xfra": "XFRA",
    "xmil": "XMIL",
    "xpar": "XPAR",
    "xams": "XAMS",
    "xbru": "XBRU",
    "xlse": "XLIS",
}

EXCHANGE_ALIASES = {
    "xnas": {"XNAS", "NASDAQ"},
    "xnys": {"XNYS", "NYSE"},
    "xcse": {"XCSE", "CSE", "COP"},
    "xsto": {"XSTO", "STO", "STK"},
    "xosl": {"XOSL", "OSL", "OSE"},
    "xhel": {"XHEL", "HEL", "HEX"},
    "xlon": {"XLON", "LSE", "LON"},
    "xetr": {"XETR", "XTRA", "ETR"},
    "xfra": {"XFRA", "FSE", "FRA"},
    "xmil": {"XMIL", "MIL"},
    "xpar": {"XPAR", "PAR"},
    "xams": {"XAMS", "AMS"},
    "xbru": {"XBRU", "BRU"},
    "xlse": {"XLIS", "LIS"},
}

ORDER_REQUEST_MIN_INTERVAL_SECONDS = 1.05
ORDER_RATE_LIMIT_MAX_RETRIES = 3
_ORDER_RATE_LIMIT_LOCK = threading.Lock()
_NEXT_ORDER_REQUEST_AT = 0.0

DEFAULT_ORDER_PRICE_TICKS = {
    "xnas": 0.01,
    "xnys": 0.01,
    "xcse": 0.05,
    "xsto": 0.10,
    "xosl": 0.10,
    "xhel": 0.01,
    "xlon": 0.01,
    "xetr": 0.05,
    "xfra": 0.05,
    "xmil": 0.01,
    "xpar": 0.01,
    "xams": 0.01,
    "xbru": 0.01,
    "xlse": 0.01,
}


class SaxoSessionError(RuntimeError):
    pass


@dataclass(frozen=True)
class SaxoInstrument:
    symbol: str
    uic: int
    asset_type: str
    exchange_id: str
    description: str
    tradable_as: list[str]
    currency_code: str | None
    isin_code: str | None


class SaxoOrderNotFoundError(SaxoSessionError):
    pass


class SaxoRateLimitError(SaxoSessionError):
    def __init__(self, message: str, *, retry_after_seconds: float | None = None) -> None:
        super().__init__(message)
        self.retry_after_seconds = retry_after_seconds


def _response_json_or_none(response: requests.Response) -> dict[str, Any] | list[Any] | None:
    try:
        return response.json()
    except ValueError:
        return None


def _extract_saxo_error(payload: Any) -> str | None:
    if isinstance(payload, dict):
        error_info = payload.get("ErrorInfo")
        if isinstance(error_info, dict):
            code = error_info.get("ErrorCode")
            message = error_info.get("Message")
            if code and message:
                return f"{code}: {message}"
            if message:
                return str(message)
            if code:
                return str(code)
        orders = payload.get("Orders")
        if isinstance(orders, list):
            for item in orders:
                nested = _extract_saxo_error(item)
                if nested:
                    return nested
        message = payload.get("Message") or payload.get("message") or payload.get("error_description")
        if message:
            return str(message)
    if isinstance(payload, list):
        for item in payload:
            nested = _extract_saxo_error(item)
            if nested:
                return nested
    return None


def _raise_for_saxo_response(response: requests.Response, *, action: str) -> dict[str, Any]:
    payload = _response_json_or_none(response)
    error_text = _extract_saxo_error(payload)
    status_code = int(getattr(response, "status_code", 200))
    response_text = str(getattr(response, "text", "") or "")
    if status_code >= 400:
        if status_code == 429:
            reset_seconds = _rate_limit_reset_seconds(response)
            detail = error_text or response_text.strip()[:300] or "rate limit exceeded"
            raise SaxoRateLimitError(
                f"{action} rate limited: HTTP 429: {detail}",
                retry_after_seconds=reset_seconds,
            )
        if status_code == 404 and error_text and "OrderNotFound" in error_text:
            raise SaxoOrderNotFoundError(error_text)
        if error_text:
            raise SaxoSessionError(f"{action} failed: {error_text}")
        snippet = response_text.strip()
        if snippet:
            raise SaxoSessionError(f"{action} failed: HTTP {status_code}: {snippet[:300]}")
        raise SaxoSessionError(f"{action} failed: HTTP {status_code}")
    if error_text:
        if "OrderNotFound" in error_text:
            raise SaxoOrderNotFoundError(error_text)
        raise SaxoSessionError(f"{action} failed: {error_text}")
    if isinstance(payload, dict):
        return payload
    return {}


def _rate_limit_reset_seconds(response: requests.Response) -> float:
    for header_name in (
        "X-RateLimit-SessionOrders-Reset",
        "X-RateLimit-Session-Reset",
        "Retry-After",
    ):
        raw_value = response.headers.get(header_name)
        if raw_value in (None, ""):
            continue
        try:
            return max(float(raw_value), 0.0)
        except (TypeError, ValueError):
            continue
    return 1.0


def _wait_for_order_slot(min_interval_seconds: float = ORDER_REQUEST_MIN_INTERVAL_SECONDS) -> None:
    global _NEXT_ORDER_REQUEST_AT
    with _ORDER_RATE_LIMIT_LOCK:
        now = time.monotonic()
        wait_seconds = max(_NEXT_ORDER_REQUEST_AT - now, 0.0)
        if wait_seconds > 0:
            time.sleep(wait_seconds)
            now = time.monotonic()
        _NEXT_ORDER_REQUEST_AT = now + float(min_interval_seconds)


def _push_back_order_slot(delay_seconds: float) -> None:
    global _NEXT_ORDER_REQUEST_AT
    with _ORDER_RATE_LIMIT_LOCK:
        _NEXT_ORDER_REQUEST_AT = max(_NEXT_ORDER_REQUEST_AT, time.monotonic() + max(delay_seconds, 0.0))


def _send_order_request(
    method: str,
    url: str,
    *,
    headers: dict[str, str],
    timeout: int = 30,
    **kwargs: Any,
) -> requests.Response:
    request_headers = dict(headers)
    if method.upper() in {"POST", "PATCH"}:
        request_headers.setdefault("x-request-id", str(uuid.uuid4()))
    last_response: requests.Response | None = None
    for attempt in range(ORDER_RATE_LIMIT_MAX_RETRIES + 1):
        _wait_for_order_slot()
        response = requests.request(
            method=method.upper(),
            url=url,
            headers=request_headers,
            timeout=timeout,
            **kwargs,
        )
        last_response = response
        if int(response.status_code) != 429:
            return response
        reset_seconds = _rate_limit_reset_seconds(response)
        _push_back_order_slot(reset_seconds + 0.25)
        if attempt >= ORDER_RATE_LIMIT_MAX_RETRIES:
            return response
        time.sleep(reset_seconds + 0.25)
    return last_response if last_response is not None else requests.Response()


def default_session_path(config: dict[str, Any]) -> Path:
    config_dir = Path(config["_meta"]["config_dir"])
    return config_dir / ".secrets" / "saxo_session.json"


def _now_utc() -> datetime:
    return datetime.now(UTC)


def _to_iso(dt: datetime) -> str:
    return dt.astimezone(UTC).isoformat(timespec="seconds")


def _parse_iso(value: str | None) -> datetime | None:
    if not value:
        return None
    normalized = value.replace("Z", "+00:00")
    parsed = datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=UTC)
    return parsed.astimezone(UTC)


def _minutes_until(value: datetime | None) -> int | None:
    if value is None:
        return None
    return max(int((value - _now_utc()).total_seconds() // 60), 0)


def build_pkce_pair() -> tuple[str, str]:
    verifier = secrets.token_urlsafe(64)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode("ascii")).digest()).decode("ascii").rstrip("=")
    return verifier, challenge


def build_authorize_url(
    *,
    environment: str,
    client_id: str,
    redirect_uri: str,
    state: str,
    auth_mode: str = "pkce",
    code_challenge: str | None = None,
) -> str:
    query = {
        "response_type": "code",
        "client_id": client_id,
        "state": state,
        "redirect_uri": redirect_uri,
    }
    if auth_mode == "pkce":
        if not code_challenge:
            raise SaxoSessionError("PKCE authorization requires a code challenge")
        query["code_challenge"] = code_challenge
        query["code_challenge_method"] = "S256"
    return f"{ENVIRONMENT_SETTINGS[environment.lower()]['auth_base_url']}/authorize?{urlencode(query, quote_via=quote)}"


def exchange_authorization_code(
    *,
    environment: str,
    auth_mode: str,
    client_id: str,
    client_secret: str,
    redirect_uri: str,
    code: str,
    code_verifier: str | None,
    timeout_seconds: int = 30,
) -> dict[str, Any]:
    token_url = f"{ENVIRONMENT_SETTINGS[environment.lower()]['auth_base_url']}/token"
    headers = {"Content-Type": "application/x-www-form-urlencoded"}
    data = {
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
    }
    auth = None
    if auth_mode == "secret":
        if not client_secret:
            raise SaxoSessionError("SAXO_CLIENT_SECRET is missing for secret-based authorization")
        auth = (client_id, client_secret)
    else:
        data["client_id"] = client_id
        data["code_verifier"] = code_verifier or ""

    response = requests.post(token_url, data=data, headers=headers, auth=auth, timeout=timeout_seconds)
    try:
        response.raise_for_status()
    except requests.HTTPError as exc:
        payload = _response_json_or_none(response)
        error_text = _extract_saxo_error(payload) or str(exc)
        raise SaxoSessionError(f"Saxo authorization token exchange failed: {error_text}") from exc
    return response.json()


def fetch_initial_session_context(
    *,
    environment: str,
    access_token: str,
    timeout_seconds: int = 30,
) -> dict[str, Any]:
    openapi_base = ENVIRONMENT_SETTINGS[environment.lower()]["openapi_base_url"]
    me_response = requests.get(
        f"{openapi_base}/port/v1/clients/me",
        headers={"Authorization": f"Bearer {access_token}", "Accept": "application/json"},
        timeout=timeout_seconds,
    )
    me_response.raise_for_status()
    me = me_response.json()
    client_key = me.get("ClientKey")
    account_key = me.get("DefaultAccountKey")
    return {
        "client_key": client_key,
        "account_key": account_key,
        "default_account_id": me.get("DefaultAccountId"),
        "client_id_display": me.get("ClientId"),
    }


def build_session_payload(
    *,
    environment: str,
    auth_mode: str,
    client_id: str,
    redirect_uri: str,
    code_verifier: str | None,
    token_response: dict[str, Any],
    session_context: dict[str, Any],
) -> dict[str, Any]:
    return {
        "environment": environment.lower(),
        "auth_mode": auth_mode,
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "code_verifier": code_verifier,
        "client_key": session_context.get("client_key"),
        "account_key": session_context.get("account_key"),
        "default_account_id": session_context.get("default_account_id"),
        "client_id_display": session_context.get("client_id_display"),
        "access_token": token_response.get("access_token"),
        "refresh_token": token_response.get("refresh_token"),
        "token_type": token_response.get("token_type", "Bearer"),
        "access_token_expires_at": _to_iso(_now_utc() + timedelta(seconds=int(token_response.get("expires_in", 0) or 0))),
        "refresh_token_expires_at": _to_iso(
            _now_utc() + timedelta(seconds=int(token_response.get("refresh_token_expires_in", 0) or 0))
        ),
        "created_at": _to_iso(_now_utc()),
        "last_refreshed_at": _to_iso(_now_utc()),
    }


def load_session(session_path: str | Path) -> dict[str, Any]:
    path = Path(session_path)
    if not path.exists():
        raise SaxoSessionError(f"Saxo session file is missing: {path}")
    return json.loads(path.read_text(encoding="utf-8"))


def save_session(session_path: str | Path, payload: dict[str, Any]) -> None:
    path = Path(session_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(0o600)


def session_is_access_token_valid(session: dict[str, Any]) -> bool:
    expires_at = _parse_iso(session.get("access_token_expires_at"))
    return bool(session.get("access_token")) and expires_at is not None and expires_at > _now_utc() + timedelta(seconds=TOKEN_SAFETY_MARGIN_SECONDS)


def session_can_refresh(session: dict[str, Any]) -> bool:
    if session.get("refresh_token_invalid_at"):
        return False
    refresh_expires_at = _parse_iso(session.get("refresh_token_expires_at"))
    return bool(session.get("refresh_token")) and refresh_expires_at is not None and refresh_expires_at > _now_utc() + timedelta(seconds=TOKEN_SAFETY_MARGIN_SECONDS)


def _refresh_access_token(session: dict[str, Any], config: dict[str, Any]) -> dict[str, Any]:
    environment = str(session.get("environment") or config["saxo"]["environment"]).lower()
    client_id = config["saxo"]["client_id"]
    client_secret = config["saxo"].get("client_secret", "")
    if not client_id:
        raise SaxoSessionError("SAXO_CLIENT_ID is missing")
    if not session_can_refresh(session):
        raise SaxoSessionError("No valid refresh token is available. Re-run the Saxo OAuth helper.")

    token_url = f"{ENVIRONMENT_SETTINGS[environment]['auth_base_url']}/token"
    data = {
        "grant_type": "refresh_token",
        "refresh_token": session["refresh_token"],
    }
    headers = {"Content-Type": "application/x-www-form-urlencoded"}
    auth = None

    # PKCE apps use client_id in-body and may require the original code_verifier.
    if session.get("auth_mode") == "pkce":
        data["client_id"] = client_id
        if session.get("code_verifier"):
            data["code_verifier"] = session["code_verifier"]
        if session.get("redirect_uri"):
            data["redirect_uri"] = session["redirect_uri"]
    else:
        if not client_secret:
            raise SaxoSessionError("SAXO_CLIENT_SECRET is missing for secret-based token refresh")
        auth = (client_id, client_secret)
        if session.get("redirect_uri"):
            data["redirect_uri"] = session["redirect_uri"]

    logger.info("Refreshing Saxo access token for %s environment", environment.upper())
    try:
        response = requests.post(token_url, data=data, headers=headers, auth=auth, timeout=30)
        response.raise_for_status()
    except requests.HTTPError as exc:
        status_code = exc.response.status_code if exc.response is not None else None
        if status_code in {400, 401}:
            raise SaxoSessionError(
                f"Saxo refresh token was rejected with HTTP {status_code}. Re-run the Saxo OAuth helper."
            ) from exc
        logger.exception("Saxo access token refresh failed for %s environment", environment.upper())
        raise
    except Exception:
        logger.exception("Saxo access token refresh failed for %s environment", environment.upper())
        raise
    token_response = response.json()
    refresh_expires_in = token_response.get("refresh_token_expires_in")
    refresh_token_expires_at = (
        _to_iso(_now_utc() + timedelta(seconds=int(refresh_expires_in)))
        if refresh_expires_in not in (None, "")
        else session.get("refresh_token_expires_at")
    )
    refreshed = {
        **session,
        "access_token": token_response["access_token"],
        "refresh_token": token_response.get("refresh_token", session.get("refresh_token")),
        "token_type": token_response.get("token_type", "Bearer"),
        "access_token_expires_at": _to_iso(_now_utc() + timedelta(seconds=int(token_response.get("expires_in", 0)))),
        "refresh_token_expires_at": refresh_token_expires_at,
        "last_refreshed_at": _to_iso(_now_utc()),
    }
    refreshed.pop("refresh_error", None)
    refreshed.pop("refresh_token_invalid_at", None)
    logger.info(
        "Saxo access token refreshed for %s environment; access expires at %s, refresh expires at %s",
        environment.upper(),
        refreshed.get("access_token_expires_at"),
        refreshed.get("refresh_token_expires_at"),
    )
    return refreshed


def ensure_access_token(config: dict[str, Any], session_path: str | Path | None = None) -> dict[str, Any]:
    path = Path(session_path or default_session_path(config))
    session = load_session(path)
    if session_is_access_token_valid(session):
        return session
    try:
        refreshed = _refresh_access_token(session, config)
    except SaxoSessionError as exc:
        if "refresh token was rejected" in str(exc):
            failed_session = {
                **session,
                "refresh_token_invalid_at": _to_iso(_now_utc()),
                "refresh_error": str(exc),
            }
            save_session(path, failed_session)
        raise
    save_session(path, refreshed)
    return refreshed


def get_auth_status(config: dict[str, Any], session_path: str | Path | None = None, *, auto_refresh: bool = False) -> dict[str, Any]:
    """Return non-secret Saxo session health for UI/API status surfaces."""
    configured_environment = str(config.get("saxo", {}).get("environment") or "sim").lower()
    path = Path(session_path or default_session_path(config))
    base: dict[str, Any] = {
        "connected": False,
        "environment": configured_environment,
        "configured_environment": configured_environment,
        "token_valid": False,
        "refresh_token_valid": False,
        "expires_at": None,
        "expires_in_minutes": None,
        "refresh_expires_at": None,
        "refresh_expires_in_minutes": None,
        "last_refreshed_at": None,
        "refreshing": False,
        "needs_reauth": True,
        "status": "missing_session",
        "status_text": "Saxo session file is missing.",
        "session_path": str(path),
        "error": None,
    }
    try:
        if auto_refresh:
            try:
                ensure_access_token(config, path)
            except SaxoSessionError:
                # Return the inspected status below; ensure_access_token annotates
                # rejected refresh tokens so the UI can show re-auth required.
                pass
        session = load_session(path)
    except SaxoSessionError as exc:
        return {**base, "error": str(exc)}
    except Exception as exc:  # noqa: BLE001
        logger.exception("Failed to inspect Saxo session status")
        return {
            **base,
            "status": "session_error",
            "status_text": "Saxo session could not be inspected.",
            "error": str(exc),
        }

    environment = str(session.get("environment") or configured_environment).lower()
    access_expires_at = _parse_iso(session.get("access_token_expires_at"))
    refresh_expires_at = _parse_iso(session.get("refresh_token_expires_at"))
    expires_in_minutes = _minutes_until(access_expires_at)
    refresh_expires_in_minutes = _minutes_until(refresh_expires_at)
    token_valid = session_is_access_token_valid(session)
    refresh_token_valid = session_can_refresh(session)
    refresh_error = session.get("refresh_error")
    needs_reauth = not token_valid and not refresh_token_valid
    if token_valid and (expires_in_minutes is None or expires_in_minutes >= 10):
        status = "healthy"
        status_text = "Connected to Saxo."
        connected = True
    elif token_valid:
        status = "expiring_soon"
        status_text = "Saxo access token is expiring soon."
        connected = True
    elif refresh_token_valid:
        status = "refresh_available"
        status_text = "Access token expired, refresh token is still valid."
        connected = False
    else:
        status = "needs_reauth"
        status_text = refresh_error or "Saxo session expired. Re-authentication is required."
        connected = False

    return {
        **base,
        "connected": connected,
        "environment": environment,
        "token_valid": token_valid,
        "refresh_token_valid": refresh_token_valid,
        "expires_at": _to_iso(access_expires_at) if access_expires_at else None,
        "expires_in_minutes": expires_in_minutes,
        "refresh_expires_at": _to_iso(refresh_expires_at) if refresh_expires_at else None,
        "refresh_expires_in_minutes": refresh_expires_in_minutes,
        "last_refreshed_at": session.get("last_refreshed_at"),
        "needs_reauth": needs_reauth,
        "status": status,
        "status_text": status_text,
        "error": refresh_error if needs_reauth else None,
    }


def _auth_headers(access_token: str) -> dict[str, str]:
    return {
        "Authorization": f"Bearer {access_token}",
        "Accept": "application/json",
        "Content-Type": "application/json",
    }


def _openapi_base_url(environment: str) -> str:
    return ENVIRONMENT_SETTINGS[environment.lower()]["openapi_base_url"]


def _account_key(config: dict[str, Any], session: dict[str, Any]) -> str:
    account_key = config["saxo"].get("account_key") or session.get("account_key")
    if not account_key:
        raise SaxoSessionError("SAXO_ACCOUNT_KEY is missing. Re-run the Saxo OAuth helper with --write-env or --write-session.")
    return str(account_key)


def _client_key(config: dict[str, Any], session: dict[str, Any]) -> str:
    client_key = config["saxo"].get("client_key") or session.get("client_key")
    if not client_key:
        raise SaxoSessionError("SAXO_CLIENT_KEY is missing. Re-run the Saxo OAuth helper with --write-env or --write-session.")
    return str(client_key)


def _symbol_parts(symbol: str) -> tuple[str, str]:
    base, _, exchange = symbol.partition(":")
    return base.strip().upper(), exchange.strip().lower()


def _tick_decimal_places(tick_size: float) -> int:
    text = f"{float(tick_size):.10f}".rstrip("0").rstrip(".")
    if "." not in text:
        return 0
    return len(text.split(".", 1)[1])


def _configured_price_tick(symbol: str, config: dict[str, Any] | None = None) -> float | None:
    _, exchange_code = _symbol_parts(symbol)
    overrides = ((config or {}).get("execution", {}) or {}).get("price_tick_overrides", {}) or {}
    exact_keys = {symbol, symbol.upper(), symbol.lower()}
    for key in exact_keys:
        if key in overrides:
            try:
                return float(overrides[key])
            except (TypeError, ValueError):
                return None
    exchange_keys = {exchange_code, exchange_code.upper(), EXCHANGE_ID_MAP.get(exchange_code, exchange_code.upper())}
    for key in exchange_keys:
        if key in overrides:
            try:
                return float(overrides[key])
            except (TypeError, ValueError):
                return None
    return DEFAULT_ORDER_PRICE_TICKS.get(exchange_code)


def _configured_price_tick_override(symbol: str, config: dict[str, Any] | None = None) -> float | None:
    _, exchange_code = _symbol_parts(symbol)
    overrides = ((config or {}).get("execution", {}) or {}).get("price_tick_overrides", {}) or {}
    exact_keys = {symbol, symbol.upper(), symbol.lower()}
    for key in exact_keys:
        if key in overrides:
            try:
                return float(overrides[key])
            except (TypeError, ValueError):
                return None
    exchange_keys = {exchange_code, exchange_code.upper(), EXCHANGE_ID_MAP.get(exchange_code, exchange_code.upper())}
    for key in exchange_keys:
        if key in overrides:
            try:
                return float(overrides[key])
            except (TypeError, ValueError):
                return None
    return None


def price_tick_size_for_symbol(symbol: str, config: dict[str, Any] | None = None, *, fallback_decimals: int | None = None) -> float:
    configured = _configured_price_tick(symbol, config)
    if configured and configured > 0:
        return configured
    decimals = max(int(fallback_decimals or 2), 0)
    return 10 ** (-decimals)


def normalize_order_price(
    symbol: str,
    price: float | int | str | None,
    config: dict[str, Any] | None = None,
    *,
    side: str = "nearest",
    fallback_decimals: int | None = None,
) -> float | None:
    if price is None:
        return None
    value = float(price)
    tick_size = price_tick_size_for_symbol(symbol, config, fallback_decimals=fallback_decimals)
    if tick_size <= 0:
        return value
    units = value / tick_size
    direction = side.casefold()
    if direction in {"buy_limit", "sell_stop", "sell_stop_limit", "floor"}:
        normalized_units = math.floor(units + 1e-9)
    elif direction in {"sell_limit", "buy_stop", "buy_stop_limit", "ceil"}:
        normalized_units = math.ceil(units - 1e-9)
    else:
        normalized_units = round(units)
    return round(normalized_units * tick_size, _tick_decimal_places(tick_size))


_INSTRUMENT_DETAILS_CACHE: dict[tuple[str, str, int, str], dict[str, Any]] = {}


def _instrument_details_cache_key(
    instrument: SaxoInstrument,
    config: dict[str, Any],
    session: dict[str, Any],
) -> tuple[str, str, int, str]:
    return (
        str(session.get("environment") or config["saxo"]["environment"]),
        _account_key(config, session),
        int(instrument.uic),
        str(instrument.asset_type),
    )


def get_instrument_details(
    instrument: SaxoInstrument,
    config: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    cache_key = _instrument_details_cache_key(instrument, config, session)
    cached = _INSTRUMENT_DETAILS_CACHE.get(cache_key)
    if cached is not None:
        return cached
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/ref/v1/instruments/details/{instrument.uic}/{instrument.asset_type}",
        params={"AccountKey": _account_key(config, session)},
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    response.raise_for_status()
    payload = response.json()
    if isinstance(payload, dict):
        _INSTRUMENT_DETAILS_CACHE[cache_key] = payload
        return payload
    return {}


def _numeric_from_keys(payload: dict[str, Any], keys: tuple[str, ...]) -> float | None:
    for key in keys:
        if key not in payload:
            continue
        try:
            value = float(payload[key])
        except (TypeError, ValueError):
            continue
        if value > 0:
            return value
    return None


def _tick_from_scheme(price: float, scheme: dict[str, Any]) -> float | None:
    elements = scheme.get("Elements") or scheme.get("elements") or []
    if isinstance(elements, list):
        parsed: list[tuple[float, float]] = []
        for item in elements:
            if not isinstance(item, dict):
                continue
            tick = _numeric_from_keys(item, ("TickSize", "tickSize", "Size", "size"))
            high = _numeric_from_keys(item, ("HighPrice", "highPrice", "UpperBound", "upperBound", "Price", "price"))
            if tick is not None and high is not None:
                parsed.append((high, tick))
        for high, tick in sorted(parsed, key=lambda value: value[0]):
            if price <= high + 1e-9:
                return tick
    return _numeric_from_keys(
        scheme,
        ("DefaultTickSize", "defaultTickSize", "TickSize", "tickSize"),
    )


def _tick_from_instrument_details(price: float, details: dict[str, Any]) -> float | None:
    direct = _numeric_from_keys(details, ("TickSize", "tickSize", "PriceTickSize", "priceTickSize"))
    if direct is not None:
        return direct
    for key in ("TickSizeScheme", "tickSizeScheme", "PriceTickSizeScheme", "priceTickSizeScheme"):
        scheme = details.get(key)
        if isinstance(scheme, dict):
            tick = _tick_from_scheme(price, scheme)
            if tick is not None:
                return tick
    display = details.get("DisplayAndFormat")
    if isinstance(display, dict):
        direct = _numeric_from_keys(display, ("TickSize", "tickSize", "PriceTickSize", "priceTickSize"))
        if direct is not None:
            return direct
        for key in ("TickSizeScheme", "tickSizeScheme", "PriceTickSizeScheme", "priceTickSizeScheme"):
            scheme = display.get(key)
            if isinstance(scheme, dict):
                tick = _tick_from_scheme(price, scheme)
                if tick is not None:
                    return tick
    return None


def normalize_broker_order_price(
    symbol: str,
    price: float | int | str | None,
    config: dict[str, Any],
    session: dict[str, Any],
    instrument: SaxoInstrument,
    *,
    side: str = "nearest",
) -> float | None:
    if price is None:
        return None
    value = float(price)
    override_tick = _configured_price_tick_override(symbol, config)
    tick_size = override_tick
    if tick_size is None:
        try:
            details = get_instrument_details(instrument, config, session)
            tick_size = _tick_from_instrument_details(value, details)
        except requests.RequestException as exc:
            logger.warning("Failed to fetch Saxo instrument details for %s: %s", symbol, exc)
    if tick_size is None or tick_size <= 0:
        tick_size = price_tick_size_for_symbol(symbol, config)
    units = value / tick_size
    direction = side.casefold()
    if direction in {"buy_limit", "sell_stop", "sell_stop_limit", "floor"}:
        normalized_units = math.floor(units + 1e-9)
    elif direction in {"sell_limit", "buy_stop", "buy_stop_limit", "ceil"}:
        normalized_units = math.ceil(units - 1e-9)
    else:
        normalized_units = round(units)
    return round(normalized_units * tick_size, _tick_decimal_places(tick_size))


def _symbol_with_suffix(symbol: str) -> str:
    base_symbol, exchange_code = _symbol_parts(symbol)
    return f"{base_symbol}:{exchange_code}" if exchange_code else base_symbol


def _exchange_aliases(exchange_code: str) -> set[str]:
    aliases = EXCHANGE_ALIASES.get(exchange_code, {exchange_code.upper()})
    return {value.upper() for value in aliases if value}


def _candidate_score(candidate: dict[str, Any], *, requested_symbol: str, base_symbol: str, exchange_code: str) -> tuple[int, int, int]:
    candidate_symbol = str(candidate.get("Symbol", "")).strip().upper()
    candidate_exchange = str(candidate.get("ExchangeId", "")).strip().upper()
    aliases = _exchange_aliases(exchange_code)
    exact_symbol = int(candidate_symbol == requested_symbol.upper())
    exact_base = int(candidate_symbol.split(":", 1)[0] == base_symbol)
    exchange_match = int(candidate_exchange in aliases or candidate_symbol.endswith(f":{exchange_code.upper()}"))
    tradable_as = candidate.get("TradableAs", []) or []
    stock_preferred = int("Stock" in {str(value) for value in tradable_as})
    return (exact_symbol, exchange_match, exact_base + stock_preferred)


def lookup_instrument(symbol: str, config: dict[str, Any], session: dict[str, Any]) -> SaxoInstrument:
    base_symbol, exchange_code = _symbol_parts(symbol)
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/ref/v1/instruments",
        params={
            "$top": 50,
            "AccountKey": _account_key(config, session),
            "AssetTypes": ",".join(TRADABLE_ASSET_TYPES),
            "IncludeNonTradable": "false",
            "Keywords": base_symbol,
        },
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    payload = _raise_for_saxo_response(response, action="Instrument lookup")
    candidates = payload.get("Data", [])
    selected = None
    if candidates:
        requested_symbol = _symbol_with_suffix(symbol)
        ranked = sorted(
            candidates,
            key=lambda item: _candidate_score(
                item,
                requested_symbol=requested_symbol,
                base_symbol=base_symbol,
                exchange_code=exchange_code,
            ),
            reverse=True,
        )
        best = ranked[0]
        if _candidate_score(best, requested_symbol=requested_symbol, base_symbol=base_symbol, exchange_code=exchange_code) > (0, 0, 0):
            selected = best
    if not selected:
        raise SaxoSessionError(f"No tradable Saxo instrument match found for {symbol}")
    return SaxoInstrument(
        symbol=symbol,
        uic=int(selected["Identifier"]),
        asset_type=str(selected["AssetType"]),
        exchange_id=str(selected.get("ExchangeId", EXCHANGE_ID_MAP.get(exchange_code, exchange_code.upper()))),
        description=str(selected.get("Description", symbol)),
        tradable_as=[str(value) for value in selected.get("TradableAs", [])],
        currency_code=selected.get("CurrencyCode"),
        isin_code=(
            selected.get("IsinCode")
            or (selected.get("DisplayAndFormat", {}) or {}).get("IsinCode")
        ),
    )


def build_market_order_payload(
    *,
    symbol: str,
    action: str,
    quantity: float,
    external_reference: str,
    config: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    return build_order_payload(
        symbol=symbol,
        action=action,
        quantity=quantity,
        external_reference=external_reference,
        config=config,
        session=session,
        order_type="Market",
    )


def build_order_payload(
    *,
    symbol: str,
    action: str,
    quantity: float,
    external_reference: str,
    config: dict[str, Any],
    session: dict[str, Any],
    order_type: str = "Market",
    limit_price: float | None = None,
    stop_price: float | None = None,
    duration_type: str = "DayOrder",
    related_orders: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    instrument = lookup_instrument(symbol, config, session)
    whole_quantity = int(quantity)
    if whole_quantity <= 0:
        raise SaxoSessionError("Order quantity must be at least 1 whole share")
    normalized_order_type = str(order_type or "Market")
    buy_sell = "Buy" if action == "BUY" else "Sell"
    payload = {
        "AccountKey": _account_key(config, session),
        "Amount": whole_quantity,
        "AssetType": instrument.asset_type,
        "BuySell": buy_sell,
        "ExternalReference": external_reference[:50],
        "ManualOrder": True,
        "OrderDuration": {"DurationType": duration_type},
        "OrderType": normalized_order_type,
        "Uic": instrument.uic,
    }
    if normalized_order_type in {"Limit", "Stop", "StopLimit"}:
        order_price = limit_price if normalized_order_type == "Limit" else stop_price
        if order_price is None:
            raise SaxoSessionError(f"{normalized_order_type} orders require a price")
        if normalized_order_type == "Limit":
            side = "buy_limit" if buy_sell == "Buy" else "sell_limit"
        else:
            side = "buy_stop" if buy_sell == "Buy" else "sell_stop"
        payload["OrderPrice"] = float(
            normalize_broker_order_price(symbol, order_price, config, session, instrument, side=side)
        )
    if normalized_order_type == "StopLimit":
        if limit_price is None or stop_price is None:
            raise SaxoSessionError("StopLimit orders require both stop_price and limit_price")
        stop_side = "buy_stop" if buy_sell == "Buy" else "sell_stop"
        limit_side = "buy_limit" if buy_sell == "Buy" else "sell_stop_limit"
        payload["OrderPrice"] = float(
            normalize_broker_order_price(symbol, stop_price, config, session, instrument, side=stop_side)
        )
        payload["StopLimitPrice"] = float(
            normalize_broker_order_price(symbol, limit_price, config, session, instrument, side=limit_side)
        )
    if related_orders:
        payload["Orders"] = []
        for item in related_orders:
            child_quantity = int(item.get("quantity") or whole_quantity)
            if child_quantity <= 0:
                continue
            child_type = str(item.get("order_type") or "Limit")
            child_duration = str(item.get("duration_type") or "GoodTillCancel")
            child_buy_sell = "Buy" if str(item.get("action")).upper() == "BUY" else "Sell"
            child_payload: dict[str, Any] = {
                "Amount": child_quantity,
                "AssetType": instrument.asset_type,
                "BuySell": child_buy_sell,
                "ManualOrder": True,
                "OrderDuration": {"DurationType": child_duration},
                "OrderType": child_type,
                "Uic": instrument.uic,
            }
            child_price = item.get("limit_price")
            child_stop = item.get("stop_price")
            if child_type in {"Limit", "Stop", "StopLimit"}:
                order_price = child_price if child_type == "Limit" else child_stop
                if order_price is None:
                    raise SaxoSessionError(f"Related {child_type} orders require a price")
                if child_type == "Limit":
                    child_side = "buy_limit" if child_buy_sell == "Buy" else "sell_limit"
                else:
                    child_side = "buy_stop" if child_buy_sell == "Buy" else "sell_stop"
                child_payload["OrderPrice"] = float(
                    normalize_broker_order_price(symbol, order_price, config, session, instrument, side=child_side)
                )
            if child_type == "StopLimit":
                if child_price is None or child_stop is None:
                    raise SaxoSessionError("Related StopLimit orders require both stop and limit prices")
                child_stop_side = "buy_stop" if child_buy_sell == "Buy" else "sell_stop"
                child_limit_side = "buy_limit" if child_buy_sell == "Buy" else "sell_stop_limit"
                child_payload["OrderPrice"] = float(
                    normalize_broker_order_price(symbol, child_stop, config, session, instrument, side=child_stop_side)
                )
                child_payload["StopLimitPrice"] = float(
                    normalize_broker_order_price(symbol, child_price, config, session, instrument, side=child_limit_side)
                )
            payload["Orders"].append(child_payload)
    return payload


def precheck_order(payload: dict[str, Any], config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    request_payload = {
        **payload,
        "FieldGroups": ["Costs", "MarginImpactBuySell"],
    }
    response = _send_order_request(
        "POST",
        f"{base_url}/trade/v2/orders/precheck",
        headers=_auth_headers(session["access_token"]),
        json=request_payload,
    )
    return _raise_for_saxo_response(response, action="Order precheck")


def place_order(payload: dict[str, Any], config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = _send_order_request(
        "POST",
        f"{base_url}/trade/v2/orders",
        headers=_auth_headers(session["access_token"]),
        json=payload,
    )
    return _raise_for_saxo_response(response, action="Order placement")


def get_chart_samples(
    *,
    uic: int,
    asset_type: str,
    config: dict[str, Any],
    session: dict[str, Any],
    horizon_minutes: int = 1,
    count: int = 240,
    mode: str = "UpTo",
    time: str | None = None,
) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    params: dict[str, Any] = {
        "AccountKey": _account_key(config, session),
        "AssetType": asset_type,
        "Uic": int(uic),
        "Horizon": int(horizon_minutes),
        "Count": int(count),
        "FieldGroups": "ChartInfo,Data,DisplayAndFormat",
    }
    if time:
        params["Time"] = time
        params["Mode"] = mode
    response = requests.get(
        f"{base_url}/chart/v3/charts",
        params=params,
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    return _raise_for_saxo_response(response, action="Chart data")


def get_balance_snapshot(config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/port/v1/balances/me",
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    return _raise_for_saxo_response(response, action="Balance snapshot")


def get_positions_snapshot(
    config: dict[str, Any],
    session: dict[str, Any],
    *,
    top: int = 200,
) -> list[dict[str, Any]]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/port/v1/positions/me",
        params={
            "$top": int(top),
            "FieldGroups": "DisplayAndFormat,PositionBase,PositionView",
        },
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    payload = _raise_for_saxo_response(response, action="Positions snapshot")
    return [dict(row) for row in payload.get("Data", [])]


def get_accounts_snapshot(
    config: dict[str, Any],
    session: dict[str, Any],
    *,
    top: int = 100,
) -> list[dict[str, Any]]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/port/v1/accounts/me",
        params={"$top": int(top)},
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    payload = _raise_for_saxo_response(response, action="Accounts snapshot")
    return [dict(row) for row in payload.get("Data", [])]


def get_instrument_exposures(
    config: dict[str, Any],
    session: dict[str, Any],
) -> list[dict[str, Any]]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/port/v1/exposure/instruments/me",
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    payload = _response_json_or_none(response)
    status_code = int(getattr(response, "status_code", 200))
    if status_code >= 400:
        error_text = _extract_saxo_error(payload)
        if error_text:
            raise SaxoSessionError(f"Instrument exposures failed: {error_text}")
        raise SaxoSessionError(f"Instrument exposures failed: HTTP {status_code}")
    if isinstance(payload, list):
        return [dict(row) for row in payload]
    return [dict(row) for row in (payload or {}).get("Data", [])]


def change_order(payload: dict[str, Any], config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = _send_order_request(
        "PATCH",
        f"{base_url}/trade/v2/orders",
        headers=_auth_headers(session["access_token"]),
        json=payload,
    )
    return _raise_for_saxo_response(response, action="Order replace")


def cancel_order(order_id: str, config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = _send_order_request(
        "DELETE",
        f"{base_url}/trade/v2/orders/{order_id}",
        params={"AccountKey": _account_key(config, session)},
        headers=_auth_headers(session["access_token"]),
    )
    return _raise_for_saxo_response(response, action="Order cancel")


def get_open_order(order_id: str, config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/port/v1/orders/{_client_key(config, session)}/{order_id}",
        params={"FieldGroups": "DisplayAndFormat"},
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    if response.status_code == 404:
        raise SaxoOrderNotFoundError(f"Saxo open order {order_id} was not found in open orders")
    response.raise_for_status()
    payload = response.json()
    data = payload.get("Data", [])
    if not data:
        raise SaxoOrderNotFoundError(f"Saxo open order {order_id} returned no rows")
    return data[0]


def get_order_activity_last(order_id: str, config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    base_url = _openapi_base_url(str(session.get("environment") or config["saxo"]["environment"]))
    response = requests.get(
        f"{base_url}/cs/v1/audit/orderactivities",
        params={
            "AccountKey": _account_key(config, session),
            "ClientKey": _client_key(config, session),
            "EntryType": "Last",
            "OrderId": order_id,
        },
        headers=_auth_headers(session["access_token"]),
        timeout=30,
    )
    response.raise_for_status()
    payload = response.json()
    data = payload.get("Data", [])
    if not data:
        raise SaxoOrderNotFoundError(f"Saxo order activity {order_id} returned no rows")
    return data[0]
