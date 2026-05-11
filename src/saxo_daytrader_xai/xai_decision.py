from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import requests

from saxo_daytrader_xai.analysis_pulses import analysis_pulse_status
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import append_audit_log, connect, init_db, record_analysis_pulse, record_swing_plan_snapshot
from saxo_daytrader_xai.market_data import fetch_live_prices
from saxo_daytrader_xai.market_news import fetch_market_intelligence
from saxo_daytrader_xai.market_schedule import get_market_status, summarize_analysis_window
from saxo_daytrader_xai.portfolio import (
    fetch_broker_account_summary,
    fetch_goal_tracking,
    fetch_latest_batch_id,
    fetch_portfolio_positions,
    fetch_portfolio_summary,
    fetch_portfolio_symbols,
)
from saxo_daytrader_xai.strategy_engine import (
    build_strategy_plan,
    strategy_capital_limits,
)
from saxo_daytrader_xai.strategy_journal import fetch_recent_journal_learnings
from saxo_daytrader_xai.watchlists import build_watchlists
from saxo_daytrader_xai.xai_request import apply_response_options, timeout_seconds


US_EXCHANGES = {"xnas", "xnys"}


def _load_default_config() -> dict[str, Any]:
    root = Path(__file__).resolve().parents[2]
    return load_config(root / "config.yaml")


def _get_connection_and_config(config: dict[str, Any] | None, connection):
    resolved_config = config or _load_default_config()
    resolved_connection = connection or connect(resolved_config["portfolio"]["database_path"])
    init_db(resolved_connection)
    return resolved_config, resolved_connection, connection is None


DECISION_REPORT_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "report_title": {"type": "string"},
        "analysis_window_active": {"type": "boolean"},
        "goal": {"type": "string"},
        "analysis_pulse_summary": {
            "type": "object",
            "properties": {
                "kind": {"type": "string"},
                "label": {"type": "string"},
                "macro_summary": {"type": "string"},
                "asia_summary": {"type": "string"},
                "us_setup_summary": {"type": "string"},
                "last_analysis_at": {"type": "string"},
            },
            "required": ["kind", "label", "macro_summary", "asia_summary", "us_setup_summary", "last_analysis_at"],
            "additionalProperties": False,
        },
        "market_regime": {
            "type": "object",
            "properties": {
                "bias": {"type": "string", "enum": ["bullish", "neutral", "defensive"]},
                "summary": {"type": "string"},
                "key_drivers": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["bias", "summary", "key_drivers"],
            "additionalProperties": False,
        },
        "portfolio_assessment": {
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "main_risks": {"type": "array", "items": {"type": "string"}},
                "strengths": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["summary", "main_risks", "strengths"],
            "additionalProperties": False,
        },
        "reasoning_steps": {"type": "array", "items": {"type": "string"}},
        "risk_rules_check": {"type": "array", "items": {"type": "string"}},
        "watchlist_focus": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "symbol": {"type": "string"},
                    "thesis": {"type": "string"},
                    "catalysts": {"type": "array", "items": {"type": "string"}},
                    "risks": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["symbol", "thesis", "catalysts", "risks"],
                "additionalProperties": False,
            },
        },
        "candidate_assets": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "symbol": {"type": "string"},
                    "direction": {"type": "string", "enum": ["BUY", "SELL", "WATCH"]},
                    "xai_score": {"type": "number"},
                    "sector": {"type": ["string", "null"]},
                    "thesis": {"type": "string"},
                    "catalysts": {"type": "array", "items": {"type": "string"}},
                    "risks": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["symbol", "direction", "xai_score", "sector", "thesis", "catalysts", "risks"],
                "additionalProperties": False,
            },
        },
        "symbol_sentiment": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "symbol": {"type": "string"},
                    "sentiment": {"type": "string", "enum": ["SELL", "UNDERWEIGHT", "HOLD", "OVERWEIGHT", "BUY"]},
                    "confidence": {"type": "number"},
                    "rationale": {"type": "string"},
                    "catalysts": {"type": "array", "items": {"type": "string"}},
                    "risk_notes": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["symbol", "sentiment", "confidence", "rationale", "catalysts", "risk_notes"],
                "additionalProperties": False,
            },
        },
        "suggested_trades": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["BUY", "SELL", "FLATTEN"]},
                    "symbol": {"type": "string"},
                    "target_weight_pct": {"type": "number"},
                    "quantity_hint": {"type": "string"},
                    "confidence": {"type": "number"},
                    "priority": {"type": "string", "enum": ["high", "medium"]},
                    "rationale": {"type": "string"},
                    "risk_notes": {"type": "array", "items": {"type": "string"}},
                },
                "required": [
                    "action",
                    "symbol",
                    "target_weight_pct",
                    "quantity_hint",
                    "confidence",
                    "priority",
                    "rationale",
                    "risk_notes",
                ],
                "additionalProperties": False,
            },
        },
        "execution_notes": {"type": "array", "items": {"type": "string"}},
        "daily_target_assessment": {"type": "string"},
    },
    "required": [
        "report_title",
        "analysis_window_active",
        "goal",
        "analysis_pulse_summary",
        "market_regime",
        "portfolio_assessment",
        "reasoning_steps",
        "risk_rules_check",
        "watchlist_focus",
        "candidate_assets",
        "symbol_sentiment",
        "suggested_trades",
        "execution_notes",
        "daily_target_assessment",
    ],
    "additionalProperties": False,
}


def _extract_output_text(response_json: dict[str, Any]) -> str:
    for item in response_json.get("output", []):
        if item.get("type") != "message":
            continue
        for content in item.get("content", []):
            if content.get("type") == "output_text":
                return content.get("text", "")
    return ""


