use dioxus::prelude::*;
use serde_json::Value as JsonValue;

use crate::{
    localization::{
        LocalizationPrefs, format_money, format_percent, format_quantity, format_timestamp,
    },
    models::DashboardView,
};

pub const CSS: &str = include_str!("../assets/app.css");
pub const FAVICON_SVG: &str = include_str!("../assets/favicon.svg");
const CHART_SCRIPT: &str = r#"
<script>
(() => {
  const bindPerformanceCharts = () => {
    document.querySelectorAll(".interactive-chart").forEach((chart) => {
      chart.addEventListener("pointermove", (event) => {
        const bounds = chart.getBoundingClientRect();
        chart.style.setProperty("--crosshair-x", `${event.clientX - bounds.left}px`);
        chart.style.setProperty("--crosshair-y", `${event.clientY - bounds.top}px`);
        chart.classList.add("is-hovering");
      });
      chart.addEventListener("pointerleave", () => chart.classList.remove("is-hovering"));
    });
  };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", bindPerformanceCharts, { once: true });
  } else {
    bindPerformanceCharts();
  }
})();
</script>
"#;

#[derive(Props, Clone, PartialEq)]
struct DashboardProps {
    data: DashboardView,
}

pub fn render_index(data: DashboardView, public_base_path: &str) -> String {
    // Dioxus components compile to Rust functions. For this server-side render
    // path we turn the component tree into an HTML string.
    let body = dioxus_ssr::render_element(rsx! { Dashboard { data } });
    let html = format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Saxo Rust</title>
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <link rel="stylesheet" href="/assets/app.css" />
  </head>
  <body>{body}{CHART_SCRIPT}</body>
</html>"#
    );
    prefix_root_relative_urls(&html, public_base_path)
}

fn prefix_root_relative_urls(html: &str, public_base_path: &str) -> String {
    if public_base_path.is_empty() {
        return html.to_string();
    }
    let base = public_base_path.trim_end_matches('/');
    html.replace("href=\"/", &format!("href=\"{base}/"))
        .replace("action=\"/", &format!("action=\"{base}/"))
        .replace("src=\"/", &format!("src=\"{base}/"))
        .replace("value=\"/", &format!("value=\"{base}/"))
}

#[component]
fn Dashboard(props: DashboardProps) -> Element {
    // `rsx!` is Dioxus' JSX-like macro. The syntax feels like React, but it is
    // checked by the Rust compiler before the binary can run.
    let data = props.data;
    let prefs = data.localization.clone();
    let sso_user = data
        .sso_session
        .get("user")
        .and_then(JsonValue::as_object)
        .cloned();
    let sso_label = sso_user
        .as_ref()
        .and_then(|user| user.get("name").and_then(JsonValue::as_str))
        .or_else(|| {
            sso_user
                .as_ref()
                .and_then(|user| user.get("email").and_then(JsonValue::as_str))
        })
        .unwrap_or("Not signed in through SSO")
        .to_string();
    let saxo_status_class = if data
        .saxo_auth
        .get("connected")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        "pill good"
    } else {
        "pill bad"
    };
    let saxo_environment = data
        .saxo_auth
        .get("environment")
        .and_then(JsonValue::as_str)
        .unwrap_or("n/a")
        .to_uppercase();
    let week_start = week_start_label(&prefs);
    let hour_cycle = hour_cycle_label(&prefs);
    rsx! {
        main { class: "shell",
            header { class: "topbar",
                div { class: "brand-copy",
                    h1 { "{data.app_name}" }
                    p { class: "muted", "Rust Axum/Dioxus trading runtime. Targeted polling keeps the active view fresh without re-running the whole page." }
                    p { class: "muted", "Last updated just now · Shortcut: R runs one scheduler cycle" }
                }
                div { class: "top-actions",
                    div { class: "pill-row right",
                        span { class: "pill", "Execution: {data.execution_mode.to_uppercase()}" }
                        span { class: "pill", "Adapter: {data.execution_adapter}" }
                        span { class: "pill", "Environment: {data.environment}" }
                        span { class: saxo_status_class, span { class: "dot" } "{saxo_environment} · {data.saxo_status}" }
                    }
                    div { class: "user-row",
                        UserMenu { sso_session: data.sso_session.clone(), prefs: prefs.clone(), active_view: data.active_view.clone(), range: data.performance_range.clone() }
                        a { class: "button secondary", href: "/api/saxo/auth/start", "Saxo Login" }
                        a { class: "button", href: "/api/health", "Health" }
                    }
                }
            }
            div { class: "notice-banner", "Analysis window inactive right now." }
            section { class: "grid",
                SummaryMetricCard {
                    label: "Portfolio Value",
                    value: format_dkk(data.total_value_dkk, &prefs),
                    subtitle: format!("Invested {}", format_dkk(data.invested_value_dkk, &prefs)),
                    tone: ""
                }
                SummaryMetricCard {
                    label: "Cash",
                    value: format_dkk(data.cash_dkk, &prefs),
                    subtitle: format!("Initial {} · Trades {}", format_dkk(data.initial_cash_dkk, &prefs), format_dkk(data.cash_from_trades_dkk, &prefs)),
                    tone: ""
                }
                SummaryMetricCard {
                    label: "Unrealised P/L",
                    value: format_dkk(data.unrealised_pnl_dkk, &prefs),
                    subtitle: format!("After tax {}", format_dkk(data.unrealised_after_tax_dkk, &prefs)),
                    tone: if data.unrealised_pnl_dkk >= 0.0 { "good-text" } else { "bad-text" }
                }
                SummaryMetricCard {
                    label: "Daily P/L Since 06:00",
                    value: format_dkk(data.daily_pnl_dkk, &prefs),
                    subtitle: format!("Open {} · Realised {}", format_dkk(data.daily_pnl_dkk, &prefs), format_dkk(0.0, &prefs)),
                    tone: if data.daily_pnl_dkk >= 0.0 { "good-text" } else { "bad-text" }
                }
            }
            TabNav { active_view: data.active_view.clone() }
            DashboardBody {
                data: data.clone(),
                prefs: prefs.clone(),
                sso_label: sso_label.clone(),
                saxo_environment: saxo_environment.clone(),
                week_start: week_start.to_string(),
                hour_cycle: hour_cycle.to_string()
            }
        }
    }
}

