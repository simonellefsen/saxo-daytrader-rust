from __future__ import annotations

import copy
import threading
from datetime import UTC, datetime
from typing import Any

from saxo_daytrader_xai.market_data import fetch_live_prices
from saxo_daytrader_xai.market_symbols import SymbolSpec


US_EXCHANGES = {"xnas", "xnys"}
UK_EXCHANGES = {"xlon"}
EU_EXCHANGES = {"xetr", "xfra", "xmil", "xpar", "xams", "xbru", "xlse"}
_WATCHLIST_CACHE_LOCK = threading.Lock()
_WATCHLIST_CACHE: dict[str, Any] = {}


NORDIC_UNIVERSE: list[SymbolSpec] = [
    SymbolSpec("ORSTED:xcse", "ORSTED.CO", "Orsted", "xcse", "Nordics", "DKK"),
    SymbolSpec("MAERSK-B:xcse", "MAERSK-B.CO", "A.P. Moller - Maersk B", "xcse", "Nordics", "DKK"),
    SymbolSpec("DSV:xcse", "DSV.CO", "DSV", "xcse", "Nordics", "DKK"),
    SymbolSpec("CARL-B:xcse", "CARL-B.CO", "Carlsberg B", "xcse", "Nordics", "DKK"),
    SymbolSpec("COLO-B:xcse", "COLO-B.CO", "Coloplast B", "xcse", "Nordics", "DKK"),
    SymbolSpec("DEMANT:xcse", "DEMANT.CO", "Demant", "xcse", "Nordics", "DKK"),
    SymbolSpec("VWS:xcse", "VWS.CO", "Vestas", "xcse", "Nordics", "DKK"),
    SymbolSpec("PNDORA:xcse", "PNDORA.CO", "Pandora", "xcse", "Nordics", "DKK"),
    SymbolSpec("GN:xcse", "GN.CO", "GN Store Nord", "xcse", "Nordics", "DKK"),
    SymbolSpec("NZYM-B:xcse", "NZYM-B.CO", "Novonesis B", "xcse", "Nordics", "DKK"),
    SymbolSpec("NETC:xcse", "NETC.CO", "Netcompany", "xcse", "Nordics", "DKK"),
    SymbolSpec("TRYG:xcse", "TRYG.CO", "Tryg", "xcse", "Nordics", "DKK"),
    SymbolSpec("ISS:xcse", "ISS.CO", "ISS", "xcse", "Nordics", "DKK"),
    SymbolSpec("BAVA:xcse", "BAVA.CO", "Bavarian Nordic", "xcse", "Nordics", "DKK"),
    SymbolSpec("ALK-B:xcse", "ALK-B.CO", "ALK-Abello B", "xcse", "Nordics", "DKK"),
    SymbolSpec("NNIT:xcse", "NNIT.CO", "NNIT", "xcse", "Nordics", "DKK"),
    SymbolSpec("AMBU-B:xcse", "AMBU-B.CO", "Ambu B", "xcse", "Nordics", "DKK"),
    SymbolSpec("FLS:xcse", "FLS.CO", "FLSmidth", "xcse", "Nordics", "DKK"),
    SymbolSpec("CHEMM:xcse", "CHEMM.CO", "Chemometec", "xcse", "Nordics", "DKK"),
    SymbolSpec("SCHP:xcse", "SCHP.CO", "Schouw & Co.", "xcse", "Nordics", "DKK"),
    SymbolSpec("RILBA:xcse", "RILBA.CO", "Ringkjobing Landbobank", "xcse", "Nordics", "DKK"),
    SymbolSpec("STG:xcse", "STG.CO", "Scandinavian Tobacco Group", "xcse", "Nordics", "DKK"),
    SymbolSpec("ROCK-B:xcse", "ROCK-B.CO", "Rockwool B", "xcse", "Nordics", "DKK"),
    SymbolSpec("MATAS:xcse", "MATAS.CO", "Matas", "xcse", "Nordics", "DKK"),
    SymbolSpec("ZEAL:xcse", "ZEAL.CO", "Zealand Pharma", "xcse", "Nordics", "DKK"),
    SymbolSpec("DANSKE:xcse", "DANSKE.CO", "Danske Bank", "xcse", "Nordics", "DKK"),
    SymbolSpec("NKT:xcse", "NKT.CO", "NKT", "xcse", "Nordics", "DKK"),
    SymbolSpec("ALMB:xcse", "ALMB.CO", "Alm. Brand", "xcse", "Nordics", "DKK"),
    SymbolSpec("DFDS:xcse", "DFDS.CO", "DFDS", "xcse", "Nordics", "DKK"),
    SymbolSpec("DNB:xosl", "DNB.OL", "DNB Bank", "xosl", "Nordics", "NOK"),
    SymbolSpec("EQNR:xosl", "EQNR.OL", "Equinor", "xosl", "Nordics", "NOK"),
    SymbolSpec("SALM:xosl", "SALM.OL", "Salmar", "xosl", "Nordics", "NOK"),
    SymbolSpec("MOWI:xosl", "MOWI.OL", "Mowi", "xosl", "Nordics", "NOK"),
    SymbolSpec("AKRBP:xosl", "AKRBP.OL", "Aker BP", "xosl", "Nordics", "NOK"),
    SymbolSpec("YAR:xosl", "YAR.OL", "Yara International", "xosl", "Nordics", "NOK"),
    SymbolSpec("NHY:xosl", "NHY.OL", "Norsk Hydro", "xosl", "Nordics", "NOK"),
    SymbolSpec("TEL:xosl", "TEL.OL", "Telenor", "xosl", "Nordics", "NOK"),
    SymbolSpec("TOM:xosl", "TOM.OL", "Tomra Systems", "xosl", "Nordics", "NOK"),
    SymbolSpec("GJF:xosl", "GJF.OL", "Gjensidige Forsikring", "xosl", "Nordics", "NOK"),
    SymbolSpec("ORK:xosl", "ORK.OL", "Orkla", "xosl", "Nordics", "NOK"),
    SymbolSpec("BAKKA:xosl", "BAKKA.OL", "Bakkafrost", "xosl", "Nordics", "NOK"),
    SymbolSpec("AUTO:xosl", "AUTO.OL", "AutoStore", "xosl", "Nordics", "NOK"),
    SymbolSpec("VOLV-B:xsto", "VOLV-B.ST", "Volvo B", "xsto", "Nordics", "SEK"),
    SymbolSpec("ATCO-A:xsto", "ATCO-A.ST", "Atlas Copco A", "xsto", "Nordics", "SEK"),
    SymbolSpec("ABB:xsto", "ABB.ST", "ABB", "xsto", "Nordics", "SEK"),
    SymbolSpec("ERIC-B:xsto", "ERIC-B.ST", "Ericsson B", "xsto", "Nordics", "SEK"),
    SymbolSpec("HEXA-B:xsto", "HEXA-B.ST", "Hexagon B", "xsto", "Nordics", "SEK"),
    SymbolSpec("NDA-SE:xsto", "NDA-SE.ST", "Nordea", "xsto", "Nordics", "SEK"),
    SymbolSpec("SAND:xsto", "SAND.ST", "Sandvik", "xsto", "Nordics", "SEK"),
    SymbolSpec("ALFA:xsto", "ALFA.ST", "Alfa Laval", "xsto", "Nordics", "SEK"),
    SymbolSpec("ASSA-B:xsto", "ASSA-B.ST", "Assa Abloy B", "xsto", "Nordics", "SEK"),
    SymbolSpec("SWED-A:xsto", "SWED-A.ST", "Swedbank A", "xsto", "Nordics", "SEK"),
    SymbolSpec("SEB-A:xsto", "SEB-A.ST", "SEB A", "xsto", "Nordics", "SEK"),
    SymbolSpec("SHB-A:xsto", "SHB-A.ST", "Handelsbanken A", "xsto", "Nordics", "SEK"),
    SymbolSpec("INVE-B:xsto", "INVE-B.ST", "Investor B", "xsto", "Nordics", "SEK"),
    SymbolSpec("TELIA:xsto", "TELIA.ST", "Telia Company", "xsto", "Nordics", "SEK"),
    SymbolSpec("SKF-B:xsto", "SKF-B.ST", "SKF B", "xsto", "Nordics", "SEK"),
    SymbolSpec("SCA-B:xsto", "SCA-B.ST", "SCA B", "xsto", "Nordics", "SEK"),
    SymbolSpec("ESSITY-B:xsto", "ESSITY-B.ST", "Essity B", "xsto", "Nordics", "SEK"),
    SymbolSpec("BOL:xsto", "BOL.ST", "Boliden", "xsto", "Nordics", "SEK"),
    SymbolSpec("ELUX-B:xsto", "ELUX-B.ST", "Electrolux B", "xsto", "Nordics", "SEK"),
    SymbolSpec("HM-B:xsto", "HM-B.ST", "H&M B", "xsto", "Nordics", "SEK"),
    SymbolSpec("KNEBV:xhel", "KNEBV.HE", "Kone", "xhel", "Nordics", "EUR"),
    SymbolSpec("NESTE:xhel", "NESTE.HE", "Neste", "xhel", "Nordics", "EUR"),
    SymbolSpec("NOKIA:xhel", "NOKIA.HE", "Nokia", "xhel", "Nordics", "EUR"),
    SymbolSpec("UPM:xhel", "UPM.HE", "UPM-Kymmene", "xhel", "Nordics", "EUR"),
    SymbolSpec("STERV:xhel", "STERV.HE", "Stora Enso R", "xhel", "Nordics", "EUR"),
    SymbolSpec("SAMPO:xhel", "SAMPO.HE", "Sampo", "xhel", "Nordics", "EUR"),
    SymbolSpec("ELISA:xhel", "ELISA.HE", "Elisa", "xhel", "Nordics", "EUR"),
    SymbolSpec("KESKOB:xhel", "KESKOB.HE", "Kesko B", "xhel", "Nordics", "EUR"),
    SymbolSpec("METSO:xhel", "METSO.HE", "Metso", "xhel", "Nordics", "EUR"),
    SymbolSpec("FORTUM:xhel", "FORTUM.HE", "Fortum", "xhel", "Nordics", "EUR"),
    SymbolSpec("WRT1V:xhel", "WRT1V.HE", "Wartsila", "xhel", "Nordics", "EUR"),
    SymbolSpec("OUT1V:xhel", "OUT1V.HE", "Outokumpu", "xhel", "Nordics", "EUR"),
]