def _summarize_market_regime(
    watchlists: dict[str, Any],
    portfolio_quotes: list[dict[str, Any]],
    market_news: dict[str, Any],
    market_status_rows: list[dict[str, Any]],
) -> dict[str, Any]:
    quote_changes = [row["change_pct"] for row in portfolio_quotes if row.get("change_pct") is not None]
    avg_quote_change = sum(quote_changes) / len(quote_changes) if quote_changes else 0.0
    nordic_leaders = [row["symbol"] for row in watchlists["nordic"][:3]]
    global_leaders = [row["symbol"] for row in watchlists["global"][:5]]
    open_markets = [row["market"] for row in market_status_rows if row["is_open"]]
    headline_titles = [item["title"] for item in market_news["market_news"][:3]]
    if avg_quote_change > 0.01:
        bias = "bullish"
    elif avg_quote_change < -0.01:
        bias = "defensive"
    else:
        bias = "neutral"
    summary = (
        f"Average live change across tracked portfolio symbols is {avg_quote_change * 100:.2f}%. "
        f"Open exchanges: {', '.join(open_markets) if open_markets else 'none currently open'}. "
        f"Top Nordic watchlist symbols: {', '.join(nordic_leaders)}. "
        f"Top global watchlist symbols: {', '.join(global_leaders)}."
    )
    return {
        "bias": bias,
        "summary": summary,
        "key_drivers": headline_titles,
    }


def _exchange_holiday_codes(market_status_rows: list[dict[str, Any]]) -> set[str]:
    return {
        str(row.get("code") or "").upper()
        for row in market_status_rows
        if row.get("holiday_name")
    }


def _pulse_category_keys(active_pulse: dict[str, Any] | None) -> set[str]:
    kind = str((active_pulse or {}).get("kind") or "")
    if kind == "europe_open_followup":
        return {"nordic", "uk", "eu"}
    if kind == "us_open_followup":
        return {"us"}
    return {"nordic", "uk", "us", "eu"}


def _filter_watchlists_for_pulse(
    watchlists: dict[str, Any],
    market_status_rows: list[dict[str, Any]],
    active_pulse: dict[str, Any] | None,
) -> dict[str, Any]:
    allowed_categories = _pulse_category_keys(active_pulse)
    holiday_codes = _exchange_holiday_codes(market_status_rows)
    categories: list[dict[str, Any]] = []
    output: dict[str, Any] = {"categories": categories}
    for category in watchlists.get("categories", []) or []:
        key = str(category.get("key") or "")
        items = [
            row
            for row in category.get("items", []) or []
            if key in allowed_categories and str(row.get("exchange") or "").upper() not in holiday_codes
        ]
        output[key] = items
        categories.append(
            {
                **category,
                "items": items,
                "pulse_included": key in allowed_categories,
                "holiday_excluded_exchange_codes": sorted(holiday_codes),
            }
        )
    for key in ("nordic", "uk", "us", "eu"):
        output.setdefault(key, [])
    output["global"] = [
        row
        for row in watchlists.get("global", []) or []
        if str(row.get("exchange") or "").upper() not in holiday_codes
        and (
            not allowed_categories
            or (
                key_for_region := str(row.get("region") or "").lower()
            ) in allowed_categories
            or (key_for_region == "europe" and "eu" in allowed_categories)
        )
    ]
    return output


def _filter_positions_for_pulse(
    positions: list[dict[str, Any]],
    active_pulse: dict[str, Any] | None,
) -> list[dict[str, Any]]:
    if str((active_pulse or {}).get("kind") or "") != "europe_open_followup":
        return positions
    return [
        row for row in positions
        if ":" not in str(row.get("symbol") or "")
        or str(row["symbol"]).split(":", 1)[1].lower() not in US_EXCHANGES
    ]


def _decision_universe(
    positions: list[dict[str, Any]],
    watchlists: dict[str, Any],
) -> dict[str, Any]:
    symbols: dict[str, dict[str, Any]] = {}

    def upsert(symbol: str, **values: Any) -> None:
        if not symbol:
            return
        row = symbols.setdefault(symbol, {"symbol": symbol, "sources": []})
        source = values.pop("source", None)
        if source and source not in row["sources"]:
            row["sources"].append(source)
        row.update({key: value for key, value in values.items() if value not in (None, "")})

    for row in positions:
        upsert(
            str(row.get("symbol") or ""),
            source="portfolio",
            name=row.get("instrument_name"),
            currency=row.get("currency"),
            current_weight_pct=row.get("allocation_pct"),
            current_quantity=row.get("quantity"),
        )

    for category in watchlists.get("categories", []) or []:
        category_key = str(category.get("key") or "")
        for row in category.get("items", []) or []:
            upsert(
                str(row.get("symbol") or ""),
                source="watchlist",
                name=row.get("name"),
                region=row.get("region") or category_key,
                exchange=row.get("exchange"),
                currency=row.get("currency"),
                current_price=row.get("current_price"),
                daily_change_pct=row.get("change_pct"),
                quote_status=row.get("quote_status"),
            )

    return {
        "count": len(symbols),
        "portfolio_symbols": len([row for row in symbols.values() if "portfolio" in row.get("sources", [])]),
        "watchlist_symbols": len([row for row in symbols.values() if "watchlist" in row.get("sources", [])]),
        "symbols": list(symbols.values()),
    }