#[component]
fn UserMenu(
    sso_session: JsonValue,
    prefs: LocalizationPrefs,
    active_view: String,
    range: String,
) -> Element {
    let user = sso_session
        .get("user")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    let email = user
        .get("email")
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string();
    let name = user
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("SSO user")
        .to_string();
    let initials = initials_for_name(&name, &email);
    let avatar = gravatar_url(&email);
    let return_to = if active_view == "performance" {
        format!("/?view=performance&range_key={range}")
    } else if active_view == "overview" {
        "/".to_string()
    } else {
        format!("/?view={active_view}")
    };
    rsx! {
        details { class: "user-menu",
            summary {
                span { class: "avatar-wrap",
                    if avatar.is_empty() {
                        span { class: "avatar-fallback", "{initials}" }
                    } else {
                        img { class: "avatar", src: "{avatar}", alt: "" }
                    }
                }
                span { class: "user-copy",
                    strong { "{name}" }
                    span { "{email}" }
                }
            }
            div { class: "user-dropdown",
                h3 { "Settings" }
                form { method: "post", action: "/api/settings/localization", class: "settings-form",
                    input { r#type: "hidden", name: "return_to", value: "{return_to}" }
                    label { "Locale" input { name: "locale", value: "{prefs.locale}" } }
                    label { "Timezone" input { name: "time_zone", value: "{prefs.time_zone}" } }
                    label { "Clock"
                        select { name: "hour_cycle",
                            option { value: "24", selected: prefs.hour_cycle == crate::localization::HourCycle::H24, "24-hour" }
                            option { value: "12", selected: prefs.hour_cycle == crate::localization::HourCycle::H12, "12-hour" }
                        }
                    }
                    label { "Week start"
                        select { name: "week_start",
                            option { value: "monday", selected: prefs.week_start == crate::localization::WeekStart::Monday, "Monday" }
                            option { value: "sunday", selected: prefs.week_start == crate::localization::WeekStart::Sunday, "Sunday" }
                            option { value: "saturday", selected: prefs.week_start == crate::localization::WeekStart::Saturday, "Saturday" }
                        }
                    }
                    label { "Thousands" input { name: "group_separator", value: "{prefs.group_separator}" } }
                    label { "Decimal" input { name: "decimal_separator", value: "{prefs.decimal_separator}" } }
                    label { "Units"
                        select { name: "measurement_system",
                            option { value: "metric", selected: prefs.measurement_system == "metric", "Metric / SI" }
                            option { value: "us", selected: prefs.measurement_system == "us", "US customary" }
                        }
                    }
                    button { class: "button", r#type: "submit", "Save settings" }
                }
                a { class: "small-button", href: "/auth/session", "Session API" }
            }
        }
    }
}

#[component]
fn TabNav(active_view: String) -> Element {
    rsx! {
        nav { class: "tabs",
            TabLink { href: "/", label: "Overview", active: active_view == "overview" }
            TabLink { href: "/?view=performance", label: "Performance", active: active_view == "performance" }
            TabLink { href: "/?view=market", label: "Market Status", active: active_view == "market" }
            TabLink { href: "/?view=watchlists", label: "Watchlists", active: active_view == "watchlists" }
            TabLink { href: "/?view=markov", label: "Markov", active: active_view == "markov" }
            TabLink { href: "/?view=decisions", label: "Decision Reports", active: active_view == "decisions" }
            TabLink { href: "/?view=prompts", label: "AI Prompts", active: active_view == "prompts" }
            TabLink { href: "/?view=hermes", label: "Hermes", active: active_view == "hermes" }
            TabLink { href: "/?view=eod", label: "End-Of-Day", active: active_view == "eod" }
            TabLink { href: "/?view=execution", label: "Execution", active: active_view == "execution" }
        }
    }
}

#[component]
fn TabLink(href: String, label: String, active: bool) -> Element {
    rsx! {
        a { class: if active { "tab active" } else { "tab" }, href: "{href}", "{label}" }
    }
}

#[component]
fn DashboardBody(
    data: DashboardView,
    prefs: LocalizationPrefs,
    sso_label: String,
    saxo_environment: String,
    week_start: String,
    hour_cycle: String,
) -> Element {
    match data.active_view.as_str() {
        "performance" => rsx! { PerformanceView { data, prefs } },
        "market" => rsx! { MarketView { data, prefs } },
        "watchlists" => rsx! { WatchlistsView { data, prefs } },
        "markov" => rsx! { MarkovView { data, prefs } },
        "decisions" => rsx! { DecisionsView { data, prefs } },
        "prompts" => rsx! { PromptsView { data, prefs } },
        "hermes" => rsx! { HermesView { data, prefs } },
        "eod" => rsx! { EndOfDayView { data, prefs } },
        "execution" => rsx! { ExecutionView { data, prefs } },
        _ => rsx! {
            OverviewView {
                data,
                prefs,
                sso_label,
                saxo_environment,
                week_start,
                hour_cycle
            }
        },
    }
}

#[component]
fn OverviewView(
    data: DashboardView,
    prefs: LocalizationPrefs,
    sso_label: String,
    saxo_environment: String,
    week_start: String,
    hour_cycle: String,
) -> Element {
    rsx! {
        section { class: "layout",
                div {
                    section { class: "section",
                        h2 { "Positions" }
                        div { class: "table-wrap",
                            table { class: "positions-table",
                                thead { tr { th { "Position" } th { "Decision" } th { "Trend" } th { "Qty" } th { "Kostpris" } th { "Current" } th { "Market value" } th { "% 1D" } th { "Gevinst/tab i alt (DKK)" } th { "Allocation" } } }
                                tbody { for row in data.positions.iter() { PositionRow { row: row.clone(), prefs: prefs.clone() } } }
                            }
                        }
                    }
                    section { class: "section",
                        h2 { "Execution Queue" }
                        div { class: "table-wrap",
                            table {
                                thead { tr { th { "ID" } th { "Created" } th { "Symbol" } th { "Action" } th { "Status" } th { "Qty" } th { "Limit" } } }
                                tbody { for row in data.orders.iter() { OrderRow { row: row.clone(), prefs: prefs.clone() } } }
                            }
                        }
                    }
                }
                aside {
                    section { class: "section",
                        h2 { "Runtime" }
                        div { class: "stack",
                            div { class: "event", strong { "Execution" } span { "{data.execution_mode} / {data.execution_adapter}" } }
                            div { class: "event", strong { "Positions" } span { "{data.position_count} active holdings" } }
                            div { class: "event", strong { "Unrealised P/L" } span { class: if data.unrealised_pnl_dkk >= 0.0 { "good-text" } else { "bad-text" }, "{format_dkk(data.unrealised_pnl_dkk, &prefs)}" } }
                            div { class: "event",
                                strong { "Saxo Session" }
                                span { "{saxo_environment} - {data.saxo_status}" }
                                div { class: "button-row",
                                    a { class: "small-button", href: "/api/saxo/session", "Session API" }
                                    a { class: "small-button", href: "/api/saxo/auth/start", "Reconnect" }
                                }
                            }
                            div { class: "event",
                                strong { "Localization" }
                                span { "{prefs.locale} / {prefs.time_zone}" }
                                span { class: "muted", "Week starts {week_start} - {hour_cycle} clock" }
                            }
                            div { class: "event",
                                strong { "SSO" }
                                span { "{sso_label}" }
                            }
                            div { class: "event mono", strong { "Database" } span { "{data.db_label}" } }
                        }
                    }
                    section { class: "section",
                        h2 { "Recent Decisions" }
                        div { class: "stack", for row in data.reports.iter() { DecisionCard { row: row.clone(), prefs: prefs.clone() } } }
                    }
                }
            }
    }
}

#[component]
fn PerformanceView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let summary = data.performance_summary.clone();
    let change = value_f64(&summary, "change_dkk");
    let range = data.performance_range.clone();
    rsx! {
        section { class: "section",
            div { class: "section-title-row",
                h2 { "Performance" }
                div { class: "range-picker",
                    RangeLink { range: "1D", active: range == "1D" }
                    RangeLink { range: "1W", active: range == "1W" }
                    RangeLink { range: "1M", active: range == "1M" }
                    RangeLink { range: "3M", active: range == "3M" }
                    RangeLink { range: "YTD", active: range == "YTD" }
                    RangeLink { range: "1Y", active: range == "1Y" }
                    RangeLink { range: "ALL", active: range == "ALL" }
                }
            }
            div { class: "mini-grid",
                MetricCard { label: "Latest value", value: format_dkk(value_f64(&summary, "latest_total_market_value_dkk"), &prefs), tone: "" }
                MetricCard { label: "Change", value: format_dkk(change, &prefs), tone: if change >= 0.0 { "good-text" } else { "bad-text" } }
                MetricCard { label: "Daily P/L", value: format_dkk(value_f64(&summary, "daily_pnl_dkk"), &prefs), tone: if value_f64(&summary, "daily_pnl_dkk") >= 0.0 { "good-text" } else { "bad-text" } }
                MetricCard { label: "Snapshots", value: text(&summary, "points"), tone: "" }
            }
            div { class: "legend-row",
                span { class: "legend-item", span { class: "legend-dot portfolio-dot" } "Portfolio value" }
                span { class: "legend-item", span { class: "legend-dot cash-dot" } "Cash balance" }
            }
            PerformanceChart { rows: data.performance_history.clone() }
            div { class: "table-wrap",
                table {
                    thead { tr { th { "Recorded" } th { "Total value" } th { "Invested" } th { "Cash" } th { "Daily P/L" } th { "Positions" } th { "Source" } } }
                    tbody {
                        for row in data.performance_history.iter().rev().take(16) {
                            PerformanceRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PerformanceRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let daily = value_f64(&row, "total_daily_pnl_dkk");
    rsx! {
        tr {
            td { "{format_timestamp(&text(&row, \"recorded_at\"), &prefs)}" }
            td { "{format_dkk(value_f64(&row, \"total_market_value_dkk\"), &prefs)}" }
            td { "{format_dkk(value_f64(&row, \"invested_market_value_dkk\"), &prefs)}" }
            td { "{format_dkk(value_f64(&row, \"cash_balance_dkk\"), &prefs)}" }
            td { class: if daily >= 0.0 { "good-text" } else { "bad-text" }, "{format_dkk(daily, &prefs)}" }
            td { "{text(&row, \"position_count\")}" }
            td { "{text(&row, \"source\")}" }
        }
    }
}

#[component]
fn RangeLink(range: &'static str, active: bool) -> Element {
    rsx! {
        a {
            class: if active { "range-button active" } else { "range-button" },
            href: "/?view=performance&range_key={range}",
            "{range}"
        }
    }
}

#[component]
fn PerformanceChart(rows: Vec<JsonValue>) -> Element {
    let chart = chart_paths(&rows);
    rsx! {
        div { class: "chart-card interactive-chart",
            svg { view_box: "0 0 1000 280", role: "img",
                line { x1: "56", y1: "24", x2: "56", y2: "236", class: "chart-axis" }
                line { x1: "944", y1: "24", x2: "944", y2: "236", class: "chart-axis" }
                line { x1: "56", y1: "236", x2: "944", y2: "236", class: "chart-axis" }
                polyline { points: "{chart.portfolio_points}", class: "chart-line portfolio-line" }
                polyline { points: "{chart.cash_points}", class: "chart-line cash-line" }
                text { x: "56", y: "18", class: "chart-label", "{chart.portfolio_max_label}" }
                text { x: "56", y: "260", class: "chart-label", "{chart.portfolio_min_label}" }
                text { x: "944", y: "18", class: "chart-label right-label", "{chart.cash_max_label}" }
                text { x: "944", y: "260", class: "chart-label right-label", "{chart.cash_min_label}" }
                text { x: "56", y: "276", class: "chart-label", "{chart.start_label}" }
                text { x: "944", y: "276", class: "chart-label right-label", "{chart.end_label}" }
            }
            div { class: "chart-crosshair", aria_hidden: "true" }
        }
    }
}

#[component]
fn MarketView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let summary = data
        .market_status
        .get("summary")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let scheduler = data
        .market_status
        .get("scheduler")
        .cloned()
        .unwrap_or(JsonValue::Null);
    rsx! {
        section { class: "layout",
            div {
                section { class: "section",
                    h2 { "Market Status" }
                    div { class: "mini-grid",
                        MetricCard { label: "Analysis Window", value: if summary.get("analysis_window_active").and_then(JsonValue::as_bool).unwrap_or(false) { "Active".to_string() } else { "Inactive".to_string() }, tone: "" }
                        MetricCard { label: "Active Markets", value: json_list_label(summary.get("active_markets")), tone: "" }
                        MetricCard { label: "Pre-sync Markets", value: json_list_label(summary.get("pre_sync_markets")), tone: "" }
                        MetricCard { label: "Last Cycle", value: text(&summary, "last_cycle_status"), tone: "" }
                    }
                    div { class: "stack loose",
                        div { class: "event",
                            strong { "Next Trading Manager Pulse" }
                            span { "{text(&summary, \"next_pulse_label\")}" }
                            span { class: "muted", "{format_timestamp(&text(&summary, \"next_pulse_at\"), &prefs)}" }
                        }
                        div { class: "event",
                            strong { "Scheduler Heartbeat" }
                            span { "{format_timestamp(&text(&summary, \"last_heartbeat_at\"), &prefs)}" }
                        }
                    }
                    div { class: "table-wrap market-table",
                        table {
                            thead {
                                tr {
                                    th { "Exchange" }
                                    th { "Status" }
                                    th { "Tradable" }
                                    th { "Open" }
                                    th { "Close" }
                                    th { "Tradable close" }
                                    th { "Pre-sync" }
                                    th { "Open window" }
                                    th { "Next open" }
                                }
                            }
                            tbody {
                                for row in data.market_status.get("items").and_then(JsonValue::as_array).cloned().unwrap_or_default() {
                                    MarketRow { row, prefs: prefs.clone() }
                                }
                            }
                        }
                    }
                }
            }
            aside {
                section { class: "section" ,
                    h2 { "Scheduler" }
                    div { class: "stack",
                        div { class: "event", strong { "Started" } span { "{format_timestamp(&text(&scheduler, \"started_at\"), &prefs)}" } }
                        div { class: "event", strong { "Last Cycle Started" } span { "{format_timestamp(&text(&scheduler, \"last_cycle_started_at\"), &prefs)}" } }
                        div { class: "event", strong { "Last Cycle Completed" } span { "{format_timestamp(&text(&scheduler, \"last_cycle_completed_at\"), &prefs)}" } }
                        div { class: "event", strong { "PID" } span { "{text(&scheduler, \"scheduler_pid\")}" } }
                    }
                }
            }
        }
    }
}

#[component]
fn WatchlistsView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let categories = data
        .watchlists
        .get("categories")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    rsx! {
        section { class: "section",
            div { class: "section-title-row",
                div {
                    h2 { "Daily Watchlist Analysis" }
                    p { class: "muted section-intro", "Quote-ranked stocks of interest for Nordic, UK, US, and EU/Euronext universes. Refreshed {format_timestamp(&text(&data.watchlists, \"generated_at\"), &prefs)}." }
                }
                div { class: "pill-row right",
                    for category in categories.iter().filter(|category| text(category, "key") != "all") {
                        span { class: "pill", "{text(category, \"label\")}: {category.get(\"items\").and_then(JsonValue::as_array).map(Vec::len).unwrap_or(0)} / {text(category, \"target_limit\")}" }
                    }
                }
            }
            div { class: "stack loose",
                for category in categories.iter().filter(|category| text(category, "key") != "all") {
                    WatchlistCategory { category: category.clone(), prefs: prefs.clone() }
                }
            }
        }
    }
}

#[component]
fn WatchlistCategory(category: JsonValue, prefs: LocalizationPrefs) -> Element {
    let items = category
        .get("items")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let leader = leader_row(&items, true);
    let laggard = leader_row(&items, false);
    let quoted_names = items
        .iter()
        .filter(|row| {
            !text(row, "currency").is_empty() || value_f64(row, "current_price_local") > 0.0
        })
        .count();
    rsx! {
        div { class: "event",
            div { class: "section-title-row compact",
                div {
                    strong { "{text(&category, \"label\")}" }
                    div { class: "muted", "Showing {items.len()} of target {text(&category, \"target_limit\")} · universe {text(&category, \"total_universe\")}" }
                }
                span { class: "pill", "{coverage_label(&category)} target coverage" }
            }
            div { class: "watch-summary-grid",
                div { class: "event flat",
                    span { class: "muted", "Daily Leader" }
                    strong { class: "good-text", "{text(&leader, \"symbol\")}" }
                    span { "{format_pct(value_f64(&leader, \"change_pct\"), &prefs)} · {format_local_money(value_f64(&leader, \"current_price_local\"), &text(&leader, \"currency\"), &prefs)}" }
                }
                div { class: "event flat",
                    span { class: "muted", "Daily Laggard" }
                    strong { class: "bad-text", "{text(&laggard, \"symbol\")}" }
                    span { "{format_pct(value_f64(&laggard, \"change_pct\"), &prefs)} · {format_local_money(value_f64(&laggard, \"current_price_local\"), &text(&laggard, \"currency\"), &prefs)}" }
                }
                div { class: "event flat",
                    span { class: "muted", "Quoted Names" }
                    strong { "{quoted_names}" }
                    span { "{items.len().saturating_sub(quoted_names)} missing quotes" }
                }
            }
            div { class: "table-wrap compact-table",
                table {
                    thead { tr { th { "Symbol" } th { "Name" } th { "Decision" } th { "Trend" } th { "Exchange" } th { "Currency" } th { "Price" } th { "Daily Change" } th { "Quote Status" } } }
                    tbody {
                        for row in items.iter() {
                            tr {
                                td { SymbolLink { symbol: text(row, "symbol"), instrument_name: text(row, "instrument_name") } }
                                td { "{text(row, \"instrument_name\")}" }
                                td { DecisionBadge { decision: row.get("decision").cloned().unwrap_or(JsonValue::Null), prefs: prefs.clone() } }
                                td { TrendSparkline { row: row.clone() } }
                                td { "{text(row, \"exchange\")}" }
                                td { "{text(row, \"currency\")}" }
                                td { "{format_local_money(value_f64(row, \"current_price_local\"), &text(row, \"currency\"), &prefs)}" }
                                td { class: if value_f64(row, "change_pct") >= 0.0 { "good-text" } else { "bad-text" }, "{format_pct(value_f64(row, \"change_pct\"), &prefs)}" }
                                td { "{fallback_text(row, \"quote_status\", &fallback_text(row, \"status\", \"ok\"))}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MarkovView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let run = data.latest_markov_run.clone();
    let config = run
        .get("config_json")
        .cloned()
        .unwrap_or_else(|| JsonValue::Null);
    let ok_count = data
        .markov_signals
        .iter()
        .filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("ok"))
        .count();
    let error_count = data
        .markov_signals
        .iter()
        .filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("error"))
        .count();
    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "Markov Method" }
                    p { class: "muted", "Daily observable Markov regime model for portfolio and watchlist assets. Signals are advisory and do not place orders." }
                }
                div { class: "pill-row right",
                    span { class: "pill", "Signals: {ok_count}" }
                    span { class: if error_count == 0 { "pill" } else { "pill bad" }, "Errors: {error_count}" }
                }
            }
            if run.is_null() {
                div { class: "event",
                    strong { "No Markov run exists yet." }
                    span { class: "muted", "The scheduler will create the first run after the configured daily time once Saxo chart data is available." }
                }
            } else {
                div { class: "mini-grid",
                    MetricCard { label: "Run Date", value: text(&run, "run_date"), tone: "" }
                    MetricCard { label: "Status", value: text(&run, "status"), tone: "" }
                    MetricCard { label: "Assets", value: text(&run, "asset_count"), tone: "" }
                    MetricCard { label: "Succeeded", value: text(&run, "success_count"), tone: "good-text" }
                    MetricCard { label: "Failed", value: text(&run, "error_count"), tone: if value_f64(&run, "error_count") > 0.0 { "bad-text" } else { "" } }
                    MetricCard { label: "Signal Horizon", value: format!("{}d", text(&config, "signal_horizon_days")), tone: "" }
                }
                div { class: "event",
                    strong { "Configuration" }
                    span { "Window {text(&config, \"window_days\")} trading days · threshold {format_pct(value_f64(&config, \"threshold\"), &prefs)} · samples {text(&config, \"sample_count\")} · daily time {text(&config, \"daily_time\")}" }
                }
            }
            div { class: "table-wrap compact-table",
                table { class: "data-table",
                    thead {
                        tr {
                            th { "Symbol" }
                            th { "State" }
                            th { "Signal" }
                            th { "Direction" }
                            th { "Bull" }
                            th { "Sideways" }
                            th { "Bear" }
                            th { "Stationary Mix" }
                            th { "20D Return" }
                            th { "Samples" }
                            th { "Status" }
                        }
                    }
                    tbody {
                        for row in data.markov_signals.iter() {
                            MarkovSignalRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MarkovSignalRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let signal = value_f64(&row, "signed_signal");
    let tone = if signal > 0.0 {
        "good-text"
    } else if signal < 0.0 {
        "bad-text"
    } else {
        ""
    };
    let status = text(&row, "status");
    rsx! {
        tr {
            td { SymbolLink { symbol: text(&row, "symbol"), instrument_name: text(&row, "instrument_name") } }
            td { "{fallback_text(&row, \"current_state\", \"n/a\")}" }
            td { class: tone, "{format_signed_pct(signal, &prefs)}" }
            td { "{fallback_text(&row, \"direction\", \"n/a\")}" }
            td { "{format_pct(value_f64(&row, \"bull_prob\"), &prefs)}" }
            td { "{format_pct(value_f64(&row, \"sideways_prob\"), &prefs)}" }
            td { "{format_pct(value_f64(&row, \"bear_prob\"), &prefs)}" }
            td { "{distribution_label(row.get(\"stationary_json\"), &prefs)}" }
            td { class: if value_f64(&row, "rolling_return") >= 0.0 { "good-text" } else { "bad-text" }, "{format_signed_pct(value_f64(&row, \"rolling_return\"), &prefs)}" }
            td { "{text(&row, \"sample_count\")}" }
            td {
                span { class: if status == "ok" { "pill good" } else { "pill bad" }, "{status}" }
                if status != "ok" {
                    div { class: "muted", "{text(&row, \"error_text\")}" }
                }
            }
        }
    }
}

#[component]
fn DecisionsView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let report = if data.selected_decision.is_null() {
        data.latest_decision.clone()
    } else {
        data.selected_decision.clone()
    };
    let report_json = report
        .get("report_json")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let suggested_trades = json_array(&report_json, "suggested_trades");
    let selected_assets = json_array(&report_json, "selected_assets");
    let candidate_assets = json_array(&report_json, "candidate_assets");
    let symbol_sentiment = json_array(&report_json, "symbol_sentiment");
    let selected_count = selected_assets.len().max(candidate_assets.len());
    let selected_id = report.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
    let error_text = text(&report, "error_text");
    let strategy_status = fallback_text(
        &report_json,
        "strategy_status",
        &fallback_text(
            &report_json,
            "summary",
            "No strategy status was stored for this report.",
        ),
    );
    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "Decision Report" }
                    p { class: "muted", "Latest xAI report plus deterministic strategy selection output. Select any recent report to inspect its outcome." }
                }
                form { method: "post", action: "/api/actions/decision-report",
                    button { class: "button primary", r#type: "submit", "Generate Report" }
                }
            }
            if report.is_null() {
                div { class: "event",
                    strong { "No decision report exists yet." }
                    span { class: "muted", "Use Generate Report to create a manual Rust fallback report, or wait for the scheduled decision pulse." }
                }
            } else {
                if !error_text.is_empty() {
                    div { class: "notice-banner warn-banner",
                        strong { "{error_text}" }
                        span { "Suggested next action: review report details, cash buffer, and active market windows before manually forcing execution." }
                    }
                }
                div { class: "mini-grid decision-summary-grid",
                    MetricCard { label: "Created", value: format_timestamp(&text(&report, "created_at"), &prefs), tone: "" }
                    MetricCard { label: "Status", value: text(&report, "status"), tone: "" }
                    MetricCard { label: "Selected Assets", value: selected_count.to_string(), tone: "" }
                    MetricCard { label: "Suggested Trades", value: suggested_trades.len().to_string(), tone: "" }
                    MetricCard { label: "Report Cadence", value: fallback_text(&report, "analysis_pulse_label", "Manual Decision Report"), tone: "" }
                    MetricCard { label: "Model", value: text(&report, "model"), tone: "" }
                }
                div { class: "decision-report-grid",
                    div { class: "stack loose",
                        div { class: "event",
                            strong { "Strategy Flow" }
                            span { class: "big-line", "{symbol_sentiment.len()} -> {selected_count} -> {suggested_trades.len()}" }
                            span { class: "muted", "sentiment -> selected -> trades" }
                        }
                        div { class: "event prewrap",
                            strong { "Strategy Status" }
                            span { "{strategy_status}" }
                        }
                        if !suggested_trades.is_empty() {
                            div { class: "table-wrap",
                                table { class: "data-table",
                                    thead { tr { th { "Symbol" } th { "Action" } th { "Priority" } th { "Confidence" } th { "Rationale" } } }
                                    tbody {
                                        for row in suggested_trades.iter() {
                                            DecisionTradeRow { row: row.clone(), prefs: prefs.clone() }
                                        }
                                    }
                                }
                            }
                        }
                        if !selected_assets.is_empty() || !candidate_assets.is_empty() {
                            div { class: "table-wrap",
                                table { class: "data-table",
                                    thead { tr { th { "Selected" } th { "Score" } th { "Notes" } } }
                                    tbody {
                                        for row in selected_assets.iter().chain(candidate_assets.iter()).take(20) {
                                            SelectedAssetRow { row: row.clone(), prefs: prefs.clone() }
                                        }
                                    }
                                }
                            }
                        } else if !symbol_sentiment.is_empty() {
                            div { class: "table-wrap",
                                table { class: "data-table",
                                    thead { tr { th { "Symbol" } th { "Sentiment" } th { "Confidence" } th { "Rationale" } } }
                                    tbody {
                                        for row in symbol_sentiment.iter().take(30) {
                                            SymbolSentimentRow { row: row.clone(), prefs: prefs.clone() }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    aside { class: "stack loose",
                        div { class: "event prewrap report-json-panel",
                            strong { "Report JSON" }
                            span { "{compact_json(Some(&report_json))}" }
                        }
                        div { class: "event prewrap",
                            strong { "Prompt" }
                            span { "{compact_json(report.get(\"request_json\"))}" }
                        }
                    }
                }
                div { class: "table-wrap",
                    table { class: "data-table recent-report-table",
                        thead { tr { th { "Created" } th { "Status" } th { "Strategy" } th { "Selected" } th { "Trades" } } }
                        tbody {
                            for row in data.reports.iter() {
                                DecisionReportRow { row: row.clone(), prefs: prefs.clone(), selected_id }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DecisionTradeRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    rsx! {
        tr {
            td { strong { "{text(&row, \"symbol\")}" } }
            td { "{text(&row, \"action\")}" }
            td { "{text(&row, \"priority\")}" }
            td { "{format_quantity(value_f64(&row, \"confidence\"), &prefs)}" }
            td { "{text(&row, \"rationale\")}" }
        }
    }
}

#[component]
fn SelectedAssetRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    rsx! {
        tr {
            td { strong { "{fallback_text(&row, \"selected\", &text(&row, \"symbol\"))}" } }
            td { "{format_quantity(value_f64(&row, \"score\"), &prefs)}" }
            td { "{fallback_text(&row, \"notes\", &text(&row, \"rationale\"))}" }
        }
    }
}

#[component]
fn SymbolSentimentRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    rsx! {
        tr {
            td { strong { "{text(&row, \"symbol\")}" } }
            td { "{fallback_text(&row, \"sentiment\", &text(&row, \"action\"))}" }
            td { "{format_quantity(value_f64(&row, \"confidence\"), &prefs)}" }
            td { "{text(&row, \"rationale\")}" }
        }
    }
}

#[component]
fn DecisionReportRow(row: JsonValue, prefs: LocalizationPrefs, selected_id: i64) -> Element {
    let id = row.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
    let report_json = row.get("report_json").cloned().unwrap_or(JsonValue::Null);
    let selected = json_array(&report_json, "selected_assets")
        .len()
        .max(json_array(&report_json, "candidate_assets").len());
    let trades = json_array(&report_json, "suggested_trades").len();
    let active = id == selected_id;
    rsx! {
        tr { class: if active { "selected-row" } else { "" },
            td { a { href: "/?view=decisions&report_id={id}", "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" } }
            td { "{text(&row, \"status\")}" }
            td { "{fallback_text(&row, \"analysis_pulse_label\", &text(&row, \"model\"))}" }
            td { "{selected}" }
            td { "{trades}" }
        }
    }
}

#[component]
fn PromptsView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let latest = data.latest_decision.clone();
    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "AI Prompts" }
                    p { class: "muted", "Runtime prompt previews for the Decision Report, Trading Manager, and end-of-day diary. Generated {format_timestamp(&text(&latest, \"created_at\"), &prefs)}." }
                }
                div { class: "pill-row right",
                    span { class: "pill", "Decision #{text(&latest, \"id\")}" }
                    span { class: "pill", "{text(&latest, \"status\")}" }
                }
            }
            div { class: "notice-banner good-banner",
                strong { "Trading Manager objective" }
                span { "Pick and manage stocks with conviction for daily, weekly, and monthly horizons. Selling requires thesis, technical, cash, risk, or opportunity evidence." }
            }
            div { class: "prompt-card",
                h3 { "Decision Report" }
                p { class: "muted", "Prompt used to generate the latest market and portfolio Decision Report." }
                div { class: "grid-2 prompt-grid",
                    div { class: "event prewrap code-panel",
                        strong { "System Prompt" }
                        span { "{fallback_text(&latest, \"prompt_text\", \"No stored prompt text is available for this report.\")}" }
                    }
                    div { class: "event prewrap",
                        strong { "User Prompt / Payload" }
                        span { "{compact_json(latest.get(\"request_json\"))}" }
                    }
                }
                div { class: "event prewrap",
                    strong { "Structured Output Schema" }
                    span { "{compact_json(latest.get(\"report_json\"))}" }
                }
            }
            div { class: "prompt-card",
                h3 { "Trading Manager" }
                p { class: "muted", "Execution-gate prompt preview. Live runs fetch full technical indicators before approving queued orders." }
                div { class: "notice-banner", "Core instruction: approve trades only when the decision thesis, technicals, cash limits, and broker state are aligned." }
                div { class: "grid-2 prompt-grid",
                    div { class: "event prewrap code-panel",
                        strong { "System Prompt" }
                        span { "You are the Trading Manager execution gate. Return strict JSON only." }
                    }
                    div { class: "event prewrap",
                        strong { "User Prompt / Payload" }
                        span { "Latest completed Decision Report, Markov signals, and current scheduler pulse context are loaded by the Rust runtime." }
                    }
                }
            }
            div { class: "prompt-card",
                h3 { "End-of-Day Diary" }
                p { class: "muted", "Prompt used after US close to turn trading performance and benchmark context into lessons for future Decision Reports." }
                div { class: "event prewrap code-panel",
                    strong { "System Prompt" }
                    span { "You are the trading diary reviewer. Return strict JSON only." }
                }
            }
        }
    }
}

#[component]
fn HermesView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let latest_reflection = data
        .hermes_reflections
        .first()
        .cloned()
        .unwrap_or(JsonValue::Null);
    let pending_experiments = data
        .hermes_experiments
        .iter()
        .filter(|row| text_or(row, "status", "pending_review") == "pending_review")
        .count();
    let latest_created = if latest_reflection.is_null() {
        "None".to_string()
    } else {
        format_timestamp(&text(&latest_reflection, "created_at"), &prefs)
    };

    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "Hermes Self-Improvement" }
                    p { class: "muted", "Operator review and lifecycle controls for Hermes reflections and one-variable experiment proposals. Promotion records a baseline audit artifact; live trading remains separately gated." }
                }
                div { class: "pill-row right",
                    span { class: "pill", "Reflections: {data.hermes_reflections.len()}" }
                    span { class: "pill", "Experiments: {data.hermes_experiments.len()}" }
                    span { class: "pill", "Pending: {pending_experiments}" }
                }
            }
            div { class: "notice-banner warn-banner",
                strong { "Safety boundary" }
                span { "Hermes can observe and propose. This dashboard can record paper/SIM lifecycle decisions, but it cannot place Saxo orders, expose secrets, or activate live broker behavior." }
            }
            if !data.active_strategy_baseline.is_null() {
                div { class: "event prewrap",
                    strong { "Active Baseline Audit Record" }
                    span {
                        "{text(&data.active_strategy_baseline, \"id\")} · goal v{text_or(&data.active_strategy_baseline, \"goal_version\", \"n/a\")} · {format_timestamp(&text(&data.active_strategy_baseline, \"activated_at\"), &prefs)}"
                    }
                    span { "{compact_json(data.active_strategy_baseline.get(\"config_json\"))}" }
                }
            } else {
                div { class: "event",
                    strong { "No promoted baseline audit record yet." }
                    span { class: "muted", "Promote a successful paper/SIM experiment to create one. Decision prompts will include it once present." }
                }
            }
            div { class: "mini-grid",
                MetricCard { label: "Latest Reflection", value: latest_created, tone: "" }
                MetricCard { label: "Goal Version", value: text_or(&latest_reflection, "goal_version", "n/a"), tone: "" }
                MetricCard { label: "Findings", value: json_item_count(&latest_reflection, "findings_json").to_string(), tone: "" }
                MetricCard { label: "Actions", value: json_item_count(&latest_reflection, "proposed_actions_json").to_string(), tone: "" }
            }
            if latest_reflection.is_null() {
                div { class: "event",
                    strong { "No Hermes reflection exists yet." }
                    span { class: "muted", "Run a manual Hermes reflection job or enable the suspended weekly CronJob after its smoke test is approved." }
                }
            } else {
                div { class: "grid-2",
                    div { class: "event prewrap",
                        strong { "Latest Summary" }
                        span { "{text_or(&latest_reflection, \"summary\", \"No summary recorded.\")}" }
                    }
                    div { class: "event prewrap",
                        strong { "Proposed Actions" }
                        span { "{compact_json(latest_reflection.get(\"proposed_actions_json\"))}" }
                    }
                }
            }
            div { class: "table-wrap",
                h3 { "Recent Reflections" }
                table {
                    thead {
                        tr {
                            th { "Created" }
                            th { "Goal" }
                            th { "Summary" }
                            th { "Findings" }
                            th { "Actions" }
                            th { "Session" }
                        }
                    }
                    tbody {
                        for row in data.hermes_reflections.iter() {
                            HermesReflectionRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
            div { class: "table-wrap",
                h3 { "Experiment Proposals" }
                table {
                    thead {
                        tr {
                            th { "Created" }
                            th { "Status" }
                            th { "Variable" }
                            th { "Old Value" }
                            th { "New Value" }
                            th { "Hypothesis" }
                            th { "Expected Effect" }
                            th { "Evidence" }
                            th { "Actions" }
                        }
                    }
                    tbody {
                        for row in data.hermes_experiments.iter() {
                            HermesExperimentRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EndOfDayView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let latest = data
        .journal_entries
        .first()
        .cloned()
        .unwrap_or(JsonValue::Null);
    rsx! {
        section { class: "layout",
            div {
                section { class: "section",
                    h2 { "End-Of-Day" }
                    if latest.is_null() {
                        div { class: "event",
                            strong { "No end-of-day diary exists yet." }
                            span { class: "muted", "The scheduler creates one after the configured daily journal time." }
                        }
                    } else {
                        div { class: "mini-grid",
                            MetricCard { label: "Journal Date", value: text(&latest, "journal_date"), tone: "" }
                            MetricCard { label: "Cadence", value: text(&latest, "cadence"), tone: "" }
                            MetricCard { label: "Status", value: text(&latest, "status"), tone: "" }
                            MetricCard { label: "Source Report", value: text(&latest, "source_report_id"), tone: "" }
                        }
                        div { class: "event prewrap",
                            strong { "Summary" }
                            span { "{text(&latest, \"summary\")}" }
                        }
                        div { class: "grid-2",
                            div { class: "event prewrap",
                                strong { "Metrics" }
                                span { "{compact_json(latest.get(\"metrics_json\"))}" }
                            }
                            div { class: "event prewrap",
                                strong { "Learnings" }
                                span { "{compact_json(latest.get(\"learnings_json\"))}" }
                            }
                        }
                        div { class: "event prewrap",
                            strong { "Diary JSON" }
                            span { "{compact_json(latest.get(\"diary_json\"))}" }
                        }
                    }
                }
            }
            aside {
                section { class: "section",
                    h2 { "Recent Journals" }
                    div { class: "stack",
                        for row in data.journal_entries.iter() {
                            div { class: "event",
                                strong { "{text(row, \"journal_date\")} - {text(row, \"cadence\")}" }
                                span { "{format_timestamp(&text(row, \"created_at\"), &prefs)}" }
                                span { class: "muted", "{text(row, \"status\")}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ExecutionView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "Execution" }
                    p { class: "muted", "Queue state, broker fills, local events, and scheduler cycles." }
                }
                div { class: "pill-row",
                    span { class: "pill", "Mode: {data.execution_mode}" }
                    span { class: "pill", "Adapter: {data.execution_adapter}" }
                    span { class: "pill", "Orders: {data.orders.len()}" }
                    span { class: "pill", "Fills: {data.execution_fills.len()}" }
                }
            }
            section { class: "broker-status-card",
                div {
                    div { class: "label", "Saxo Broker Status" }
                    h3 { "{data.saxo_status}" }
                    p { class: "muted", "Trading mutations remain disabled until the Rust Saxo execution engine is fully ported." }
                }
                div { class: "broker-status-grid",
                    span { "Environment " strong { "{text(&data.saxo_auth, \"environment\")}" } }
                    span { "Token " strong { "{bool_label(&data.saxo_auth, \"token_valid\")}" } }
                    span { "Refresh token " strong { "{bool_label(&data.saxo_auth, \"refresh_token_valid\")}" } }
                    span { "Expires " strong { "{text(&data.saxo_auth, \"expires_in_minutes\")} min" } }
                }
            }

            // SIM-only: Reset portfolio from Live Positioner export
            {
                let saxo_env = data
                    .saxo_auth
                    .get("environment")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                if saxo_env == "SIM" {
                    rsx! {
                        section { class: "section sim-reset-card",
                            h3 { "Reset SIM Portfolio from Live Export" }
                            p { class: "muted",
                                "Upload a Positioner CSV exported from your Live Saxo account. "
                                "This will completely replace the current SIM portfolio state (lots, cost basis, cash) with your real holdings."
                            }
                            form {
                                action: "/api/portfolio/reset-from-live-csv",
                                method: "post",
                                enctype: "multipart/form-data",
                                class: "sim-reset-form",

                                div { class: "form-row",
                                    label { "Positioner CSV exported from your Live account" }
                                    input {
                                        r#type: "file",
                                        name: "file",
                                        accept: ".csv",
                                        required: true
                                    }
                                }

                                div { class: "form-row",
                                    label { "Cash balance (DKK) at the time of the Live export" }
                                    input {
                                        r#type: "number",
                                        name: "cash_dkk",
                                        step: "0.01",
                                        required: true,
                                        placeholder: "e.g. 20860.00"
                                    }
                                    span { class: "hint", "Enter the exact cash figure from your Live account on the export date." }
                                }

                                div { class: "form-row checkbox",
                                    label {
                                        input {
                                            r#type: "checkbox",
                                            name: "also_sync_sim_broker",
                                            value: "true"
                                        }
                                        " Also place market orders on this SIM account to match the Live holdings"
                                    }
                                }

                                div { class: "form-row checkbox danger",
                                    label {
                                        input {
                                            r#type: "checkbox",
                                            name: "confirm_wipe",
                                            value: "true",
                                            required: true
                                        }
                                        strong { " I understand this will permanently wipe the current SIM portfolio (lots, cost basis, cash) and replace it with the Live snapshot." }
                                    }
                                }

                                button {
                                    class: "button danger",
                                    r#type: "submit",
                                    id: "reset-submit-btn",
                                    "Reset SIM Portfolio from Live CSV"
                                }
                            }

                            // Note: client-side fetch enhancement temporarily removed because it was
                            // breaking the rsx! macro parser. The form now does a normal submit and the
                            // server returns a nice full-page success HTML (with auto-redirect back to
                            // the Execution tab). This is good enough for a first release and unblocks
                            // the user immediately.
                        }
                    }
                } else {
                    rsx! { "" }
                }
            }
            div { class: "table-wrap",
                h3 { "Execution Orders" }
                table {
                    thead { tr { th { "ID" } th { "Created" } th { "Symbol" } th { "Action" } th { "Strategy" } th { "Role" } th { "Order Type" } th { "Status" } th { "Qty" } th { "Price" } th { "Limit" } th { "Stop" } th { "Error" } } }
                    tbody {
                        for row in data.orders.iter() {
                            ExecutionOrderRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
            div { class: "table-wrap",
                h3 { "Recent Broker Fills" }
                table {
                    thead { tr { th { "Fill Time" } th { "Order" } th { "Symbol" } th { "Side" } th { "Status" } th { "Delta Qty" } th { "Cumulative Qty" } th { "Average Price" } th { "Ledger" } } }
                    tbody {
                        for row in data.execution_fills.iter() {
                            ExecutionFillRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
            div { class: "grid-2",
                div { class: "table-wrap",
                    h3 { "Order Events" }
                    table {
                        thead { tr { th { "Created" } th { "Order" } th { "Type" } th { "Status" } th { "Message" } } }
                        tbody {
                            for row in data.execution_events.iter().take(20) {
                                tr {
                                    td { "{format_timestamp(&text(row, \"created_at\"), &prefs)}" }
                                    td { "{text(row, \"execution_order_id\")}" }
                                    td { "{text(row, \"event_type\")}" }
                                    td { "{text(row, \"status\")}" }
                                    td { "{text(row, \"message\")}{text(row, \"error_text\")}" }
                                }
                            }
                        }
                    }
                }
                div { class: "table-wrap",
                    h3 { "Scheduler Cycles" }
                    table {
                        thead { tr { th { "Started" } th { "Status" } th { "Decision" } th { "Queue" } } }
                        tbody {
                            for row in data.scheduler_cycles.iter().take(12) {
                                tr {
                                    td { "{format_timestamp(&text(row, \"started_at\"), &prefs)}" }
                                    td { "{text(row, \"status\")}" }
                                    td { "{bool_label(row, \"generated_decision\")}" }
                                    td { "{text(row, \"queue_status\")}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MetricCard(label: String, value: String, tone: String) -> Element {
    let value_class = match tone.as_str() {
        "good-text" => "value good-text",
        "bad-text" => "value bad-text",
        _ => "value",
    };
    rsx! {
        div { class: "card",
            div { class: "label", "{label}" }
            div { class: value_class, "{value}" }
        }
    }
}

#[component]
fn SummaryMetricCard(label: String, value: String, subtitle: String, tone: String) -> Element {
    let value_class = match tone.as_str() {
        "good-text" => "value good-text",
        "bad-text" => "value bad-text",
        _ => "value",
    };
    rsx! {
        div { class: "card summary-card",
            div { class: "label", "{label}" }
            div { class: value_class, "{value}" }
            div { class: "muted summary-subtitle", "{subtitle}" }
        }
    }
}

#[component]
fn PositionRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let quantity = format_quantity(value_f64(&row, "quantity"), &prefs);
    let currency = text(&row, "currency");
    let market_value = format_dkk(value_f64(&row, "market_value_dkk"), &prefs);
    let unrealised_value = value_f64(&row, "unrealised_pnl_dkk");
    let total_return_pct = position_total_return_pct(&row);
    let unrealised = format_signed_dkk(unrealised_value, &prefs);
    let total_return = format_signed_pct(total_return_pct, &prefs);
    let daily_value = value_f64(&row, "daily_pnl_dkk");
    let daily_pct = position_daily_return_pct(&row);
    let daily = format_signed_dkk(daily_value, &prefs);
    let daily_return = format_signed_pct(daily_pct, &prefs);
    let allocation = format_pct(value_f64(&row, "allocation_pct"), &prefs);
    let cost_price = format_position_price(position_cost_price_local(&row), &currency, &prefs);
    let current_price =
        format_position_price(value_f64(&row, "current_price_local"), &currency, &prefs);
    rsx! {
        tr {
            td { PositionSymbolCell { row: row.clone(), prefs: prefs.clone() } }
            td { DecisionBadge { decision: row.get("decision").cloned().unwrap_or(JsonValue::Null), prefs: prefs.clone() } }
            td { TrendSparkline { row: row.clone() } }
            td { "{quantity}" }
            td { "{cost_price}" }
            td { "{current_price}" }
            td { "{market_value}" }
            td { class: if daily_value >= 0.0 { "good-text" } else { "bad-text" },
                div { class: "metric-stack",
                    span { "{daily_return}" }
                    span { "{daily}" }
                }
            }
            td { class: if unrealised_value >= 0.0 { "good-text" } else { "bad-text" },
                div { class: "metric-stack",
                    span { "{total_return}" }
                    span { "{unrealised}" }
                }
            }
            td { "{allocation}" }
        }
    }
}

#[component]
fn PositionSymbolCell(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let symbol = text(&row, "symbol");
    let instrument_name = text(&row, "instrument_name");
    let currency = text(&row, "currency");
    let modal_id = position_modal_id(&symbol);
    rsx! {
        div { class: "position-symbol-cell",
            a {
                class: "symbol-link",
                href: "#{modal_id}",
                title: "Open position details for {symbol}",
                "{symbol}"
            }
            span { class: "position-name", "{instrument_name}" }
            span { class: "asset-pill", "{asset_label(&row)} · {currency}" }
            PositionDetailModal { row, prefs }
        }
    }
}

#[component]
fn PositionDetailModal(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let symbol = text(&row, "symbol");
    let instrument_name = text(&row, "instrument_name");
    let modal_id = position_modal_id(&symbol);
    let yahoo_url = yahoo_finance_url(&symbol);
    let tradingview_symbol = tradingview_symbol(&symbol);
    let currency = text(&row, "currency");
    let cost_price = format_position_price(position_cost_price_local(&row), &currency, &prefs);
    let open_price = format_position_price(value_f64(&row, "open_price_local"), &currency, &prefs);
    let current_price =
        format_position_price(value_f64(&row, "current_price_local"), &currency, &prefs);
    let total_return_pct = position_total_return_pct(&row);
    let daily_pct = position_daily_return_pct(&row);
    let unrealised_value = value_f64(&row, "unrealised_pnl_dkk");
    let daily_value = value_f64(&row, "daily_pnl_dkk");
    rsx! {
        div { id: "{modal_id}", class: "modal-target",
            a { class: "modal-dismiss", href: "#", "Close" }
            section { class: "chart-modal position-detail-modal", role: "dialog", aria_label: "Position details for {symbol}",
                div { class: "section-title-row",
                    div {
                        h2 { "{symbol}" }
                        p { class: "muted", "{instrument_name}" }
                    }
                    div { class: "button-row",
                        a { class: "small-button", href: "{yahoo_url}", target: "_blank", rel: "noopener noreferrer", "Yahoo" }
                        a { class: "small-button", href: "{tradingview_page_url(&tradingview_symbol)}", target: "_blank", rel: "noopener noreferrer", "TradingView" }
                        a { class: "small-button", href: "#", "Close" }
                    }
                }
                div { class: "position-detail-grid",
                    section { class: "detail-panel",
                        h3 { "Gevinst/tab" }
                        DetailLine { label: "Kostpris", value: format_dkk(value_f64(&row, "cost_basis_dkk"), &prefs), tone: "" }
                        DetailLine { label: "Aktuel", value: format_dkk(value_f64(&row, "market_value_dkk"), &prefs), tone: "" }
                        DetailLine { label: "Afkast", value: format!("{} · {}", format_signed_dkk(unrealised_value, &prefs), format_signed_pct(total_return_pct, &prefs)), tone: if unrealised_value >= 0.0 { "good-text" } else { "bad-text" } }
                        DetailLine { label: "1D", value: format!("{} · {}", format_signed_dkk(daily_value, &prefs), format_signed_pct(daily_pct, &prefs)), tone: if daily_value >= 0.0 { "good-text" } else { "bad-text" } }
                    }
                    section { class: "detail-panel",
                        h3 { "Position" }
                        DetailLine { label: "Antal", value: format_quantity(value_f64(&row, "quantity"), &prefs), tone: "" }
                        DetailLine { label: "Kostpris pr. aktie", value: cost_price, tone: "" }
                        DetailLine { label: "Åbningskurs", value: open_price, tone: "" }
                        DetailLine { label: "Aktuel kurs", value: current_price, tone: "" }
                    }
                    section { class: "detail-panel",
                        h3 { "Om virksomheden" }
                        DetailLine { label: "Symbol", value: symbol.clone(), tone: "" }
                        DetailLine { label: "ISIN", value: text_or(&row, "isin", "n/a"), tone: "" }
                        DetailLine { label: "Type", value: asset_label(&row), tone: "" }
                        DetailLine { label: "Marked", value: text_or(&row, "market_status", "n/a"), tone: "" }
                    }
                    section { class: "detail-panel muted-detail-panel",
                        h3 { "ESG, nyheder og analytikere" }
                        p { class: "muted", "Saxo's public OpenAPI portfolio data covers positions, exposure, prices, and instrument reference data. I did not find documented OpenAPI endpoints for the ESG-risk, news, or analyst-consensus panels shown in the Saxo app, so this view leaves those fields empty until we add a licensed data source." }
                    }
                }
            }
        }
    }
}

#[component]
fn DetailLine(label: String, value: String, tone: String) -> Element {
    rsx! {
        div { class: "detail-line",
            span { "{label}" }
            strong { class: "{tone}", "{value}" }
        }
    }
}

#[component]
fn SymbolLink(symbol: String, instrument_name: String) -> Element {
    let yahoo_url = yahoo_finance_url(&symbol);
    let title = if instrument_name.is_empty() {
        symbol.clone()
    } else {
        instrument_name
    };
    rsx! {
        a {
            class: "symbol-link",
            href: "{yahoo_url}",
            target: "_blank",
            rel: "noopener noreferrer",
            title: "{title}",
            "{symbol}"
        }
    }
}

#[component]
fn DecisionBadge(decision: JsonValue, prefs: LocalizationPrefs) -> Element {
    if decision.is_null() {
        return rsx! { span { class: "muted", "n/a" } };
    }
    let sentiment = text(&decision, "sentiment").to_uppercase();
    let action = text(&decision, "action");
    let created_at = text(&decision, "created_at");
    let decision_time = format_timestamp(&created_at, &prefs);
    let rationale = text(&decision, "target_rationale");
    let fallback_rationale = text(&decision, "rationale");
    let tooltip = if rationale.is_empty() {
        fallback_rationale
    } else {
        rationale
    };
    let sentiment_label = if sentiment.is_empty() {
        "HOLD".to_string()
    } else {
        sentiment.clone()
    };
    let tone = match sentiment.as_str() {
        "BUY" | "OVERWEIGHT" => "decision-chip good",
        "SELL" => "decision-chip bad",
        "UNDERWEIGHT" => "decision-chip warn",
        _ => "decision-chip neutral",
    };
    rsx! {
        span { class: "decision-cell", title: "{tooltip}",
            span { class: "decision-topline",
                span { class: tone, "{sentiment_label}" }
                if !action.is_empty() {
                    span { class: "decision-action", "{action}" }
                }
            }
            if !created_at.is_empty() {
                span { class: "decision-age", "{decision_time}" }
            }
        }
    }
}

#[component]
fn TrendSparkline(row: JsonValue) -> Element {
    let symbol = text(&row, "symbol");
    let modal_id = modal_id_for_symbol(&symbol);
    let tradingview_symbol = tradingview_symbol(&symbol);
    let trend = sparkline_points(&row);
    let tone = if trend.positive {
        "sparkline-line sparkline-good"
    } else {
        "sparkline-line sparkline-bad"
    };
    rsx! {
        span {
            a { class: "sparkline-link", href: "#{modal_id}", title: "Open TradingView chart for {symbol}",
                svg { class: "sparkline", view_box: "0 0 84 28", role: "img",
                    polyline { points: "{trend.points}", class: tone }
                }
            }
            div { id: "{modal_id}", class: "modal-target",
                a { class: "modal-dismiss", href: "#", "Close" }
                section { class: "chart-modal", role: "dialog", aria_label: "TradingView chart for {symbol}",
                    div { class: "section-title-row",
                        h2 { "{symbol}" }
                        div { class: "button-row",
                            a { class: "small-button", href: "{tradingview_page_url(&tradingview_symbol)}", target: "_blank", "Open on TradingView" }
                            a { class: "small-button", href: "#", "Close" }
                        }
                    }
                    iframe {
                        class: "tradingview-frame",
                        src: "{tradingview_url(&tradingview_symbol)}",
                        title: "TradingView chart for {symbol}"
                    }
                }
            }
        }
    }
}

#[component]
fn MarketRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let tradable = row
        .get("is_tradable")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let tradable_label = if tradable { "Yes" } else { "No" };
    rsx! {
        tr {
            td { strong { "{text(&row, \"market\")}" } div { class: "muted", "{text(&row, \"code\")} / {text(&row, \"timezone\")}" } }
            td { "{text(&row, \"status_reason\")}" }
            td { span { class: if tradable { "status good-status" } else { "status" }, "{tradable_label}" } }
            td { "{format_timestamp(&text(&row, \"session_open_at_utc\"), &prefs)}" }
            td { "{format_timestamp(&text(&row, \"session_close_at_utc\"), &prefs)}" }
            td { "{format_timestamp(&text(&row, \"tradable_close_at_utc\"), &prefs)}" }
            td { "{bool_label(&row, \"pre_analysis_sync_active\")}" }
            td { "{bool_label(&row, \"open_analysis_window_active\")}" }
            td { "{format_timestamp(&text(&row, \"next_open_at_utc\"), &prefs)}" }
        }
    }
}

#[component]
fn OrderRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let id = text(&row, "id");
    let created_at = format_timestamp(&text(&row, "created_at"), &prefs);
    let symbol = text(&row, "symbol");
    let action = text(&row, "action");
    let status = text(&row, "status");
    let quantity = format_quantity(value_f64(&row, "quantity"), &prefs);
    let limit = format_dkk(value_f64(&row, "limit_price_local"), &prefs);
    rsx! {
        tr {
            td { "{id}" }
            td { "{created_at}" }
            td { "{symbol}" }
            td { "{action}" }
            td { span { class: "status", "{status}" } }
            td { "{quantity}" }
            td { "{limit}" }
        }
    }
}

#[component]
fn ExecutionOrderRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    rsx! {
        tr {
            td { "{text(&row, \"id\")}" }
            td { "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            td { "{text(&row, \"symbol\")}" }
            td { "{text(&row, \"action\")}" }
            td { "{fallback_text(&row, \"strategy_type\", \"manual\")}" }
            td { "{fallback_text(&row, \"strategy_role\", \"primary\")}" }
            td { "{fallback_text(&row, \"order_type\", \"Market\")}" }
            td { span { class: "status", "{text(&row, \"status\")}" } }
            td { "{format_quantity(value_f64(&row, \"quantity\"), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"price_local\"), &text(&row, \"currency\"), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"limit_price_local\"), &text(&row, \"currency\"), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"stop_price_local\"), &text(&row, \"currency\"), &prefs)}" }
            td { class: "muted", "{text(&row, \"error_text\")}" }
        }
    }
}

#[component]
fn ExecutionFillRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    rsx! {
        tr {
            td { "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            td { "{text(&row, \"execution_order_id\")}" }
            td { "{text(&row, \"symbol\")}" }
            td { "{text(&row, \"side\")}" }
            td { "{fallback_text(&row, \"fill_status\", &text(&row, \"order_status\"))}" }
            td { "{format_quantity(value_f64(&row, \"delta_quantity\"), &prefs)}" }
            td { "{format_quantity(value_f64(&row, \"cumulative_quantity\"), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"average_price_local\"), &text(&row, \"currency\"), &prefs)}" }
            td { "{fallback_text(&row, \"ledger_id\", \"broker-only\")}" }
        }
    }
}

#[component]
fn HermesReflectionRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    rsx! {
        tr {
            td { "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            td { "{text_or(&row, \"goal_version\", \"n/a\")}" }
            td { "{text_or(&row, \"summary\", \"No summary recorded.\")}" }
            td { "{json_item_count(&row, \"findings_json\")}" }
            td { "{json_item_count(&row, \"proposed_actions_json\")}" }
            td { class: "muted", "{text(&row, \"source_session_id\")}" }
        }
    }
}

#[component]
fn HermesExperimentRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let status = text_or(&row, "status", "pending_review");
    let status_class = match status.as_str() {
        "approved_paper"
        | "active_paper"
        | "approved_sim"
        | "active_sim"
        | "ready_for_promotion"
        | "promoted" => "status good-status",
        "rejected" | "paper_failed" | "sim_failed" | "failed" => "status bad-status",
        _ => "status",
    };
    let experiment_id = text(&row, "id");
    rsx! {
        tr {
            td { "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            td { span { class: "{status_class}", "{status}" } }
            td { "{text(&row, \"changed_variable_path\")}" }
            td { class: "mono", "{short_json(row.get(\"old_value_json\"))}" }
            td { class: "mono", "{short_json(row.get(\"new_value_json\"))}" }
            td { "{text_or(&row, \"hypothesis\", \"No hypothesis recorded.\")}" }
            td { "{text_or(&row, \"expected_effect\", \"n/a\")}" }
            td { class: "mono", "{short_json(row.get(\"evidence_json\"))}" }
            td {
                div { class: "inline-actions",
                    for (action, label, tone) in hermes_transition_actions(&status) {
                        form {
                            method: "post",
                            action: "/api/hermes/experiments/{experiment_id}/transition",
                            input { r#type: "hidden", name: "action", value: "{action}" }
                            input { r#type: "hidden", name: "return_to", value: "/?view=hermes" }
                            button {
                                class: if tone == "danger" { "small-button danger" } else { "small-button" },
                                r#type: "submit",
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DecisionCard(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let id = text(&row, "id");
    let created_at = format_timestamp(&text(&row, "created_at"), &prefs);
    let status = text(&row, "status");
    rsx! {
        div { class: "event",
            strong { "Decision #{id}" }
            div { class: "muted", "{created_at} - {status}" }
        }
    }
}

fn value_f64(value: &JsonValue, key: &str) -> f64 {
    value
        .get(key)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
        .unwrap_or(0.0)
}

fn text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn text_or(value: &JsonValue, key: &str, fallback: &str) -> String {
    let text = text(value, key);
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn json_list_label(value: Option<&JsonValue>) -> String {
    let Some(items) = value.and_then(JsonValue::as_array) else {
        return "None".to_string();
    };
    if items.is_empty() {
        return "None".to_string();
    }
    items
        .iter()
        .filter_map(JsonValue::as_str)
        .take(4)
        .collect::<Vec<_>>()
        .join(", ")
}

fn json_item_count(value: &JsonValue, key: &str) -> usize {
    match value.get(key) {
        Some(JsonValue::Array(items)) => items.len(),
        Some(JsonValue::Object(map)) => map.len(),
        Some(JsonValue::String(text)) if !text.trim().is_empty() => 1,
        Some(value) if !value.is_null() => 1,
        _ => 0,
    }
}

fn hermes_transition_actions(status: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    match status {
        "pending_review" => vec![
            ("approve_paper", "Approve Paper", ""),
            ("reject", "Reject", "danger"),
        ],
        "approved_paper" => vec![
            ("activate_paper", "Start Paper", ""),
            ("reject", "Reject", "danger"),
        ],
        "active_paper" => vec![
            ("approve_sim", "Approve SIM", ""),
            ("mark_paper_failed", "Paper Failed", "danger"),
        ],
        "approved_sim" => vec![
            ("activate_sim", "Start SIM", ""),
            ("reject", "Reject", "danger"),
        ],
        "active_sim" => vec![
            ("ready_for_promotion", "Ready", ""),
            ("mark_sim_failed", "SIM Failed", "danger"),
        ],
        "ready_for_promotion" => vec![
            ("promote", "Promote Baseline", ""),
            ("reject", "Reject", "danger"),
        ],
        _ => Vec::new(),
    }
}

fn leader_row(items: &[JsonValue], high: bool) -> JsonValue {
    let mut rows = items
        .iter()
        .filter(|row| {
            row.get("change_pct")
                .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
                .is_some()
        })
        .cloned()
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        value_f64(left, "change_pct")
            .partial_cmp(&value_f64(right, "change_pct"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if high {
        rows.pop().unwrap_or(JsonValue::Null)
    } else {
        rows.into_iter().next().unwrap_or(JsonValue::Null)
    }
}

fn coverage_label(category: &JsonValue) -> String {
    let items = category
        .get("items")
        .and_then(JsonValue::as_array)
        .map(Vec::len)
        .unwrap_or(0) as f64;
    let target = value_f64(category, "target_limit").max(1.0);
    format!("{:.0}%", (items / target * 100.0).min(100.0))
}

fn bool_label(value: &JsonValue, key: &str) -> &'static str {
    if value.get(key).and_then(JsonValue::as_bool).unwrap_or(false) {
        "Yes"
    } else {
        "No"
    }
}

fn fallback_text(value: &JsonValue, key: &str, fallback: &str) -> String {
    let value = text(value, key);
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn format_local_money(value: f64, currency: &str, prefs: &LocalizationPrefs) -> String {
    if value.abs() < f64::EPSILON {
        "n/a".to_string()
    } else {
        format_money(value, currency, prefs)
    }
}

fn format_position_price(value: f64, currency: &str, prefs: &LocalizationPrefs) -> String {
    if value.abs() < f64::EPSILON {
        "n/a".to_string()
    } else {
        format!(
            "{} {}",
            crate::localization::format_number(value, 2, prefs),
            currency.trim().to_uppercase()
        )
    }
}

fn format_signed_dkk(value: f64, prefs: &LocalizationPrefs) -> String {
    let sign = if value > 0.0 { "+" } else { "" };
    format!("{sign}{}", format_dkk(value, prefs))
}

fn format_signed_pct(value: f64, prefs: &LocalizationPrefs) -> String {
    let sign = if value > 0.0 { "+" } else { "" };
    format!("{sign}{}", format_pct(value, prefs))
}

fn position_cost_price_local(row: &JsonValue) -> f64 {
    [
        value_f64(row, "cost_basis_local"),
        value_f64(row, "paid_price_local"),
        value_f64(row, "open_price_local"),
    ]
    .into_iter()
    .find(|value| value.abs() > f64::EPSILON)
    .unwrap_or(0.0)
}

fn position_total_return_pct(row: &JsonValue) -> f64 {
    let explicit = value_f64(row, "total_return_pct");
    if explicit.abs() > f64::EPSILON {
        return explicit;
    }
    let cost_basis = value_f64(row, "cost_basis_dkk");
    if cost_basis.abs() > f64::EPSILON {
        value_f64(row, "unrealised_pnl_dkk") / cost_basis
    } else {
        0.0
    }
}

fn position_daily_return_pct(row: &JsonValue) -> f64 {
    let explicit = value_f64(row, "daily_change_pct");
    if explicit.abs() > f64::EPSILON {
        return explicit;
    }
    let daily = value_f64(row, "daily_pnl_dkk");
    let prior_value = value_f64(row, "market_value_dkk") - daily;
    if prior_value.abs() > f64::EPSILON {
        daily / prior_value
    } else {
        0.0
    }
}

fn position_modal_id(symbol: &str) -> String {
    format!("position-{}", modal_id_for_symbol(symbol))
}

fn asset_label(row: &JsonValue) -> String {
    text_or(row, "asset_class", "Equity")
}

struct ChartPaths {
    portfolio_points: String,
    cash_points: String,
    portfolio_min_label: String,
    portfolio_max_label: String,
    cash_min_label: String,
    cash_max_label: String,
    start_label: String,
    end_label: String,
}

fn chart_paths(rows: &[JsonValue]) -> ChartPaths {
    let portfolio = rows
        .iter()
        .map(|row| value_f64(row, "total_market_value_dkk"))
        .collect::<Vec<_>>();
    let cash = rows
        .iter()
        .map(|row| value_f64(row, "cash_balance_dkk"))
        .collect::<Vec<_>>();
    let portfolio_min = min_value(&portfolio);
    let portfolio_max = max_value(&portfolio);
    let cash_min = min_value(&cash);
    let cash_max = max_value(&cash);
    ChartPaths {
        portfolio_points: series_points(&portfolio, portfolio_min, portfolio_max),
        cash_points: series_points(&cash, cash_min, cash_max),
        portfolio_min_label: format!("{} DKK", portfolio_min.round() as i64),
        portfolio_max_label: format!("{} DKK", portfolio_max.round() as i64),
        cash_min_label: format!("{} DKK", cash_min.round() as i64),
        cash_max_label: format!("{} DKK", cash_max.round() as i64),
        start_label: rows
            .first()
            .map(|row| text(row, "recorded_at"))
            .unwrap_or_default(),
        end_label: rows
            .last()
            .map(|row| text(row, "recorded_at"))
            .unwrap_or_default(),
    }
}

fn series_points(values: &[f64], min: f64, max: f64) -> String {
    if values.is_empty() {
        return "56,236 944,236".to_string();
    }
    if values.len() == 1 {
        let y = 236.0 - ((values[0] - min) / (max - min).abs().max(1.0)) * 212.0;
        return format!("56,{y:.1} 944,{y:.1}");
    }
    let span = (max - min).abs().max(1.0);
    let last_index = (values.len().saturating_sub(1)).max(1) as f64;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = 56.0 + (index as f64 / last_index) * 888.0;
            let y = 236.0 - ((*value - min) / span) * 212.0;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn min_value(values: &[f64]) -> f64 {
    values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .reduce(f64::min)
        .unwrap_or(0.0)
}

fn max_value(values: &[f64]) -> f64 {
    values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .reduce(f64::max)
        .unwrap_or(0.0)
}

fn modal_id_for_symbol(symbol: &str) -> String {
    format!(
        "chart-{}",
        symbol
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>()
    )
}

fn tradingview_symbol(symbol: &str) -> String {
    let (base, exchange) = symbol.split_once(':').unwrap_or((symbol, ""));
    let tv_exchange = match exchange.to_lowercase().as_str() {
        "xnas" => "NASDAQ",
        "xnys" => "NYSE",
        "xcse" => "OMXCOP",
        "xlon" => "LSE",
        "xetr" => "XETR",
        "xams" => "EURONEXT",
        "xsto" => "OMXSTO",
        "xosl" => "OSL",
        "xhel" => "OMXHEX",
        "xmil" => "MIL",
        _ => "NASDAQ",
    };
    format!("{tv_exchange}:{base}")
}

fn tradingview_url(symbol: &str) -> String {
    format!(
        "https://www.tradingview-widget.com/embed-widget/advanced-chart/?symbol={}&interval=D&timezone=Europe%2FCopenhagen&theme=light&style=1&locale=en&allow_symbol_change=true&calendar=false&details=true&support_host=https%3A%2F%2Fwww.tradingview.com",
        symbol.replace(':', "%3A")
    )
}

fn tradingview_page_url(symbol: &str) -> String {
    format!(
        "https://www.tradingview.com/chart/?symbol={}",
        symbol.replace(':', "%3A")
    )
}

struct Sparkline {
    points: String,
    positive: bool,
}

fn sparkline_points(row: &JsonValue) -> Sparkline {
    let change_pct = value_f64(row, "change_pct");
    let technical = row
        .get("decision")
        .and_then(|decision| decision.get("source"))
        .and_then(|source| source.get("technical"))
        .cloned()
        .unwrap_or(JsonValue::Null);
    let trend_bias = text(&technical, "trend_bias");
    let positive = if trend_bias == "bearish" {
        false
    } else if trend_bias == "bullish" {
        true
    } else {
        change_pct >= 0.0
    };
    let slope = if positive { -1.0 } else { 1.0 };
    let strength = change_pct.abs().clamp(0.002, 0.08) / 0.08;
    let seed = symbol_seed(&text(row, "symbol"));
    let mut points = Vec::new();
    for idx in 0..6 {
        let x = 4.0 + idx as f64 * 15.0;
        let wiggle = (((seed + idx as u64 * 17) % 11) as f64 - 5.0) * 0.55;
        let y = 20.0 + slope * idx as f64 * (2.2 + strength * 2.4) + wiggle;
        points.push(format!("{x:.1},{:.1}", y.clamp(4.0, 24.0)));
    }
    Sparkline {
        points: points.join(" "),
        positive,
    }
}

fn symbol_seed(symbol: &str) -> u64 {
    symbol.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    })
}

fn yahoo_finance_url(symbol: &str) -> String {
    let yahoo = yahoo_ticker(symbol);
    format!("https://finance.yahoo.com/quote/{yahoo}")
}

fn yahoo_ticker(symbol: &str) -> String {
    let (base, exchange) = symbol.split_once(':').unwrap_or((symbol, ""));
    let base = base.replace('/', "-");
    let suffix = match exchange.to_lowercase().as_str() {
        "xcse" => ".CO",
        "xlon" => ".L",
        "xetr" => ".DE",
        "xams" => ".AS",
        "xsto" => ".ST",
        "xosl" => ".OL",
        "xhel" => ".HE",
        "xmil" => ".MI",
        "xwar" => ".WA",
        "xbru" => ".BR",
        "xpar" => ".PA",
        "xnas" | "xnys" => "",
        _ => "",
    };
    format!("{base}{suffix}")
}

fn compact_json(value: Option<&JsonValue>) -> String {
    let Some(value) = value else {
        return "No report payload available.".to_string();
    };
    let rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let max_len = 1600;
    if rendered.len() > max_len {
        format!("{}...", &rendered[..max_len])
    } else {
        rendered
    }
}

fn short_json(value: Option<&JsonValue>) -> String {
    let Some(value) = value else {
        return "n/a".to_string();
    };
    let rendered = match value {
        JsonValue::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| value.to_string()),
    };
    let max_len = 220;
    if rendered.len() > max_len {
        format!("{}...", &rendered[..max_len])
    } else {
        rendered
    }
}

fn json_array(value: &JsonValue, key: &str) -> Vec<JsonValue> {
    value
        .get(key)
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
}

fn gravatar_url(email: &str) -> String {
    let normalized = email.trim().to_lowercase();
    if normalized.is_empty() {
        return String::new();
    }
    let digest = md5::compute(normalized.as_bytes());
    format!("https://www.gravatar.com/avatar/{digest:x}?d=404&s=96")
}

fn initials_for_name(name: &str, email: &str) -> String {
    let source = if name.trim().is_empty() { email } else { name };
    let initials = source
        .split(|ch: char| ch.is_whitespace() || ch == '.' || ch == '@' || ch == '-')
        .filter(|part| !part.is_empty())
        .take(2)
        .filter_map(|part| part.chars().next())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    if initials.is_empty() {
        "SSO".to_string()
    } else {
        initials
    }
}

#[cfg(test)]
fn number(value: &JsonValue, key: &str, decimals: usize) -> String {
    let number = value_f64(value, key);
    let prefs = default_prefs();
    crate::localization::format_number(number, decimals, &prefs)
}

fn format_dkk(value: f64, prefs: &LocalizationPrefs) -> String {
    format_money(value, "DKK", prefs)
}

fn format_pct(value: f64, prefs: &LocalizationPrefs) -> String {
    format_percent(value, prefs)
}

fn distribution_label(value: Option<&JsonValue>, prefs: &LocalizationPrefs) -> String {
    let Some(value) = value else {
        return "n/a".to_string();
    };
    format!(
        "B {} / S {} / Bear {}",
        format_pct(value_f64(value, "Bull"), prefs),
        format_pct(value_f64(value, "Sideways"), prefs),
        format_pct(value_f64(value, "Bear"), prefs)
    )
}

#[cfg(test)]
fn default_prefs() -> LocalizationPrefs {
    LocalizationPrefs {
        locale: "en-DK".to_string(),
        time_zone: "Europe/Copenhagen".to_string(),
        hour_cycle: crate::localization::HourCycle::H24,
        week_start: crate::localization::WeekStart::Monday,
        group_separator: ",".to_string(),
        decimal_separator: ".".to_string(),
        measurement_system: "metric".to_string(),
    }
}

fn week_start_label(prefs: &LocalizationPrefs) -> &'static str {
    match prefs.week_start {
        crate::localization::WeekStart::Monday => "Monday",
        crate::localization::WeekStart::Sunday => "Sunday",
        crate::localization::WeekStart::Saturday => "Saturday",
    }
}

fn hour_cycle_label(prefs: &LocalizationPrefs) -> &'static str {
    match prefs.hour_cycle {
        crate::localization::HourCycle::H12 => "12-hour",
        crate::localization::HourCycle::H24 => "24-hour",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_dashboard_numbers_for_display() {
        let prefs = default_prefs();
        assert_eq!(format_dkk(1234.4, &prefs), "1,234 DKK");
        assert_eq!(format_pct(0.125, &prefs), "12.5%");
        assert_eq!(format_pct(12.5, &prefs), "12.5%");
    }

    #[test]
    fn extracts_display_text_from_json() {
        let value = json!({"symbol": "AAPL:xnas", "quantity": 12});
        assert_eq!(text(&value, "symbol"), "AAPL:xnas");
        assert_eq!(number(&value, "quantity", 0), "12");
    }

    #[test]
    fn prefixes_root_relative_urls_for_shared_ngrok_base_path() {
        let html = r#"<a href="/api/health">Health</a><form action="/api/actions/decision-report"><input value="/?view=market" /><img src="/favicon.svg" /><a href="https://example.com">External</a>"#;
        let prefixed = prefix_root_relative_urls(html, "/saxo-daytrader");

        assert!(prefixed.contains(r#"href="/saxo-daytrader/api/health""#));
        assert!(prefixed.contains(r#"action="/saxo-daytrader/api/actions/decision-report""#));
        assert!(prefixed.contains(r#"value="/saxo-daytrader/?view=market""#));
        assert!(prefixed.contains(r#"src="/saxo-daytrader/favicon.svg""#));
        assert!(prefixed.contains(r#"href="https://example.com""#));
    }
}