GLOBAL_UNIVERSE: list[SymbolSpec] = [
    SymbolSpec("AAPL:xnas", "AAPL", "Apple", "xnas", "Global", "USD"),
    SymbolSpec("MSFT:xnas", "MSFT", "Microsoft", "xnas", "Global", "USD"),
    SymbolSpec("NVDA:xnas", "NVDA", "NVIDIA", "xnas", "Global", "USD"),
    SymbolSpec("AMD:xnas", "AMD", "Advanced Micro Devices", "xnas", "Global", "USD"),
    SymbolSpec("GOOGL:xnas", "GOOGL", "Alphabet Class A", "xnas", "Global", "USD"),
    SymbolSpec("AMZN:xnas", "AMZN", "Amazon", "xnas", "Global", "USD"),
    SymbolSpec("META:xnas", "META", "Meta Platforms", "xnas", "Global", "USD"),
    SymbolSpec("PLTR:xnas", "PLTR", "Palantir", "xnas", "Global", "USD"),
    SymbolSpec("MSTR:xnas", "MSTR", "MicroStrategy", "xnas", "Global", "USD"),
    SymbolSpec("NFLX:xnas", "NFLX", "Netflix", "xnas", "Global", "USD"),
    SymbolSpec("AVGO:xnas", "AVGO", "Broadcom", "xnas", "Global", "USD"),
    SymbolSpec("ASML:xnas", "ASML", "ASML ADR", "xnas", "Global", "USD"),
    SymbolSpec("ADBE:xnas", "ADBE", "Adobe", "xnas", "Global", "USD"),
    SymbolSpec("QCOM:xnas", "QCOM", "Qualcomm", "xnas", "Global", "USD"),
    SymbolSpec("INTC:xnas", "INTC", "Intel", "xnas", "Global", "USD"),
    SymbolSpec("CSCO:xnas", "CSCO", "Cisco", "xnas", "Global", "USD"),
    SymbolSpec("AMAT:xnas", "AMAT", "Applied Materials", "xnas", "Global", "USD"),
    SymbolSpec("ARM:xnas", "ARM", "Arm Holdings ADR", "xnas", "Global", "USD"),
    SymbolSpec("FIGR:xnas", "FIGR", "FIGS", "xnas", "Global", "USD"),
    SymbolSpec("RIVN:xnas", "RIVN", "Rivian", "xnas", "Global", "USD"),
    SymbolSpec("JNJ:xnys", "JNJ", "Johnson & Johnson", "xnys", "Global", "USD"),
    SymbolSpec("JPM:xnys", "JPM", "JPMorgan Chase", "xnys", "Global", "USD"),
    SymbolSpec("V:xnys", "V", "Visa", "xnys", "Global", "USD"),
    SymbolSpec("MA:xnys", "MA", "Mastercard", "xnys", "Global", "USD"),
    SymbolSpec("AJG:xnys", "AJG", "Arthur J. Gallagher", "xnys", "Global", "USD"),
    SymbolSpec("ZTS:xnys", "ZTS", "Zoetis", "xnys", "Global", "USD"),
    SymbolSpec("LLY:xnys", "LLY", "Eli Lilly", "xnys", "Global", "USD"),
    SymbolSpec("GS:xnys", "GS", "Goldman Sachs", "xnys", "Global", "USD"),
    SymbolSpec("CAT:xnys", "CAT", "Caterpillar", "xnys", "Global", "USD"),
    SymbolSpec("GE:xnys", "GE", "GE Aerospace", "xnys", "Global", "USD"),
    SymbolSpec("UBER:xnys", "UBER", "Uber", "xnys", "Global", "USD"),
    SymbolSpec("SHOP:xnys", "SHOP", "Shopify", "xnys", "Global", "USD"),
    SymbolSpec("SAP:xetr", "SAP.DE", "SAP", "xetr", "Global", "EUR"),
    SymbolSpec("SIE:xetr", "SIE.DE", "Siemens", "xetr", "Global", "EUR"),
    SymbolSpec("AIR:xpar", "AIR.PA", "Airbus", "xpar", "Global", "EUR"),
    SymbolSpec("MC:xpar", "MC.PA", "LVMH", "xpar", "Global", "EUR"),
    SymbolSpec("OR:xpar", "OR.PA", "L'Oreal", "xpar", "Global", "EUR"),
    SymbolSpec("SAN:xpar", "SAN.PA", "Sanofi", "xpar", "Global", "EUR"),
    SymbolSpec("SU:xpar", "SU.PA", "Schneider Electric", "xpar", "Global", "EUR"),
    SymbolSpec("ABI:xbru", "ABI.BR", "AB InBev", "xbru", "Global", "EUR"),
    SymbolSpec("ASML:xams", "ASML.AS", "ASML Amsterdam", "xams", "Global", "EUR"),
    SymbolSpec("ADYEN:xams", "ADYEN.AS", "Adyen", "xams", "Global", "EUR"),
    SymbolSpec("RMS:xpar", "RMS.PA", "Hermes", "xpar", "Global", "EUR"),
    SymbolSpec("SHELL:xlon", "SHEL.L", "Shell", "xlon", "Global", "GBP"),
    SymbolSpec("AZN:xlon", "AZN.L", "AstraZeneca", "xlon", "Global", "GBP"),
    SymbolSpec("ULVR:xlon", "ULVR.L", "Unilever", "xlon", "Global", "GBP"),
    SymbolSpec("HSBA:xlon", "HSBA.L", "HSBC", "xlon", "Global", "GBP"),
    SymbolSpec("RIO:xlon", "RIO.L", "Rio Tinto", "xlon", "Global", "GBP"),
    SymbolSpec("RR:xlon", "RR.L", "Rolls-Royce", "xlon", "Global", "GBP"),
    SymbolSpec("QOMP:xetr", "QOMP.DE", "iShares MSCI World Quality Factor", "xetr", "Global", "EUR"),
    SymbolSpec("ARKI:xlon", "ARKI.L", "ARK AI & Robotics UCITS", "xlon", "Global", "USD"),
    SymbolSpec("ARKK:xmil", "ARKK.MI", "ARK Innovation UCITS", "xmil", "Global", "EUR"),
    SymbolSpec("ADI:xnas", "ADI", "Analog Devices", "xnas", "Global", "USD"),
    SymbolSpec("LMND:xnys", "LMND", "Lemonade", "xnys", "Global", "USD"),
    SymbolSpec("AMGN:xnas", "AMGN", "Amgen", "xnas", "Global", "USD"),
    SymbolSpec("BKNG:xnas", "BKNG", "Booking Holdings", "xnas", "Global", "USD"),
    SymbolSpec("DDOG:xnas", "DDOG", "Datadog", "xnas", "Global", "USD"),
    SymbolSpec("GILD:xnas", "GILD", "Gilead Sciences", "xnas", "Global", "USD"),
    SymbolSpec("INTU:xnas", "INTU", "Intuit", "xnas", "Global", "USD"),
    SymbolSpec("ISRG:xnas", "ISRG", "Intuitive Surgical", "xnas", "Global", "USD"),
    SymbolSpec("MDB:xnas", "MDB", "MongoDB", "xnas", "Global", "USD"),
    SymbolSpec("MELI:xnas", "MELI", "MercadoLibre", "xnas", "Global", "USD"),
    SymbolSpec("MU:xnas", "MU", "Micron", "xnas", "Global", "USD"),
    SymbolSpec("PANW:xnas", "PANW", "Palo Alto Networks", "xnas", "Global", "USD"),
    SymbolSpec("PEP:xnas", "PEP", "PepsiCo", "xnas", "Global", "USD"),
    SymbolSpec("PYPL:xnas", "PYPL", "PayPal", "xnas", "Global", "USD"),
    SymbolSpec("SBUX:xnas", "SBUX", "Starbucks", "xnas", "Global", "USD"),
    SymbolSpec("TXN:xnas", "TXN", "Texas Instruments", "xnas", "Global", "USD"),
    SymbolSpec("ABNB:xnas", "ABNB", "Airbnb", "xnas", "Global", "USD"),
    SymbolSpec("CEG:xnas", "CEG", "Constellation Energy", "xnas", "Global", "USD"),
    SymbolSpec("BAC:xnys", "BAC", "Bank of America", "xnys", "Global", "USD"),
    SymbolSpec("BRK-B:xnys", "BRK-B", "Berkshire Hathaway B", "xnys", "Global", "USD"),
    SymbolSpec("CMG:xnys", "CMG", "Chipotle Mexican Grill", "xnys", "Global", "USD"),
    SymbolSpec("COP:xnys", "COP", "ConocoPhillips", "xnys", "Global", "USD"),
    SymbolSpec("COST:xnys", "COST", "Costco", "xnys", "Global", "USD"),
    SymbolSpec("CRM:xnys", "CRM", "Salesforce", "xnys", "Global", "USD"),
    SymbolSpec("DE:xnys", "DE", "Deere & Co.", "xnys", "Global", "USD"),
    SymbolSpec("DIS:xnys", "DIS", "Walt Disney", "xnys", "Global", "USD"),
    SymbolSpec("HD:xnys", "HD", "Home Depot", "xnys", "Global", "USD"),
    SymbolSpec("HON:xnys", "HON", "Honeywell", "xnys", "Global", "USD"),
    SymbolSpec("IBM:xnys", "IBM", "IBM", "xnys", "Global", "USD"),
    SymbolSpec("KO:xnys", "KO", "Coca-Cola", "xnys", "Global", "USD"),
    SymbolSpec("LIN:xnys", "LIN", "Linde", "xnys", "Global", "USD"),
    SymbolSpec("MCD:xnys", "MCD", "McDonald's", "xnys", "Global", "USD"),
    SymbolSpec("NKE:xnys", "NKE", "Nike", "xnys", "Global", "USD"),
    SymbolSpec("NOW:xnys", "NOW", "ServiceNow", "xnys", "Global", "USD"),
    SymbolSpec("ORCL:xnys", "ORCL", "Oracle", "xnys", "Global", "USD"),
    SymbolSpec("PG:xnys", "PG", "Procter & Gamble", "xnys", "Global", "USD"),
    SymbolSpec("RTX:xnys", "RTX", "RTX", "xnys", "Global", "USD"),
    SymbolSpec("SNOW:xnys", "SNOW", "Snowflake", "xnys", "Global", "USD"),
    SymbolSpec("SPOT:xnys", "SPOT", "Spotify", "xnys", "Global", "USD"),
    SymbolSpec("TMO:xnys", "TMO", "Thermo Fisher Scientific", "xnys", "Global", "USD"),
    SymbolSpec("TSM:xnys", "TSM", "Taiwan Semiconductor ADR", "xnys", "Global", "USD"),
    SymbolSpec("UNH:xnys", "UNH", "UnitedHealth Group", "xnys", "Global", "USD"),
    SymbolSpec("WMT:xnys", "WMT", "Walmart", "xnys", "Global", "USD"),
    SymbolSpec("XOM:xnys", "XOM", "Exxon Mobil", "xnys", "Global", "USD"),
    SymbolSpec("ALV:xetr", "ALV.DE", "Allianz", "xetr", "Global", "EUR"),
    SymbolSpec("ADS:xetr", "ADS.DE", "Adidas", "xetr", "Global", "EUR"),
    SymbolSpec("BAS:xetr", "BAS.DE", "BASF", "xetr", "Global", "EUR"),
    SymbolSpec("BMW:xetr", "BMW.DE", "BMW", "xetr", "Global", "EUR"),
    SymbolSpec("DB1:xetr", "DB1.DE", "Deutsche Boerse", "xetr", "Global", "EUR"),
    SymbolSpec("DTE:xetr", "DTE.DE", "Deutsche Telekom", "xetr", "Global", "EUR"),
    SymbolSpec("IFX:xetr", "IFX.DE", "Infineon", "xetr", "Global", "EUR"),
    SymbolSpec("MBG:xetr", "MBG.DE", "Mercedes-Benz Group", "xetr", "Global", "EUR"),
    SymbolSpec("VOW3:xetr", "VOW3.DE", "Volkswagen Pref", "xetr", "Global", "EUR"),
    SymbolSpec("BNP:xpar", "BNP.PA", "BNP Paribas", "xpar", "Global", "EUR"),
    SymbolSpec("CAP:xpar", "CAP.PA", "Capgemini", "xpar", "Global", "EUR"),
    SymbolSpec("DG:xpar", "DG.PA", "Vinci", "xpar", "Global", "EUR"),
    SymbolSpec("ENGI:xpar", "ENGI.PA", "Engie", "xpar", "Global", "EUR"),
    SymbolSpec("SAF:xpar", "SAF.PA", "Safran", "xpar", "Global", "EUR"),
    SymbolSpec("TTE:xpar", "TTE.PA", "TotalEnergies", "xpar", "Global", "EUR"),
    SymbolSpec("ENEL:xmil", "ENEL.MI", "Enel", "xmil", "Global", "EUR"),
    SymbolSpec("ENI:xmil", "ENI.MI", "Eni", "xmil", "Global", "EUR"),
    SymbolSpec("ISP:xmil", "ISP.MI", "Intesa Sanpaolo", "xmil", "Global", "EUR"),
    SymbolSpec("LDO:xmil", "LDO.MI", "Leonardo", "xmil", "Global", "EUR"),
    SymbolSpec("STLAM:xmil", "STLAM.MI", "Stellantis", "xmil", "Global", "EUR"),
    SymbolSpec("UCG:xmil", "UCG.MI", "UniCredit", "xmil", "Global", "EUR"),
    SymbolSpec("BARC:xlon", "BARC.L", "Barclays", "xlon", "Global", "GBP"),
    SymbolSpec("GSK:xlon", "GSK.L", "GSK", "xlon", "Global", "GBP"),
    SymbolSpec("LSEG:xlon", "LSEG.L", "London Stock Exchange Group", "xlon", "Global", "GBP"),
    SymbolSpec("NG:xlon", "NG.L", "National Grid", "xlon", "Global", "GBP"),
    SymbolSpec("REL:xlon", "REL.L", "RELX", "xlon", "Global", "GBP"),
]