def _build_context(config: dict[str, Any], connection) -> dict[str, Any]:
    batch_id = fetch_latest_batch_id(connection)
    initial_cash_dkk = float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0)
    prefer_broker_cash = (
        str(config.get("execution", {}).get("mode")) == "live"
        and str(config.get("execution", {}).get("adapter")) == "saxo"
    )
    portfolio_summary = fetch_portfolio_summary(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
    )
    portfolio_positions = fetch_portfolio_positions(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
    )
    portfolio_symbols = fetch_portfolio_symbols(connection, batch_id=batch_id)
    goal_tracking = fetch_goal_tracking(connection, config)
    watchlists = build_watchlists(config)
    market_status_rows = get_market_status(config)
    analysis_pulses = analysis_pulse_status(config, market_status_rows)
    active_pulse = analysis_pulses["active_pulses"][0] if analysis_pulses["active_pulses"] else None
    scoped_watchlists = _filter_watchlists_for_pulse(watchlists, market_status_rows, active_pulse)
    watchlist_symbols: list[str] = []
    for category in scoped_watchlists.get("categories", []):
        for row in category.get("items", []):
            symbol = str(row.get("symbol") or "")
            if symbol and symbol not in watchlist_symbols:
                watchlist_symbols.append(symbol)
    for row in scoped_watchlists.get("global", []):
        symbol = str(row.get("symbol") or "")
        if symbol and symbol not in watchlist_symbols:
            watchlist_symbols.append(symbol)
    market_news = fetch_market_intelligence(config, portfolio_symbols[:8], watchlist_symbols[:12])
    analysis_summary = summarize_analysis_window(market_status_rows)
    analysis_summary = {
        **analysis_summary,
        "analysis_window_active": bool(analysis_summary["analysis_window_active"] or analysis_pulses["due"]),
        "analysis_pulses_due": bool(analysis_pulses["due"]),
        "active_pulses": analysis_pulses["active_pulses"],
        "next_pulse_at": analysis_pulses["next_pulse_at"],
        "next_pulse_label": analysis_pulses["next_pulse_label"],
    }
    market_status_by_code = {
        str(row.get("code") or "").lower(): row
        for row in market_status_rows
    }
    broker_account = fetch_broker_account_summary(connection)
    journal_learnings = fetch_recent_journal_learnings(connection, limit=6)
    live_quotes = fetch_live_prices(
        portfolio_symbols[:10],
        timeout_seconds=config["market_data"]["request_timeout_seconds"],
    )
    quote_by_symbol = {row["symbol"]: row for row in live_quotes}
    enriched_positions = []
    for row in portfolio_positions:
        quote = quote_by_symbol.get(row["symbol"], {})
        enriched_positions.append(
            {
                "symbol": row["symbol"],
                "instrument_name": row["instrument_name"],
                "isin": row["isin"],
                "quantity": row["quantity"],
                "currency": row["currency"],
                "market_value_dkk": row["market_value_dkk"],
                "cost_basis_dkk": row["cost_basis_dkk"],
                "unrealised_pnl_dkk": row["unrealised_pnl_dkk"],
                "allocation_pct": row["allocation_pct"],
                "current_price_local": quote.get("current_price", row["current_price_local"]),
                "daily_change_pct": quote.get("change_pct"),
                "market_tradable_now": bool(
                    market_status_by_code.get(str(row["symbol"]).split(":", 1)[1].lower(), {}).get("is_tradable")
                ) if ":" in str(row["symbol"]) else None,
                "market_status_reason": (
                    market_status_by_code.get(str(row["symbol"]).split(":", 1)[1].lower(), {}).get("status_reason")
                    if ":" in str(row["symbol"])
                    else None
                ),
            }
        )

    analysis_positions = _filter_positions_for_pulse(enriched_positions, active_pulse)
    market_regime = _summarize_market_regime(watchlists, live_quotes, market_news, market_status_rows)
    capital_limits = strategy_capital_limits(
        config=config,
        total_market_value_dkk=float(portfolio_summary.get("total_market_value_dkk") or 0.0),
        invested_market_value_dkk=float(portfolio_summary.get("invested_market_value_dkk") or 0.0),
        cash_balance_dkk=float(portfolio_summary.get("cash_balance_dkk") or 0.0),
    )
    cash_shortfall_dkk = max(
        float(capital_limits["min_cash_buffer_dkk"]) - float(portfolio_summary.get("cash_balance_dkk") or 0.0),
        0.0,
    )
    tradable_positions = [
        row
        for row in enriched_positions
        if bool(row.get("market_tradable_now")) and float(row.get("quantity") or 0.0) > 0.0
    ]
    tradable_positions.sort(
        key=lambda row: (
            float(row.get("allocation_pct") or 0.0),
            float(row.get("unrealised_pnl_dkk") or 0.0),
        ),
        reverse=True,
    )
    cash_management = {
        "cash_balance_dkk": float(portfolio_summary.get("cash_balance_dkk") or 0.0),
        "invested_market_value_dkk": float(portfolio_summary.get("invested_market_value_dkk") or 0.0),
        "portfolio_value_dkk": float(portfolio_summary.get("total_market_value_dkk") or 0.0),
        "capital_limits": capital_limits,
        "cash_buffer_shortfall_dkk": cash_shortfall_dkk,
        "requires_cash_raise": cash_shortfall_dkk > 1.0,
        "tradable_trim_candidates": [
            {
                "symbol": row["symbol"],
                "allocation_pct": row["allocation_pct"],
                "market_value_dkk": row["market_value_dkk"],
                "unrealised_pnl_dkk": row["unrealised_pnl_dkk"],
                "daily_pnl_dkk": row["daily_change_pct"],
                "market_status_reason": row.get("market_status_reason"),
            }
            for row in tradable_positions[:6]
        ],
    }
    return {
        "batch_id": batch_id,
        "portfolio_summary": portfolio_summary,
        "portfolio_positions": analysis_positions,
        "all_portfolio_positions": enriched_positions,
        "watchlists": {
            "categories": scoped_watchlists.get("categories", []),
            "nordic": scoped_watchlists["nordic"][: int(config["market_data"]["watchlists"].get("nordic_limit", 100))],
            "uk": scoped_watchlists.get("uk", [])[: int(config["market_data"]["watchlists"].get("uk_limit", 25))],
            "us": scoped_watchlists.get("us", [])[: int(config["market_data"]["watchlists"].get("us_limit", 100))],
            "eu": scoped_watchlists.get("eu", [])[: int(config["market_data"]["watchlists"].get("eu_limit", 75))],
            "global": scoped_watchlists["global"][: int(config["market_data"]["watchlists"].get("global_limit", 100))],
        },
        "analysis_universe": {
            "pulse_kind": str((active_pulse or {}).get("kind") or "manual"),
            "included_categories": sorted(_pulse_category_keys(active_pulse)),
            "holiday_excluded_exchange_codes": sorted(_exchange_holiday_codes(market_status_rows)),
            "portfolio_scope": "exclude_us" if str((active_pulse or {}).get("kind") or "") == "europe_open_followup" else "all",
        },
        "decision_universe": _decision_universe(analysis_positions, scoped_watchlists),
        "goal_tracking": goal_tracking,
        "broker_account": broker_account,
        "market_news": market_news,
        "market_status": market_status_rows,
        "analysis_summary": analysis_summary,
        "analysis_pulses": analysis_pulses,
        "analysis_pulse": active_pulse,
        "market_regime": market_regime,
        "cash_management": cash_management,
        "journal_learnings": journal_learnings,
    }


