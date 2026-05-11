from __future__ import annotations

from typing import Any

from saxo_daytrader_xai.fx_service import fetch_ecb_fx_rates, fx_rate_to_dkk
from saxo_daytrader_xai.market_symbols import parse_exchange_code
from saxo_daytrader_xai.swing_indicators import fetch_daily_swing_indicators


SENTIMENT_SCALE = ("SELL", "UNDERWEIGHT", "HOLD", "OVERWEIGHT", "BUY")
ACTIONABLE_SENTIMENTS = {"SELL", "UNDERWEIGHT", "OVERWEIGHT", "BUY"}
DEFAULT_NEVER_TRADE_SYMBOLS = {"novob:xcse", "tsla:xnas"}
DEFAULT_SENTIMENT_SOURCES = {"portfolio_default", "watchlist_default", "watchlist_guardrail_fill"}


def swing_strategy_enabled(config: dict[str, Any]) -> bool:
    strategy_cfg = config.get("strategy", {})
    return bool(strategy_cfg.get("enabled", True)) and str(strategy_cfg.get("mode", "swing")).lower() == "swing"


def _swing_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("strategy", {}).get("swing", {})


def _safe_float(value: Any, default: float = 0.0) -> float:
    try:
        if value is None:
            return default
        return float(value)
    except (TypeError, ValueError):
        return default


def _confidence(value: Any, default: float = 50.0) -> float:
    score = _safe_float(value, default)
    if 0.0 <= score <= 1.0:
        score *= 100.0
    return max(0.0, min(score, 100.0))


def _norm_symbol(symbol: Any) -> str:
    return str(symbol or "").strip().lower()


def _never_trade_symbols(config: dict[str, Any]) -> set[str]:
    cfg = _swing_cfg(config)
    symbols = set(DEFAULT_NEVER_TRADE_SYMBOLS)
    symbols.update(_norm_symbol(symbol) for symbol in cfg.get("never_trade_symbols", []) or [])
    symbols.update(_norm_symbol(symbol) for symbol in config.get("risk", {}).get("excluded_symbols", []) or [])
    return {symbol for symbol in symbols if symbol}


def _watchlist_rows(context: dict[str, Any]) -> dict[str, dict[str, Any]]:
    rows: dict[str, dict[str, Any]] = {}
    watchlists = dict(context.get("watchlists") or {})
    for category in watchlists.get("categories", []) or []:
        for row in category.get("items", []) or []:
            symbol_key = _norm_symbol(row.get("symbol"))
            if symbol_key and symbol_key not in rows:
                rows[symbol_key] = dict(row)
    for key in ("nordic", "uk", "us", "eu", "global"):
        for row in watchlists.get(key, []) or []:
            symbol_key = _norm_symbol(row.get("symbol"))
            if symbol_key and symbol_key not in rows:
                rows[symbol_key] = dict(row)
    return rows