def _rank_watchlist_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(
        rows,
        key=lambda row: (
            row["change_pct"] is None,
            -(row["change_pct"] or -999.0),
            -float(row["current_price"] or 0.0),
            row["symbol"],
        ),
    )


def _build_watchlist_rows(
    entries: list[SymbolSpec],
    config: dict[str, Any],
    excluded_symbols: set[str],
) -> list[dict[str, Any]]:
    filtered_entries = [entry for entry in entries if entry.symbol not in excluded_symbols]
    quotes = fetch_live_prices(
        [entry.symbol for entry in filtered_entries],
        timeout_seconds=config["market_data"]["request_timeout_seconds"],
        symbol_to_yahoo={entry.symbol: entry.yahoo_symbol for entry in filtered_entries},
    )
    quote_by_symbol = {row["symbol"]: row for row in quotes}
    rows = []
    for entry in filtered_entries:
        quote = quote_by_symbol.get(entry.symbol, {})
        rows.append(
            {
                "symbol": entry.symbol,
                "name": entry.name,
                "yahoo_symbol": entry.yahoo_symbol,
                "exchange": entry.exchange_code.upper(),
                "region": entry.region,
                "currency": entry.currency,
                "current_price": quote.get("current_price"),
                "change_pct": quote.get("change_pct"),
                "quote_source": quote.get("source", "unavailable"),
                "quote_status": quote.get("status", "No quote"),
            }
        )
    return _rank_watchlist_rows(rows)


