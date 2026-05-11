from __future__ import annotations

from dataclasses import dataclass


EXCHANGE_TO_YAHOO_SUFFIX = {
    "xnas": "",
    "xnys": "",
    "xcse": ".CO",
    "xsto": ".ST",
    "xosl": ".OL",
    "xhel": ".HE",
    "xlon": ".L",
    "xetr": ".DE",
    "xfra": ".DE",
    "xmil": ".MI",
    "xpar": ".PA",
    "xams": ".AS",
    "xbru": ".BR",
    "xlse": ".LS",
}


EXCHANGE_LABELS = {
    "xnas": "Nasdaq US",
    "xnys": "NYSE",
    "xcse": "Nasdaq Copenhagen",
    "xsto": "Nasdaq Stockholm",
    "xosl": "Oslo Bors",
    "xhel": "Nasdaq Helsinki",
    "xlon": "London Stock Exchange",
    "xetr": "Xetra",
    "xfra": "Frankfurt",
    "xmil": "Borsa Italiana",
    "xpar": "Euronext Paris",
    "xams": "Euronext Amsterdam",
    "xbru": "Euronext Brussels",
    "xlse": "Euronext Lisbon",
}


@dataclass(frozen=True)
class SymbolSpec:
    symbol: str
    yahoo_symbol: str
    name: str
    exchange_code: str
    region: str
    currency: str


def parse_exchange_code(symbol: str) -> str:
    parts = symbol.split(":", 1)
    return parts[1].lower() if len(parts) == 2 else ""


def symbol_base(symbol: str) -> str:
    return symbol.split(":", 1)[0].strip().upper()


def saxo_to_yahoo(symbol: str) -> str:
    if ":" not in symbol:
        return symbol.strip().upper()
    exchange_code = parse_exchange_code(symbol)
    suffix = EXCHANGE_TO_YAHOO_SUFFIX.get(exchange_code, "")
    return f"{symbol_base(symbol)}{suffix}"


def exchange_label(symbol: str) -> str:
    code = parse_exchange_code(symbol)
    return EXCHANGE_LABELS.get(code, code.upper() if code else "Unknown")