def build_trading_prompt(context: dict[str, Any], config: dict[str, Any]) -> dict[str, str]:
    swing_cfg = config.get("strategy", {}).get("swing", {})
    excluded_symbols = list(config["risk"]["excluded_symbols"]) + list(swing_cfg.get("never_trade_symbols", []) or [])
    excluded_symbols_text = ", ".join(dict.fromkeys(excluded_symbols)) if excluded_symbols else "none configured"
    system_prompt = f"""
You are the portfolio decision engine for a Danish SaxoInvestor disciplined swing/day-trading system.

Core goal for every decision:
{config['xai']['goal']}

Hard rules:
- Use exactly this per-symbol sentiment scale: SELL, UNDERWEIGHT, HOLD, OVERWEIGHT, BUY.
- Never trade or recommend trading these symbols under any circumstances, even if already held: {excluded_symbols_text}.
- New BUY recommendations must be present in the supplied current Watchlist context; existing Portfolio symbols are also in scope for HOLD, SELL, or FLATTEN decisions.
- Never short. Long-only portfolio.
- Total holdings must stay between {int(swing_cfg.get('min_holdings', 10))} and {int(swing_cfg.get('max_holdings', 25))}; every target holding must be between {float(swing_cfg.get('min_holding_weight_pct', 0.05)) * 100:.0f}% and {float(swing_cfg.get('max_holding_weight_pct', 0.25)) * 100:.0f}% of total equity.
- Respect the {float(swing_cfg.get('cash_buffer_pct', 0.10)) * 100:g}% cash buffer. If cash is below buffer, prefer SELL / FLATTEN recommendations over new BUY recommendations.
- Treat all pnl, commission, and taxation impacts in DKK.
- Prefer liquid, news-catalyst-driven names in Nordic, EU/Euronext, UK, and US markets.
- The strategy is not an end-of-day flattening strategy. Prefer positions you believe in across daily, weekly, and monthly horizons.
- Only propose holdings you would actually want to own tomorrow morning and through the short foreseeable future if the thesis remains intact.
- SELL or FLATTEN should mean the thesis is impaired, technical/risk evidence has deteriorated, cash/risk limits require de-risking, or capital should rotate into a clearly stronger open-market opportunity. Do not sell merely because a session is ending.
- Respect the supplied analysis_universe constraints: the Nordic/EU open report excludes US watchlist/US portfolio exposure, the US open report uses the US watchlist, and holiday exchange codes are out of scope.
- Treat recent strategy diary instructions as operational memory: avoid repeating documented execution, risk, or thesis mistakes unless current evidence clearly invalidates the lesson.

Output requirements:
- Return only structured data conforming to the provided schema.
- Provide explicit step-by-step rationale in the reasoning_steps field.
- Fill symbol_sentiment for every symbol in the supplied Decision coverage universe using the exact sentiment scale. Use concise HOLD rationales when the symbol has no actionable edge, but do not omit Watchlist symbols just because they are not currently held.
- Build candidate_assets from the combined Watchlist + Portfolio universe, not only from current holdings.
- Suggested trades must use only BUY, SELL, or FLATTEN with confidence as a 0-100 number and priority high/medium.
""".strip()

    user_prompt = f"""
Current portfolio snapshot JSON:
{json.dumps({'summary': context['portfolio_summary'], 'positions': context['portfolio_positions']}, ensure_ascii=False, indent=2)}

Market regime summary JSON:
{json.dumps(context['market_regime'], ensure_ascii=False, indent=2)}

Analysis pulse JSON:
{json.dumps({'current_pulse': context['analysis_pulse'], 'pulse_status': context['analysis_pulses']}, ensure_ascii=False, indent=2)}

Watchlist opportunities JSON:
{json.dumps(context['watchlists'], ensure_ascii=False, indent=2)}

Decision coverage universe JSON:
{json.dumps(context['decision_universe'], ensure_ascii=False, indent=2)}

Analysis universe constraints JSON:
{json.dumps(context['analysis_universe'], ensure_ascii=False, indent=2)}

News and macro context JSON:
{json.dumps({'market_news': context['market_news']['market_news'][:8], 'macro_events': context['market_news']['macro_events'][:6], 'crypto_news': context['market_news'].get('crypto_news', [])[:6], 'earnings_calendar': context['market_news']['earnings_calendar'][:8]}, ensure_ascii=False, indent=2)}

Market status JSON:
{json.dumps({'analysis_summary': context['analysis_summary'], 'markets': context['market_status']}, ensure_ascii=False, indent=2)}

Goal tracking JSON:
{json.dumps(context['goal_tracking'], ensure_ascii=False, indent=2)}

Broker account JSON:
{json.dumps(context['broker_account'], ensure_ascii=False, indent=2)}

Cash management JSON:
{json.dumps(context['cash_management'], ensure_ascii=False, indent=2)}

Recent strategy journal diaries and learnings JSON:
{json.dumps(context['journal_learnings'], ensure_ascii=False, indent=2)}

Task:
1. Identify whether this is the Nordic/EU open +1h15 report, US open +1h15 report, or a manual analysis.
2. Synthesize Asia, macro, geopolitical, earnings, commodities, crypto, US setup, and the recent strategy diary into one actionable market view.
3. Apply that view to every symbol in the combined Watchlist + Portfolio decision universe using symbol_sentiment with exactly SELL, UNDERWEIGHT, HOLD, OVERWEIGHT, BUY.
4. Return a candidate asset pool of high-conviction liquid names selected from the combined Watchlist + Portfolio universe; candidate_assets is the upstream idea list, not final execution.
5. Suggest only practical BUY, SELL, or FLATTEN actions that respect watchlist-only, blacklist, 10-25 holdings, 5-25% weights, long-only, cash buffer, Danish tax drag, and commission drag.
6. Treat holding a strong position as an active decision. Do not recommend FLATTEN just to finish the day in cash.
7. For each suggested trade, include a concise news/macro-driven rationale and concrete risk notes for swing holding.
8. Explicitly assess progress versus the DKK 5,000 weekly and DKK 20,000 monthly pre-tax goals using the supplied goal_tracking JSON.
9. If no high-conviction trade exists, keep suggested_trades empty and explain the constraint in execution_notes.
10. Produce a concise but concrete Decision Report for the operator.
""".strip()

    return {
        "system": system_prompt,
        "user": user_prompt,
    }