def _category_payload(
    *,
    key: str,
    label: str,
    target_limit: int,
    total_universe: int,
    rows: list[dict[str, Any]],
) -> dict[str, Any]:
    return {
        "key": key,
        "label": label,
        "target_limit": target_limit,
        "total_universe": total_universe,
        "items": rows[:target_limit],
    }


def _cache_key(config: dict[str, Any]) -> tuple[Any, ...]:
    watchlist_cfg = config["market_data"]["watchlists"]
    excluded_symbols = tuple(sorted(config.get("risk", {}).get("excluded_symbols", [])))
    return (
        excluded_symbols,
        int(watchlist_cfg.get("nordic_limit", 100)),
        int(watchlist_cfg.get("uk_limit", 25)),
        int(watchlist_cfg.get("us_limit", 100)),
        int(watchlist_cfg.get("eu_limit", 75)),
        int(watchlist_cfg.get("global_limit", 100)),
    )


def _cache_ttl_seconds(config: dict[str, Any]) -> int:
    return max(int(config.get("market_data", {}).get("refresh_interval_seconds", 300) or 300), 30)


def _cached_payload(payload: dict[str, Any], *, stale: bool, refreshing: bool) -> dict[str, Any]:
    output = copy.deepcopy(payload)
    output["cache_stale"] = stale
    output["cache_refreshing"] = refreshing
    return output


