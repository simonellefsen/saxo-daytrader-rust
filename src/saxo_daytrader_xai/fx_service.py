from __future__ import annotations

import xml.etree.ElementTree as ET
from datetime import UTC, datetime
from typing import Any

import requests


ECB_DAILY_URL = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml"
ECB_NS = {
    "gesmes": "http://www.gesmes.org/xml/2002-08-01",
    "eurofxref": "http://www.ecb.int/vocabulary/2002-08-01/eurofxref",
}
FALLBACK_RATES = {
    "EUR": 1.0,
    "DKK": 7.4604,
    "USD": 7.0215,
    "SEK": 0.6881,
    "NOK": 0.6492,
    "GBP": 8.2178,
}


def _fallback_snapshot(reason: str) -> dict[str, Any]:
    return {
        "base": "EUR",
        "as_of": datetime.now(UTC).isoformat(timespec="seconds"),
        "rates": FALLBACK_RATES.copy(),
        "source": "fallback",
        "warning": reason,
    }


def fetch_ecb_fx_rates() -> dict[str, Any]:
    try:
        response = requests.get(ECB_DAILY_URL, timeout=20)
        response.raise_for_status()
        root = ET.fromstring(response.content)
        rates: dict[str, float] = {"EUR": 1.0}
        for cube in root.findall(".//eurofxref:Cube[@currency]", ECB_NS):
            rates[cube.attrib["currency"]] = float(cube.attrib["rate"])
        return {
            "base": "EUR",
            "as_of": datetime.now(UTC).isoformat(timespec="seconds"),
            "rates": rates,
            "source": "ecb",
        }
    except Exception as exc:  # noqa: BLE001
        return _fallback_snapshot(str(exc))


def fx_rate_to_dkk(currency: str, fx_snapshot: dict[str, Any] | None = None) -> float:
    code = currency.upper()
    if code == "DKK":
        return 1.0
    snapshot = fx_snapshot or fetch_ecb_fx_rates()
    rates = snapshot["rates"]
    if code == "EUR":
        return rates["DKK"]
    if code not in rates:
        raise ValueError(f"ECB snapshot does not contain {code}")
    return rates["DKK"] / rates[code]