def build_decision_prompt_preview(config: dict[str, Any], connection) -> dict[str, Any]:
    context = _build_context(config, connection)
    prompt = build_trading_prompt(context, config)
    return {
        "kind": "decision_report",
        "title": "Decision Report",
        "description": "Prompt used to generate the two daily market/portfolio Decision Reports.",
        "system_prompt": prompt["system"],
        "user_prompt": prompt["user"],
        "schema": DECISION_REPORT_SCHEMA,
        "context_summary": {
            "batch_id": context.get("batch_id"),
            "analysis_pulse": context.get("analysis_pulse"),
            "portfolio_positions": len(context.get("portfolio_positions") or []),
            "decision_universe_symbols": (context.get("decision_universe") or {}).get("count"),
            "watchlist_categories": [
                {
                    "key": category.get("key"),
                    "items": len(category.get("items") or []),
                }
                for category in (context.get("watchlists") or {}).get("categories", [])
            ],
        },
    }


def _mock_decision_report(context: dict[str, Any], config: dict[str, Any]) -> dict[str, Any]:
    top_position = context["portfolio_positions"][0]["symbol"] if context["portfolio_positions"] else "NO_HOLDINGS"
    top_watch = context["watchlists"]["global"][0]["symbol"] if context["watchlists"]["global"] else top_position
    return {
        "report_title": "Mock Decision Report",
        "analysis_window_active": bool(context["analysis_summary"]["analysis_window_active"]),
        "goal": config["xai"]["goal"],
        "analysis_pulse_summary": {
            "kind": str((context.get("analysis_pulse") or {}).get("kind") or "manual"),
            "label": str((context.get("analysis_pulse") or {}).get("label") or "Manual analysis"),
            "macro_summary": "Mock mode: macro synthesis was not requested from xAI.",
            "asia_summary": "Mock mode: Asia pulse inputs were assembled but not interpreted by xAI.",
            "us_setup_summary": "Mock mode: US setup requires live model synthesis.",
            "last_analysis_at": datetime.now(UTC).isoformat(timespec="seconds"),
        },
        "market_regime": context["market_regime"],
        "portfolio_assessment": {
            "summary": "Mock mode: conservative stance because no live xAI call was made.",
            "main_risks": ["Decision engine ran in mock mode."],
            "strengths": ["Portfolio and market context assembled successfully."],
        },
        "reasoning_steps": [
            "Check whether the analysis window is active and review broad market tone.",
            "Review the largest portfolio exposures and recent price moves.",
            "Prefer no-action or small adjustments until the live xAI API is used.",
        ],
        "risk_rules_check": [
            "Excluded symbols remain blocked.",
            "No shorting allowed.",
            "Target holdings must stay between 5 and 25 percent.",
            "Portfolio must keep a 10 percent cash buffer.",
        ],
        "watchlist_focus": [
            {
                "symbol": top_watch,
                "thesis": "Highest-ranked watchlist symbol from current inputs.",
                "catalysts": ["Watchlist ranking and live quote inputs"],
                "risks": ["Mock report only; requires live xAI confirmation"],
            }
        ],
        "candidate_assets": [
            {
                "symbol": top_watch,
                "direction": "WATCH",
                "xai_score": 50.0,
                "sector": None,
                "thesis": "Mock candidate because no live xAI response was requested.",
                "catalysts": ["Mock mode candidate pool"],
                "risks": ["Requires live xAI confirmation before deployment"],
            }
        ],
        "symbol_sentiment": [
            {
                "symbol": top_watch,
                "sentiment": "HOLD",
                "confidence": 50.0,
                "rationale": "Mock mode does not assign actionable sentiment.",
                "catalysts": ["Watchlist presence"],
                "risk_notes": ["Requires live xAI confirmation before deployment"],
            }
        ],
        "suggested_trades": [],
        "execution_notes": ["Mock mode only. No live model response was requested."],
        "daily_target_assessment": (
            "Mock mode only. "
            f"Observed day pnl is {context['goal_tracking']['periods']['day']['pnl_dkk']:.2f} DKK "
            f"versus target {context['goal_tracking']['periods']['day']['target_dkk']:.2f} DKK. "
            f"Week is {context['goal_tracking']['periods']['week']['pnl_dkk']:.2f} DKK versus "
            f"{context['goal_tracking']['periods']['week']['target_dkk']:.2f} DKK target-to-date; "
            f"month is {context['goal_tracking']['periods']['month']['pnl_dkk']:.2f} DKK versus "
            f"{context['goal_tracking']['periods']['month']['target_dkk']:.2f} DKK target-to-date."
        ),
    }


