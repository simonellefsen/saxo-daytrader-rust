from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import requests

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.market_symbols import parse_exchange_code, symbol_base
from saxo_daytrader_xai.saxo_openapi import SaxoSessionError, ensure_access_token, lookup_instrument


@dataclass(frozen=True)
class InstrumentIdentity:
    symbol: str
    instrument_name: str
    isin: str | None = None
    figi: str | None = None
    source: str = "fallback"


def _load_default_config() -> dict[str, Any]:
    root = Path(__file__).resolve().parents[2]
    return load_config(root / "config.yaml")


def _saxo_identity(symbol: str, config: dict[str, Any]) -> InstrumentIdentity | None:
    try:
        session = ensure_access_token(config, config.get("saxo", {}).get("session_path"))
        instrument = lookup_instrument(symbol, config, session)
        return InstrumentIdentity(
            symbol=symbol,
            instrument_name=instrument.description or symbol,
            isin=(instrument.isin_code or None),
            figi=None,
            source="saxo",
        )
    except (SaxoSessionError, requests.RequestException, ValueError):
        return None


def _openfigi_enabled(config: dict[str, Any]) -> bool:
    openfigi_cfg = config.get("openfigi", {})
    return bool(openfigi_cfg.get("enabled", False))


def _openfigi_identity(symbol: str, currency: str | None, config: dict[str, Any]) -> InstrumentIdentity | None:
    if not _openfigi_enabled(config):
        return None
    openfigi_cfg = config.get("openfigi", {})
    base_url = str(openfigi_cfg.get("base_url") or "https://api.openfigi.com/v3").rstrip("/")
    timeout_seconds = int(openfigi_cfg.get("timeout_seconds", 10))
    api_key = str(openfigi_cfg.get("api_key") or "").strip()
    exchange_code = parse_exchange_code(symbol).upper()
    job: dict[str, Any] = {
        "idType": "TICKER",
        "idValue": symbol_base(symbol),
        "marketSecDes": "Equity",
    }
    if exchange_code:
        job["micCode"] = exchange_code
    if currency:
        job["currency"] = currency
    headers = {
        "Accept": "application/json",
        "Content-Type": "application/json",
    }
    if api_key:
        headers["X-OPENFIGI-APIKEY"] = api_key
    try:
        response = requests.post(
            f"{base_url}/mapping",
            json=[job],
            headers=headers,
            timeout=timeout_seconds,
        )
        response.raise_for_status()
        payload = response.json()
    except (requests.RequestException, ValueError):
        return None
    if not isinstance(payload, list) or not payload:
        return None
    first = payload[0]
    if not isinstance(first, dict):
        return None
    data = first.get("data")
    if not isinstance(data, list) or not data:
        return None
    candidate = data[0]
    if not isinstance(candidate, dict):
        return None
    return InstrumentIdentity(
        symbol=symbol,
        instrument_name=str(candidate.get("name") or candidate.get("securityDescription") or symbol),
        isin=None,
        figi=(str(candidate.get("figi")) if candidate.get("figi") else None),
        source="openfigi",
    )


def resolve_instrument_identity(
    symbol: str,
    *,
    currency: str | None = None,
    config: dict[str, Any] | None = None,
) -> InstrumentIdentity:
    resolved_config = config or _load_default_config()
    saxo_identity = _saxo_identity(symbol, resolved_config)
    if saxo_identity and (saxo_identity.isin or saxo_identity.instrument_name != symbol):
        return saxo_identity
    openfigi_identity = _openfigi_identity(symbol, currency, resolved_config)
    if openfigi_identity:
        if saxo_identity and saxo_identity.isin:
            return InstrumentIdentity(
                symbol=symbol,
                instrument_name=saxo_identity.instrument_name or openfigi_identity.instrument_name,
                isin=saxo_identity.isin,
                figi=openfigi_identity.figi,
                source="saxo+openfigi",
            )
        return openfigi_identity
    if saxo_identity:
        return saxo_identity
    return InstrumentIdentity(symbol=symbol, instrument_name=symbol, isin=None, figi=None, source="fallback")