def _position_rows(context: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {
        _norm_symbol(row.get("symbol")): dict(row)
        for row in context.get("portfolio_positions", []) or []
        if _norm_symbol(row.get("symbol"))
    }


def _sentiment_rank(sentiment: str) -> int:
    return {
        "BUY": 4,
        "OVERWEIGHT": 3,
        "HOLD": 2,
        "UNDERWEIGHT": 1,
        "SELL": 0,
    }.get(sentiment, 2)


def _normalize_sentiment(value: Any) -> str:
    sentiment = str(value or "HOLD").strip().upper()
    return sentiment if sentiment in SENTIMENT_SCALE else "HOLD"


def _sentiment_from_action(action: Any) -> str:
    normalized = str(action or "").strip().upper()
    if normalized == "BUY":
        return "BUY"
    if normalized in {"SELL", "FLATTEN"}:
        return "SELL"
    if normalized == "HOLD":
        return "HOLD"
    return "HOLD"


def _extract_sentiment_universe(report_json: dict[str, Any], context: dict[str, Any]) -> dict[str, dict[str, Any]]:
    output: dict[str, dict[str, Any]] = {}

    def upsert(symbol: Any, *, sentiment: str, confidence: Any, rationale: Any, source: str, extra: dict[str, Any] | None = None) -> None:
        symbol_key = _norm_symbol(symbol)
        if not symbol_key:
            return
        row = output.get(symbol_key)
        confidence_score = _confidence(confidence)
        if row:
            incoming_default = source in DEFAULT_SENTIMENT_SOURCES
            existing_default = str(row.get("source") or "") in DEFAULT_SENTIMENT_SOURCES
            if incoming_default and not existing_default:
                return
            if not incoming_default and not existing_default and _sentiment_rank(row["sentiment"]) > _sentiment_rank(sentiment):
                return
        output[symbol_key] = {
            "symbol": str(symbol).strip(),
            "sentiment": _normalize_sentiment(sentiment),
            "confidence": confidence_score,
            "rationale": str(rationale or ""),
            "catalysts": list((extra or {}).get("catalysts") or []),
            "risk_notes": list((extra or {}).get("risk_notes") or (extra or {}).get("risks") or []),
            "macro_bias": str((report_json.get("market_regime") or {}).get("bias") or ""),
            "source": source,
        }

    for row in report_json.get("symbol_sentiment", []) or []:
        upsert(
            row.get("symbol"),
            sentiment=_normalize_sentiment(row.get("sentiment")),
            confidence=row.get("confidence"),
            rationale=row.get("rationale"),
            source="symbol_sentiment",
            extra=row,
        )
    for row in report_json.get("suggested_trades", []) or []:
        upsert(
            row.get("symbol"),
            sentiment=_sentiment_from_action(row.get("action")),
            confidence=row.get("confidence"),
            rationale=row.get("rationale"),
            source="suggested_trades",
            extra={"risk_notes": row.get("risk_notes") or []},
        )
    for row in report_json.get("candidate_assets", []) or []:
        direction = str(row.get("direction") or "").strip().upper()
        sentiment = "BUY" if direction == "BUY" else "SELL" if direction == "SELL" else "HOLD"
        upsert(
            row.get("symbol"),
            sentiment=sentiment,
            confidence=row.get("xai_score"),
            rationale=row.get("thesis"),
            source="candidate_assets",
            extra=row,
        )
    for row in report_json.get("watchlist_focus", []) or []:
        upsert(
            row.get("symbol"),
            sentiment="OVERWEIGHT",
            confidence=60.0,
            rationale=row.get("thesis"),
            source="watchlist_focus",
            extra=row,
        )
    for row in context.get("portfolio_positions", []) or []:
        upsert(
            row.get("symbol"),
            sentiment="HOLD",
            confidence=50.0,
            rationale="Existing portfolio holding with no stronger model sentiment.",
            source="portfolio_default",
        )
    for row in _watchlist_rows(context).values():
        daily_change = _safe_float(row.get("change_pct"), 0.0)
        upsert(
            row.get("symbol"),
            sentiment="HOLD",
            confidence=50.0 + max(min(daily_change * 100.0, 10.0), -10.0),
            rationale="Watchlist symbol with no stronger model sentiment.",
            source="watchlist_default",
            extra={
                "catalysts": ["Current watchlist membership"],
                "risk_notes": ["No explicit AI catalyst; default HOLD until evidence improves."],
            },
        )
    return output


def _current_weight(position: dict[str, Any] | None) -> float:
    if not position:
        return 0.0
    allocation = _safe_float(position.get("allocation_pct"), 0.0)
    if allocation > 1.0:
        allocation /= 100.0
    return max(0.0, allocation)


def _row_price_and_currency(symbol_key: str, position: dict[str, Any] | None, watchlist_row: dict[str, Any] | None) -> tuple[float, str]:
    if position:
        price = _safe_float(position.get("current_price_local"), 0.0)
        currency = str(position.get("currency") or "")
        if price > 0 and currency:
            return price, currency
    row = watchlist_row or {}
    return _safe_float(row.get("current_price"), 0.0), str(row.get("currency") or "DKK")


def _trade_priority(sentiment: str, confidence: float) -> str:
    if sentiment in {"BUY", "SELL"} or confidence >= 75.0:
        return "high"
    return "medium"


def _delayed_limit_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("execution", {}).get("delayed_price_limit_orders", {})


def _delayed_limit_price(
    *,
    action: str,
    reference_price: float,
    config: dict[str, Any],
) -> float | None:
    cfg = _delayed_limit_cfg(config)
    if not bool(cfg.get("enabled", True)) or reference_price <= 0:
        return None
    if action == "BUY":
        offset_bps = _safe_float(cfg.get("buy_limit_offset_bps"), 20.0)
        return reference_price * (1.0 + (offset_bps / 10_000.0))
    offset_bps = _safe_float(cfg.get("sell_limit_offset_bps"), 20.0)
    return reference_price * max(0.0, 1.0 - (offset_bps / 10_000.0))


def _exchange_region(symbol: str) -> str:
    exchange = parse_exchange_code(symbol).lower()
    if exchange in {"xnas", "xnys"}:
        return "US"
    if exchange == "xlon":
        return "UK"
    if exchange in {"xcse", "xsto", "xosl", "xhel"}:
        return "Nordic"
    return "EU/Euronext"


def _merge_daily_technical_view(row: dict[str, Any], technical: dict[str, Any] | None) -> dict[str, Any]:
    if not technical:
        return row
    output = {**row, "technical": technical}
    if technical.get("status") != "ok":
        output["risk_notes"] = list(output.get("risk_notes") or []) + list(technical.get("notes") or [])
        return output
    original_sentiment = str(output.get("sentiment") or "HOLD")
    technical_sentiment = str(technical.get("sentiment") or "HOLD")
    technical_score = _safe_float(technical.get("technical_score"), 0.0)
    output["confidence"] = round((_confidence(output.get("confidence")) * 0.65) + (technical_score * 0.35), 2)
    output["rationale"] = (
        f"{output.get('rationale') or ''} "
        f"Daily technicals: {technical.get('trend_bias', 'n/a')} trend, "
        f"{technical.get('confluence_count', 0)}/{technical.get('min_confluences', 3)} confluences, "
        f"{technical.get('reward_risk', 'n/a')}R setup."
    ).strip()
    if original_sentiment in {"BUY", "OVERWEIGHT"} and technical_sentiment not in {"BUY", "OVERWEIGHT"}:
        output["sentiment"] = "HOLD"
        output["risk_notes"] = list(output.get("risk_notes") or []) + [
            "Daily indicator confluence filter blocked the long entry."
        ]
    elif (
        original_sentiment == "HOLD"
        and str(output.get("source") or "") in {"watchlist_default", "watchlist_guardrail_fill"}
        and technical_sentiment in {"BUY", "OVERWEIGHT"}
    ):
        output["sentiment"] = "OVERWEIGHT"
        output["source"] = "watchlist_technical_candidate"
        output["rationale"] = (
            f"{output.get('rationale') or ''} Promoted from Watchlist HOLD because daily technicals show an actionable long setup."
        ).strip()
    elif original_sentiment in {"SELL", "UNDERWEIGHT"} and technical_sentiment == "SELL":
        output["confidence"] = min(100.0, _confidence(output.get("confidence")) + 8.0)
    output["catalysts"] = list(output.get("catalysts") or []) + list(technical.get("confluences") or [])[:3]
    return output


def _append_target(
    targets: list[dict[str, Any]],
    *,
    symbol: str,
    sentiment: str,
    action: str,
    current_weight: float,
    target_weight: float,
    current_quantity: float,
    target_quantity: float | None,
    delta_quantity: float | None,
    estimated_value_dkk: float | None,
    priority: str,
    confidence: float,
    rationale: str,
    risk: dict[str, Any],
) -> None:
    targets.append(
        {
            "symbol": symbol,
            "sentiment": sentiment,
            "action": action,
            "current_weight_pct": round(current_weight * 100.0, 2),
            "target_weight_pct": round(target_weight * 100.0, 2),
            "current_quantity": current_quantity,
            "target_quantity": target_quantity,
            "estimated_delta_quantity": delta_quantity,
            "estimated_value_dkk": estimated_value_dkk,
            "priority": priority,
            "confidence": round(confidence, 2),
            "rationale": rationale,
            "risk": risk,
        }
    )


def build_swing_strategy_plan(
    *,
    report_json: dict[str, Any],
    context: dict[str, Any],
    config: dict[str, Any],
) -> dict[str, Any]:
    if not swing_strategy_enabled(config):
        return {"status": "disabled", "selected_assets": [], "swing_orders": [], "notes": ["Swing strategy is disabled."]}

    cfg = _swing_cfg(config)
    min_holdings = int(cfg.get("min_holdings", 10) or 10)
    max_holdings = int(cfg.get("max_holdings", 25) or 25)
    min_weight = _safe_float(cfg.get("min_holding_weight_pct", 0.05), 0.05)
    max_weight = _safe_float(cfg.get("max_holding_weight_pct", 0.25), 0.25)
    cash_buffer = _safe_float(cfg.get("cash_buffer_pct", 0.10), 0.10)
    risk_per_trade = _safe_float(cfg.get("risk_per_trade_pct", 0.01), 0.01)
    min_trade_value_dkk = _safe_float(config.get("execution", {}).get("min_trade_value_dkk"), 500.0)
    daily_order_capacity = int(config.get("execution", {}).get("max_daily_orders", 50) or 50)
    max_deployed_weight = max(0.0, min(1.0, 1.0 - cash_buffer))
    effective_max_holdings = max(min_holdings, min(max_holdings, int(max_deployed_weight / max(min_weight, 1e-9))))

    notes = [
        "Swing mode is active: watchlist-only universe, hard blacklist, target allocation before quantity.",
        "Default ladders are disabled; swing orders are single target-weight adjustments unless scaling is explicit.",
    ]
    if effective_max_holdings < max_holdings:
        notes.append(
            f"Configured 10% cash buffer and {min_weight * 100:.0f}% minimum holding weight make "
            f"{effective_max_holdings} the feasible maximum target count, despite max_holdings={max_holdings}."
        )

    watchlist_map = _watchlist_rows(context)
    position_map = _position_rows(context)
    blocked = _never_trade_symbols(config)
    sentiment_map = _extract_sentiment_universe(report_json, context)
    technical_symbols = [
        row["symbol"]
        for symbol_key, row in sentiment_map.items()
        if symbol_key not in blocked
        and symbol_key in watchlist_map
        and (
            row["sentiment"] in ACTIONABLE_SENTIMENTS
            or symbol_key in position_map
            or str(row.get("source") or "") == "watchlist_default"
        )
    ]
    technical_by_symbol = fetch_daily_swing_indicators(technical_symbols, config)
    fx_snapshot = fetch_ecb_fx_rates()
    portfolio_summary = dict(context.get("portfolio_summary") or {})
    total_equity_dkk = _safe_float(portfolio_summary.get("total_market_value_dkk"), 0.0)
    cash_balance_dkk = _safe_float(portfolio_summary.get("cash_balance_dkk"), 0.0)
    reserved_cash_dkk = total_equity_dkk * cash_buffer
    available_buy_cash_dkk = max(cash_balance_dkk - reserved_cash_dkk, 0.0)

    sentiment_rows: list[dict[str, Any]] = []
    eligible_rows: list[dict[str, Any]] = []
    blocked_positions: list[str] = []
    non_watchlist_positions: list[str] = []
    for symbol_key, sentiment_row in sentiment_map.items():
        symbol = sentiment_row["symbol"]
        position = position_map.get(symbol_key)
        watchlist_row = watchlist_map.get(symbol_key)
        is_blocked = symbol_key in blocked
        is_watchlist = watchlist_row is not None
        current_weight = _current_weight(position)
        row = {
            **sentiment_row,
            "symbol": symbol,
            "in_watchlist": is_watchlist,
            "blocked": is_blocked,
            "current_weight_pct": round(current_weight * 100.0, 2),
            "current_quantity": _safe_float((position or {}).get("quantity"), 0.0),
            "watchlist_region": str((watchlist_row or {}).get("region") or _exchange_region(symbol)),
        }
        row = _merge_daily_technical_view(row, technical_by_symbol.get(symbol))
        sentiment_rows.append(row)
        if is_blocked and position:
            blocked_positions.append(symbol)
        if position and not is_watchlist:
            non_watchlist_positions.append(symbol)
        if is_blocked:
            continue
        if not is_watchlist and not position:
            continue
        if row["sentiment"] in ACTIONABLE_SENTIMENTS or position:
            eligible_rows.append(row)

    top_watchlist_fillers: list[dict[str, Any]] = []
    for symbol_key, watchlist_row in watchlist_map.items():
        if symbol_key in blocked or symbol_key in sentiment_map:
            continue
        top_watchlist_fillers.append(
            {
                "symbol": watchlist_row["symbol"],
                "sentiment": "HOLD",
                "confidence": 50.0 + max(min(_safe_float(watchlist_row.get("change_pct"), 0.0) * 100.0, 10.0), -10.0),
                "rationale": "Watchlist-ranked liquid name available as a portfolio filler if needed to satisfy holding-count guardrails.",
                "catalysts": [],
                "risk_notes": ["No explicit AI catalyst; filler candidate only."],
                "macro_bias": str((report_json.get("market_regime") or {}).get("bias") or ""),
                "source": "watchlist_guardrail_fill",
                "in_watchlist": True,
                "blocked": False,
                "current_weight_pct": 0.0,
                "current_quantity": 0.0,
                "watchlist_region": str(watchlist_row.get("region") or _exchange_region(watchlist_row["symbol"])),
            }
        )
    eligible_rows.extend(top_watchlist_fillers)
    eligible_rows.sort(
        key=lambda row: (
            _sentiment_rank(str(row.get("sentiment") or "HOLD")),
            _safe_float(row.get("confidence"), 0.0),
            _safe_float((watchlist_map.get(_norm_symbol(row.get("symbol"))) or {}).get("change_pct"), -999.0),
        ),
        reverse=True,
    )

    target_count = min(effective_max_holdings, max(min_holdings, len([row for row in eligible_rows if row.get("current_quantity") or row["sentiment"] in {"BUY", "OVERWEIGHT"}])))
    selected_rows = eligible_rows[:target_count]
    selected_symbols = {_norm_symbol(row["symbol"]) for row in selected_rows}
    target_weight = min(max_weight, max(min_weight, max_deployed_weight / max(target_count, 1)))

    position_targets: list[dict[str, Any]] = []
    suggested_trades: list[dict[str, Any]] = []
    swing_orders: list[dict[str, Any]] = []

    def add_actionable_trade(
        *,
        row: dict[str, Any],
        action: str,
        target_weight_for_symbol: float,
        estimated_delta_quantity: float,
        estimated_value_dkk: float,
        order_action: str,
        price_local: float,
        currency: str,
    ) -> None:
        symbol = str(row["symbol"])
        confidence = _confidence(row.get("confidence"))
        priority = _trade_priority(str(row.get("sentiment") or "HOLD"), confidence)
        rationale = str(row.get("rationale") or "Swing target adjustment generated by portfolio guardrails.")
        risk = {
            "risk_per_trade_pct": risk_per_trade,
            "stop_loss": "Use recent swing low or 1.5-2x daily ATR below entry; broker stop placement remains explicit.",
            "minimum_reward_risk": "1:2",
            "hold_period_days": "5-30",
        }
        limit_price = _delayed_limit_price(
            action=order_action,
            reference_price=price_local,
            config=config,
        )
        order_type = "Limit" if limit_price is not None else "Market"
        order_price_local = limit_price if limit_price is not None else price_local
        suggested_trades.append(
            {
                "symbol": symbol,
                "action": action,
                "priority": priority,
                "confidence": round(confidence, 2),
                "target_weight_pct": round(target_weight_for_symbol * 100.0, 2),
                "quantity_hint": f"{order_action} {estimated_delta_quantity:.0f} share(s)",
                "rationale": rationale,
                "risk_notes": [risk["stop_loss"], f"Max risk per trade {risk_per_trade * 100:.1f}% of equity."],
            }
        )
        swing_orders.append(
            {
                "symbol": symbol,
                "action": order_action,
                "order_type": order_type,
                "requested_weight_pct": target_weight_for_symbol,
                "quantity": float(estimated_delta_quantity),
                "price_local": order_price_local,
                "limit_price_local": limit_price,
                "currency": currency,
                "estimated_value_dkk": estimated_value_dkk * (order_price_local / max(price_local, 1e-9)),
                "session_tag": str((context.get("analysis_pulse") or {}).get("kind") or "swing"),
                "strategy_type": "swing",
                "strategy_role": action.lower(),
                "strategy_key": f"swing:{(context.get('analysis_pulse') or {}).get('key') or 'manual'}:{symbol}:{action.lower()}",
                "strategy_metadata": {
                    "sentiment": row.get("sentiment"),
                    "confidence": confidence,
                    "risk": risk,
                    "source": row.get("source"),
                    "technical": row.get("technical"),
                    "reference_price_local": price_local,
                    "price_delay_minutes_assumed": int(_delayed_limit_cfg(config).get("assumed_delay_minutes", 15) or 15),
                    "limit_offset_bps": _safe_float(
                        _delayed_limit_cfg(config).get("buy_limit_offset_bps" if order_action == "BUY" else "sell_limit_offset_bps"),
                        20.0,
                    ),
                },
            }
        )

    ordered_for_actions = sorted(
        eligible_rows,
        key=lambda row: 0 if row.get("sentiment") in {"SELL", "UNDERWEIGHT"} else 1,
    )
    for row in ordered_for_actions:
        if len(swing_orders) >= daily_order_capacity:
            break
        symbol = str(row["symbol"])
        symbol_key = _norm_symbol(symbol)
        position = position_map.get(symbol_key)
        watchlist_row = watchlist_map.get(symbol_key)
        current_weight = _current_weight(position)
        current_quantity = _safe_float((position or {}).get("quantity"), 0.0)
        price_local, currency = _row_price_and_currency(symbol_key, position, watchlist_row)
        fx_rate = fx_rate_to_dkk(currency, fx_snapshot)
        if price_local <= 0 or fx_rate <= 0:
            notes.append(f"{symbol}: skipped because no usable price/FX was available for swing sizing.")
            continue

        sentiment = str(row.get("sentiment") or "HOLD")
        if sentiment == "SELL" and position:
            desired_weight = 0.0
            action = "FLATTEN"
        elif sentiment == "UNDERWEIGHT" and position:
            desired_weight = min_weight
            action = "SELL"
        elif symbol_key in selected_symbols:
            if sentiment == "HOLD" and not position:
                notes.append(f"{symbol}: retained as watchlist filler only; no BUY without OVERWEIGHT/BUY sentiment.")
                continue
            desired_weight = target_weight
            action = "BUY" if desired_weight > current_weight else "SELL" if desired_weight < current_weight else "HOLD"
        else:
            continue

        if action == "BUY" and watchlist_row is None:
            notes.append(f"{symbol}: skipped BUY because new capital can only be deployed into current Watchlist securities.")
            continue

        delta_value_dkk = (desired_weight - current_weight) * total_equity_dkk
        if action == "HOLD" or abs(delta_value_dkk) < min_trade_value_dkk:
            _append_target(
                position_targets,
                symbol=symbol,
                sentiment=sentiment,
                action="HOLD",
                current_weight=current_weight,
                target_weight=desired_weight,
                current_quantity=current_quantity,
                target_quantity=current_quantity,
                delta_quantity=0.0,
                estimated_value_dkk=0.0,
                priority=_trade_priority(sentiment, _confidence(row.get("confidence"))),
                confidence=_confidence(row.get("confidence")),
                rationale=str(row.get("rationale") or ""),
                risk={},
            )
            continue

        raw_quantity = abs(delta_value_dkk) / max(price_local * fx_rate, 1e-9)
        whole_quantity = float(int(raw_quantity))
        estimated_value_dkk = whole_quantity * price_local * fx_rate
        if whole_quantity <= 0 or estimated_value_dkk < min_trade_value_dkk:
            notes.append(f"{symbol}: skipped because target adjustment is below whole-share/min-trade sizing.")
            continue
        if action == "BUY":
            if estimated_value_dkk > available_buy_cash_dkk + 1e-9:
                affordable_quantity = float(int(available_buy_cash_dkk / max(price_local * fx_rate, 1e-9)))
                estimated_value_dkk = affordable_quantity * price_local * fx_rate
                whole_quantity = affordable_quantity
            if whole_quantity <= 0 or estimated_value_dkk < min_trade_value_dkk:
                notes.append(f"{symbol}: skipped BUY because the 10% cash buffer leaves insufficient deployable cash.")
                continue
            available_buy_cash_dkk -= estimated_value_dkk
            order_action = "BUY"
            target_quantity = current_quantity + whole_quantity
        else:
            whole_quantity = min(whole_quantity, current_quantity)
            if whole_quantity <= 0:
                continue
            order_action = "SELL"
            target_quantity = max(current_quantity - whole_quantity, 0.0)
            available_buy_cash_dkk += estimated_value_dkk

        add_actionable_trade(
            row=row,
            action=action,
            target_weight_for_symbol=desired_weight,
            estimated_delta_quantity=whole_quantity,
            estimated_value_dkk=estimated_value_dkk,
            order_action=order_action,
            price_local=price_local,
            currency=currency,
        )
        _append_target(
            position_targets,
            symbol=symbol,
            sentiment=sentiment,
            action=action,
            current_weight=current_weight,
            target_weight=desired_weight,
            current_quantity=current_quantity,
            target_quantity=target_quantity,
            delta_quantity=whole_quantity,
            estimated_value_dkk=estimated_value_dkk,
            priority=_trade_priority(sentiment, _confidence(row.get("confidence"))),
            confidence=_confidence(row.get("confidence")),
            rationale=str(row.get("rationale") or ""),
            risk={"risk_per_trade_pct": risk_per_trade, "minimum_reward_risk": "1:2"},
        )

    if blocked_positions:
        notes.append(
            "Hard blacklist positions detected but not traded under the never-trade rule: "
            + ", ".join(sorted(set(blocked_positions)))
        )
    if non_watchlist_positions:
        notes.append(
            "Existing non-watchlist positions remain eligible for HOLD/SELL/FLATTEN, but not new BUY exposure: "
            + ", ".join(sorted(set(non_watchlist_positions)))
        )
    selected_assets = [
        {
            "symbol": row["symbol"],
            "sentiment": row["sentiment"],
            "score": round((_sentiment_rank(row["sentiment"]) * 20.0) + (_confidence(row.get("confidence")) * 0.2), 2),
            "confidence": round(_confidence(row.get("confidence")), 2),
            "target_weight_pct": round(target_weight * 100.0, 2) if _norm_symbol(row["symbol"]) in selected_symbols else 0.0,
            "region": row.get("watchlist_region"),
            "notes": [row.get("rationale") or "", f"Source: {row.get('source')}"],
        }
        for row in selected_rows
    ]
    status = "ok" if swing_orders else "selected_without_orders"
    if not selected_assets:
        status = "no_watchlist_candidates"
    return {
        "status": status,
        "mode": "swing",
        "selected_assets": selected_assets,
        "swing_orders": swing_orders,
        "ladder_orders": [],
        "suggested_trades": suggested_trades,
        "position_targets": position_targets,
        "sentiment_universe": sentiment_rows,
        "flow_counts": {
            "macro_inputs": len((context.get("market_news") or {}).get("market_news", []) or []),
            "sentiment_symbols": len(sentiment_rows),
            "constraint_checked": len(eligible_rows),
            "trade_count": len(suggested_trades),
        },
        "constraints": {
            "min_holdings": min_holdings,
            "max_holdings": max_holdings,
            "effective_max_holdings": effective_max_holdings,
            "min_holding_weight_pct": min_weight * 100.0,
            "max_holding_weight_pct": max_weight * 100.0,
            "cash_buffer_pct": cash_buffer * 100.0,
            "never_trade_symbols": sorted(blocked),
            "watchlist_symbols": len(watchlist_map),
        },
        "capital_limits": {
            "total_equity_dkk": total_equity_dkk,
            "cash_balance_dkk": cash_balance_dkk,
            "reserved_cash_dkk": reserved_cash_dkk,
            "remaining_deployable_cash_dkk": available_buy_cash_dkk,
        },
        "notes": notes,
    }
