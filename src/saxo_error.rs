use serde_json::{Value as JsonValue, json};

/// Returns a stable, non-secret diagnostic classification for a Saxo execution
/// failure. The raw broker error remains available in the local audit record,
/// while this object is safe for the UI, notifications, and Hermes context.
pub(crate) fn classify_execution_error(status: &str, error_text: &str) -> JsonValue {
    let status = status.to_ascii_lowercase();
    let error = error_text.to_ascii_lowercase();
    let (code, label, remediation, retry_policy) = if status == "broker_state_unknown" {
        (
            "broker_state_unknown",
            "Broker state unknown",
            "Reconcile the order in Saxo before any manual retry.",
            "reconcile_before_retry",
        )
    } else if status == "broker_expired" || error.contains("expired") {
        (
            "order_expired",
            "Order expired unfilled",
            "Refresh the tradable price and review whether a new DayOrder is still warranted.",
            "review_and_resubmit",
        )
    } else if status == "broker_done_for_day" || error.contains("doneforday") {
        (
            "done_for_day",
            "Order done for day",
            "Review the next market session before creating a fresh order.",
            "review_next_session",
        )
    } else if status == "broker_cancelled" || error.contains("cancelled") {
        (
            "broker_cancelled",
            "Broker cancelled order",
            "Inspect the broker order history before creating a replacement order.",
            "manual_review",
        )
    } else if error.contains("rejected") {
        (
            "broker_rejected",
            "Broker rejected order",
            "Review the sanitized Saxo diagnostics and correct the underlying order issue.",
            "manual_review",
        )
    } else if status == "waiting_for_market_open"
        || error.contains("exchange closed")
        || error.contains("market is closed")
    {
        (
            "market_closed",
            "Market closed",
            "Wait for the instrument's next verified trading session.",
            "wait_for_market_open",
        )
    } else if status == "invalid_quantity"
        || error.contains("quantity")
        || error.contains("amount must")
    {
        (
            "quantity",
            "Invalid quantity",
            "Review whole-share quantity, holdings, and active SELL reservations.",
            "review_and_resubmit",
        )
    } else if error.contains("unauthorized")
        || error.contains("http 401")
        || error.contains("http 403")
        || error.contains("access token")
        || error.contains("refresh token")
        || error.contains("token expired")
    {
        (
            "session_expired",
            "Saxo session expired",
            "Re-authenticate Saxo and submit a newly reviewed order.",
            "manual_after_reauth",
        )
    } else if error.contains("rate limited") || error.contains("http 429") {
        (
            "rate_limited",
            "Saxo rate limited",
            "Wait for the Saxo rate-limit reset before creating a fresh reviewed order.",
            "manual_after_backoff",
        )
    } else if error.contains("commission") || error.contains("commissiongroup") {
        (
            "commission_setup",
            "Commission setup",
            "Verify the Saxo account's commission setup for this instrument and market.",
            "manual_after_setup",
        )
    } else if error.contains("insufficient")
        || error.contains("not enough cash")
        || error.contains("buying power")
        || error.contains("margin available")
    {
        (
            "insufficient_cash",
            "Insufficient cash",
            "Reduce the requested exposure or wait for available settled buying power.",
            "review_budget",
        )
    } else if error.contains("tick") || error.contains("increment") || error.contains("price step")
    {
        (
            "tick_size",
            "Invalid tick size",
            "Recalculate the limit or stop price using Saxo's instrument tick scheme.",
            "review_and_resubmit",
        )
    } else if error.contains("limit price")
        || error.contains("stop price")
        || error.contains("outside allowed range")
        || error.contains("orderprice")
    {
        (
            "price_invalid",
            "Invalid order price",
            "Refresh a tradable quote and submit a newly reviewed price.",
            "review_and_resubmit",
        )
    } else if error.contains("sell blocked before saxo precheck")
        || error.contains("active sell reservations")
        || error.contains("saxo holdings")
    {
        (
            "position_quantity",
            "Insufficient available position",
            "Reconcile Saxo holdings and active SELL reservations before another SELL.",
            "reconcile_before_retry",
        )
    } else if error.contains("sellordersalreadyexistforownedcontracts") {
        (
            "sell_order_already_exists",
            "A sell order already exists for this holding",
            "Saxo allows one resting sell per owned holding. Reconcile or cancel the existing order before placing another.",
            "reconcile_before_retry",
        )
    } else if error.contains("ordertypenotsupported") {
        (
            "order_type_not_supported",
            "Order type not supported for this instrument",
            "Read SupportedOrderTypes from instrument reference data and use a type the instrument allows. Precheck acceptance does not confirm order-type support.",
            "manual_review",
        )
    } else if error.contains("no tradable saxo instrument")
        || error.contains("instrument is not tradable")
        || error.contains("looking up saxo instrument")
        || error.contains("instrument match")
        || error.contains("instrument resolve")
    {
        (
            "instrument_not_tradable",
            "Instrument not tradable",
            "Verify the symbol, exchange, asset type, and Saxo trading entitlement.",
            "review_instrument",
        )
    } else {
        (
            "unknown",
            "Unclassified Saxo failure",
            "Review the sanitized execution diagnostics before creating another order.",
            "manual_review",
        )
    };

    json!({
        "version": 1,
        "code": code,
        "label": label,
        "remediation": remediation,
        "retry_policy": retry_policy
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_high_value_saxo_execution_failures() {
        assert_eq!(
            classify_execution_error(
                "execution_failed",
                "Order precheck failed: Tick size invalid"
            )["code"],
            json!("tick_size")
        );
        assert_eq!(
            classify_execution_error(
                "execution_failed",
                "HTTP 401 Unauthorized while placing order"
            )["code"],
            json!("session_expired")
        );
        assert_eq!(
            classify_execution_error(
                "execution_failed",
                "Insufficient cash for requested buy order"
            )["code"],
            json!("insufficient_cash")
        );
        assert_eq!(
            classify_execution_error("execution_failed", "CommissionGroup is not configured")["code"],
            json!("commission_setup")
        );
    }

    #[test]
    fn ambiguous_placement_never_advises_automatic_retry() {
        let taxonomy = classify_execution_error(
            "broker_state_unknown",
            "Order placement failed: TradeNotCompleted",
        );
        assert_eq!(taxonomy["code"], json!("broker_state_unknown"));
        assert_eq!(taxonomy["retry_policy"], json!("reconcile_before_retry"));
    }

    #[test]
    fn classifies_the_protective_stop_broker_errors_seen_on_2026_07_25() {
        // Saxo permits one resting sell per owned holding. A batch that retries
        // a position whose stop is already working hits this, and it must not
        // read as an unclassified failure.
        let existing = classify_execution_error(
            "execution_failed",
            "Order precheck failed: SellOrdersAlreadyExistForOwnedContracts: A sell order already exists for this instrument.",
        );
        assert_eq!(existing["code"], "sell_order_already_exists");
        assert_eq!(existing["retry_policy"], "reconcile_before_retry");

        // `Stop` is the FX form; equities need `StopIfTraded`. Precheck accepts
        // both, so this only ever surfaces at placement.
        let unsupported = classify_execution_error(
            "execution_failed",
            "Order placement failed: OrderTypeNotSupported: The chosen order type is not supported for this instrument type.",
        );
        assert_eq!(unsupported["code"], "order_type_not_supported");
        assert_eq!(unsupported["retry_policy"], "manual_review");
    }
}