def _create_response_request(prompt: dict[str, str], config: dict[str, Any], include_encrypted_reasoning: bool) -> dict[str, Any]:
    request_json = {
        "model": config["xai"]["model"],
        "input": [
            {"role": "system", "content": prompt["system"]},
            {"role": "user", "content": prompt["user"]},
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "decision_report",
                "schema": DECISION_REPORT_SCHEMA,
                "strict": True,
            }
        },
    }
    if include_encrypted_reasoning:
        request_json["include"] = ["reasoning.encrypted_content"]
    return apply_response_options(request_json, config)


def request_decision_report(prompt: dict[str, str], config: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    api_key = config["xai"]["api_key"]
    if not api_key:
        raise ValueError("XAI_API_KEY is missing")
    request_json = _create_response_request(
        prompt,
        config,
        include_encrypted_reasoning=bool(config["xai"].get("include_encrypted_reasoning")),
    )
    response = requests.post(
        f"{config['xai']['base_url'].rstrip('/')}/responses",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        json=request_json,
        timeout=timeout_seconds(config),
    )
    response.raise_for_status()
    response_json = response.json()
    report_text = _extract_output_text(response_json)
    if not report_text:
        raise ValueError("xAI response did not contain structured output text")
    return request_json, {"raw": response_json, "parsed": json.loads(report_text)}


def _parse_utc_datetime(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=UTC)
    return parsed.astimezone(UTC)


def _is_retryable_xai_error(exc: Exception) -> bool:
    if isinstance(exc, (requests.Timeout, requests.ConnectionError)):
        return True
    if isinstance(exc, requests.HTTPError):
        status_code = getattr(exc.response, "status_code", None)
        return status_code == 429 or (isinstance(status_code, int) and 500 <= status_code < 600)
    return False


def _xai_retry_after_seconds(exc: Exception) -> float | None:
    response = getattr(exc, "response", None)
    if response is None:
        return None
    raw_value = response.headers.get("Retry-After") if getattr(response, "headers", None) else None
    if raw_value in (None, ""):
        return None
    try:
        return max(float(raw_value), 0.0)
    except (TypeError, ValueError):
        return None


def _xai_backoff_cfg(config: dict[str, Any]) -> dict[str, int]:
    cfg = config.get("xai", {})
    return {
        "initial_seconds": int(cfg.get("retry_initial_backoff_seconds", 60) or 60),
        "max_seconds": int(cfg.get("retry_max_backoff_seconds", 600) or 600),
    }


def _latest_deferred_xai_attempt_count(connection, pulse_key: str | None) -> int:
    if not pulse_key:
        return 0
    row = connection.execute(
        """
        SELECT report_json
        FROM decision_reports
        WHERE analysis_pulse_key = ?
          AND status = 'xai_deferred'
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        """,
        (pulse_key,),
    ).fetchone()
    if not row:
        return 0
    payload = _loads_json_field(row["report_json"], {})
    return int((payload.get("xai_retry") or {}).get("attempt_count") or 0)


def _xai_backoff_payload(
    *,
    connection,
    config: dict[str, Any],
    pulse_key: str | None,
    now: datetime,
    retry_after_seconds: float | None,
) -> dict[str, Any]:
    attempt_count = _latest_deferred_xai_attempt_count(connection, pulse_key) + 1
    cfg = _xai_backoff_cfg(config)
    exponential_delay = min(
        float(cfg["max_seconds"]),
        float(cfg["initial_seconds"]) * (2 ** max(attempt_count - 1, 0)),
    )
    delay_seconds = max(exponential_delay, float(retry_after_seconds or 0.0))
    next_attempt_at = now.astimezone(UTC) + timedelta(seconds=delay_seconds)
    return {
        "attempt_count": attempt_count,
        "delay_seconds": round(delay_seconds, 3),
        "initial_seconds": cfg["initial_seconds"],
        "max_seconds": cfg["max_seconds"],
        "retry_after_seconds": retry_after_seconds,
        "next_attempt_at": next_attempt_at.isoformat(timespec="seconds"),
    }


def _latest_report_row(connection) -> dict[str, Any] | None:
    row = connection.execute(
        """
        SELECT *
        FROM decision_reports
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        """
    ).fetchone()
    return dict(row) if row else None


def _has_report_for_pulse(connection, pulse_key: str) -> bool:
    row = connection.execute(
        """
        SELECT status, report_json
        FROM decision_reports
        WHERE analysis_pulse_key = ?
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        """,
        (pulse_key,),
    ).fetchone()
    if row is None:
        return False
    status = str(row["status"] or "")
    if status == "xai_deferred":
        report_json = _loads_json_field(row["report_json"], {})
        next_attempt_at = _parse_utc_datetime((report_json.get("xai_retry") or {}).get("next_attempt_at"))
        return bool(next_attempt_at and next_attempt_at > datetime.now(UTC))
    return True


def fetch_latest_decision_report(connection) -> dict[str, Any] | None:
    row = _latest_report_row(connection)
    if not row:
        return None
    row["request_json"] = json.loads(row["request_json"]) if row.get("request_json") else None
    row["response_json"] = json.loads(row["response_json"]) if row.get("response_json") else None
    row["report_json"] = json.loads(row["report_json"]) if row.get("report_json") else None
    return row


def fetch_recent_decision_reports(connection, limit: int = 20) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM decision_reports
        ORDER BY created_at DESC, id DESC
        LIMIT ?
        """,
        (int(limit),),
    ).fetchall()
    output: list[dict[str, Any]] = []
    for row in rows:
        item = dict(row)
        item["request_json"] = json.loads(item["request_json"]) if item.get("request_json") else None
        item["response_json"] = json.loads(item["response_json"]) if item.get("response_json") else None
        item["report_json"] = json.loads(item["report_json"]) if item.get("report_json") else None
        output.append(item)
    return output


def _loads_json_field(value: Any, fallback: Any) -> Any:
    if not value:
        return fallback
    try:
        return json.loads(value)
    except (TypeError, json.JSONDecodeError):
        return fallback


def fetch_latest_symbol_decisions(connection) -> dict[str, dict[str, Any]]:
    report_row = connection.execute(
        """
        SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label
        FROM decision_reports
        WHERE report_json IS NOT NULL
        ORDER BY id DESC
        LIMIT 1
        """
    ).fetchone()
    if not report_row:
        return {}

    report = dict(report_row)
    report_id = int(report["id"])
    decisions: dict[str, dict[str, Any]] = {}
    sentiment_rows = connection.execute(
        """
        SELECT *
        FROM swing_sentiment_snapshots
        WHERE report_id = ?
        ORDER BY symbol ASC, id DESC
        """,
        (report_id,),
    ).fetchall()
    for row in sentiment_rows:
        item = dict(row)
        source = _loads_json_field(item.get("source_json"), {})
        symbol = str(item.get("symbol") or "")
        if not symbol:
            continue
        decisions[symbol] = {
            "symbol": symbol,
            "report_id": report_id,
            "created_at": report.get("created_at"),
            "status": report.get("status"),
            "pulse_key": report.get("analysis_pulse_key"),
            "pulse_label": report.get("analysis_pulse_label"),
            "sentiment": item.get("sentiment"),
            "confidence": float(item.get("confidence") or 0.0),
            "macro_bias": item.get("macro_bias"),
            "rationale": item.get("rationale"),
            "catalysts": _loads_json_field(item.get("catalysts_json"), []),
            "risk_notes": _loads_json_field(item.get("risk_notes_json"), []),
            "source": source.get("source"),
            "blocked": bool(source.get("blocked", False)),
            "in_watchlist": source.get("in_watchlist"),
            "watchlist_region": source.get("watchlist_region"),
            "technical": source.get("technical"),
        }

    target_rows = connection.execute(
        """
        SELECT *
        FROM swing_position_targets
        WHERE report_id = ?
        ORDER BY symbol ASC, id DESC
        """,
        (report_id,),
    ).fetchall()
    for row in target_rows:
        item = dict(row)
        symbol = str(item.get("symbol") or "")
        if not symbol:
            continue
        decision = decisions.setdefault(
            symbol,
            {
                "symbol": symbol,
                "report_id": report_id,
                "created_at": report.get("created_at"),
                "status": report.get("status"),
                "pulse_key": report.get("analysis_pulse_key"),
                "pulse_label": report.get("analysis_pulse_label"),
                "sentiment": item.get("sentiment"),
            },
        )
        decision.update(
            {
                "action": item.get("action"),
                "priority": item.get("priority"),
                "target_confidence": float(item.get("confidence") or 0.0),
                "target_rationale": item.get("rationale"),
                "current_weight_pct": float(item.get("current_weight_pct") or 0.0),
                "target_weight_pct": float(item.get("target_weight_pct") or 0.0),
                "current_quantity": float(item.get("current_quantity") or 0.0),
                "target_quantity": item.get("target_quantity"),
                "estimated_delta_quantity": item.get("estimated_delta_quantity"),
                "estimated_value_dkk": item.get("estimated_value_dkk"),
                "risk": _loads_json_field(item.get("risk_json"), {}),
            }
        )
    return decisions


def estimate_next_decision_report(connection, config: dict[str, Any], reference_time: datetime | None = None) -> dict[str, Any]:
    now = (reference_time or datetime.now(UTC)).astimezone(UTC)
    status_rows = get_market_status(config, reference_time=now)
    pulse_summary = analysis_pulse_status(config, status_rows, reference_time=now)
    for pulse in pulse_summary["active_pulses"]:
        if not _has_report_for_pulse(connection, str(pulse["key"])):
            return {
                "next_report_at": pulse["target_at_utc"],
                "reason": f"{pulse['label']} is due now",
            }

    if pulse_summary.get("next_pulse_at"):
        return {
            "next_report_at": pulse_summary["next_pulse_at"],
            "reason": f"Next {pulse_summary.get('next_pulse_label') or 'analysis pulse'}",
        }

    return {
        "next_report_at": None,
        "reason": "No upcoming decision pulse found in the current calendar horizon",
    }


def should_auto_run_decision_report(connection, config: dict[str, Any], analysis_window_active: bool) -> bool:
    now = datetime.now(UTC)
    status_rows = get_market_status(config, reference_time=now)
    pulse_summary = analysis_pulse_status(config, status_rows, reference_time=now)
    for pulse in pulse_summary["active_pulses"]:
        if not _has_report_for_pulse(connection, str(pulse["key"])):
            return True
    return False


def generate_decision_report(
    *,
    config: dict[str, Any] | None = None,
    connection=None,
    force_mock: bool = False,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    created_at = datetime.now(UTC).isoformat(timespec="seconds")
    try:
        context = _build_context(resolved_config, resolved_connection)
        prompt = build_trading_prompt(context, resolved_config)
        batch_id = context["batch_id"]
        request_json: dict[str, Any] = {}
        response_json: dict[str, Any] | None = None
        report_json: dict[str, Any] | None = None
        error_text = None
        status = "completed"
        response_id = None
        active_pulse = context.get("analysis_pulse") or {}

        try:
            if force_mock or not resolved_config["xai"]["api_key"]:
                report_json = _mock_decision_report(context, resolved_config)
                request_json = {"mode": "mock"}
                response_json = {"mode": "mock"}
            else:
                request_json, response_bundle = request_decision_report(prompt, resolved_config)
                response_json = response_bundle["raw"]
                report_json = response_bundle["parsed"]
                response_id = response_json.get("id")
        except Exception as exc:  # noqa: BLE001
            error_text = str(exc)
            report_json = _mock_decision_report(context, resolved_config)
            if _is_retryable_xai_error(exc):
                status = "xai_deferred"
                backoff = _xai_backoff_payload(
                    connection=resolved_connection,
                    config=resolved_config,
                    pulse_key=active_pulse.get("key"),
                    now=datetime.now(UTC),
                    retry_after_seconds=_xai_retry_after_seconds(exc),
                )
                request_json = {
                    "mode": "xai_deferred",
                    "retryable_error": type(exc).__name__,
                    "backoff": backoff,
                }
                response_json = {"mode": "xai_deferred", "error": error_text, "backoff": backoff}
                report_json["xai_retry"] = backoff
                report_json["execution_notes"] = [
                    f"xAI request did not complete; retry scheduled at {backoff['next_attempt_at']}.",
                    "No deterministic strategy output will be executed for this deferred report.",
                ]
            else:
                status = "xai_fallback"

        if status == "xai_deferred":
            report_json["strategy_plan"] = {
                "status": "xai_deferred",
                "selected_assets": [],
                "ladder_orders": [],
                "swing_orders": [],
                "suggested_trades": [],
                "notes": ["Strategy planning deferred until xAI returns a completed Decision Report."],
            }
            report_json["suggested_trades"] = []
        else:
            try:
                strategy_plan = build_strategy_plan(
                    report_json=report_json,
                    context=context,
                    config=resolved_config,
                )
                report_json["strategy_plan"] = strategy_plan
                if strategy_plan.get("mode") == "swing":
                    report_json["suggested_trades"] = list(strategy_plan.get("suggested_trades") or [])
            except Exception as exc:  # noqa: BLE001
                report_json["strategy_plan"] = {
                    "status": "failed",
                    "selected_assets": [],
                    "ladder_orders": [],
                    "notes": [f"Strategy plan generation failed: {exc}"],
                }
        report_json["created_at"] = created_at
        report_json["analysis_summary"] = context["analysis_summary"]
        report_json["analysis_pulse"] = context.get("analysis_pulse")
        report_json["analysis_pulses"] = context.get("analysis_pulses")
        report_json["cash_management"] = context["cash_management"]

        cursor = resolved_connection.execute(
            """
            INSERT INTO decision_reports (
                created_at,
                report_date,
                batch_id,
                model,
                status,
                analysis_window_active,
                response_id,
                prompt_text,
                request_json,
                response_json,
                report_json,
                error_text,
                analysis_pulse_key,
                analysis_pulse_label
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                created_at,
                created_at[:10],
                batch_id,
                resolved_config["xai"]["model"],
                status,
                1 if context["analysis_summary"]["analysis_window_active"] else 0,
                response_id,
                json.dumps(prompt, ensure_ascii=False, sort_keys=True),
                json.dumps(request_json, ensure_ascii=False, sort_keys=True),
                json.dumps(response_json, ensure_ascii=False, sort_keys=True) if response_json is not None else None,
                json.dumps(report_json, ensure_ascii=False, sort_keys=True),
                error_text,
                active_pulse.get("key"),
                active_pulse.get("label"),
            ),
        )
        report_id = int(cursor.lastrowid)
        resolved_connection.commit()
        record_analysis_pulse(
            resolved_connection,
            pulse=context.get("analysis_pulse"),
            report_id=report_id,
            status=status,
        )
        record_swing_plan_snapshot(
            resolved_connection,
            report_id=report_id,
            report_json=report_json,
        )

        append_audit_log(
            resolved_connection,
            "decision_report_generated",
            {
                "report_id": report_id,
                "batch_id": batch_id,
                "status": status,
                "response_id": response_id,
                "analysis_window_active": context["analysis_summary"]["analysis_window_active"],
                "analysis_pulse_key": active_pulse.get("key"),
            },
        )
        return {
            "id": report_id,
            "created_at": created_at,
            "status": status,
            "response_id": response_id,
            "report_json": report_json,
            "error_text": error_text,
            "context": context,
        }
    finally:
        if should_close:
            resolved_connection.close()
