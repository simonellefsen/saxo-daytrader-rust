from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai import saxo_openapi
from saxo_daytrader_xai.saxo_openapi import SaxoSessionError, lookup_instrument


class _FakeResponse:
    def __init__(self, payload: dict):
        self._payload = payload

    def raise_for_status(self) -> None:
        return None

    def json(self) -> dict:
        return self._payload


def main() -> int:
    config = {
        "_meta": {"config_dir": str(ROOT)},
        "saxo": {
            "environment": "SIM",
            "account_key": "demo-account",
        },
    }
    session = {
        "environment": "sim",
        "access_token": "demo-access-token",
        "account_key": "demo-account",
    }

    original_get = saxo_openapi.requests.get
    seen_params: list[dict] = []

    def fake_get(url: str, **kwargs):
        seen_params.append(dict(kwargs.get("params", {})))
        return _FakeResponse(
            {
                "Data": [
                    {
                        "Identifier": 288,
                        "AssetType": "Stock",
                        "ExchangeId": "NASDAQ",
                        "Symbol": "SBUX:xnas",
                        "Description": "Starbucks Corp.",
                        "TradableAs": ["Stock"],
                        "CurrencyCode": "USD",
                    },
                    {
                        "Identifier": 2271754,
                        "AssetType": "Stock",
                        "ExchangeId": "FSE",
                        "Symbol": "SRB:xetr",
                        "Description": "Starbucks Corp.",
                        "TradableAs": ["Stock"],
                        "CurrencyCode": "EUR",
                    },
                ]
            }
        )

    saxo_openapi.requests.get = fake_get
    try:
        instrument = lookup_instrument("SBUX:xnas", config, session)
    finally:
        saxo_openapi.requests.get = original_get

    assert instrument.uic == 288, instrument
    assert instrument.exchange_id == "NASDAQ", instrument
    assert instrument.description == "Starbucks Corp.", instrument
    assert seen_params, seen_params
    assert "ExchangeId" not in seen_params[0], seen_params[0]

    def fake_empty_get(url: str, **kwargs):
        return _FakeResponse({"Data": []})

    saxo_openapi.requests.get = fake_empty_get
    try:
        try:
            lookup_instrument("FIGR:xnas", config, session)
        except SaxoSessionError as exc:
            assert "No tradable Saxo instrument match found for FIGR:xnas" in str(exc), exc
        else:
            raise AssertionError("Expected FIGR:xnas lookup to fail when Saxo returns no candidates")
    finally:
        saxo_openapi.requests.get = original_get

    print("Phase 30 validation passed.")
    print(f"Resolved UIC for SBUX:xnas: {instrument.uic}")
    print(f"Resolved exchange id: {instrument.exchange_id}")
    print("FIGR:xnas remains unavailable when Saxo returns no tradable candidates.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