def _refresh_cache_in_background(config: dict[str, Any]) -> None:
    refresh_config = copy.deepcopy(config)

    def refresh() -> None:
        try:
            build_watchlists(refresh_config, force_refresh=True)
        finally:
            with _WATCHLIST_CACHE_LOCK:
                _WATCHLIST_CACHE["refreshing"] = False

    thread = threading.Thread(target=refresh, name="watchlist-cache-refresh", daemon=True)
    thread.start()


def build_watchlists(config: dict[str, Any], *, force_refresh: bool = False) -> dict[str, Any]:
    cache_key = _cache_key(config)
    now = datetime.now(UTC)
    ttl_seconds = _cache_ttl_seconds(config)
    start_background_refresh = False
    with _WATCHLIST_CACHE_LOCK:
        cached = _WATCHLIST_CACHE.get("payload")
        cached_key = _WATCHLIST_CACHE.get("key")
        cached_at = _WATCHLIST_CACHE.get("cached_at")
        if not force_refresh and cached is not None and cached_key == cache_key and isinstance(cached_at, datetime):
            stale = (now - cached_at).total_seconds() >= ttl_seconds
            if stale and not bool(_WATCHLIST_CACHE.get("refreshing")):
                _WATCHLIST_CACHE["refreshing"] = True
                start_background_refresh = True
            payload = _cached_payload(cached, stale=stale, refreshing=start_background_refresh)
            if start_background_refresh:
                _refresh_cache_in_background(config)
            return payload

    excluded_symbols = set(config.get("risk", {}).get("excluded_symbols", []))
    watchlist_cfg = config["market_data"]["watchlists"]
    nordic_rows = _build_watchlist_rows(NORDIC_UNIVERSE, config, excluded_symbols)
    global_rows = _build_watchlist_rows(GLOBAL_UNIVERSE, config, excluded_symbols)
    uk_rows = [row for row in global_rows if str(row["exchange"]).lower() in UK_EXCHANGES]
    us_rows = [row for row in global_rows if str(row["exchange"]).lower() in US_EXCHANGES]
    eu_rows = [row for row in global_rows if str(row["exchange"]).lower() in EU_EXCHANGES]
    categories = [
        _category_payload(
            key="nordic",
            label="Nordics",
            target_limit=int(watchlist_cfg.get("nordic_limit", 100)),
            total_universe=len([entry for entry in NORDIC_UNIVERSE if entry.symbol not in excluded_symbols]),
            rows=nordic_rows,
        ),
        _category_payload(
            key="uk",
            label="UK",
            target_limit=int(watchlist_cfg.get("uk_limit", 25)),
            total_universe=len(
                [
                    entry
                    for entry in GLOBAL_UNIVERSE
                    if entry.symbol not in excluded_symbols and entry.exchange_code in UK_EXCHANGES
                ]
            ),
            rows=uk_rows,
        ),
        _category_payload(
            key="us",
            label="US",
            target_limit=int(watchlist_cfg.get("us_limit", 100)),
            total_universe=len(
                [
                    entry
                    for entry in GLOBAL_UNIVERSE
                    if entry.symbol not in excluded_symbols and entry.exchange_code in US_EXCHANGES
                ]
            ),
            rows=us_rows,
        ),
        _category_payload(
            key="eu",
            label="EU / Euronext",
            target_limit=int(watchlist_cfg.get("eu_limit", 75)),
            total_universe=len(
                [
                    entry
                    for entry in GLOBAL_UNIVERSE
                    if entry.symbol not in excluded_symbols and entry.exchange_code in EU_EXCHANGES
                ]
            ),
            rows=eu_rows,
        ),
    ]
    payload = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "cache_ttl_seconds": ttl_seconds,
        "cache_stale": False,
        "cache_refreshing": False,
        "categories": categories,
        "nordic": categories[0]["items"],
        "uk": categories[1]["items"],
        "us": categories[2]["items"],
        "eu": categories[3]["items"],
        "global": global_rows[: int(watchlist_cfg.get("global_limit", 100))],
    }
    with _WATCHLIST_CACHE_LOCK:
        _WATCHLIST_CACHE["key"] = cache_key
        _WATCHLIST_CACHE["cached_at"] = now
        _WATCHLIST_CACHE["payload"] = copy.deepcopy(payload)
    return payload
