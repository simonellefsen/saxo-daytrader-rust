use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use dioxus::prelude::*;
use serde_json::{Map, Value as JsonValue};

use crate::{
    localization::{
        LocalizationPrefs, format_money, format_percent, format_quantity, format_timestamp,
    },
    models::DashboardView,
};

pub const CSS: &str = include_str!("../assets/app.css");
pub const FAVICON_SVG: &str = include_str!("../assets/favicon.svg");
const APP_SCRIPT: &str = r#"
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
  const appBasePath = () => {
    const configured = document.body?.dataset?.publicBasePath || "";
    if (configured) return configured.replace(/\/$/, "");
    const stylesheet = document.querySelector('link[rel="stylesheet"][href$="/assets/app.css"]');
    if (!stylesheet) {
      const path = window.location.pathname;
      return path.startsWith("/saxo-daytrader") ? "/saxo-daytrader" : "";
    }
    const path = new URL(stylesheet.href, window.location.href).pathname;
    return path.replace(/\/assets\/app\.css$/, "");
  };
  const bindDecisionReportForms = () => {
    document.querySelectorAll("form[data-decision-report-form]").forEach((form) => {
      if (form.dataset.bound === "true") return;
      form.dataset.bound = "true";
      form.addEventListener("submit", () => {
        const button = form.querySelector("button[type='submit']");
        if (!button) return;
        button.disabled = true;
        button.dataset.originalLabel = button.textContent || "";
        button.textContent = button.dataset.pendingLabel || "Generating Report...";
        form.classList.add("is-submitting");
      });
    });
  };
  const bindDecisionReportPendingRefresh = () => {
    const pending = document.querySelector("[data-decision-report-pending='true']");
    if (!pending || pending.dataset.bound === "true") return;
    pending.dataset.bound = "true";
    const base = appBasePath();
    const poll = async () => {
      try {
        const response = await fetch(`${base}/api/decision/latest`, { headers: { "Accept": "application/json" } });
        if (!response.ok) return;
        const payload = await response.json();
        const report = payload.report || {};
        const status = report.status || "";
        const id = report.id ? String(report.id) : "";
        const baselineId = pending.dataset.baselineReportId || "";
        const baselineStatus = pending.dataset.baselineReportStatus || "";
        const changed = id !== baselineId || status !== baselineStatus;
        if (changed && status && status !== "xai_deferred" && status !== "pending") {
          const suffix = id ? `&report_id=${encodeURIComponent(id)}` : "";
          window.location.href = `${base}/?view=decisions${suffix}`;
        }
      } catch (_) {
      }
    };
    window.setTimeout(poll, 4000);
    window.setInterval(poll, 10000);
  };
  const bindApp = () => {
    bindPerformanceCharts();
    bindDecisionReportForms();
    bindDecisionReportPendingRefresh();
  };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", bindApp, { once: true });
  } else {
    bindApp();
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
  <body data-public-base-path="{public_base_path}">{body}{APP_SCRIPT}</body>
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
    let (decision_health_class, decision_health_label) = decision_health(&data.latest_decision);
    let operation_items = operations_health(&data);
    let saxo_status_display = truncate_chars(&data.saxo_status, 90);
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
                    }
                    div { class: "pill-row right",
                        span { class: decision_health_class, span { class: "dot" } "{decision_health_label}" }
                        span { class: saxo_status_class, span { class: "dot" } "{saxo_environment} · {saxo_status_display}" }
                    }
                    div { class: "user-row",
                        UserMenu { sso_session: data.sso_session.clone(), prefs: prefs.clone(), active_view: data.active_view.clone(), range: data.performance_range.clone(), ai_settings: data.ai_settings.clone() }
                        a { class: "button secondary", href: "/api/saxo/auth/start", "Saxo Login" }
                        a { class: "button", href: "/api/health", "Health" }
                    }
                }
            }
            OperationsHealthBanner { items: operation_items }
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
    ai_settings: JsonValue,
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
    let ai_model = fallback_text(&ai_settings, "model", "openai/gpt-5.5");
    let ai_source = fallback_text(&ai_settings, "source", "config");
    let ai_config_model = fallback_text(&ai_settings, "config_model", "openai/gpt-5.5");
    let key_status = ai_settings.get("api_key").cloned().unwrap_or_default();
    let key_source = fallback_text(&key_status, "source", "missing");
    let key_hint = if key_status
        .get("configured")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        let masked = fallback_text(&key_status, "masked", "•••");
        let updated = key_status
            .get("updated_at")
            .and_then(JsonValue::as_str)
            .map(|value| format!(" · updated {value}"))
            .unwrap_or_default();
        format!("Active key: {masked} · source: {key_source}{updated}")
    } else {
        "No API key configured — decision reports cannot be submitted.".to_string()
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
                form { method: "post", action: "/api/settings/ai", class: "settings-form settings-form-wide",
                    input { r#type: "hidden", name: "return_to", value: "{return_to}" }
                    label { "OpenRouter model"
                        input { name: "model", value: "{ai_model}", list: "ai-model-options" }
                        datalist { id: "ai-model-options",
                            option { value: "openrouter/fusion" }
                            option { value: "openai/gpt-5.5" }
                            option { value: "openai/gpt-5" }
                            option { value: "anthropic/claude-sonnet-4.5" }
                        }
                    }
                    div { class: "settings-hint", "Active: {ai_model} · source: {ai_source} · config: {ai_config_model}" }
                    button { class: "button", r#type: "submit", "Save AI model" }
                }
                form { method: "post", action: "/api/settings/ai-key", class: "settings-form settings-form-wide",
                    input { r#type: "hidden", name: "return_to", value: "{return_to}" }
                    label { "OpenRouter API key"
                        input {
                            r#type: "password",
                            name: "api_key",
                            placeholder: "sk-or-… (leave empty to clear the override)",
                            autocomplete: "off",
                        }
                    }
                    div { class: "settings-hint", "{key_hint}" }
                    button { class: "button", r#type: "submit", "Save API key" }
                }
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

#[derive(Clone, Debug, PartialEq)]
struct OperationHealthItem {
    label: String,
    status: String,
    tone: &'static str,
    detail: String,
}

#[component]
fn OperationsHealthBanner(items: Vec<OperationHealthItem>) -> Element {
    let banner_class = if items.iter().any(|item| item.tone == "bad") {
        "operations-banner bad"
    } else if items.iter().any(|item| item.tone == "warn") {
        "operations-banner warn"
    } else {
        "operations-banner good"
    };
    rsx! {
        section { class: "{banner_class}", "aria-label": "Operations health",
            div { class: "operations-banner-title", "Operations" }
            div { class: "operations-chip-row",
                for item in items {
                    span { class: "operation-chip {item.tone}", title: "{item.detail}",
                        strong { "{item.label}" }
                        span { "{item.status}" }
                    }
                }
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
            TabLink { href: "/?view=quiver", label: "Quiver", active: active_view == "quiver" }
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
        "quiver" => rsx! { QuiverView { data, prefs } },
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
                                tbody { for row in data.positions.iter() {
                                    PositionRow {
                                        row: row.clone(),
                                        prefs: prefs.clone(),
                                        decision_stale_after_days: data.position_decision_stale_after_days,
                                    }
                                } }
                            }
                        }
                    }
                    section { class: "section",
                        h2 { "Execution Queue" }
                        div { class: "table-wrap",
                            table {
                                thead { tr { th { "ID" } th { "Created" } th { "Symbol" } th { "Action" } th { "Status" } th { "Qty" } th { "Limit" } th { "Expiry" } } }
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
                    CashDeploymentPanel { trading_manager: data.trading_manager.clone(), prefs: prefs.clone() }
                    IntegrityPanel { integrity: data.integrity.clone(), prefs: prefs.clone() }
                    InstrumentQuarantinePanel { trading_manager: data.trading_manager.clone(), prefs: prefs.clone() }
                    section { class: "section",
                        h2 { "Recent Decisions" }
                        div { class: "stack", for row in data.reports.iter() { DecisionCard { row: row.clone(), prefs: prefs.clone() } } }
                    }
                }
            }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct CashDeploymentSummary {
    status: String,
    tone: &'static str,
    run_label: String,
    available_buy_budget_dkk: f64,
    excess_cash_pct: f64,
    approved_buy_count: i64,
    skipped_buy_count: i64,
    candidate_buy_count: i64,
    breaker_active: bool,
    breaker_threshold_breached: bool,
    breaker_override_active: bool,
    breaker_month_pnl_dkk: f64,
    breaker_threshold_dkk: f64,
    breaker_soft_reduction_active: bool,
    breaker_soft_threshold_dkk: f64,
    breaker_soft_buy_multiplier: f64,
    breaker_override_month_key: String,
    breaker_override_updated_at: String,
    breaker_override_notes: String,
    description: String,
}

#[component]
fn CashDeploymentPanel(trading_manager: JsonValue, prefs: LocalizationPrefs) -> Element {
    let latest = trading_manager
        .get("latest_run")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let summary = cash_deployment_summary(&latest, &prefs);
    rsx! {
        section { class: "section",
            h2 { "Cash Deployment" }
            if latest.is_null() {
                div { class: "event",
                    strong { "No Trading Manager run yet." }
                    span { class: "muted", "Cash deployment diagnostics will appear after the next decision report is processed." }
                }
            } else {
                div { class: "cash-deployment-panel",
                    div { class: "event",
                        strong { "Latest manager run" }
                        span { "{summary.run_label}" }
                        span { class: "status {summary.tone}", "{summary.status}" }
                    }
                    div { class: "cash-diagnostic-grid",
                        div { span { class: "label", "Buy budget" } strong { "{format_dkk(summary.available_buy_budget_dkk, &prefs)}" } }
                        div { span { class: "label", "Excess cash" } strong { "{format_pct(summary.excess_cash_pct, &prefs)}" } }
                        div { span { class: "label", "BUY candidates" } strong { "{summary.candidate_buy_count}" } }
                        div { span { class: "label", "Approved / blocked" } strong { "{summary.approved_buy_count} / {summary.skipped_buy_count}" } }
                    }
                    div { class: "event cash-diagnostic-reason",
                        strong { "Reason" }
                        span { "{summary.description}" }
                    }
                    if summary.breaker_threshold_breached || summary.breaker_soft_reduction_active {
                        div { class: "event cash-diagnostic-reason",
                            strong { "Monthly-loss circuit breaker" }
                            span {
                                if summary.breaker_active {
                                    "BUYs are suspended because month P/L {format_dkk(summary.breaker_month_pnl_dkk, &prefs)} breached the {format_dkk(summary.breaker_threshold_dkk, &prefs)} floor."
                                } else if summary.breaker_soft_reduction_active {
                                    "BUY budget is reduced to {format_pct(summary.breaker_soft_buy_multiplier, &prefs)} because month P/L {format_dkk(summary.breaker_month_pnl_dkk, &prefs)} reached the {format_dkk(summary.breaker_soft_threshold_dkk, &prefs)} soft-loss floor. SELLs are unchanged."
                                } else if summary.breaker_override_active {
                                    "Threshold is breached, but an operator override resumed BUYs for {summary.breaker_override_month_key}."
                                } else {
                                    "Threshold was breached in this run."
                                }
                            }
                            if !summary.breaker_override_updated_at.is_empty() {
                                span { class: "muted", "Override updated {summary.breaker_override_updated_at}" }
                            }
                            if !summary.breaker_override_notes.is_empty() {
                                span { class: "muted", "Notes: {summary.breaker_override_notes}" }
                            }
                            form { method: "post", action: "/api/settings/monthly-loss-breaker", class: "inline-form",
                                input { r#type: "hidden", name: "return_to", value: "/" }
                                if summary.breaker_override_active {
                                    input { r#type: "hidden", name: "action", value: "clear_override" }
                                    input { r#type: "text", name: "notes", placeholder: "Reason for clearing override" }
                                    button { class: "button secondary", r#type: "submit", "Clear Override" }
                                } else {
                                    input { r#type: "hidden", name: "action", value: "resume_buys" }
                                    input { r#type: "text", name: "notes", placeholder: "Reason for resuming BUYs this month" }
                                    button { class: "button", r#type: "submit", "Resume BUYs This Month" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn cash_deployment_summary(
    latest_run: &JsonValue,
    prefs: &LocalizationPrefs,
) -> CashDeploymentSummary {
    let manager = latest_run
        .get("manager_json")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let diagnostics = manager
        .get("reinvestment_diagnostics")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let budget = diagnostics
        .get("capital_budget")
        .or_else(|| manager.get("capital_budget"))
        .cloned()
        .unwrap_or(JsonValue::Null);
    let breaker = manager
        .get("monthly_loss_circuit_breaker")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let breaker_override = breaker.get("override").cloned().unwrap_or(JsonValue::Null);
    let status = fallback_text(
        &diagnostics,
        "status",
        &fallback_text(latest_run, "status", "unknown"),
    );
    let tone = cash_deployment_tone(&status);
    let created_at = format_timestamp(&text(latest_run, "created_at"), prefs);
    let report_id = text(latest_run, "report_id");
    let run_label = if report_id.is_empty() {
        created_at
    } else {
        format!("Report #{report_id} · {created_at}")
    };
    CashDeploymentSummary {
        status,
        tone,
        run_label,
        available_buy_budget_dkk: value_f64(&budget, "available_buy_budget_dkk"),
        excess_cash_pct: value_f64(&budget, "excess_cash_pct"),
        approved_buy_count: value_i64(&diagnostics, "approved_buy_count"),
        skipped_buy_count: value_i64(&diagnostics, "skipped_buy_count"),
        candidate_buy_count: value_i64(&diagnostics, "buy_candidate_count"),
        breaker_active: breaker
            .get("active")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        breaker_threshold_breached: breaker
            .get("threshold_breached")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        breaker_override_active: breaker
            .get("override_active")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        breaker_month_pnl_dkk: value_f64(&breaker, "month_pnl_dkk"),
        breaker_threshold_dkk: value_f64(&breaker, "threshold_dkk"),
        breaker_soft_reduction_active: breaker
            .get("soft_reduction_active")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        breaker_soft_threshold_dkk: value_f64(&breaker, "soft_threshold_dkk"),
        breaker_soft_buy_multiplier: value_f64(&breaker, "soft_buy_multiplier"),
        breaker_override_month_key: text(&breaker_override, "month_key"),
        breaker_override_updated_at: format_timestamp(
            &text(&breaker_override, "updated_at"),
            prefs,
        ),
        breaker_override_notes: text(&breaker_override, "notes"),
        description: fallback_text(
            &diagnostics,
            "description",
            "No reinvestment diagnostic was recorded for this Trading Manager run.",
        ),
    }
}

fn cash_deployment_tone(status: &str) -> &'static str {
    match status {
        "reinvestment_candidates_approved" => "good-status",
        "no_reinvestment_pressure" | "no_cash_budget_available" => "",
        "excess_cash_without_buy_candidates" | "excess_cash_with_blocked_buy_candidates" => {
            "warn-status"
        }
        _ => "",
    }
}

#[component]
fn IntegrityPanel(integrity: JsonValue, prefs: LocalizationPrefs) -> Element {
    let summary = integrity_summary(&integrity, &prefs);
    let warnings = integrity
        .get("warnings")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mismatches = integrity
        .get("mismatches")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let expiry_pending_orders = integrity
        .get("expiry_pending_orders")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    rsx! {
        section { class: "section",
            h2 { "Integrity" }
            div { class: "cash-deployment-panel",
                div { class: "event",
                    strong { "Status" }
                    span { class: "status {summary.0}", "{summary.1}" }
                    span { class: "muted", "{summary.2}" }
                }
                if !mismatches.is_empty() || !warnings.is_empty() {
                    div { class: "stack",
                        for row in mismatches.iter().chain(warnings.iter()) {
                            IntegrityIssueRow { row: row.clone() }
                        }
                    }
                }
                if !expiry_pending_orders.is_empty() {
                    div { class: "table-wrap",
                        table {
                            thead { tr { th { "Order" } th { "Symbol" } th { "Status" } th { "Expiry" } } }
                            tbody {
                                for row in expiry_pending_orders.iter() {
                                    {
                                        let order_id = text(row, "id");
                                        let symbol = text(row, "symbol");
                                        let status = text(row, "status");
                                        let expiry = format_timestamp(&text(row, "expected_expiry_at_utc"), &prefs);
                                        rsx! {
                                    tr {
                                        td { "#{order_id}" }
                                        td { "{symbol}" }
                                        td { "{status}" }
                                        td { "{expiry}" }
                                    }
                                        }
                                    }
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
fn IntegrityIssueRow(row: JsonValue) -> Element {
    let code = text(&row, "code");
    let severity = text(&row, "severity");
    let message = text(&row, "message");
    let issue_key = text(&row, "issue_key");
    let acknowledged = row
        .get("acknowledged")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let acknowledgement = row
        .get("acknowledgement")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let acknowledgement_notes = text(&acknowledgement, "notes");
    let acknowledgement_updated_at = text(&acknowledgement, "updated_at");
    let tone = match severity.as_str() {
        "error" => "bad-status",
        "warning" => "warn-status",
        _ => "",
    };
    rsx! {
        div { class: "event", title: "{message}",
            strong { "{code}" }
            span { class: "status {tone}", "{severity}" }
            span { class: "muted", "{message}" }
            if acknowledged {
                span { class: "status warn-status", "acknowledged" }
                if !acknowledgement_notes.is_empty() {
                    span { class: "muted", "{acknowledgement_notes}" }
                }
                if !acknowledgement_updated_at.is_empty() {
                    span { class: "muted", "ack {acknowledgement_updated_at}" }
                }
                form { method: "post", action: "/api/settings/overview-integrity", class: "inline-form compact-inline-form",
                    input { r#type: "hidden", name: "return_to", value: "/" }
                    input { r#type: "hidden", name: "operation", value: "clear_acknowledgement" }
                    input { r#type: "hidden", name: "issue_key", value: "{issue_key}" }
                    input { r#type: "hidden", name: "code", value: "{code}" }
                    input { r#type: "hidden", name: "severity", value: "{severity}" }
                    input { r#type: "text", name: "notes", placeholder: "Reason for clearing" }
                    button { class: "button secondary", r#type: "submit", "Clear ack" }
                }
            } else if !issue_key.is_empty() {
                form { method: "post", action: "/api/settings/overview-integrity", class: "inline-form compact-inline-form",
                    input { r#type: "hidden", name: "return_to", value: "/" }
                    input { r#type: "hidden", name: "operation", value: "acknowledge" }
                    input { r#type: "hidden", name: "issue_key", value: "{issue_key}" }
                    input { r#type: "hidden", name: "code", value: "{code}" }
                    input { r#type: "hidden", name: "severity", value: "{severity}" }
                    input { r#type: "text", name: "notes", placeholder: "Acknowledgement note" }
                    button { class: "button", r#type: "submit", "Acknowledge" }
                }
            }
        }
    }
}

fn integrity_summary(
    integrity: &JsonValue,
    prefs: &LocalizationPrefs,
) -> (&'static str, String, String) {
    let mismatch_count = integrity
        .get("mismatches")
        .and_then(JsonValue::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let warning_count = integrity
        .get("warnings")
        .and_then(JsonValue::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let checked_at = format_timestamp(&text(integrity, "checked_at"), prefs);
    let acknowledged_count = value_i64(integrity, "acknowledged_issue_count");
    let ack_suffix = if acknowledged_count > 0 {
        format!(" · {acknowledged_count} acknowledged")
    } else {
        String::new()
    };
    if mismatch_count > 0 {
        (
            "bad-status",
            format!("{mismatch_count} error"),
            format!("{warning_count} warning(s) · checked {checked_at}{ack_suffix}"),
        )
    } else if warning_count > 0 {
        (
            "warn-status",
            format!("{warning_count} warning"),
            format!("checked {checked_at}{ack_suffix}"),
        )
    } else {
        (
            "good-status",
            "clear".to_string(),
            format!("checked {checked_at}"),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
struct InstrumentQuarantineSummary {
    enabled: bool,
    status: String,
    tone: &'static str,
    active_count: i64,
    blocked_count: i64,
    override_count: i64,
    lookback_days: i64,
    min_failures: i64,
    active_days: i64,
    active: Vec<JsonValue>,
    description: String,
}

#[component]
fn InstrumentQuarantinePanel(trading_manager: JsonValue, prefs: LocalizationPrefs) -> Element {
    let latest = trading_manager
        .get("latest_run")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let summary = instrument_quarantine_summary(&latest);
    rsx! {
        section { class: "section",
            h2 { "Instrument Quarantine" }
            if latest.is_null() {
                div { class: "event",
                    strong { "No Trading Manager run yet." }
                    span { class: "muted", "Quarantine diagnostics will appear after the next decision report is processed." }
                }
            } else {
                div { class: "cash-deployment-panel",
                    div { class: "event",
                        strong { "Status" }
                        span { class: "status {summary.tone}", "{summary.status}" }
                        span { class: "muted", "{summary.description}" }
                    }
                    div { class: "cash-diagnostic-grid",
                        div { span { class: "label", "Active" } strong { "{summary.active_count}" } }
                        div { span { class: "label", "Blocked / overridden" } strong { "{summary.blocked_count} / {summary.override_count}" } }
                        div { span { class: "label", "Lookback" } strong { "{summary.lookback_days}d" } }
                        div { span { class: "label", "Min failures" } strong { "{summary.min_failures}" } }
                        div { span { class: "label", "Active window" } strong { "{summary.active_days}d" } }
                    }
                    if !summary.active.is_empty() {
                        div { class: "table-wrap",
                            table {
                                thead { tr { th { "Symbol" } th { "Side" } th { "Failure" } th { "Count" } th { "Expires" } th { "Override" } } }
                                tbody {
                                    for row in summary.active.iter() {
                                        InstrumentQuarantineRow { row: row.clone(), prefs: prefs.clone() }
                                    }
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
fn InstrumentQuarantineRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let sample = fallback_text(&row, "sample_error", "No sample error recorded.");
    let symbol = text(&row, "symbol");
    let action = text(&row, "action");
    let signature = text(&row, "signature");
    let override_active = row
        .get("override_active")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let override_notes = text(&row, "override_notes");
    rsx! {
        tr { title: "{sample}",
            td { class: "mono", "{symbol}" }
            td { "{action}" }
            td { "{signature}" }
            td { "{value_i64(&row, \"failure_count\")}" }
            td { "{format_timestamp(&text(&row, \"expires_at\"), &prefs)}" }
            td {
                if override_active {
                    span { class: "status warn-status", "overridden" }
                    if !override_notes.is_empty() {
                        div { class: "muted", "{override_notes}" }
                    }
                    form { method: "post", action: "/api/settings/instrument-quarantine", class: "inline-form compact-inline-form",
                        input { r#type: "hidden", name: "return_to", value: "/" }
                        input { r#type: "hidden", name: "operation", value: "clear_override" }
                        input { r#type: "hidden", name: "symbol", value: "{symbol}" }
                        input { r#type: "hidden", name: "side", value: "{action}" }
                        input { r#type: "hidden", name: "signature", value: "{signature}" }
                        input { r#type: "text", name: "notes", placeholder: "Reason for clearing" }
                        button { class: "button secondary", r#type: "submit", "Clear" }
                    }
                } else {
                    form { method: "post", action: "/api/settings/instrument-quarantine", class: "inline-form compact-inline-form",
                        input { r#type: "hidden", name: "return_to", value: "/" }
                        input { r#type: "hidden", name: "operation", value: "override" }
                        input { r#type: "hidden", name: "symbol", value: "{symbol}" }
                        input { r#type: "hidden", name: "side", value: "{action}" }
                        input { r#type: "hidden", name: "signature", value: "{signature}" }
                        input { r#type: "text", name: "notes", placeholder: "Override reason" }
                        button { class: "button", r#type: "submit", "Override" }
                    }
                }
            }
        }
    }
}

fn instrument_quarantine_summary(latest_run: &JsonValue) -> InstrumentQuarantineSummary {
    let quarantine = latest_run
        .get("manager_json")
        .and_then(|value| value.get("instrument_quarantine"))
        .cloned()
        .unwrap_or(JsonValue::Null);
    let enabled = quarantine
        .get("enabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let active = quarantine
        .get("active")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let active_count = quarantine
        .get("active_count")
        .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
        .unwrap_or(active.len() as i64);
    let blocked_count = quarantine
        .get("blocked_count")
        .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
        .unwrap_or_else(|| {
            active
                .iter()
                .filter(|row| {
                    !row.get("override_active")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false)
                })
                .count() as i64
        });
    let override_count = quarantine
        .get("override_count")
        .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
        .unwrap_or_else(|| active_count.saturating_sub(blocked_count));
    let (status, tone, description) = if !enabled {
        (
            "disabled".to_string(),
            "",
            "The quarantine gate was disabled for the latest manager run.".to_string(),
        )
    } else if blocked_count > 0 {
        (
            "active".to_string(),
            "warn-status",
            format!("{blocked_count} symbol/action pair(s) are blocked before queueing."),
        )
    } else if active_count > 0 {
        (
            "overridden".to_string(),
            "warn-status",
            format!("{override_count} active quarantine(s) have operator overrides."),
        )
    } else {
        (
            "clear".to_string(),
            "good-status",
            "No active instrument quarantines in the latest manager run.".to_string(),
        )
    };
    InstrumentQuarantineSummary {
        enabled,
        status,
        tone,
        active_count,
        blocked_count,
        override_count,
        lookback_days: value_i64(&quarantine, "lookback_days"),
        min_failures: value_i64(&quarantine, "min_failures"),
        active_days: value_i64(&quarantine, "active_days"),
        active,
        description,
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
    let price_monitor = data
        .market_status
        .get("price_monitor")
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
                        MetricCard { label: "Quote Monitor", value: price_monitor_status_label(&price_monitor), tone: "" }
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
                        div { class: "event",
                            strong { "Quote Monitor" }
                            span { "{price_monitor_status_label(&price_monitor)}" }
                            span { class: "muted", "{price_monitor_detail(&price_monitor, &prefs)}" }
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
                    WatchlistCategory {
                        category: category.clone(),
                        prefs: prefs.clone(),
                        decision_stale_after_days: data.position_decision_stale_after_days,
                    }
                }
            }
        }
    }
}

#[component]
fn WatchlistCategory(
    category: JsonValue,
    prefs: LocalizationPrefs,
    decision_stale_after_days: i64,
) -> Element {
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
                                td {
                                    DecisionBadge {
                                        decision: row.get("decision").cloned().unwrap_or(JsonValue::Null),
                                        prefs: prefs.clone(),
                                        stale_after_days: decision_stale_after_days,
                                    }
                                }
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
    let ok_count = value_i64(&run, "success_count");
    let error_count = value_i64(&run, "error_count");
    let total_pages =
        ((data.markov_signal_total + data.markov_page_size - 1) / data.markov_page_size).max(1);
    let previous_page_href = format!("/?view=markov&markov_page={}", data.markov_page - 1);
    let next_page_href = format!("/?view=markov&markov_page={}", data.markov_page + 1);
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
                div { class: "section-title-row compact",
                    h3 { "Signals" }
                    span { class: "muted", "{data.markov_signal_total} total · page {data.markov_page} of {total_pages}" }
                }
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
                div { class: "button-row table-pagination",
                    if data.markov_page > 1 {
                        a { class: "small-button", href: "{previous_page_href}", "Previous" }
                    }
                    if data.markov_page < total_pages {
                        a { class: "small-button", href: "{next_page_href}", "Next" }
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
fn QuiverView(data: DashboardView, prefs: LocalizationPrefs) -> Element {
    let run = data.latest_quiver_run.clone();
    let config = run
        .get("config_json")
        .cloned()
        .unwrap_or_else(|| JsonValue::Null);
    let ok_count = value_i64(&run, "success_count");
    let error_count = value_i64(&run, "error_count");
    let total_pages =
        ((data.quiver_signal_total + data.quiver_page_size - 1) / data.quiver_page_size).max(1);
    let previous_page_href = format!("/?view=quiver&quiver_page={}", data.quiver_page - 1);
    let next_page_href = format!("/?view=quiver&quiver_page={}", data.quiver_page + 1);
    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "Quiver Signals" }
                    p { class: "muted", "Daily QuiverQuant alternative-data signals for US portfolio and watchlist assets. Signals are advisory and do not place orders." }
                }
                div { class: "pill-row right",
                    span { class: "pill", "Signals: {ok_count}" }
                    span { class: if error_count == 0 { "pill" } else { "pill bad" }, "Errors: {error_count}" }
                }
            }
            if run.is_null() {
                div { class: "event",
                    strong { "No Quiver run exists yet." }
                    span { class: "muted", "The scheduler will create the first run after the configured daily time once QUIVERQUANT_API_KEY is available." }
                }
            } else {
                div { class: "mini-grid",
                    MetricCard { label: "Run Date", value: text(&run, "run_date"), tone: "" }
                    MetricCard { label: "Status", value: text(&run, "status"), tone: "" }
                    MetricCard { label: "Assets", value: text(&run, "asset_count"), tone: "" }
                    MetricCard { label: "Succeeded", value: text(&run, "success_count"), tone: "good-text" }
                    MetricCard { label: "Failed", value: text(&run, "error_count"), tone: if value_f64(&run, "error_count") > 0.0 { "bad-text" } else { "" } }
                    MetricCard { label: "Lookback", value: format!("{}d", text(&config, "lookback_days")), tone: "" }
                }
                div { class: "event",
                    strong { "Configuration" }
                    span { "Sources congress_trading · US symbols only · daily time {text(&config, \"daily_time\")} · max symbols {text(&config, \"max_symbols\")}" }
                }
            }
            div { class: "table-wrap compact-table",
                div { class: "section-title-row compact",
                    h3 { "Signals" }
                    span { class: "muted", "{data.quiver_signal_total} total · page {data.quiver_page} of {total_pages}" }
                }
                table { class: "data-table",
                    thead {
                        tr {
                            th { "Symbol" }
                            th { "Ticker" }
                            th { "Signal" }
                            th { "Direction" }
                            th { "Confidence" }
                            th { "Events" }
                            th { "Purchases" }
                            th { "Sales" }
                            th { "Net Amount" }
                            th { "Latest" }
                            th { "Status" }
                        }
                    }
                    tbody {
                        for row in data.quiver_signals.iter() {
                            QuiverSignalRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
                div { class: "button-row table-pagination",
                    if data.quiver_page > 1 {
                        a { class: "small-button", href: "{previous_page_href}", "Previous" }
                    }
                    if data.quiver_page < total_pages {
                        a { class: "small-button", href: "{next_page_href}", "Next" }
                    }
                }
            }
        }
    }
}

#[component]
fn QuiverSignalRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let signal = value_f64(&row, "signal");
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
            td { "{fallback_text(&row, \"ticker\", \"n/a\")}" }
            td { class: tone, "{format_signed_pct(signal, &prefs)}" }
            td { "{fallback_text(&row, \"direction\", \"n/a\")}" }
            td { "{format_pct(value_f64(&row, \"confidence\"), &prefs)}" }
            td { "{text(&row, \"event_count\")}" }
            td { "{text(&row, \"congress_purchase_count\")}" }
            td { "{text(&row, \"congress_sale_count\")}" }
            td { class: if value_f64(&row, "net_congress_amount") >= 0.0 { "good-text" } else { "bad-text" }, "{format_money(value_f64(&row, \"net_congress_amount\"), \"USD\", &prefs)}" }
            td { "{fallback_text(&row, \"latest_event_date\", \"n/a\")}" }
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
    let pending_report = data
        .reports
        .iter()
        .find(|row| matches!(text(row, "status").as_str(), "xai_deferred" | "pending"))
        .cloned();
    let report_generation_pending = pending_report.is_some() || data.manual_report_in_flight;
    let pending_report_id = pending_report
        .as_ref()
        .and_then(|row| row.get("id").and_then(JsonValue::as_i64))
        .unwrap_or(0);
    // Baseline for the completion poll: the newest report at render time.
    // The poll only navigates when the latest report differs from this,
    // so a spawned manual run cannot cause a reload loop while it works.
    let baseline_report_id = data
        .reports
        .first()
        .and_then(|row| row.get("id").and_then(JsonValue::as_i64))
        .unwrap_or(0);
    let baseline_report_status = data
        .reports
        .first()
        .map(|row| text(row, "status"))
        .unwrap_or_default();
    let generate_label = if report_generation_pending {
        "Generating Report..."
    } else {
        "Generate Report"
    };
    let dry_run_label = if report_generation_pending {
        "Generating..."
    } else {
        "Dry Run Report"
    };
    let europe_pulse =
        decision_pulse_health_from_status(&data.decision_pulse_statuses, "europe_open_followup")
            .unwrap_or_else(|| {
                decision_pulse_health(
                    &data.reports,
                    "europe_open_followup:",
                    "Nordic/EU Open +1h15",
                )
            });
    let us_pulse =
        decision_pulse_health_from_status(&data.decision_pulse_statuses, "us_open_followup")
            .unwrap_or_else(|| {
                decision_pulse_health(&data.reports, "us_open_followup:", "US Open +1h15")
            });
    let manual_pulse = decision_pulse_health_from_status(&data.decision_pulse_statuses, "manual")
        .unwrap_or_else(|| decision_pulse_health(&data.reports, "manual:", "Manual / Dry Run"));
    let diagnostics = decision_report_diagnostics(&report);
    let quality = decision_report_quality(&report, &report_json, &diagnostics);
    let candidate_waterfall = report
        .get("candidate_scoring_waterfall")
        .cloned()
        .unwrap_or(JsonValue::Null);
    rsx! {
        section { class: "section stack loose",
            div { class: "section-title-row",
                div {
                    h2 { "Decision Report" }
                    p { class: "muted", "Latest xAI report plus deterministic strategy selection output. Select any recent report to inspect its outcome." }
                }
                div { class: "button-row",
                    form { method: "post", action: "/api/actions/decision-report-dry-run", "data-decision-report-form": "true",
                        button {
                            class: "button secondary",
                            r#type: "submit",
                            disabled: report_generation_pending,
                            "data-pending-label": "Generating Dry Run...",
                            "{dry_run_label}"
                        }
                    }
                    form { method: "post", action: "/api/actions/decision-report", "data-decision-report-form": "true",
                        button {
                            class: "button primary",
                            r#type: "submit",
                            disabled: report_generation_pending,
                            "data-pending-label": "Generating Report...",
                            "{generate_label}"
                        }
                    }
                }
            }
            if report_generation_pending {
                div {
                    class: "notice-banner",
                    "data-decision-report-pending": "true",
                    "data-baseline-report-id": "{baseline_report_id}",
                    "data-baseline-report-status": "{baseline_report_status}",
                    strong { "Decision report is running" }
                    if pending_report.is_some() {
                        span { "Report #{pending_report_id} is still pending. The page will refresh when it completes or fails." }
                    } else {
                        span { "A manual report is being generated in the background. The page will refresh when it completes or fails." }
                    }
                }
            }
            div { class: "mini-grid decision-summary-grid",
                DecisionPulseHealthCard { health: europe_pulse, prefs: prefs.clone() }
                DecisionPulseHealthCard { health: us_pulse, prefs: prefs.clone() }
                DecisionPulseHealthCard { health: manual_pulse, prefs: prefs.clone() }
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
                CandidateScoringWaterfallPanel { waterfall: candidate_waterfall, prefs: prefs.clone() }
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
                        DecisionReportQualityPanel { quality }
                        DecisionReportDiagnosticsPanel { diagnostics: diagnostics.clone() }
                        DecisionReportDebugPanel { debug: decision_report_debug_payload(&report, &report_json) }
                    }
                }
                div { class: "table-wrap",
                    p { class: "muted", "Recent rows are lightweight metadata. Select a report to load its trade counts and debug detail." }
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
fn CandidateScoringWaterfallPanel(waterfall: JsonValue, prefs: LocalizationPrefs) -> Element {
    let status = text(&waterfall, "status");
    let summary = waterfall.get("summary").cloned().unwrap_or(JsonValue::Null);
    let candidates = json_array(&waterfall, "candidates");
    let approved_count = value_i64(&summary, "approved_count");
    let skipped_count = value_i64(&summary, "skipped_count");
    let not_reached_count = value_i64(&summary, "not_reached_count");
    rsx! {
        div { class: "event candidate-scoring-panel",
            strong { "Candidate Scoring Waterfall" }
            p { class: "muted", "Stored deterministic manager-gate snapshot. It does not make a provider or Saxo call, and raw advisory rationale is excluded." }
            if status != "available" {
                span { class: "muted", "No Trading Manager run has processed this report yet." }
            } else {
                div { class: "quality-score-row",
                    span { class: "status good", "{approved_count} approved" }
                    span { class: "status warn", "{skipped_count} blocked" }
                    span { class: "status", "{not_reached_count} not reached" }
                }
                if candidates.is_empty() {
                    span { class: "muted", "The manager run contained no candidate orders." }
                } else {
                    div { class: "table-wrap candidate-scoring-table",
                        table {
                            thead { tr { th { "Candidate" } th { "Market / Risk" } th { "Technical" } th { "Markov" } th { "Hermes" } th { "Outcome" } th { "Gate" } } }
                            tbody {
                                for row in candidates.iter() {
                                    CandidateScoringWaterfallRow { row: row.clone(), prefs: prefs.clone() }
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
fn CandidateScoringWaterfallRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let symbol = fallback_text(&row, "symbol", "Legacy candidate");
    let action = text(&row, "action");
    let market = row.get("market").cloned().unwrap_or(JsonValue::Null);
    let technical = row.get("technical").cloned().unwrap_or(JsonValue::Null);
    let final_technical = row
        .get("final_technical")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let markov = row.get("markov").cloned().unwrap_or(JsonValue::Null);
    let hermes = row.get("hermes").cloned().unwrap_or(JsonValue::Null);
    let outcome = text(&row, "outcome");
    let outcome_class = match outcome.as_str() {
        "approved" => "status good",
        "skipped" => "status bad",
        _ => "status",
    };
    let market_label = if market
        .get("risk_excluded")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        "risk excluded".to_string()
    } else if market
        .get("quarantine_active")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        "instrument quarantined".to_string()
    } else if market
        .get("exchange_open")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        format!("{} open", fallback_text(&market, "exchange", "market"))
    } else {
        format!("{} closed", fallback_text(&market, "exchange", "market"))
    };
    let preflight_technical_label = candidate_technical_label(&technical);
    let final_technical_recorded = text(&final_technical, "status") == "ok";
    let technical_label = if final_technical_recorded {
        candidate_technical_label(&final_technical)
    } else {
        preflight_technical_label.clone()
    };
    let markov_label = if text(&markov, "status") == "unavailable" {
        "unavailable".to_string()
    } else {
        format!(
            "{} / {}{}",
            fallback_text(&markov, "direction", "n/a"),
            format_signed_pct(value_f64(&markov, "signed_signal"), &prefs),
            if markov
                .get("fresh")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
            {
                " fresh"
            } else {
                " stale"
            },
        )
    };
    let hermes_effect = fallback_text(&hermes, "effect", "not recorded").replace('_', " ");
    let gate_code = text(&row, "gate_code").replace('_', " ");
    let gate_detail = candidate_gate_detail(&row, &final_technical, final_technical_recorded);
    let requested_quantity = value_f64(&hermes, "requested_quantity");
    let resulting_quantity = value_f64(&hermes, "resulting_quantity");
    let hermes_label = if requested_quantity > 0.0 {
        format!(
            "{}: {} -> {}",
            hermes_effect,
            format_quantity(requested_quantity, &prefs),
            format_quantity(resulting_quantity, &prefs),
        )
    } else {
        hermes_effect
    };
    rsx! {
        tr {
            td { strong { "{symbol}" } span { class: "muted block", "{action}" } }
            td { "{market_label}" }
            td {
                "{technical_label}"
                if final_technical_recorded {
                    span { class: "muted block", "preflight: {preflight_technical_label}" }
                } else {
                    span { class: "muted block", "preflight only" }
                }
            }
            td { "{markov_label}" }
            td { "{hermes_label}" }
            td { span { class: outcome_class, "{outcome}" } }
            td {
                "{gate_code}"
                if !gate_detail.is_empty() {
                    span { class: "muted block", "{gate_detail}" }
                }
            }
        }
    }
}

fn candidate_technical_label(technical: &JsonValue) -> String {
    if text(technical, "status") != "ok" {
        return "unavailable".to_string();
    }
    let confluences = value_i64(technical, "confluence_count");
    let minimum = value_i64(technical, "min_confluences");
    if minimum <= 0 {
        return format!(
            "{} / {}",
            fallback_text(technical, "sentiment", "n/a"),
            fallback_text(technical, "trend_bias", "n/a"),
        );
    }
    format!(
        "{} / {} / {}/{}",
        fallback_text(technical, "sentiment", "n/a"),
        fallback_text(technical, "trend_bias", "n/a"),
        confluences,
        minimum,
    )
}

fn candidate_gate_detail(
    row: &JsonValue,
    final_technical: &JsonValue,
    final_technical_recorded: bool,
) -> String {
    if text(row, "gate_code") != "technical" {
        return String::new();
    }
    if !final_technical_recorded {
        return "Final technical snapshot was not recorded for this legacy run.".to_string();
    }
    let sentiment = fallback_text(final_technical, "sentiment", "HOLD");
    let trend = fallback_text(final_technical, "trend_bias", "neutral");
    match text(row, "action").as_str() {
        "BUY" => format!(
            "Final {sentiment}/{trend}; BUY needs BUY or OVERWEIGHT, bullish trend, and enough confluences."
        ),
        "SELL" => format!(
            "Final {sentiment}/{trend}; SELL needs SELL or UNDERWEIGHT, or a bearish trend."
        ),
        _ => "Final technical result did not satisfy the action gate.".to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DecisionPulseHealth {
    label: String,
    latest_status: String,
    latest_created_at: String,
    latest_id: i64,
    latest_tone: &'static str,
    last_success_at: String,
    last_success_id: i64,
    last_failure_at: String,
    last_failure_id: i64,
    last_failure_status: String,
    attempts_7d: i64,
}

#[derive(Clone, Debug, PartialEq)]
struct DecisionReportDiagnostics {
    provider: String,
    model: String,
    response_format: String,
    strict_schema: String,
    root_object: String,
    capital_plan_object: String,
    schema_status: String,
    schema_tone: &'static str,
    request_bytes: usize,
    response_id: String,
    response_present: String,
    error_category: String,
    error_excerpt: String,
}

#[derive(Clone, Debug, PartialEq)]
struct DecisionReportQuality {
    score: i64,
    tone: &'static str,
    status_label: String,
    warning_count: usize,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct DecisionReportDebugPayload {
    prompt: String,
    request: String,
    response: String,
    normalized: String,
}

fn decision_pulse_health(
    reports: &[JsonValue],
    pulse_key_prefix: &str,
    label: &str,
) -> DecisionPulseHealth {
    let latest = reports.iter().find(|row| {
        text(row, "analysis_pulse_key")
            .to_lowercase()
            .starts_with(pulse_key_prefix)
    });
    let last_success = reports.iter().find(|row| {
        text(row, "analysis_pulse_key")
            .to_lowercase()
            .starts_with(pulse_key_prefix)
            && matches!(text(row, "status").as_str(), "completed" | "xai_fallback")
    });
    let latest_status = latest
        .map(|row| fallback_text(row, "status", "missing"))
        .unwrap_or_else(|| "missing".to_string());

    DecisionPulseHealth {
        label: label.to_string(),
        latest_tone: decision_status_text_tone(&latest_status),
        latest_status,
        latest_created_at: latest
            .map(|row| text(row, "created_at"))
            .unwrap_or_default(),
        latest_id: latest
            .and_then(|row| row.get("id").and_then(JsonValue::as_i64))
            .unwrap_or(0),
        last_success_at: last_success
            .map(|row| text(row, "created_at"))
            .unwrap_or_default(),
        last_success_id: last_success
            .and_then(|row| row.get("id").and_then(JsonValue::as_i64))
            .unwrap_or(0),
        last_failure_at: String::new(),
        last_failure_id: 0,
        last_failure_status: String::new(),
        attempts_7d: reports
            .iter()
            .filter(|row| {
                text(row, "analysis_pulse_key")
                    .to_lowercase()
                    .starts_with(pulse_key_prefix)
            })
            .count() as i64,
    }
}

fn decision_pulse_health_from_status(
    statuses: &[JsonValue],
    key: &str,
) -> Option<DecisionPulseHealth> {
    let row = statuses.iter().find(|row| text(row, "key") == key)?;
    let latest = row.get("latest").unwrap_or(&JsonValue::Null);
    let last_success = row.get("last_success").unwrap_or(&JsonValue::Null);
    let last_failure = row.get("last_failure").unwrap_or(&JsonValue::Null);
    let latest_status = if latest.is_null() {
        "missing".to_string()
    } else {
        fallback_text(latest, "status", "missing")
    };
    Some(DecisionPulseHealth {
        label: fallback_text(row, "label", key),
        latest_tone: decision_status_text_tone(&latest_status),
        latest_status,
        latest_created_at: text(latest, "created_at"),
        latest_id: latest.get("id").and_then(JsonValue::as_i64).unwrap_or(0),
        last_success_at: text(last_success, "created_at"),
        last_success_id: last_success
            .get("id")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        last_failure_at: text(last_failure, "created_at"),
        last_failure_id: last_failure
            .get("id")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        last_failure_status: text(last_failure, "status"),
        attempts_7d: value_i64(row, "attempts_7d"),
    })
}

fn decision_status_text_tone(status: &str) -> &'static str {
    match status {
        "completed" | "xai_fallback" => "good-text",
        "xai_error" | "error" | "failed" | "missing" => "bad-text",
        "pending" | "xai_deferred" => "",
        _ => "",
    }
}

fn decision_report_diagnostics(report: &JsonValue) -> DecisionReportDiagnostics {
    let request = report.get("request_json").unwrap_or(&JsonValue::Null);
    let response = report.get("response_json").unwrap_or(&JsonValue::Null);
    let response_format = request.get("response_format").unwrap_or(&JsonValue::Null);
    let schema = response_format
        .get("json_schema")
        .and_then(|json_schema| json_schema.get("schema"))
        .unwrap_or(&JsonValue::Null);
    let root_object = additional_properties_label(schema);
    let capital_plan_object = additional_properties_label(
        schema
            .get("properties")
            .and_then(|properties| properties.get("capital_plan"))
            .unwrap_or(&JsonValue::Null),
    );
    let strict_schema = response_format
        .get("json_schema")
        .and_then(|json_schema| json_schema.get("strict"))
        .and_then(JsonValue::as_bool)
        .map(|flag| if flag { "true" } else { "false" })
        .unwrap_or("n/a")
        .to_string();
    let schema_ok =
        root_object == "strict" && capital_plan_object == "strict" && strict_schema == "true";
    let error_text = text(report, "error_text");

    DecisionReportDiagnostics {
        provider: decision_report_provider_label(request),
        model: fallback_text(report, "model", &text(request, "model")),
        response_format: fallback_text(response_format, "type", "n/a"),
        strict_schema,
        root_object,
        capital_plan_object,
        schema_status: if schema_ok {
            "strict".to_string()
        } else {
            "needs review".to_string()
        },
        schema_tone: if schema_ok { "good-text" } else { "bad-text" },
        request_bytes: serde_json::to_string(request)
            .map(|rendered| rendered.len())
            .unwrap_or(0),
        response_id: fallback_text(report, "response_id", &text(response, "id")),
        response_present: if response.is_null() {
            "no".to_string()
        } else {
            "yes".to_string()
        },
        error_category: decision_error_category(&error_text).to_string(),
        error_excerpt: if error_text.is_empty() {
            "No error recorded.".to_string()
        } else {
            truncate_chars(&error_text, 420)
        },
    }
}

fn decision_report_provider_label(request: &JsonValue) -> String {
    match text(
        request.get("response_format").unwrap_or(&JsonValue::Null),
        "type",
    )
    .as_str()
    {
        "json_schema" => "openrouter/json_schema".to_string(),
        "json_object" => "json_object".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "n/a".to_string(),
    }
}

fn additional_properties_label(schema: &JsonValue) -> String {
    match schema
        .get("additionalProperties")
        .and_then(JsonValue::as_bool)
    {
        Some(false) => "strict".to_string(),
        Some(true) => "open".to_string(),
        None => "missing".to_string(),
    }
}

fn decision_error_category(error_text: &str) -> &'static str {
    let lower = error_text.to_lowercase();
    if lower.is_empty() {
        "none"
    } else if lower.contains("invalid_json_schema") || lower.contains("invalid schema") {
        "schema"
    } else if lower.contains("credit") || lower.contains("spending limit") {
        "provider credits"
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "timeout"
    } else if lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("401")
        || lower.contains("403")
    {
        "auth"
    } else if lower.contains("parse") || lower.contains("normalized into strict json") {
        "parse"
    } else if lower.contains("http 400") || lower.contains("provider returned error") {
        "provider"
    } else {
        "unknown"
    }
}

fn decision_report_quality(
    report: &JsonValue,
    report_json: &JsonValue,
    diagnostics: &DecisionReportDiagnostics,
) -> DecisionReportQuality {
    let mut score = 0_i64;
    let mut warnings = Vec::new();
    let status = text(report, "status");
    if status == "completed" {
        score += 20;
    } else {
        warnings.push(format!("Report status is {status}; expected completed."));
    }

    if diagnostics.schema_status == "strict" {
        score += 20;
    } else {
        warnings.push("Provider response schema is not fully strict.".to_string());
    }

    if report_json.is_object() {
        score += 10;
    } else {
        warnings.push("Normalized report payload is missing or not an object.".to_string());
    }

    let required_sections = [
        "market_view",
        "capital_plan",
        "selected_assets",
        "symbol_sentiment",
        "suggested_trades",
    ];
    let mut section_score = 0_i64;
    for section in required_sections {
        if report_json.get(section).is_some() {
            section_score += 6;
        } else {
            warnings.push(format!("Missing normalized section: {section}."));
        }
    }
    score += section_score.min(30);

    let trades = json_array(report_json, "suggested_trades");
    let invalid_trade_count = trades
        .iter()
        .filter(|trade| !decision_trade_shape_ok(trade))
        .count();
    if invalid_trade_count == 0 {
        score += 10;
    } else {
        warnings.push(format!(
            "{invalid_trade_count} suggested trade(s) have incomplete order shape."
        ));
    }

    let scope = report_json
        .get("market_scope_enforcement")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let scope_status = text(&scope, "status");
    if scope_status.is_empty() {
        warnings.push("Market-scope enforcement metadata is missing.".to_string());
    } else {
        score += 10;
        let filtered = json_array(&scope, "filtered_out_symbols");
        if !filtered.is_empty() {
            warnings.push(format!(
                "Market-scope enforcement filtered {} symbol(s) out of the model response.",
                filtered.len()
            ));
        }
    }

    let score = score.clamp(0, 100);
    let tone = if score >= 80 {
        "good-text"
    } else if score >= 60 {
        "warn-text"
    } else {
        "bad-text"
    };
    let status_label = if warnings.is_empty() {
        "ready".to_string()
    } else if score >= 80 {
        "ready with notes".to_string()
    } else if score >= 60 {
        "review".to_string()
    } else {
        "poor".to_string()
    };
    DecisionReportQuality {
        score,
        tone,
        status_label,
        warning_count: warnings.len(),
        warnings,
    }
}

fn decision_trade_shape_ok(trade: &JsonValue) -> bool {
    let order_type = text(trade, "order_type");
    let action = text(trade, "action");
    let has_basic_shape = !text(trade, "symbol").is_empty()
        && matches!(action.as_str(), "BUY" | "SELL")
        && value_f64(trade, "quantity") > 0.0
        && matches!(order_type.as_str(), "Market" | "Limit")
        && value_f64(trade, "estimated_value_dkk") > 0.0
        && !text(trade, "strategy_key").is_empty();
    if !has_basic_shape {
        return false;
    }
    order_type != "Limit"
        || trade
            .get("limit_price_local")
            .and_then(JsonValue::as_f64)
            .unwrap_or(0.0)
            > 0.0
}

#[component]
fn DecisionReportQualityPanel(quality: DecisionReportQuality) -> Element {
    rsx! {
        div { class: "event report-quality-panel",
            strong { "Report Quality" }
            div { class: "quality-score-row",
                span { class: "quality-score {quality.tone}", "{quality.score}/100" }
                span { class: "status", "{quality.status_label}" }
                span { class: "muted", "{quality.warning_count} warning(s)" }
            }
            if quality.warnings.is_empty() {
                p { class: "muted", "No quality warnings for the selected report." }
            } else {
                ul { class: "quality-warning-list",
                    for warning in quality.warnings.iter() {
                        li { "{warning}" }
                    }
                }
            }
        }
    }
}

#[component]
fn DecisionReportDiagnosticsPanel(diagnostics: DecisionReportDiagnostics) -> Element {
    let request_kb = format!("{:.1} KB", diagnostics.request_bytes as f64 / 1024.0);
    let schema_class = format!("diagnostic-value {}", diagnostics.schema_tone);
    rsx! {
        div { class: "event report-diagnostics-panel",
            strong { "Provider Diagnostics" }
            div { class: "diagnostic-grid",
                div { span { class: "label", "Provider" } span { class: "diagnostic-value", "{diagnostics.provider}" } }
                div { span { class: "label", "Model" } span { class: "diagnostic-value", "{diagnostics.model}" } }
                div { span { class: "label", "Format" } span { class: "diagnostic-value", "{diagnostics.response_format}" } }
                div { span { class: "label", "Strict" } span { class: "diagnostic-value", "{diagnostics.strict_schema}" } }
                div { span { class: "label", "Root Object" } span { class: "diagnostic-value", "{diagnostics.root_object}" } }
                div { span { class: "label", "Capital Plan" } span { class: "diagnostic-value", "{diagnostics.capital_plan_object}" } }
                div { span { class: "label", "Schema" } span { class: schema_class, "{diagnostics.schema_status}" } }
                div { span { class: "label", "Request Size" } span { class: "diagnostic-value", "{request_kb}" } }
                div { span { class: "label", "Response" } span { class: "diagnostic-value", "{diagnostics.response_present}" } }
                div { span { class: "label", "Response ID" } span { class: "diagnostic-value", "{diagnostics.response_id}" } }
                div { span { class: "label", "Error Type" } span { class: "diagnostic-value", "{diagnostics.error_category}" } }
            }
            if diagnostics.error_category != "none" {
                div { class: "muted diagnostic-error", "{diagnostics.error_excerpt}" }
            }
        }
    }
}

#[component]
fn DecisionReportDebugPanel(debug: DecisionReportDebugPayload) -> Element {
    rsx! {
        div { class: "event report-debug-panel",
            strong { "Sanitized Debug Payloads" }
            p { class: "muted", "Expandable prompt, request, provider response, and normalized report payloads. Secret-like fields are redacted before rendering." }
            DebugPayloadDetails { label: "Prompt", body: debug.prompt }
            DebugPayloadDetails { label: "Request", body: debug.request }
            DebugPayloadDetails { label: "Provider Response", body: debug.response }
            DebugPayloadDetails { label: "Normalized Report", body: debug.normalized }
        }
    }
}

#[component]
fn DebugPayloadDetails(label: &'static str, body: String) -> Element {
    rsx! {
        details { class: "debug-payload-details",
            summary { "{label}" }
            pre { "{body}" }
        }
    }
}

fn decision_report_debug_payload(
    report: &JsonValue,
    normalized_report: &JsonValue,
) -> DecisionReportDebugPayload {
    DecisionReportDebugPayload {
        prompt: compact_debug_text(&text(report, "prompt_text"), 4_000),
        request: compact_json_redacted(report.get("request_json"), 4_000),
        response: compact_json_redacted(report.get("response_json"), 4_000),
        normalized: compact_json_redacted(Some(normalized_report), 4_000),
    }
}

#[component]
fn DecisionPulseHealthCard(health: DecisionPulseHealth, prefs: LocalizationPrefs) -> Element {
    let latest = if health.latest_id > 0 {
        format!(
            "#{} · {}",
            health.latest_id,
            format_timestamp(&health.latest_created_at, &prefs)
        )
    } else {
        "No report yet".to_string()
    };
    let last_success = if health.last_success_id > 0 {
        format!(
            "Last OK #{} · {}",
            health.last_success_id,
            format_timestamp(&health.last_success_at, &prefs)
        )
    } else {
        "No successful report in recent history".to_string()
    };
    let last_failure = if health.last_failure_id > 0 {
        format!(
            "Last failure #{} · {} · {}",
            health.last_failure_id,
            format_timestamp(&health.last_failure_at, &prefs),
            health.last_failure_status
        )
    } else {
        "No failure recorded".to_string()
    };
    rsx! {
        div { class: "card",
            div { class: "label", "{health.label}" }
            div { class: "value {health.latest_tone}", "{health.latest_status}" }
            div { class: "muted summary-subtitle", "{latest}" }
            div { class: "muted summary-subtitle", "{last_success}" }
            div { class: "muted summary-subtitle", "{last_failure}" }
            div { class: "muted summary-subtitle", "{health.attempts_7d} attempts / 7d" }
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
    let selected = if report_json.is_null() {
        "-".to_string()
    } else {
        json_array(&report_json, "selected_assets")
            .len()
            .max(json_array(&report_json, "candidate_assets").len())
            .to_string()
    };
    let trades = if report_json.is_null() {
        "-".to_string()
    } else {
        json_array(&report_json, "suggested_trades")
            .len()
            .to_string()
    };
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
    let advised_reports = data
        .hermes_decision_advice_audit
        .iter()
        .filter(|row| !text(row, "advice_id").is_empty())
        .count();
    let changed_reports = data
        .hermes_decision_advice_audit
        .iter()
        .filter(|row| hermes_advice_impact(row).0 != "no-op")
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
                    span { class: "pill", "Advised reports: {advised_reports}" }
                    span { class: "pill", "Changed: {changed_reports}" }
                    span { class: "pill", "Counterfactuals: {data.hermes_counterfactuals.len()}" }
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
            div { class: "table-wrap",
                h3 { "Decision Advice Audit" }
                table {
                    thead {
                        tr {
                            th { "Report" }
                            th { "Pulse" }
                            th { "Advice" }
                            th { "Self-check" }
                            th { "Recommendation" }
                            th { "Orders" }
                            th { "Impact" }
                            th { "Manager" }
                            th { "Summary" }
                        }
                    }
                    tbody {
                        for row in data.hermes_decision_advice_audit.iter() {
                            HermesAdviceAuditRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
            }
            div { class: "table-wrap",
                h3 { "Counterfactual Tracking" }
                p { class: "muted", "Quote-to-quote shadow outcomes for trade quantity Hermes blocked or reduced. They are observational estimates only and exclude fees, FX, slippage, and broker execution." }
                table {
                    thead {
                        tr {
                            th { "Report" }
                            th { "Symbol" }
                            th { "Source" }
                            th { "Shadow Qty" }
                            th { "Reference" }
                            th { "Latest" }
                            th { "Estimated Return" }
                            th { "Estimated P/L" }
                            th { "Status" }
                        }
                    }
                    tbody {
                        for row in data.hermes_counterfactuals.iter() {
                            HermesCounterfactualRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
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
                            th { "Age" }
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
    let total_pages = ((data.execution_order_total + data.execution_page_size - 1)
        / data.execution_page_size)
        .max(1);
    let previous_page_href = format!(
        "/?view=execution&execution_page={}",
        data.execution_page - 1
    );
    let next_page_href = format!(
        "/?view=execution&execution_page={}",
        data.execution_page + 1
    );
    let scheduler_total_pages = ((data.scheduler_cycle_total + data.scheduler_page_size - 1)
        / data.scheduler_page_size)
        .max(1);
    let previous_scheduler_page_href = format!(
        "/?view=execution&scheduler_page={}",
        data.scheduler_page - 1
    );
    let next_scheduler_page_href = format!(
        "/?view=execution&scheduler_page={}",
        data.scheduler_page + 1
    );
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
                div { class: "section-title-row compact",
                    h3 { "Execution Orders" }
                    span { class: "muted", "{data.execution_order_total} total · page {data.execution_page} of {total_pages}" }
                }
                table {
                    thead { tr { th { "ID" } th { "Created" } th { "Symbol" } th { "Action" } th { "Strategy" } th { "Role" } th { "Order Type" } th { "Status" } th { "Qty" } th { "Price" } th { "Limit" } th { "Stop" } th { "Expiry" } th { "Attribution" } th { "Error" } } }
                    tbody {
                        for row in data.orders.iter() {
                            ExecutionOrderRow { row: row.clone(), prefs: prefs.clone() }
                        }
                    }
                }
                div { class: "button-row table-pagination",
                    if data.execution_page > 1 {
                        a { class: "small-button", href: "{previous_page_href}", "Previous" }
                    }
                    if data.execution_page < total_pages {
                        a { class: "small-button", href: "{next_page_href}", "Next" }
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
                                ExecutionEventRow { row: row.clone(), prefs: prefs.clone() }
                            }
                        }
                    }
                }
                div { class: "table-wrap",
                    div { class: "section-title-row compact",
                        h3 { "Scheduler Cycles" }
                        span { class: "muted", "{data.scheduler_cycle_total} total · page {data.scheduler_page} of {scheduler_total_pages}" }
                    }
                    table {
                        thead { tr { th { "Started" } th { "Runtime" } th { "Status" } th { "Decision" } th { "Queue" } th { "Alerts" } th { "Ops Alerts" } } }
                        tbody {
                            for row in data.scheduler_cycles.iter() {
                                tr {
                                    td { "{format_timestamp(&text(row, \"started_at\"), &prefs)}" }
                                    td { "{scheduler_cycle_duration(row)}" }
                                    td { "{text(row, \"status\")}" }
                                    td { "{bool_label(row, \"generated_decision\")}" }
                                    td { "{text(row, \"queue_status\")}" }
                                    td { "{text_or(row, \"notifications_status\", \"n/a\")}" }
                                    td { "{scheduler_cycle_json_status(row, \"operational_notifications\")}" }
                                }
                            }
                        }
                    }
                    div { class: "button-row table-pagination",
                        if data.scheduler_page > 1 {
                            a { class: "small-button", href: "{previous_scheduler_page_href}", "Previous" }
                        }
                        if data.scheduler_page < scheduler_total_pages {
                            a { class: "small-button", href: "{next_scheduler_page_href}", "Next" }
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
fn PositionRow(
    row: JsonValue,
    prefs: LocalizationPrefs,
    decision_stale_after_days: i64,
) -> Element {
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
            td {
                DecisionBadge {
                    decision: row.get("decision").cloned().unwrap_or(JsonValue::Null),
                    prefs: prefs.clone(),
                    stale_after_days: decision_stale_after_days,
                }
            }
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
fn DecisionBadge(decision: JsonValue, prefs: LocalizationPrefs, stale_after_days: i64) -> Element {
    if decision.is_null() {
        return rsx! { span { class: "muted", "n/a" } };
    }
    let sentiment = text(&decision, "sentiment").to_uppercase();
    let action = text(&decision, "action");
    let created_at = text(&decision, "created_at");
    let decision_time = format_timestamp(&created_at, &prefs);
    let (age, stale) = position_decision_age_status(&created_at, Utc::now(), stale_after_days);
    let rationale = text(&decision, "target_rationale");
    let fallback_rationale = text(&decision, "rationale");
    let rationale_tooltip = if rationale.is_empty() {
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
    let chip_label = if stale {
        format!("Stale · {sentiment_label}")
    } else {
        sentiment_label
    };
    let chip_tone = if stale { "decision-chip stale" } else { tone };
    let age_label = if created_at.is_empty() {
        "timestamp unavailable".to_string()
    } else {
        format!("{age} old")
    };
    let tooltip = if stale {
        let stale_note = if decision_time.is_empty() {
            "This decision has no usable timestamp and is not treated as current advice."
                .to_string()
        } else {
            format!("This decision is stale; it was created {decision_time}.")
        };
        if rationale_tooltip.is_empty() {
            stale_note
        } else {
            format!("{stale_note} {rationale_tooltip}")
        }
    } else {
        rationale_tooltip
    };
    rsx! {
        span { class: "decision-cell", title: "{tooltip}",
            span { class: "decision-topline",
                span { class: chip_tone, "{chip_label}" }
                if !action.is_empty() {
                    span { class: "decision-action", "{action}" }
                }
            }
            span { class: if stale { "decision-age stale" } else { "decision-age" }, "{age_label}" }
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
    let currency = execution_order_price_currency(&row);
    let limit = format_local_money(value_f64(&row, "limit_price_local"), &currency, &prefs);
    let expiry = execution_order_lifecycle_label(&row, &prefs);
    let lifecycle_detail = execution_order_lifecycle_detail(&row, &prefs);
    rsx! {
        tr {
            td { "{id}" }
            td { "{created_at}" }
            td { "{symbol}" }
            td { "{action}" }
            td { span { class: "status", "{status}" } }
            td { "{quantity}" }
            td { "{limit}" }
            td { class: "muted", title: "{lifecycle_detail}", "{expiry}" }
        }
    }
}

#[component]
fn ExecutionOrderRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let status = text(&row, "status");
    let detail = execution_status_detail(&row);
    let reason = execution_status_reason(&row);
    let tooltip = execution_status_tooltip(&row, &reason, &detail);
    let reason_class = execution_reason_class(&reason);
    let status_class = execution_status_class(&status);
    let detail_preview = if detail.is_empty() {
        String::new()
    } else {
        truncate_chars(&detail, 120)
    };
    let detail_block = execution_detail_block(&row, &detail);
    let (attribution_label, attribution_tone) = execution_attribution_label(&row);
    let attribution_detail = execution_attribution_detail(&row, &prefs);
    let expiry = execution_order_lifecycle_label(&row, &prefs);
    let lifecycle_detail = execution_order_lifecycle_detail(&row, &prefs);
    rsx! {
        tr {
            td { "{text(&row, \"id\")}" }
            td { "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            td { "{text(&row, \"symbol\")}" }
            td { "{text(&row, \"action\")}" }
            td { "{fallback_text(&row, \"strategy_type\", \"manual\")}" }
            td { "{fallback_text(&row, \"strategy_role\", \"primary\")}" }
            td { "{fallback_text(&row, \"order_type\", \"Market\")}" }
            td { class: "execution-status-cell",
                span { class: "{status_class}", title: "{tooltip}", "{status}" }
                if !reason.is_empty() {
                    span { class: "{reason_class}", title: "{tooltip}", "{reason}" }
                }
            }
            td { "{format_quantity(value_f64(&row, \"quantity\"), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"price_local\"), &execution_order_price_currency(&row), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"limit_price_local\"), &execution_order_price_currency(&row), &prefs)}" }
            td { "{format_local_money(value_f64(&row, \"stop_price_local\"), &execution_order_price_currency(&row), &prefs)}" }
            td { class: "muted", title: "{lifecycle_detail}", "{expiry}" }
            td { class: "muted attribution-cell",
                details { class: "error-details attribution-details",
                    summary { span { class: "{attribution_tone}", "{attribution_label}" } }
                    pre { "{attribution_detail}" }
                }
            }
            td { class: "muted error-cell",
                if !detail.is_empty() {
                    details { class: "error-details", title: "{tooltip}",
                        summary { "{detail_preview}" }
                        pre { "{detail_block}" }
                    }
                } else if !reason.is_empty() {
                    span { class: "{reason_class}", title: "{tooltip}", "{reason}" }
                }
            }
        }
    }
}

fn execution_attribution_label(row: &JsonValue) -> (String, &'static str) {
    let null = JsonValue::Null;
    let attribution = row.get("attribution").unwrap_or(&null);
    match text(attribution, "delta").as_str() {
        "allowed_executed" => ("Hermes allow".to_string(), "good-text"),
        "allowed_queued" => ("Hermes allow".to_string(), ""),
        "reduced_or_capped" => ("Hermes reduce".to_string(), "warn-text"),
        "manager_overrode_review" => ("Review overrode".to_string(), "bad-text"),
        "manager_only" => ("Manager only".to_string(), ""),
        "no_advice" => ("No advice".to_string(), "warn-text"),
        value if value.ends_with("_skipped") => ("Skipped".to_string(), "warn-text"),
        value if !value.is_empty() => (value.replace('_', " "), ""),
        _ => ("No attribution".to_string(), "warn-text"),
    }
}

fn execution_attribution_detail(row: &JsonValue, prefs: &LocalizationPrefs) -> String {
    let null = JsonValue::Null;
    let attribution = row.get("attribution").unwrap_or(&null);
    let report = attribution.get("report").unwrap_or(&null);
    let manager = attribution.get("trading_manager").unwrap_or(&null);
    let manager_decision = manager.get("decision").unwrap_or(&null);
    let hermes = attribution.get("hermes").unwrap_or(&null);
    let hermes_order = hermes.get("order_advice").unwrap_or(&null);
    let technical = attribution.get("technical").unwrap_or(&null);
    let markov = attribution.get("markov").unwrap_or(&null);

    let mut lines = Vec::new();
    lines.push(format!(
        "Delta: {}",
        fallback_text(attribution, "delta", "n/a")
    ));
    lines.push(format!(
        "Report: #{} · {} · {} · {}",
        text_or(report, "id", "n/a"),
        fallback_text(report, "pulse_label", "n/a"),
        fallback_text(report, "model", "n/a"),
        format_timestamp(&text(report, "created_at"), prefs)
    ));
    lines.push(format!(
        "Trading Manager: run #{} · status {} · {} · decision {}",
        text_or(manager, "run_id", "n/a"),
        fallback_text(manager, "status", "n/a"),
        fallback_text(manager, "manager_key", "n/a"),
        fallback_text(manager_decision, "manager_decision", "n/a")
    ));
    let manager_reason = [
        text(manager_decision, "approval_reason"),
        text(manager_decision, "skip_reason"),
        text(manager_decision, "reason"),
        text(manager_decision, "technical_gate_reason"),
    ]
    .into_iter()
    .find(|value| !value.trim().is_empty())
    .unwrap_or_default();
    if !manager_reason.is_empty() {
        lines.push(format!("Manager reason: {manager_reason}"));
    }
    lines.push(format!(
        "Hermes: advice #{} · status {} · recommendation {} · action {}",
        text_or(hermes, "advice_id", "n/a"),
        fallback_text(hermes, "status", "n/a"),
        fallback_text(hermes, "recommendation", "n/a"),
        fallback_text(hermes_order, "action", "n/a")
    ));
    let hermes_reason = [
        text(hermes_order, "reason"),
        text(hermes_order, "rationale"),
        text(hermes, "summary"),
    ]
    .into_iter()
    .find(|value| !value.trim().is_empty())
    .unwrap_or_default();
    if !hermes_reason.is_empty() {
        lines.push(format!(
            "Hermes reason: {}",
            truncate_chars(&hermes_reason, 420)
        ));
    }
    if !technical.is_null() {
        lines.push(format!(
            "Daily indicators: {} · {} · trend {} · confluence {}/{} · RR {}",
            fallback_text(technical, "run_date", "n/a"),
            fallback_text(technical, "sentiment", "n/a"),
            fallback_text(technical, "trend_bias", "n/a"),
            text_or(technical, "confluence_count", "0"),
            text_or(technical, "min_confluences", "0"),
            crate::localization::format_number(value_f64(technical, "reward_risk"), 2, prefs)
        ));
        let error = text(technical, "error_text");
        if !error.is_empty() {
            lines.push(format!(
                "Daily indicator error: {}",
                truncate_chars(&error, 260)
            ));
        }
    }
    if !markov.is_null() {
        lines.push(format!(
            "Markov: {} · state {} · direction {} · signal {} · bull {} · bear {}",
            fallback_text(markov, "run_date", "n/a"),
            fallback_text(markov, "current_state", "n/a"),
            fallback_text(markov, "direction", "n/a"),
            format_signed_pct(value_f64(markov, "signed_signal"), prefs),
            format_pct(value_f64(markov, "bull_prob"), prefs),
            format_pct(value_f64(markov, "bear_prob"), prefs)
        ));
        let error = text(markov, "error_text");
        if !error.is_empty() {
            lines.push(format!("Markov error: {}", truncate_chars(&error, 260)));
        }
    }
    lines.join("\n")
}

#[component]
fn ExecutionEventRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let status = fallback_text(&row, "broker_status", &text(&row, "status"));
    let detail = execution_event_detail(&row);
    let reason = execution_event_reason(&row);
    let status_class = execution_status_class(&status);
    let reason_class = execution_reason_class(&reason);
    let tooltip = execution_event_tooltip(&row, &status, &reason, &detail);
    rsx! {
        tr {
            td { "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            td { "{text(&row, \"execution_order_id\")}" }
            td { "{text(&row, \"event_type\")}" }
            td { class: "execution-status-cell",
                span { class: "{status_class}", title: "{tooltip}", "{status}" }
                if !reason.is_empty() {
                    span { class: "{reason_class}", title: "{tooltip}", "{reason}" }
                }
            }
            td { class: "muted error-cell",
                if !detail.is_empty() {
                    span { class: "event-message", title: "{tooltip}", "{truncate_chars(&detail, 180)}" }
                } else {
                    span { class: "muted", "n/a" }
                }
            }
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
    let (age, age_class) = hermes_experiment_age_status(
        &status,
        &text(&row, "created_at"),
        Utc::now(),
        HERMES_EXPERIMENT_REVIEW_STALE_DAYS,
    );
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
            td { span { class: "{age_class}", "{age}" } }
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
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|v| v as f64))
                .or_else(|| value.as_str()?.parse().ok())
        })
        .unwrap_or(0.0)
}

fn value_i64(value: &JsonValue, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|v| v as i64))
                .or_else(|| value.as_str()?.parse().ok())
        })
        .unwrap_or(0)
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

const HERMES_EXPERIMENT_REVIEW_STALE_DAYS: i64 = 14;

fn position_decision_age_status(
    created_at: &str,
    now: DateTime<Utc>,
    stale_after_days: i64,
) -> (String, bool) {
    let Some(created_at) = parse_utc_timestamp(created_at) else {
        return ("undated".to_string(), true);
    };
    let age_days = (now - created_at).num_days().max(0);
    let label = if age_days == 0 {
        "<1d".to_string()
    } else {
        format!("{age_days}d")
    };
    (label, age_days >= stale_after_days.max(1))
}

fn hermes_experiment_age_status(
    status: &str,
    created_at: &str,
    now: DateTime<Utc>,
    stale_days: i64,
) -> (String, &'static str) {
    let Some(created_at) = parse_utc_timestamp(created_at) else {
        return ("n/a".to_string(), "muted");
    };
    let age_days = (now - created_at).num_days().max(0);
    let label = if age_days == 0 {
        "<1d".to_string()
    } else {
        format!("{age_days}d")
    };
    if status == "pending_review" && age_days >= stale_days.max(1) {
        (label, "warn-text")
    } else {
        (label, "muted")
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

#[component]
fn HermesAdviceAuditRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let advice_status = hermes_advice_status_label(&row);
    let (impact, impact_tone) = hermes_advice_impact(&row);
    let (order_counts, order_detail) = hermes_advice_order_counts(&row);
    let summary = fallback_text(&row, "advice_summary", "No advice summary recorded.");
    let pulse = fallback_text(&row, "analysis_pulse_label", "n/a");
    let recommendation = fallback_text(&row, "advice_recommendation", "n/a");
    let manager_status = fallback_text(&row, "manager_status", "not run");
    let (self_check_label, self_check_tone, self_check_detail) =
        hermes_context_self_check_label(&row);
    rsx! {
        tr {
            td {
                span { "#{text(&row, \"report_id\")}" }
                small { class: "muted block", "{format_timestamp(&text(&row, \"report_created_at\"), &prefs)}" }
            }
            td { "{pulse}" }
            td {
                span { class: "status {hermes_advice_status_tone(&advice_status)}", title: "{hermes_advice_detail(&row)}", "{advice_status}" }
            }
            td {
                span { class: "status {self_check_tone}", title: "{self_check_detail}", "{self_check_label}" }
            }
            td { "{recommendation}" }
            td { title: "{order_detail}", "{order_counts}" }
            td {
                span { class: "{impact_tone}", title: "{hermes_advice_impact_detail(&row)}", "{impact}" }
            }
            td { "{manager_status}" }
            td { class: "muted error-cell",
                span { class: "event-message", title: "{summary}", "{truncate_chars(&summary, 180)}" }
            }
        }
    }
}

#[component]
fn HermesCounterfactualRow(row: JsonValue, prefs: LocalizationPrefs) -> Element {
    let currency = fallback_text(&row, "currency", "DKK");
    let reference = optional_json_number(&row, "reference_price_local")
        .map(|value| format_money(value, &currency, &prefs))
        .unwrap_or_else(|| "n/a".to_string());
    let latest = optional_json_number(&row, "latest_price_local")
        .map(|value| format_money(value, &currency, &prefs))
        .unwrap_or_else(|| "n/a".to_string());
    let estimated_return = optional_json_number(&row, "estimated_return_pct")
        .map(|value| format_percent(value, &prefs))
        .unwrap_or_else(|| "n/a".to_string());
    let estimated_pnl = optional_json_number(&row, "estimated_pnl_local")
        .map(|value| format_money(value, &currency, &prefs))
        .unwrap_or_else(|| "n/a".to_string());
    let return_tone = optional_json_number(&row, "estimated_return_pct")
        .map(|value| {
            if value > 0.0 {
                "good-text"
            } else if value < 0.0 {
                "bad-text"
            } else {
                ""
            }
        })
        .unwrap_or("");
    let status = fallback_text(&row, "status", "unknown");
    let source = fallback_text(&row, "source_effect", "unknown").replace('_', " ");
    let action = fallback_text(&row, "action", "n/a");
    rsx! {
        tr {
            td {
                span { "#{text(&row, \"report_id\")}" }
                small { class: "muted block", "{format_timestamp(&text(&row, \"created_at\"), &prefs)}" }
            }
            td {
                strong { "{text(&row, \"symbol\")}" }
                small { class: "muted block", "{action}" }
            }
            td { "{source}" }
            td { "{format_quantity(value_f64(&row, \"shadow_quantity\"), &prefs)}" }
            td { "{reference}" }
            td {
                span { "{latest}" }
                if !text(&row, "latest_price_at").is_empty() {
                    small { class: "muted block", "{format_timestamp(&text(&row, \"latest_price_at\"), &prefs)}" }
                }
            }
            td { class: "{return_tone}", "{estimated_return}" }
            td { class: "{return_tone}", "{estimated_pnl}" }
            td {
                span {
                    class: "status {counterfactual_status_tone(&status)}",
                    title: "Shadow observations never place or modify Saxo orders.",
                    "{status}"
                }
            }
        }
    }
}

fn optional_json_number(row: &JsonValue, key: &str) -> Option<f64> {
    row.get(key)
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|value| value as f64))
                .or_else(|| value.as_str()?.parse::<f64>().ok())
        })
        .filter(|value| value.is_finite())
}

fn counterfactual_status_tone(status: &str) -> &'static str {
    match status {
        "tracking" => "good-status",
        "unpriced" => "warn-status",
        _ => "",
    }
}

fn hermes_context_self_check(row: &JsonValue) -> Option<&JsonValue> {
    row.get("advice_raw_payload_json")
        .and_then(|value| value.get("context_self_check"))
        .or_else(|| {
            row.get("manager_json")
                .and_then(|value| value.get("hermes_decision_advice"))
                .and_then(|value| value.get("context_self_check"))
        })
}

fn hermes_context_self_check_label(row: &JsonValue) -> (String, &'static str, String) {
    let Some(check) = hermes_context_self_check(row) else {
        return (
            "missing".to_string(),
            "warn-status",
            "Hermes advice did not include a context self-check.".to_string(),
        );
    };
    let missing = check
        .get("missing")
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(JsonValue::as_str)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if check.get("complete").and_then(JsonValue::as_bool) == Some(true) && missing.is_empty() {
        return (
            "complete".to_string(),
            "good-status",
            "Hermes reported that required decision report, Markov, EOD, positions, and experiment context were reviewed.".to_string(),
        );
    }
    let detail = if missing.is_empty() {
        "Hermes context self-check was present but not complete.".to_string()
    } else {
        format!(
            "Hermes context self-check is missing: {}.",
            missing.join(", ")
        )
    };
    ("missing".to_string(), "warn-status", detail)
}

fn hermes_advice_status_label(row: &JsonValue) -> String {
    let advice_status = text(row, "advice_status");
    if !advice_status.is_empty() {
        return advice_status;
    }
    let manager_status = row
        .get("manager_json")
        .and_then(|value| value.get("hermes_decision_advice"))
        .and_then(|value| value.get("status"))
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    if !manager_status.is_empty() {
        return manager_status.to_string();
    }
    "not_seen".to_string()
}

fn hermes_advice_status_tone(status: &str) -> &'static str {
    match status {
        "received" => "good-status",
        "not_seen" | "timeout" | "not_configured" | "submit_failed" => "warn-status",
        "error" => "bad-status",
        _ => "",
    }
}

fn hermes_advice_detail(row: &JsonValue) -> String {
    let advice_id = text(row, "advice_id");
    let source_session_id = text(row, "advice_source_session_id");
    let created_at = text(row, "advice_created_at");
    if advice_id.is_empty() {
        return format!(
            "No persisted Hermes decision advice row. Manager advisory status: {}.",
            hermes_advice_status_label(row)
        );
    }
    format!(
        "Advice {} recorded at {} from session {}.",
        advice_id,
        if created_at.is_empty() {
            "unknown time".to_string()
        } else {
            created_at
        },
        if source_session_id.is_empty() {
            "n/a".to_string()
        } else {
            source_session_id
        }
    )
}

fn hermes_advice_order_counts(row: &JsonValue) -> (String, String) {
    let items = row
        .get("order_advice_json")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut allow = 0usize;
    let mut reduce = 0usize;
    let mut stand_down = 0usize;
    let mut review = 0usize;
    for item in &items {
        match text(item, "action").trim().to_lowercase().as_str() {
            "allow" => allow += 1,
            "reduce" => reduce += 1,
            "stand_down" => stand_down += 1,
            "review" => review += 1,
            _ => {}
        }
    }
    let label = if items.is_empty() {
        "none".to_string()
    } else {
        format!("{} items", items.len())
    };
    let detail = format!(
        "allow: {allow}, reduce: {reduce}, stand_down: {stand_down}, review: {review}; queued: {}, executed: {}, failed: {}",
        text_or(row, "queued_order_count", "0"),
        text_or(row, "executed_order_count", "0"),
        text_or(row, "failed_order_count", "0")
    );
    (label, detail)
}

fn hermes_advice_delta(row: &JsonValue) -> Option<&JsonValue> {
    row.get("manager_json")
        .and_then(|value| value.get("hermes_advice_delta"))
}

fn hermes_advice_delta_count(delta: &JsonValue, effect: &str) -> usize {
    delta
        .get("effect_counts")
        .and_then(|value| value.get(effect))
        .and_then(JsonValue::as_u64)
        .unwrap_or(0) as usize
}

fn hermes_advice_impact(row: &JsonValue) -> (String, &'static str) {
    if let Some(delta) = hermes_advice_delta(row) {
        let context = hermes_advice_delta_count(delta, "context_gate_blocked");
        let blocked = hermes_advice_delta_count(delta, "blocked_by_order_advice")
            + hermes_advice_delta_count(delta, "blocked_by_global_stand_down")
            + hermes_advice_delta_count(delta, "blocked_by_reduce_below_one_share");
        let review = hermes_advice_delta_count(delta, "review_required_by_global_advice");
        let reduced = hermes_advice_delta_count(delta, "reduced");
        let allowed = hermes_advice_delta_count(delta, "allowed");
        if context > 0 {
            return (format!("context gate {context}"), "warn-text");
        }
        if blocked > 0 || review > 0 || reduced > 0 {
            let mut parts = Vec::new();
            if blocked > 0 {
                parts.push(format!("blocked {blocked}"));
            }
            if review > 0 {
                parts.push(format!("review {review}"));
            }
            if reduced > 0 {
                parts.push(format!("reduced {reduced}"));
            }
            return (
                parts.join(", "),
                if blocked > 0 { "bad-text" } else { "warn-text" },
            );
        }
        if allowed > 0 {
            return (format!("allowed {allowed}"), "good-text");
        }
        if hermes_advice_delta_count(delta, "record_only_no_op") > 0 {
            return ("record-only".to_string(), "");
        }
    }
    let recommendation = text(row, "advice_recommendation").trim().to_lowercase();
    let status = hermes_advice_status_label(row);
    let mode = row
        .get("manager_json")
        .and_then(|value| value.get("hermes_decision_advice"))
        .and_then(|value| value.get("mode"))
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let context_self_check_complete = hermes_context_self_check(row)
        .and_then(|check| check.get("complete"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    if mode == "conservative" && !context_self_check_complete {
        return ("context review gate".to_string(), "warn-text");
    }
    let items = row
        .get("order_advice_json")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let conservative_actions = items
        .iter()
        .filter(|item| {
            matches!(
                text(item, "action").trim().to_lowercase().as_str(),
                "reduce" | "stand_down" | "review"
            )
        })
        .count();
    if conservative_actions > 0 {
        return (
            format!("restricted {conservative_actions}"),
            if items.iter().any(|item| {
                text(item, "action")
                    .trim()
                    .eq_ignore_ascii_case("stand_down")
            }) {
                "bad-text"
            } else {
                "warn-text"
            },
        );
    }
    if recommendation == "stand_down" {
        return ("global block".to_string(), "bad-text");
    }
    if recommendation == "review" {
        return ("review gate".to_string(), "warn-text");
    }
    if mode == "conservative"
        && matches!(
            status.as_str(),
            "timeout" | "error" | "not_configured" | "submit_failed"
        )
    {
        return ("review fallback".to_string(), "warn-text");
    }
    ("no-op".to_string(), "")
}

fn hermes_advice_impact_detail(row: &JsonValue) -> String {
    let (orders, order_detail) = hermes_advice_order_counts(row);
    let manager = row
        .get("manager_json")
        .and_then(|value| value.get("hermes_decision_advice"))
        .cloned()
        .unwrap_or(JsonValue::Null);
    let context_self_check = hermes_context_self_check(row)
        .cloned()
        .unwrap_or(JsonValue::Null);
    let delta = hermes_advice_delta(row).cloned().unwrap_or(JsonValue::Null);
    format!(
        "Recommendation: {}. Order advice: {} ({order_detail}). Context self-check: {}. Normalized delta: {}. Manager advice: {}",
        fallback_text(row, "advice_recommendation", "n/a"),
        orders,
        compact_json(Some(&context_self_check)),
        compact_json(Some(&delta)),
        compact_json(Some(&manager))
    )
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

fn scheduler_cycle_json_status(row: &JsonValue, key: &str) -> String {
    row.get("cycle_json")
        .and_then(JsonValue::as_str)
        .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
        .and_then(|value| {
            value
                .get(key)
                .and_then(|item| item.get("status"))
                .and_then(JsonValue::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "n/a".to_string())
}

fn scheduler_cycle_duration(row: &JsonValue) -> String {
    row.get("cycle_json")
        .and_then(JsonValue::as_str)
        .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
        .and_then(|value| value.get("duration_ms").and_then(json_duration_ms))
        .map(format_duration_ms)
        .unwrap_or_else(|| "n/a".to_string())
}

fn json_duration_ms(value: &JsonValue) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|duration| duration.is_finite() && *duration >= 0.0)
            .map(|duration| duration.round() as u64)
    })
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms} ms")
    } else if duration_ms < 60_000 {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    } else {
        let total_seconds = duration_ms / 1_000;
        format!("{}m {}s", total_seconds / 60, total_seconds % 60)
    }
}

fn operations_health(data: &DashboardView) -> Vec<OperationHealthItem> {
    operations_health_at(data, Utc::now())
}

fn operations_health_at(data: &DashboardView, now: DateTime<Utc>) -> Vec<OperationHealthItem> {
    vec![
        saxo_operation_health(&data.saxo_auth),
        integrity_operation_health(&data.integrity),
        scheduler_operation_health(&data.market_status, now),
        decision_pulse_operation_health(
            &data.decision_pulse_statuses,
            "europe_open_followup",
            "EU Report",
        ),
        decision_pulse_operation_health(
            &data.decision_pulse_statuses,
            "us_open_followup",
            "US Report",
        ),
        run_operation_health(
            "Markov",
            &data.latest_markov_run,
            data.run_schedules.get("markov").unwrap_or(&JsonValue::Null),
            now,
        ),
        run_operation_health(
            "Quiver",
            &data.latest_quiver_run,
            data.run_schedules.get("quiver").unwrap_or(&JsonValue::Null),
            now,
        ),
        run_operation_health(
            "Indicators",
            &data.latest_daily_indicator_run,
            data.run_schedules
                .get("indicators")
                .unwrap_or(&JsonValue::Null),
            now,
        ),
        quote_operation_health(&data.positions, &data.market_status, now),
        execution_operation_health(&data.orders),
    ]
}

fn saxo_operation_health(auth: &JsonValue) -> OperationHealthItem {
    let connected = auth
        .get("connected")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let status = text_or(auth, "status", "unknown");
    let detail = text_or(auth, "status_text", "No Saxo session status is available.");
    if !connected {
        return OperationHealthItem {
            label: "Saxo".to_string(),
            status: if status == "needs_reauth" || status == "missing_session" {
                "reauth".to_string()
            } else {
                status
            },
            tone: "bad",
            detail,
        };
    }

    let expires_in = value_f64(auth, "expires_in_minutes");
    if expires_in > 0.0 && expires_in <= 10.0 {
        OperationHealthItem {
            label: "Saxo".to_string(),
            status: "expiring".to_string(),
            tone: "warn",
            detail: format!("{detail} Access token expires in {:.0} min.", expires_in),
        }
    } else {
        OperationHealthItem {
            label: "Saxo".to_string(),
            status: "ok".to_string(),
            tone: "good",
            detail,
        }
    }
}

fn scheduler_operation_health(
    market_status: &JsonValue,
    now: DateTime<Utc>,
) -> OperationHealthItem {
    let summary = market_status.get("summary").unwrap_or(&JsonValue::Null);
    let heartbeat = text(summary, "last_heartbeat_at");
    let last_cycle_status = text_or(summary, "last_cycle_status", "unknown");
    let Some(age_minutes) = age_minutes(&heartbeat, now) else {
        return OperationHealthItem {
            label: "Scheduler".to_string(),
            status: "unknown".to_string(),
            tone: "warn",
            detail: "No scheduler heartbeat timestamp is available.".to_string(),
        };
    };
    let (tone, status) = if age_minutes > 60.0 {
        ("bad", "stale")
    } else if age_minutes > 20.0 {
        ("warn", "delayed")
    } else if last_cycle_status == "ok" || last_cycle_status.is_empty() {
        ("good", "ok")
    } else {
        ("warn", "check")
    };
    OperationHealthItem {
        label: "Scheduler".to_string(),
        status: status.to_string(),
        tone,
        detail: format!(
            "Last heartbeat {} ago. Last cycle status: {}.",
            age_label(age_minutes),
            last_cycle_status
        ),
    }
}

fn integrity_operation_health(integrity: &JsonValue) -> OperationHealthItem {
    let mismatch_count = integrity
        .get("mismatches")
        .and_then(JsonValue::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let warning_count = integrity
        .get("warnings")
        .and_then(JsonValue::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let expiry_pending_count = integrity
        .get("expiry_pending_orders")
        .and_then(JsonValue::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let checked_at = text(integrity, "checked_at");
    let checked_label = if checked_at.is_empty() {
        "not checked".to_string()
    } else {
        format!("checked {}", checked_at)
    };

    if mismatch_count > 0 {
        OperationHealthItem {
            label: "Integrity".to_string(),
            status: format!("{mismatch_count} error"),
            tone: "bad",
            detail: format!(
                "{mismatch_count} integrity mismatch(es), {warning_count} warning(s); {checked_label}."
            ),
        }
    } else if warning_count > 0 {
        let status = if expiry_pending_count > 0 {
            "expiry sync".to_string()
        } else {
            format!("{warning_count} warn")
        };
        OperationHealthItem {
            label: "Integrity".to_string(),
            status,
            tone: "warn",
            detail: format!(
                "{warning_count} integrity warning(s), including {expiry_pending_count} DayOrder expiry-sync pending row(s); {checked_label}."
            ),
        }
    } else {
        OperationHealthItem {
            label: "Integrity".to_string(),
            status: "ok".to_string(),
            tone: "good",
            detail: format!("Integrity checks are clear; {checked_label}."),
        }
    }
}

fn execution_operation_health(orders: &[JsonValue]) -> OperationHealthItem {
    let expiry_pending = orders
        .iter()
        .filter(|order| text(order, "lifecycle_state") == "expiry_pending_broker_sync")
        .count();
    if expiry_pending > 0 {
        return OperationHealthItem {
            label: "Execution".to_string(),
            status: "expiry sync".to_string(),
            tone: "warn",
            detail: format!(
                "{expiry_pending} active Saxo DayOrder(s) passed expected expiry and need broker sync confirmation."
            ),
        };
    }

    let broker_live = orders
        .iter()
        .filter(|order| active_broker_status(&text(order, "status")))
        .count();
    if broker_live > 0 {
        OperationHealthItem {
            label: "Execution".to_string(),
            status: format!("{broker_live} live"),
            tone: "good",
            detail: format!("{broker_live} Saxo order(s) are awaiting broker fill or status sync."),
        }
    } else {
        OperationHealthItem {
            label: "Execution".to_string(),
            status: "ok".to_string(),
            tone: "good",
            detail: "No active Saxo broker orders are awaiting sync.".to_string(),
        }
    }
}

fn active_broker_status(status: &str) -> bool {
    matches!(
        status,
        "submitted_to_broker"
            | "broker_working"
            | "broker_amended"
            | "broker_partially_filled"
            | "broker_replace_requested"
            | "broker_cancel_requested"
    )
}

fn decision_pulse_operation_health(
    statuses: &[JsonValue],
    key: &str,
    label: &str,
) -> OperationHealthItem {
    let Some(pulse) = statuses.iter().find(|row| text(row, "key") == key) else {
        return OperationHealthItem {
            label: label.to_string(),
            status: "unknown".to_string(),
            tone: "warn",
            detail: "Decision-pulse status was unavailable while the dashboard loaded.".to_string(),
        };
    };
    let latest = pulse.get("latest").unwrap_or(&JsonValue::Null);
    if latest.is_null() {
        return OperationHealthItem {
            label: label.to_string(),
            status: "missing".to_string(),
            tone: "warn",
            detail: format!("No {label} decision report has been recorded yet."),
        };
    }

    let latest_status = fallback_text(latest, "status", "unknown");
    let (tone, status) = match latest_status.as_str() {
        "completed" | "xai_fallback" => ("good", "ok"),
        "pending" | "xai_deferred" => ("warn", "pending"),
        "xai_error" | "error" | "failed" | "parse_error" => ("bad", "failed"),
        _ => ("warn", "check"),
    };
    let last_success = pulse.get("last_success").unwrap_or(&JsonValue::Null);
    let success_detail = if last_success.is_null() {
        "No successful report is recorded yet.".to_string()
    } else {
        format!(
            "Last success #{} at {}.",
            fallback_text(last_success, "id", "n/a"),
            fallback_text(last_success, "created_at", "unknown time"),
        )
    };
    OperationHealthItem {
        label: label.to_string(),
        status: status.to_string(),
        tone,
        detail: format!(
            "Latest report #{} at {} is {}. {success_detail}",
            fallback_text(latest, "id", "n/a"),
            fallback_text(latest, "created_at", "unknown time"),
            latest_status,
        ),
    }
}

fn run_operation_health(
    label: &str,
    run: &JsonValue,
    schedule: &JsonValue,
    now: DateTime<Utc>,
) -> OperationHealthItem {
    let schedule_state = scheduled_run_state(schedule, now);
    if run.is_null() {
        let (tone, status) = match schedule_state {
            ScheduledRunState::Disabled => ("neutral", "disabled"),
            ScheduledRunState::Weekend { .. } => ("neutral", "idle (weekend)"),
            ScheduledRunState::BeforeDue { .. } => ("neutral", "waiting"),
            ScheduledRunState::Due { .. } | ScheduledRunState::Unknown => ("warn", "missing"),
        };
        return OperationHealthItem {
            label: label.to_string(),
            status: status.to_string(),
            tone,
            detail: scheduled_run_missing_detail(label, schedule_state),
        };
    }
    let status = text_or(run, "status", "unknown");
    let run_date = text(run, "run_date");
    let error_count = value_f64(run, "error_count");
    let lower_status = status.to_ascii_lowercase();
    let (tone, display) = if lower_status.contains("error") || lower_status.contains("failed") {
        ("bad", "failed")
    } else if error_count > 0.0 {
        ("warn", "partial")
    } else {
        scheduled_run_tone_and_status(&run_date, schedule_state)
    };
    OperationHealthItem {
        label: label.to_string(),
        status: display.to_string(),
        tone,
        detail: format!(
            "{} run date {}. Status: {}. Succeeded: {}. Failed: {}. {}",
            label,
            if run_date.is_empty() {
                "unknown".to_string()
            } else {
                run_date
            },
            status,
            fallback_text(run, "success_count", "0"),
            fallback_text(run, "error_count", "0"),
            scheduled_run_detail(schedule_state),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ScheduledRunState {
    Disabled,
    Weekend {
        expected_run_date: NaiveDate,
    },
    BeforeDue {
        expected_run_date: NaiveDate,
        due_time: NaiveTime,
    },
    Due {
        expected_run_date: NaiveDate,
        due_time: NaiveTime,
    },
    Unknown,
}

fn scheduled_run_state(schedule: &JsonValue, now: DateTime<Utc>) -> ScheduledRunState {
    if schedule.is_null() {
        return ScheduledRunState::Unknown;
    }
    if !schedule
        .get("enabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(true)
    {
        return ScheduledRunState::Disabled;
    }
    let timezone = text(schedule, "timezone")
        .parse::<Tz>()
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let due_time = NaiveTime::parse_from_str(&text(schedule, "daily_time"), "%H:%M")
        .unwrap_or_else(|_| NaiveTime::from_hms_opt(23, 30, 0).expect("valid default time"));
    let weekdays_only = schedule
        .get("run_weekdays_only")
        .and_then(JsonValue::as_bool)
        .unwrap_or(true);
    let now_local = now.with_timezone(&timezone);
    let local_date = now_local.date_naive();
    let expected_run_date =
        latest_scheduled_run_date(local_date, now_local.time(), due_time, weekdays_only);
    if weekdays_only && local_date.weekday().number_from_monday() > 5 {
        ScheduledRunState::Weekend { expected_run_date }
    } else if now_local.time() < due_time {
        ScheduledRunState::BeforeDue {
            expected_run_date,
            due_time,
        }
    } else {
        ScheduledRunState::Due {
            expected_run_date,
            due_time,
        }
    }
}

fn latest_scheduled_run_date(
    local_date: NaiveDate,
    local_time: NaiveTime,
    due_time: NaiveTime,
    weekdays_only: bool,
) -> NaiveDate {
    let mut expected = if local_time < due_time {
        local_date - Duration::days(1)
    } else {
        local_date
    };
    while weekdays_only && expected.weekday().number_from_monday() > 5 {
        expected -= Duration::days(1);
    }
    expected
}

fn scheduled_run_tone_and_status(
    run_date: &str,
    state: ScheduledRunState,
) -> (&'static str, &'static str) {
    let parsed_run_date = NaiveDate::parse_from_str(run_date, "%Y-%m-%d").ok();
    match state {
        ScheduledRunState::Disabled => ("neutral", "disabled"),
        ScheduledRunState::Weekend { expected_run_date } => {
            if parsed_run_date.is_some_and(|value| value >= expected_run_date) {
                ("neutral", "idle (weekend)")
            } else {
                ("warn", "stale")
            }
        }
        ScheduledRunState::BeforeDue {
            expected_run_date, ..
        } => {
            if parsed_run_date.is_some_and(|value| value >= expected_run_date) {
                ("neutral", "waiting")
            } else {
                ("warn", "stale")
            }
        }
        ScheduledRunState::Due {
            expected_run_date, ..
        } => {
            if parsed_run_date.is_some_and(|value| value >= expected_run_date) {
                ("good", "fresh")
            } else {
                ("warn", "stale")
            }
        }
        ScheduledRunState::Unknown => {
            if parsed_run_date.is_some() {
                ("warn", "schedule unknown")
            } else {
                ("warn", "unknown")
            }
        }
    }
}

fn scheduled_run_detail(state: ScheduledRunState) -> String {
    match state {
        ScheduledRunState::Disabled => "The configured job is disabled.".to_string(),
        ScheduledRunState::Weekend { expected_run_date } => format!(
            "No weekday run is due during the weekend; the latest expected run date is {expected_run_date}."
        ),
        ScheduledRunState::BeforeDue {
            expected_run_date,
            due_time,
        } => format!(
            "The next local run is due at {}. The latest expected run date remains {expected_run_date}.",
            due_time.format("%H:%M")
        ),
        ScheduledRunState::Due {
            expected_run_date,
            due_time,
        } => format!(
            "A run is due after {} for {expected_run_date}.",
            due_time.format("%H:%M")
        ),
        ScheduledRunState::Unknown => "No usable schedule configuration is available.".to_string(),
    }
}

fn scheduled_run_missing_detail(label: &str, state: ScheduledRunState) -> String {
    match state {
        ScheduledRunState::Disabled => format!("{label} is disabled and has no recorded run."),
        ScheduledRunState::Weekend { .. } => {
            format!("No {label} run is expected during the configured weekday-only weekend window.")
        }
        ScheduledRunState::BeforeDue { due_time, .. } => format!(
            "No {label} run is due until {} local time.",
            due_time.format("%H:%M")
        ),
        ScheduledRunState::Due { .. } | ScheduledRunState::Unknown => {
            format!("No {label} run has been recorded yet.")
        }
    }
}

fn price_monitor_summary(monitor: &JsonValue) -> JsonValue {
    monitor
        .get("summary_json")
        .cloned()
        .unwrap_or(JsonValue::Null)
}

fn price_monitor_status_label(monitor: &JsonValue) -> String {
    let status = text(monitor, "status");
    if status.is_empty() {
        return "unknown".to_string();
    }
    let summary = price_monitor_summary(monitor);
    match status.as_str() {
        "ok" => {
            let updated = value_f64(&summary, "updated") as i64;
            format!("ok · {updated} updated")
        }
        "market_closed" => {
            let skipped = value_f64(&summary, "skipped_closed") as i64;
            format!("closed · {skipped} skipped")
        }
        "partial" => {
            let updated = value_f64(&summary, "updated") as i64;
            format!("partial · {updated} updated")
        }
        "no_session" => "no session".to_string(),
        other => other.to_string(),
    }
}

fn price_monitor_detail(monitor: &JsonValue, prefs: &LocalizationPrefs) -> String {
    if monitor.is_null() {
        return "No price monitor status has been recorded yet.".to_string();
    }
    let updated_at = format_timestamp(&text(monitor, "updated_at"), prefs);
    let summary = price_monitor_summary(monitor);
    let status = text(monitor, "status");
    let skipped = value_f64(&summary, "skipped_closed") as usize;
    let mut detail = format!("Last status {status} at {updated_at}.");
    if skipped > 0 {
        detail.push_str(&format!(
            " Skipped known-closed symbols: {}.",
            price_monitor_skipped_symbols(&summary, 8)
        ));
    }
    let reason = text(&summary, "reason");
    if !reason.is_empty() {
        detail.push_str(&format!(" Reason: {reason}."));
    }
    detail
}

fn price_monitor_skipped_symbols(summary: &JsonValue, limit: usize) -> String {
    let items = summary
        .get("skipped_closed_symbols")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return "none".to_string();
    }
    let names = items
        .iter()
        .take(limit)
        .map(|row| {
            let symbol = text(row, "symbol");
            let exchange = text(row, "exchange");
            if exchange.is_empty() {
                symbol
            } else {
                format!("{symbol} ({exchange})")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > limit {
        format!("{names}, +{} more", items.len() - limit)
    } else {
        names
    }
}

fn quote_operation_health(
    positions: &[JsonValue],
    market_status: &JsonValue,
    now: DateTime<Utc>,
) -> OperationHealthItem {
    let monitor = market_status
        .get("price_monitor")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let monitor_status = text(&monitor, "status");
    let monitor_summary = price_monitor_summary(&monitor);
    if monitor_status == "market_closed" {
        let skipped = value_f64(&monitor_summary, "skipped_closed") as usize;
        return OperationHealthItem {
            label: "Quotes".to_string(),
            status: "closed".to_string(),
            tone: "good",
            detail: if skipped > 0 {
                format!(
                    "Price monitor paused because known exchanges are closed; skipped {skipped} symbol(s): {}.",
                    price_monitor_skipped_symbols(&monitor_summary, 6)
                )
            } else {
                "Price monitor paused because known exchanges are closed.".to_string()
            },
        };
    }
    if monitor_status == "no_session" {
        return OperationHealthItem {
            label: "Quotes".to_string(),
            status: "no session".to_string(),
            tone: "warn",
            detail: "Price monitor cannot refresh quotes until the Saxo session is valid."
                .to_string(),
        };
    }
    if monitor_status == "partial" {
        let errors = monitor_summary
            .get("errors")
            .and_then(JsonValue::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        return OperationHealthItem {
            label: "Quotes".to_string(),
            status: "partial".to_string(),
            tone: "warn",
            detail: format!("Latest price monitor refresh was partial with {errors} error(s)."),
        };
    }
    let latest = positions
        .iter()
        .filter_map(|row| parse_utc_timestamp(&text(row, "latest_quote_updated_at")))
        .max();
    let Some(latest) = latest else {
        return OperationHealthItem {
            label: "Quotes".to_string(),
            status: "unknown".to_string(),
            tone: "warn",
            detail: "No quote freshness timestamp is available for current positions.".to_string(),
        };
    };
    let age_minutes = (now - latest).num_seconds().max(0) as f64 / 60.0;
    let (tone, status) = if age_minutes > 36.0 * 60.0 {
        ("bad", "stale")
    } else if age_minutes > 12.0 * 60.0 {
        ("warn", "old")
    } else {
        ("good", "fresh")
    };
    OperationHealthItem {
        label: "Quotes".to_string(),
        status: status.to_string(),
        tone,
        detail: format!("Newest quote snapshot is {} old.", age_label(age_minutes)),
    }
}

fn execution_status_class(status: &str) -> &'static str {
    let lower = status.to_ascii_lowercase();
    if lower == "broker_state_unknown" {
        "status warn-status"
    } else if lower.contains("failed")
        || lower.contains("rejected")
        || lower.contains("invalid")
        || lower.contains("cancelled")
        || lower.contains("expired")
        || lower.contains("done_for_day")
    {
        "status bad-status"
    } else if matches!(
        status,
        "executed" | "submitted_to_broker" | "broker_working"
    ) {
        "status good-status"
    } else {
        "status"
    }
}

fn execution_reason_class(reason: &str) -> &'static str {
    if reason == "Broker working" {
        "status good-status"
    } else if reason == "Broker state unknown" {
        "status warn-status"
    } else {
        "status detail-status"
    }
}

fn execution_status_detail(row: &JsonValue) -> String {
    let error = text(row, "error_text");
    if !error.is_empty() {
        return error;
    }
    diagnostic_payload(row, "execution_result_json")
        .and_then(|payload| diagnostic_detail_from_json(&payload))
        .unwrap_or_default()
}

fn execution_status_reason(row: &JsonValue) -> String {
    if text(row, "lifecycle_state") == "expiry_pending_broker_sync" {
        return "Expiry sync pending".to_string();
    }
    if let Some(taxonomy) = execution_error_taxonomy(row) {
        let label = text(&taxonomy, "label");
        if !label.is_empty() {
            return label;
        }
    }
    let status = text(row, "status");
    let detail = execution_status_detail(row);
    classify_execution_detail(&status, &detail)
}

fn execution_status_tooltip(row: &JsonValue, reason: &str, detail: &str) -> String {
    let mut lines = Vec::new();
    let status = text(row, "status");
    if !status.is_empty() {
        lines.push(format!("status: {status}"));
    }
    let broker_order_id = text(row, "broker_order_id");
    if !broker_order_id.is_empty() {
        lines.push(format!("broker order: {broker_order_id}"));
    }
    if !reason.is_empty() {
        lines.push(format!("reason: {reason}"));
    }
    if let Some(taxonomy) = execution_error_taxonomy(row) {
        let code = text(&taxonomy, "code");
        if !code.is_empty() {
            lines.push(format!("category: {code}"));
        }
        let remediation = text(&taxonomy, "remediation");
        if !remediation.is_empty() {
            lines.push(format!("next step: {remediation}"));
        }
        let retry_policy = text(&taxonomy, "retry_policy");
        if !retry_policy.is_empty() {
            lines.push(format!("retry: {retry_policy}"));
        }
    }
    let lifecycle_state = text(row, "lifecycle_state");
    if !lifecycle_state.is_empty() {
        lines.push(format!("lifecycle state: {lifecycle_state}"));
    }
    let broker_visibility = execution_broker_sync_text(row, "broker_visibility");
    if !broker_visibility.is_empty() {
        lines.push(format!("broker visibility: {broker_visibility}"));
    }
    let broker_visibility_note = execution_broker_sync_text(row, "broker_visibility_note");
    if !broker_visibility_note.is_empty() {
        lines.push(format!(
            "broker visibility detail: {broker_visibility_note}"
        ));
    }
    if !detail.is_empty() {
        lines.push(format!("detail: {detail}"));
    } else if status == "broker_state_unknown" {
        lines.push(
            "detail: Saxo may have received the placement request; automatic retry is blocked until broker reconciliation."
                .to_string(),
        );
    } else if status == "broker_working" {
        lines.push(
            "detail: order accepted by Saxo; waiting for broker status/fill sync".to_string(),
        );
    }
    let duration = text(row, "order_duration_type");
    let expiry = text(row, "expected_expiry_at_utc");
    if !duration.is_empty() || !expiry.is_empty() {
        lines.push(format!(
            "lifecycle: duration {}; expected expiry {}",
            if duration.is_empty() {
                "n/a"
            } else {
                &duration
            },
            if expiry.is_empty() { "n/a" } else { &expiry }
        ));
    }
    lines.join("\n")
}

fn execution_error_taxonomy(row: &JsonValue) -> Option<JsonValue> {
    diagnostic_payload(row, "execution_result_json")
        .and_then(|payload| payload.get("error_taxonomy").cloned())
}

fn execution_broker_sync_text(row: &JsonValue, key: &str) -> String {
    row.get("execution_result_json")
        .and_then(|value| value.get("broker_sync"))
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

fn execution_order_lifecycle_label(row: &JsonValue, prefs: &LocalizationPrefs) -> String {
    let duration = text(row, "order_duration_type");
    let expiry_at = text(row, "expected_expiry_at_utc");
    if duration.eq_ignore_ascii_case("DayOrder") && !expiry_at.is_empty() {
        return format_timestamp(&expiry_at, prefs);
    }
    if !duration.is_empty() {
        return duration;
    }
    String::new()
}

fn execution_order_lifecycle_detail(row: &JsonValue, prefs: &LocalizationPrefs) -> String {
    let mut lines = Vec::new();
    let duration = text(row, "order_duration_type");
    if !duration.is_empty() {
        lines.push(format!("duration {duration}"));
    }
    let expiry_at = text(row, "expected_expiry_at_utc");
    if !expiry_at.is_empty() {
        lines.push(format!(
            "expected expiry {}",
            format_timestamp(&expiry_at, prefs)
        ));
    }
    let market = text(row, "expected_expiry_market");
    if !market.is_empty() {
        lines.push(format!("market {market}"));
    }
    let lifecycle_state = text(row, "lifecycle_state");
    if !lifecycle_state.is_empty() {
        lines.push(format!("state {lifecycle_state}"));
    }
    let broker_visibility = execution_broker_sync_text(row, "broker_visibility");
    if !broker_visibility.is_empty() {
        lines.push(format!("broker visibility {broker_visibility}"));
    }
    let broker_visibility_note = execution_broker_sync_text(row, "broker_visibility_note");
    if !broker_visibility_note.is_empty() {
        lines.push(broker_visibility_note);
    }
    let note = text(row, "lifecycle_note");
    if !note.is_empty() {
        lines.push(note);
    }
    lines.join("; ")
}

fn execution_event_detail(row: &JsonValue) -> String {
    let message = text(row, "message");
    let error = text(row, "error_text");
    match (message.is_empty(), error.is_empty()) {
        (false, false) => format!("{message}: {error}"),
        (false, true) => message,
        (true, false) => error,
        (true, true) => diagnostic_payload(row, "raw_payload_json")
            .and_then(|payload| diagnostic_detail_from_json(&payload))
            .unwrap_or_default(),
    }
}

fn execution_event_reason(row: &JsonValue) -> String {
    let status = fallback_text(row, "broker_status", &text(row, "status"));
    let detail = execution_event_detail(row);
    classify_execution_detail(&status, &detail)
}

fn execution_event_tooltip(row: &JsonValue, status: &str, reason: &str, detail: &str) -> String {
    let mut lines = Vec::new();
    if !status.is_empty() {
        lines.push(format!("status: {status}"));
    }
    let event_type = text(row, "event_type");
    if !event_type.is_empty() {
        lines.push(format!("event: {event_type}"));
    }
    let order_id = text(row, "execution_order_id");
    if !order_id.is_empty() {
        lines.push(format!("order: {order_id}"));
    }
    if !reason.is_empty() {
        lines.push(format!("reason: {reason}"));
    }
    if !detail.is_empty() {
        lines.push(format!("detail: {detail}"));
    }
    lines.join("\n")
}

fn classify_execution_detail(status: &str, detail: &str) -> String {
    let lower_status = status.to_ascii_lowercase();
    let lower_detail = detail.to_ascii_lowercase();
    if lower_status == "broker_state_unknown" {
        "Broker state unknown".to_string()
    } else if let Some(reason) = saxo_precheck_reason(detail) {
        reason
    } else if lower_detail.contains("sell blocked before saxo precheck") {
        "Sell guard".to_string()
    } else if lower_detail.contains("no tradable saxo instrument")
        || lower_detail.contains("looking up saxo instrument")
        || lower_detail.contains("instrument match")
        || lower_detail.contains("instrument is not tradable")
    {
        "Resolve failed".to_string()
    } else if lower_detail.contains("exchange closed")
        || lower_detail.contains("market is closed")
        || lower_status == "waiting_for_market_open"
    {
        "Market closed".to_string()
    } else if lower_detail.contains("rate limited") || lower_detail.contains("http 429") {
        "Rate limited".to_string()
    } else if lower_detail.contains("unauthorized")
        || lower_detail.contains("access token")
        || lower_detail.contains("http 401")
        || lower_detail.contains("http 403")
    {
        "Saxo auth".to_string()
    } else if lower_detail.contains("insufficient")
        || lower_detail.contains("not enough cash")
        || lower_detail.contains("buying power")
    {
        "Insufficient cash".to_string()
    } else if lower_detail.contains("tick")
        || lower_detail.contains("increment")
        || lower_detail.contains("price step")
    {
        "Tick size".to_string()
    } else if lower_status == "invalid_quantity"
        || lower_detail.contains("quantity")
        || lower_detail.contains("amount")
    {
        "Invalid quantity".to_string()
    } else if lower_detail.contains("limit price") || lower_detail.contains("stop price") {
        "Invalid price".to_string()
    } else if lower_status == "broker_working" {
        "Broker working".to_string()
    } else if lower_status == "broker_expired" || lower_detail.contains("expired") {
        "Expired unfilled".to_string()
    } else if lower_status == "broker_done_for_day" || lower_detail.contains("doneforday") {
        "Done for day".to_string()
    } else if lower_status.contains("failed") || lower_status.contains("rejected") {
        "Broker rejected".to_string()
    } else {
        String::new()
    }
}

fn saxo_precheck_reason(detail: &str) -> Option<String> {
    let marker = "Order precheck failed:";
    let after_marker = detail.split_once(marker)?.1.trim();
    let reason = after_marker
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('.');
    if reason.is_empty() {
        None
    } else {
        Some(reason.to_string())
    }
}

fn execution_detail_block(row: &JsonValue, detail: &str) -> String {
    let mut lines = Vec::new();
    if !detail.is_empty() {
        lines.push(format!("error: {detail}"));
    }
    if let Some(payload) = diagnostic_payload(row, "execution_result_json") {
        let sanitized = sanitize_diagnostic_json(&payload);
        if !sanitized.is_null() && sanitized != json_empty_object() {
            let pretty =
                serde_json::to_string_pretty(&sanitized).unwrap_or_else(|_| sanitized.to_string());
            lines.push(format!("diagnostics:\n{pretty}"));
        }
    }
    lines.join("\n\n")
}

fn diagnostic_payload(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value)
            .ok()
            .or_else(|| Some(JsonValue::String(value.clone()))),
        JsonValue::Null => None,
        value => Some(value.clone()),
    }
}

fn diagnostic_detail_from_json(value: &JsonValue) -> Option<String> {
    diagnostic_detail_from_json_inner(value, true)
}

fn diagnostic_detail_from_json_inner(
    value: &JsonValue,
    allow_direct_string: bool,
) -> Option<String> {
    match value {
        JsonValue::String(text) => {
            (allow_direct_string && !text.trim().is_empty()).then(|| text.clone())
        }
        JsonValue::Object(map) => {
            for key in [
                "error_text",
                "error",
                "message",
                "Message",
                "reason",
                "Reason",
                "description",
                "Description",
            ] {
                if let Some(value) = map.get(key) {
                    if let Some(text) = diagnostic_detail_from_json_inner(value, true) {
                        return Some(text);
                    }
                }
            }
            for value in map.values() {
                if matches!(value, JsonValue::Object(_) | JsonValue::Array(_)) {
                    if let Some(text) = diagnostic_detail_from_json_inner(value, false) {
                        return Some(text);
                    }
                }
            }
            None
        }
        JsonValue::Array(items) => items
            .iter()
            .find_map(|item| diagnostic_detail_from_json_inner(item, allow_direct_string)),
        _ => None,
    }
}

fn sanitize_diagnostic_json(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let mut sanitized = serde_json::Map::new();
            for (key, value) in map {
                if is_sensitive_diagnostic_key(key) {
                    continue;
                }
                sanitized.insert(key.clone(), sanitize_diagnostic_json(value));
            }
            JsonValue::Object(sanitized)
        }
        JsonValue::Array(items) => JsonValue::Array(
            items
                .iter()
                .map(sanitize_diagnostic_json)
                .collect::<Vec<_>>(),
        ),
        other => other.clone(),
    }
}

fn is_sensitive_diagnostic_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "accountkey"
            | "accountid"
            | "account_id"
            | "clientkey"
            | "clientid"
            | "client_id"
            | "userid"
            | "user_id"
            | "handledby"
            | "correlationkey"
            | "access_token"
            | "refresh_token"
            | "authorization"
            | "token"
            | "client_secret"
            | "api_key"
    )
}

fn json_empty_object() -> JsonValue {
    JsonValue::Object(serde_json::Map::new())
}

fn format_local_money(value: f64, currency: &str, prefs: &LocalizationPrefs) -> String {
    if value.abs() < f64::EPSILON {
        "n/a".to_string()
    } else {
        format_money(value, currency, prefs)
    }
}

fn age_minutes(value: &str, now: DateTime<Utc>) -> Option<f64> {
    let timestamp = parse_utc_timestamp(value)?;
    Some((now - timestamp).num_seconds().max(0) as f64 / 60.0)
}

fn parse_utc_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| value.and_utc())
        })
}

fn age_label(minutes: f64) -> String {
    if minutes < 90.0 {
        format!("{:.0} min", minutes.max(0.0))
    } else if minutes < 36.0 * 60.0 {
        format!("{:.1} h", minutes / 60.0)
    } else {
        format!("{:.1} d", minutes / 60.0 / 24.0)
    }
}

fn execution_order_price_currency(row: &JsonValue) -> String {
    let currency = text(row, "currency");
    if !currency.trim().is_empty() {
        return currency;
    }
    if let Some(currency) = row
        .get("execution_result_json")
        .and_then(|value| value.get("broker_sync"))
        .and_then(|value| value.get("broker_payload"))
        .and_then(|value| value.get("DisplayAndFormat"))
        .and_then(|value| value.get("Currency"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return currency.to_string();
    }
    currency_for_symbol(&text(row, "symbol")).to_string()
}

fn currency_for_symbol(symbol: &str) -> &'static str {
    match symbol
        .split_once(':')
        .map(|(_, exchange)| exchange.to_ascii_lowercase())
        .as_deref()
    {
        Some("xnas") | Some("xnys") | Some("arcx") | Some("bats") => "USD",
        Some("xcse") => "DKK",
        Some("xmil") | Some("xetr") | Some("xams") | Some("xpar") => "EUR",
        Some("xlon") | Some("xlse") => "GBP",
        Some("xsto") => "SEK",
        Some("xosl") => "NOK",
        Some("xswx") => "CHF",
        _ => "DKK",
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

fn compact_json_redacted(value: Option<&JsonValue>, max_len: usize) -> String {
    let Some(value) = value else {
        return "No payload available.".to_string();
    };
    let redacted = redact_debug_json(value);
    let rendered = serde_json::to_string_pretty(&redacted).unwrap_or_else(|_| redacted.to_string());
    compact_debug_text(&rendered, max_len)
}

fn compact_debug_text(value: &str, max_len: usize) -> String {
    let redacted = redact_debug_text(value);
    if redacted.len() > max_len {
        format!("{}...", &redacted[..max_len])
    } else if redacted.trim().is_empty() {
        "No payload available.".to_string()
    } else {
        redacted
    }
}

fn redact_debug_json(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(object) => JsonValue::Object(
            object
                .iter()
                .map(|(key, value)| {
                    if is_sensitive_debug_key(key) {
                        (key.clone(), JsonValue::String("[redacted]".to_string()))
                    } else {
                        (key.clone(), redact_debug_json(value))
                    }
                })
                .collect::<Map<String, JsonValue>>(),
        ),
        JsonValue::Array(values) => {
            JsonValue::Array(values.iter().map(redact_debug_json).collect())
        }
        JsonValue::String(value) => JsonValue::String(redact_debug_text(value)),
        _ => value.clone(),
    }
}

fn is_sensitive_debug_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    [
        "api_key",
        "authorization",
        "bearer",
        "token",
        "refresh",
        "secret",
        "password",
        "accountkey",
        "account_key",
        "clientkey",
        "client_key",
        "database_url",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn redact_debug_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let trimmed = word.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']' | '{' | '}'
                )
            });
            if looks_like_secret_token(trimmed) {
                word.replace(trimmed, "[redacted]")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_secret_token(value: &str) -> bool {
    if value.starts_with("sk-") || value.starts_with("Bearer") {
        return true;
    }
    value.len() >= 32
        && value.chars().any(char::is_alphabetic)
        && value.chars().any(char::is_numeric)
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
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

/// UTF-8-safe prefix truncation for status text rendered in layout-sensitive
/// places like the topbar pills.
fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut result: String = value.chars().take(max_chars).collect();
    result.push('…');
    result
}

/// Health pill for the decision engine derived from the latest report.
/// Credit/spending-limit failures get their own label because they need
/// operator action (top up xAI credits) rather than a code fix.
fn decision_health(latest_decision: &JsonValue) -> (&'static str, String) {
    let status = latest_decision
        .get("status")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    match status {
        "completed" | "xai_fallback" => ("pill good", "Decisions: OK".to_string()),
        "pending" | "xai_deferred" => ("pill", "Decisions: Pending".to_string()),
        "xai_error" => {
            let error_text = latest_decision
                .get("error_text")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_lowercase();
            if error_text.contains("credits") || error_text.contains("spending limit") {
                ("pill bad", "Decisions: xAI out of credits".to_string())
            } else {
                ("pill bad", "Decisions: xAI error".to_string())
            }
        }
        "" => ("pill", "Decisions: None yet".to_string()),
        other => ("pill bad", format!("Decisions: {other}")),
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
    fn waterfall_technical_gate_explains_final_verified_sell_signal() {
        let row = json!({"action": "SELL", "gate_code": "technical"});
        let final_technical = json!({
            "status": "ok",
            "sentiment": "HOLD",
            "trend_bias": "neutral",
            "confluence_count": 1,
            "min_confluences": 3,
        });

        assert_eq!(
            candidate_technical_label(&final_technical),
            "HOLD / neutral / 1/3"
        );
        assert_eq!(
            candidate_gate_detail(&row, &final_technical, true),
            "Final HOLD/neutral; SELL needs SELL or UNDERWEIGHT, or a bearish trend."
        );
    }

    #[test]
    fn waterfall_technical_gate_labels_legacy_runs_without_final_snapshot() {
        let row = json!({"action": "BUY", "gate_code": "technical"});
        assert_eq!(
            candidate_gate_detail(&row, &JsonValue::Null, false),
            "Final technical snapshot was not recorded for this legacy run."
        );
    }

    #[test]
    fn execution_order_price_currency_uses_broker_payload_before_symbol_fallback() {
        let row = json!({
            "symbol": "AMD:xnas",
            "currency": "",
            "execution_result_json": {
                "broker_sync": {
                    "broker_payload": {
                        "DisplayAndFormat": {"Currency": "USD"}
                    }
                }
            }
        });
        assert_eq!(execution_order_price_currency(&row), "USD");
    }

    #[test]
    fn execution_order_price_currency_falls_back_to_symbol_exchange() {
        assert_eq!(
            execution_order_price_currency(&json!({"symbol": "AMD:xnas", "currency": ""})),
            "USD"
        );
        assert_eq!(
            execution_order_price_currency(&json!({"symbol": "ORSTED:xcse", "currency": ""})),
            "DKK"
        );
    }

    #[test]
    fn hermes_pending_experiment_age_warns_after_review_threshold() {
        let now = DateTime::parse_from_rfc3339("2026-07-09T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            hermes_experiment_age_status("pending_review", "2026-06-16T12:00:00Z", now, 14),
            ("23d".to_string(), "warn-text")
        );
    }

    #[test]
    fn hermes_non_pending_experiment_age_stays_neutral() {
        let now = DateTime::parse_from_rfc3339("2026-07-09T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            hermes_experiment_age_status("approved_paper", "2026-06-16T12:00:00Z", now, 14),
            ("23d".to_string(), "muted")
        );
    }

    #[test]
    fn position_decision_age_marks_old_recommendations_stale() {
        let now = DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            position_decision_age_status("2026-05-08T12:00:00Z", now, 7),
            ("67d".to_string(), true)
        );
        assert_eq!(
            position_decision_age_status("2026-07-13T12:00:00Z", now, 7),
            ("1d".to_string(), false)
        );
    }

    #[test]
    fn position_decision_age_fails_closed_for_missing_timestamp() {
        let now = DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            position_decision_age_status("", now, 7),
            ("undated".to_string(), true)
        );
    }

    #[test]
    fn execution_order_lifecycle_formats_day_order_expiry() {
        let prefs = default_prefs();
        let row = json!({
            "order_duration_type": "DayOrder",
            "expected_expiry_at_utc": "2026-07-09T19:45:00Z",
            "expected_expiry_market": "New York Stock Exchange",
            "lifecycle_note": "DayOrder remains live until broker fill, cancel, reject, or exchange-day expiry sync."
        });

        assert!(execution_order_lifecycle_label(&row, &prefs).contains("2026"));
        let detail = execution_order_lifecycle_detail(&row, &prefs);
        assert!(detail.contains("duration DayOrder"));
        assert!(detail.contains("New York Stock Exchange"));
        assert!(detail.contains("DayOrder remains live"));
    }

    #[test]
    fn execution_status_tooltip_includes_broker_visibility() {
        let row = json!({
            "status": "broker_working",
            "broker_order_id": "5039132483",
            "execution_result_json": {
                "broker_sync": {
                    "broker_visibility": "activity_only",
                    "broker_visibility_note": "Saxo open-order lookup returned no active order; using latest audit activity as broker status fallback."
                }
            }
        });

        let tooltip = execution_status_tooltip(&row, "Broker working", "");
        assert!(tooltip.contains("broker visibility: activity_only"));
        assert!(tooltip.contains("audit activity"));
    }

    #[test]
    fn execution_status_reason_flags_expiry_pending_sync() {
        let row = json!({
            "status": "broker_working",
            "lifecycle_state": "expiry_pending_broker_sync",
            "expected_expiry_at_utc": "2026-07-09T20:00:00Z"
        });

        assert_eq!(execution_status_reason(&row), "Expiry sync pending");
        let tooltip = execution_status_tooltip(&row, "Expiry sync pending", "");
        assert!(tooltip.contains("lifecycle state: expiry_pending_broker_sync"));
    }

    #[test]
    fn execution_operation_health_warns_on_expiry_pending_sync() {
        let item = execution_operation_health(&[json!({
            "status": "broker_working",
            "lifecycle_state": "expiry_pending_broker_sync"
        })]);

        assert_eq!(item.label, "Execution");
        assert_eq!(item.status, "expiry sync");
        assert_eq!(item.tone, "warn");
        assert!(item.detail.contains("broker sync confirmation"));
    }

    #[test]
    fn quote_operation_health_surfaces_closed_market_pause() {
        let item = quote_operation_health(
            &[],
            &json!({
                "price_monitor": {
                    "status": "market_closed",
                    "summary_json": {
                        "skipped_closed": 1,
                        "skipped_closed_symbols": [
                            {"symbol": "NOVOb:xcse", "exchange": "XCSE"}
                        ]
                    }
                }
            }),
            Utc::now(),
        );

        assert_eq!(item.label, "Quotes");
        assert_eq!(item.status, "closed");
        assert_eq!(item.tone, "good");
        assert!(item.detail.contains("NOVOb:xcse"));
    }

    #[test]
    fn price_monitor_label_counts_skipped_symbols() {
        let monitor = json!({
            "status": "market_closed",
            "summary_json": {
                "skipped_closed": 2,
                "skipped_closed_symbols": [
                    {"symbol": "A:xnys", "exchange": "XNYS"},
                    {"symbol": "B:xnas", "exchange": "XNAS"}
                ]
            }
        });

        assert_eq!(price_monitor_status_label(&monitor), "closed · 2 skipped");
        assert_eq!(
            price_monitor_skipped_symbols(&price_monitor_summary(&monitor), 1),
            "A:xnys (XNYS), +1 more"
        );
    }

    #[test]
    fn execution_attribution_label_summarizes_hermes_delta() {
        let row = json!({"attribution": {"delta": "allowed_executed"}});
        assert_eq!(
            execution_attribution_label(&row),
            ("Hermes allow".to_string(), "good-text")
        );

        let review_row = json!({"attribution": {"delta": "manager_overrode_review"}});
        assert_eq!(
            execution_attribution_label(&review_row),
            ("Review overrode".to_string(), "bad-text")
        );
    }

    #[test]
    fn classifies_execution_precheck_errors_from_order_detail() {
        let row = json!({
            "status": "execution_failed",
            "error_text": "Order precheck failed: Limit price is outside allowed range: Saxo refused the order"
        });

        assert_eq!(
            execution_status_reason(&row),
            "Limit price is outside allowed range"
        );
        assert!(
            execution_status_tooltip(
                &row,
                &execution_status_reason(&row),
                &execution_status_detail(&row)
            )
            .contains("Limit price")
        );
    }

    #[test]
    fn classifies_execution_broker_working_as_pending_broker_state() {
        let row = json!({
            "status": "broker_working",
            "broker_order_id": "5038883136",
            "execution_result_json": {
                "broker_sync": {
                    "broker_payload": {
                        "OrderId": "5038883136"
                    }
                }
            }
        });

        assert_eq!(execution_status_reason(&row), "Broker working");
        assert_eq!(
            execution_reason_class("Broker working"),
            "status good-status"
        );
        assert!(
            execution_status_tooltip(&row, &execution_status_reason(&row), "")
                .contains("waiting for broker status")
        );
    }

    #[test]
    fn classifies_broker_expired_as_unfilled_day_order() {
        let row = json!({
            "status": "broker_expired",
            "broker_order_id": "5038961909",
            "error_text": "Expired"
        });

        assert_eq!(execution_status_reason(&row), "Expired unfilled");
        assert_eq!(
            execution_status_class("broker_expired"),
            "status bad-status"
        );
        assert!(
            execution_status_tooltip(
                &row,
                &execution_status_reason(&row),
                &execution_status_detail(&row)
            )
            .contains("Expired unfilled")
        );
    }

    #[test]
    fn classifies_unknown_broker_placement_as_a_retry_blocking_warning() {
        let row = json!({
            "status": "broker_state_unknown",
            "error_text": "Saxo order placement outcome is unknown; automatic retry is blocked pending broker reconciliation: Order placement failed: TradeNotCompleted"
        });

        assert_eq!(execution_status_reason(&row), "Broker state unknown");
        assert_eq!(
            execution_status_class("broker_state_unknown"),
            "status warn-status"
        );
        assert_eq!(
            execution_reason_class(&execution_status_reason(&row)),
            "status warn-status"
        );
        assert!(
            execution_status_tooltip(
                &row,
                &execution_status_reason(&row),
                &execution_status_detail(&row)
            )
            .contains("automatic retry is blocked")
        );
    }

    #[test]
    fn extracts_execution_error_detail_from_nested_payload() {
        let row = json!({
            "status": "execution_failed",
            "execution_result_json": {
                "precheck": {
                    "ErrorInfo": {
                        "Message": "Insufficient cash for requested buy order"
                    }
                }
            }
        });

        assert_eq!(
            execution_status_detail(&row),
            "Insufficient cash for requested buy order"
        );
        assert_eq!(execution_status_reason(&row), "Insufficient cash");
    }

    #[test]
    fn execution_status_prefers_persisted_saxo_taxonomy_and_remediation() {
        let row = json!({
            "status": "execution_failed",
            "error_text": "Order precheck failed: limit price outside allowed range",
            "execution_result_json": {
                "error_taxonomy": {
                    "code": "tick_size",
                    "label": "Invalid tick size",
                    "remediation": "Recalculate the limit price using Saxo's instrument tick scheme.",
                    "retry_policy": "review_and_resubmit"
                }
            }
        });

        assert_eq!(execution_status_reason(&row), "Invalid tick size");
        let tooltip = execution_status_tooltip(
            &row,
            &execution_status_reason(&row),
            &execution_status_detail(&row),
        );
        assert!(tooltip.contains("category: tick_size"));
        assert!(tooltip.contains("next step: Recalculate the limit price"));
        assert!(tooltip.contains("retry: review_and_resubmit"));
    }

    #[test]
    fn classifies_execution_event_errors() {
        let row = json!({
            "execution_order_id": 119,
            "event_type": "broker_sync",
            "status": "execution_failed",
            "message": "Saxo placement failed",
            "error_text": "HTTP 401 Unauthorized while placing order"
        });

        assert_eq!(execution_event_reason(&row), "Saxo auth");
        assert!(
            execution_event_tooltip(
                &row,
                "execution_failed",
                "Saxo auth",
                &execution_event_detail(&row)
            )
            .contains("order: 119")
        );
    }

    #[test]
    fn execution_event_detail_does_not_surface_broker_identifiers() {
        let row = json!({
            "execution_order_id": 137,
            "event_type": "broker_final_fill",
            "status": "FinalFill",
            "raw_payload_json": {
                "AccountId": "22109870",
                "ClientId": "22109870",
                "UserId": "22109870",
                "Status": "FinalFill"
            }
        });

        assert_eq!(execution_event_detail(&row), "");

        let sanitized = sanitize_diagnostic_json(row.get("raw_payload_json").unwrap());
        assert!(sanitized.get("AccountId").is_none());
        assert!(sanitized.get("ClientId").is_none());
        assert!(sanitized.get("UserId").is_none());
        assert_eq!(
            sanitized.get("Status").and_then(JsonValue::as_str),
            Some("FinalFill")
        );
    }

    #[test]
    fn derives_saxo_operation_health_from_reauth_state() {
        let item = saxo_operation_health(&json!({
            "connected": false,
            "status": "needs_reauth",
            "status_text": "Saxo session expired. Re-authentication is required."
        }));

        assert_eq!(item.label, "Saxo");
        assert_eq!(item.status, "reauth");
        assert_eq!(item.tone, "bad");
        assert!(item.detail.contains("Re-authentication"));
    }

    #[test]
    fn integrity_operation_health_warns_on_expiry_pending_orders() {
        let item = integrity_operation_health(&json!({
            "healthy": false,
            "warnings": [{
                "code": "day_order_expiry_sync_pending",
                "severity": "warning",
                "message": "One or more Saxo DayOrders passed expected exchange-calendar expiry."
            }],
            "mismatches": [],
            "expiry_pending_orders": [{"id": 204, "symbol": "BAC:xnys"}],
            "checked_at": "2026-07-09T20:15:00Z"
        }));

        assert_eq!(item.label, "Integrity");
        assert_eq!(item.status, "expiry sync");
        assert_eq!(item.tone, "warn");
        assert!(item.detail.contains("DayOrder"));
    }

    #[test]
    fn integrity_summary_mentions_acknowledged_issues() {
        let summary = integrity_summary(
            &json!({
                "healthy": false,
                "mismatches": [{
                    "code": "portfolio_identity_mismatch",
                    "severity": "error",
                    "acknowledged": true
                }],
                "warnings": [],
                "acknowledged_issue_count": 1,
                "checked_at": "2026-07-10T06:00:00Z"
            }),
            &default_prefs(),
        );

        assert_eq!(summary.0, "bad-status");
        assert_eq!(summary.1, "1 error");
        assert!(summary.2.contains("1 acknowledged"));
    }

    #[test]
    fn flags_stale_scheduler_heartbeat() {
        let now = DateTime::parse_from_rfc3339("2026-06-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let item = scheduler_operation_health(
            &json!({
                "summary": {
                    "last_heartbeat_at": "2026-06-24T10:45:00Z",
                    "last_cycle_status": "ok"
                }
            }),
            now,
        );

        assert_eq!(item.status, "stale");
        assert_eq!(item.tone, "bad");
        assert!(item.detail.contains("75 min"));
    }

    #[test]
    fn flags_partial_and_stale_runtime_runs() {
        let now = DateTime::parse_from_rfc3339("2026-06-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let partial = run_operation_health(
            "Markov",
            &json!({
                "run_date": "2026-06-24",
                "status": "completed",
                "success_count": 10,
                "error_count": 2
            }),
            &json!({
                "enabled": true,
                "timezone": "Europe/Copenhagen",
                "daily_time": "10:00",
                "run_weekdays_only": true,
            }),
            now,
        );
        assert_eq!(partial.status, "partial");
        assert_eq!(partial.tone, "warn");

        let stale = run_operation_health(
            "Indicators",
            &json!({
                "run_date": "2026-06-23",
                "status": "completed",
                "success_count": 12,
                "error_count": 0
            }),
            &json!({
                "enabled": true,
                "timezone": "Europe/Copenhagen",
                "daily_time": "10:00",
                "run_weekdays_only": true,
            }),
            now,
        );
        assert_eq!(stale.status, "stale");
        assert_eq!(stale.tone, "warn");
    }

    #[test]
    fn weekday_only_run_is_neutral_during_the_weekend() {
        let now = DateTime::parse_from_rfc3339("2026-07-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let item = run_operation_health(
            "Markov",
            &json!({
                "run_date": "2026-07-10",
                "status": "completed",
                "success_count": 20,
                "error_count": 0
            }),
            &json!({
                "enabled": true,
                "timezone": "Europe/Copenhagen",
                "daily_time": "23:30",
                "run_weekdays_only": true,
            }),
            now,
        );

        assert_eq!(item.status, "idle (weekend)");
        assert_eq!(item.tone, "neutral");
        assert!(item.detail.contains("No weekday run is due"));
    }

    #[test]
    fn weekday_only_run_is_stale_after_its_due_time() {
        let now = DateTime::parse_from_rfc3339("2026-07-14T22:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let item = run_operation_health(
            "Quiver",
            &json!({
                "run_date": "2026-07-11",
                "status": "completed",
                "success_count": 20,
                "error_count": 0
            }),
            &json!({
                "enabled": true,
                "timezone": "Europe/Copenhagen",
                "daily_time": "21:00",
                "run_weekdays_only": true,
            }),
            now,
        );

        assert_eq!(item.status, "stale");
        assert_eq!(item.tone, "warn");
    }

    #[test]
    fn derives_quote_operation_health_from_latest_position_quote() {
        let now = DateTime::parse_from_rfc3339("2026-06-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let positions = vec![
            json!({"symbol": "AMD:xnas", "latest_quote_updated_at": "2026-06-22T12:00:00Z"}),
            json!({"symbol": "BAC:xnys", "latest_quote_updated_at": "2026-06-24T11:55:00Z"}),
        ];

        let item = quote_operation_health(&positions, &json!({}), now);
        assert_eq!(item.status, "fresh");
        assert_eq!(item.tone, "good");
        assert!(item.detail.contains("5 min"));
    }

    #[test]
    fn summarizes_received_hermes_advice_actions() {
        let row = json!({
            "advice_id": "hermes-decision-advice-1",
            "advice_status": "received",
            "advice_recommendation": "proceed",
            "order_advice_json": [
                {"symbol": "AMD:xnas", "action": "allow", "reason": "fresh Markov"},
                {"symbol": "ARM:xnas", "action": "reduce", "reason": "volatile"}
            ],
            "queued_order_count": 2,
            "executed_order_count": 1,
            "failed_order_count": 0,
            "manager_json": {
                "hermes_decision_advice": {
                    "mode": "conservative",
                    "status": "received",
                    "context_self_check": {
                        "complete": true,
                        "missing": []
                    }
                }
            }
        });

        assert_eq!(hermes_advice_status_label(&row), "received");
        assert_eq!(hermes_advice_status_tone("received"), "good-status");
        assert_eq!(
            hermes_advice_order_counts(&row),
            (
                "2 items".to_string(),
                "allow: 1, reduce: 1, stand_down: 0, review: 0; queued: 2, executed: 1, failed: 0"
                    .to_string()
            )
        );
        assert_eq!(
            hermes_advice_impact(&row),
            ("restricted 1".to_string(), "warn-text")
        );
    }

    #[test]
    fn flags_conservative_hermes_timeout_as_context_review_gate() {
        let row = json!({
            "advice_recommendation": "",
            "order_advice_json": [],
            "manager_json": {
                "hermes_decision_advice": {
                    "mode": "conservative",
                    "status": "timeout"
                }
            }
        });

        assert_eq!(hermes_advice_status_label(&row), "timeout");
        assert_eq!(
            hermes_advice_impact(&row),
            ("context review gate".to_string(), "warn-text")
        );
        assert!(hermes_advice_detail(&row).contains("No persisted"));
    }

    #[test]
    fn flags_incomplete_conservative_hermes_context_as_review_gate() {
        let row = json!({
            "advice_recommendation": "proceed",
            "order_advice_json": [{"action": "allow"}],
            "advice_raw_payload_json": {
                "context_self_check": {
                    "complete": false,
                    "missing": ["current_positions"]
                }
            },
            "manager_json": {
                "hermes_decision_advice": {
                    "mode": "conservative",
                    "status": "received"
                }
            }
        });

        assert_eq!(
            hermes_advice_impact(&row),
            ("context review gate".to_string(), "warn-text")
        );
        assert!(hermes_advice_impact_detail(&row).contains("current_positions"));
    }

    #[test]
    fn summarizes_cash_deployment_from_latest_manager_run() {
        let latest_run = json!({
            "created_at": "2026-06-25T14:50:00Z",
            "report_id": 123,
            "status": "completed",
            "manager_json": {
                "reinvestment_diagnostics": {
                    "status": "excess_cash_with_blocked_buy_candidates",
                    "description": "Cash is above policy, but BUY candidates were blocked.",
                    "buy_candidate_count": 4,
                    "approved_buy_count": 0,
                    "skipped_buy_count": 4,
                    "capital_budget": {
                        "available_buy_budget_dkk": 12500.0,
                        "excess_cash_pct": 0.08
                    }
                }
            }
        });

        let summary = cash_deployment_summary(&latest_run, &default_prefs());
        assert_eq!(summary.status, "excess_cash_with_blocked_buy_candidates");
        assert_eq!(summary.tone, "warn-status");
        assert!(summary.run_label.contains("Report #123"));
        assert_eq!(summary.available_buy_budget_dkk, 12500.0);
        assert_eq!(summary.excess_cash_pct, 0.08);
        assert_eq!(summary.candidate_buy_count, 4);
        assert_eq!(summary.approved_buy_count, 0);
        assert_eq!(summary.skipped_buy_count, 4);
        assert!(summary.description.contains("blocked"));
    }

    #[test]
    fn summarizes_monthly_loss_breaker_override_from_manager_run() {
        let latest_run = json!({
            "created_at": "2026-07-10T08:00:00Z",
            "status": "completed_no_orders",
            "manager_json": {
                "monthly_loss_circuit_breaker": {
                    "active": false,
                    "threshold_breached": true,
                    "month_pnl_dkk": -12000.0,
                    "threshold_dkk": -10000.0,
                    "override_active": true,
                    "override": {
                        "enabled": true,
                        "month_key": "2026-07",
                        "updated_at": "2026-07-10T07:55:00Z"
                    }
                }
            }
        });

        let summary = cash_deployment_summary(&latest_run, &default_prefs());
        assert!(summary.breaker_threshold_breached);
        assert!(!summary.breaker_active);
        assert!(summary.breaker_override_active);
        assert_eq!(summary.breaker_override_month_key, "2026-07");
        assert_eq!(summary.breaker_month_pnl_dkk, -12000.0);
        assert_eq!(summary.breaker_threshold_dkk, -10000.0);
    }

    #[test]
    fn summarizes_monthly_loss_soft_reduction_from_manager_run() {
        let latest_run = json!({
            "created_at": "2026-07-21T08:00:00Z",
            "status": "completed_no_orders",
            "manager_json": {
                "monthly_loss_circuit_breaker": {
                    "active": false,
                    "threshold_breached": false,
                    "month_pnl_dkk": -30000.0,
                    "threshold_dkk": -50000.0,
                    "soft_threshold_dkk": -25000.0,
                    "soft_buy_multiplier": 0.5,
                    "soft_reduction_active": true,
                    "override_active": false
                }
            }
        });

        let summary = cash_deployment_summary(&latest_run, &default_prefs());
        assert!(!summary.breaker_active);
        assert!(!summary.breaker_threshold_breached);
        assert!(summary.breaker_soft_reduction_active);
        assert_eq!(summary.breaker_soft_threshold_dkk, -25000.0);
        assert_eq!(summary.breaker_soft_buy_multiplier, 0.5);
    }

    #[test]
    fn cash_deployment_tone_marks_approved_reinvestment_good() {
        assert_eq!(
            cash_deployment_tone("reinvestment_candidates_approved"),
            "good-status"
        );
        assert_eq!(cash_deployment_tone("no_reinvestment_pressure"), "");
        assert_eq!(
            cash_deployment_tone("excess_cash_without_buy_candidates"),
            "warn-status"
        );
    }

    #[test]
    fn summarizes_clear_instrument_quarantine_state() {
        let latest_run = json!({
            "manager_json": {
                "instrument_quarantine": {
                    "enabled": true,
                    "lookback_days": 14,
                    "min_failures": 3,
                    "active_days": 14,
                    "active_count": 0,
                    "active": []
                }
            }
        });

        let summary = instrument_quarantine_summary(&latest_run);
        assert!(summary.enabled);
        assert_eq!(summary.status, "clear");
        assert_eq!(summary.tone, "good-status");
        assert_eq!(summary.active_count, 0);
        assert!(summary.active.is_empty());
    }

    #[test]
    fn summarizes_active_instrument_quarantines() {
        let latest_run = json!({
            "manager_json": {
                "instrument_quarantine": {
                    "enabled": true,
                    "lookback_days": 14,
                    "min_failures": 3,
                    "active_days": 14,
                    "active": [{
                        "symbol": "ARKK:xmil",
                        "action": "BUY",
                        "signature": "commission_not_configured",
                        "failure_count": 3,
                        "expires_at": "2026-07-22T10:00:00Z"
                    }]
                }
            }
        });

        let summary = instrument_quarantine_summary(&latest_run);
        assert_eq!(summary.status, "active");
        assert_eq!(summary.tone, "warn-status");
        assert_eq!(summary.active_count, 1);
        assert_eq!(text(&summary.active[0], "symbol"), "ARKK:xmil");
        assert!(summary.description.contains("blocked"));
    }

    #[test]
    fn summarizes_overridden_instrument_quarantines() {
        let latest_run = json!({
            "manager_json": {
                "instrument_quarantine": {
                    "enabled": true,
                    "lookback_days": 14,
                    "min_failures": 3,
                    "active_days": 14,
                    "blocked_count": 0,
                    "override_count": 1,
                    "active": [{
                        "symbol": "ARKK:xmil",
                        "action": "BUY",
                        "signature": "commission_not_configured",
                        "failure_count": 3,
                        "override_active": true,
                        "override_notes": "operator verified",
                        "expires_at": "2026-07-22T10:00:00Z"
                    }]
                }
            }
        });

        let summary = instrument_quarantine_summary(&latest_run);
        assert_eq!(summary.status, "overridden");
        assert_eq!(summary.blocked_count, 0);
        assert_eq!(summary.override_count, 1);
        assert!(summary.description.contains("operator overrides"));
    }

    #[test]
    fn derives_decision_health_pill_from_latest_report() {
        let (class, label) = decision_health(&json!({"status": "completed"}));
        assert_eq!(class, "pill good");
        assert_eq!(label, "Decisions: OK");

        let (class, label) = decision_health(&json!({"status": "xai_fallback"}));
        assert_eq!(class, "pill good");
        assert_eq!(label, "Decisions: OK");

        let (class, label) = decision_health(&json!({
            "status": "xai_error",
            "error_text": "xAI deferred submit failed with HTTP 403 Forbidden: {\"code\":\"permission-denied\",\"error\":\"Your team has either used all available credits or reached its monthly spending limit.\"}"
        }));
        assert_eq!(class, "pill bad");
        assert_eq!(label, "Decisions: xAI out of credits");

        let (class, label) = decision_health(&json!({
            "status": "xai_error",
            "error_text": "xAI deferred submit failed with HTTP 500"
        }));
        assert_eq!(class, "pill bad");
        assert_eq!(label, "Decisions: xAI error");

        let (class, label) = decision_health(&json!({"status": "pending"}));
        assert_eq!(class, "pill");
        assert_eq!(label, "Decisions: Pending");

        let (class, label) = decision_health(&json!({"status": "xai_deferred"}));
        assert_eq!(class, "pill");
        assert_eq!(label, "Decisions: Pending");

        let (class, label) = decision_health(&JsonValue::Null);
        assert_eq!(class, "pill");
        assert_eq!(label, "Decisions: None yet");
    }

    #[test]
    fn derives_decision_pulse_health_from_recent_reports() {
        let reports = vec![
            json!({
                "id": 12,
                "created_at": "2026-06-24T14:45:00Z",
                "status": "xai_error",
                "analysis_pulse_key": "us_open_followup:2026-06-24",
            }),
            json!({
                "id": 11,
                "created_at": "2026-06-23T14:45:00Z",
                "status": "xai_fallback",
                "analysis_pulse_key": "us_open_followup:2026-06-23",
            }),
            json!({
                "id": 10,
                "created_at": "2026-06-24T08:15:00Z",
                "status": "completed",
                "analysis_pulse_key": "europe_open_followup:2026-06-24",
            }),
        ];

        let us = decision_pulse_health(&reports, "us_open_followup:", "US Open +1h15");
        assert_eq!(us.latest_id, 12);
        assert_eq!(us.latest_status, "xai_error");
        assert_eq!(us.latest_tone, "bad-text");
        assert_eq!(us.last_success_id, 11);
        assert_eq!(us.attempts_7d, 2);

        let europe =
            decision_pulse_health(&reports, "europe_open_followup:", "Nordic/EU Open +1h15");
        assert_eq!(europe.latest_id, 10);
        assert_eq!(europe.latest_status, "completed");
        assert_eq!(europe.latest_tone, "good-text");
        assert_eq!(europe.last_success_id, 10);
        assert_eq!(europe.attempts_7d, 1);

        let manual = decision_pulse_health(&reports, "manual:", "Manual / Dry Run");
        assert_eq!(manual.latest_status, "missing");
        assert_eq!(manual.latest_tone, "bad-text");
        assert_eq!(manual.last_success_id, 0);
        assert_eq!(manual.attempts_7d, 0);
    }

    #[test]
    fn derives_operation_health_from_per_pulse_report_status() {
        let statuses = vec![json!({
            "key": "us_open_followup",
            "latest": {
                "id": 77,
                "created_at": "2026-07-14T14:45:00Z",
                "status": "xai_fallback"
            },
            "last_success": {
                "id": 77,
                "created_at": "2026-07-14T14:45:00Z",
                "status": "xai_fallback"
            }
        })];

        let health = decision_pulse_operation_health(&statuses, "us_open_followup", "US Report");
        assert_eq!(health.label, "US Report");
        assert_eq!(health.status, "ok");
        assert_eq!(health.tone, "good");
        assert!(health.detail.contains("Last success #77"));

        let missing =
            decision_pulse_operation_health(&statuses, "europe_open_followup", "EU Report");
        assert_eq!(missing.status, "unknown");
        assert_eq!(missing.tone, "warn");
    }

    #[test]
    fn derives_decision_pulse_health_from_backend_status() {
        let statuses = vec![json!({
            "key": "us_open_followup",
            "label": "US Open +1h15",
            "latest": {
                "id": 12,
                "created_at": "2026-06-24T14:45:00Z",
                "status": "xai_error"
            },
            "last_success": {
                "id": 11,
                "created_at": "2026-06-23T14:45:00Z",
                "status": "completed"
            },
            "last_failure": {
                "id": 12,
                "created_at": "2026-06-24T14:45:00Z",
                "status": "xai_error"
            },
            "attempts_7d": 4
        })];

        let us = decision_pulse_health_from_status(&statuses, "us_open_followup")
            .expect("backend status exists");
        assert_eq!(us.latest_id, 12);
        assert_eq!(us.latest_status, "xai_error");
        assert_eq!(us.latest_tone, "bad-text");
        assert_eq!(us.last_success_id, 11);
        assert_eq!(us.last_failure_id, 12);
        assert_eq!(us.last_failure_status, "xai_error");
        assert_eq!(us.attempts_7d, 4);
        assert!(decision_pulse_health_from_status(&statuses, "manual").is_none());
    }

    #[test]
    fn derives_decision_report_diagnostics_from_schema_and_error() {
        let report = json!({
            "model": "openai/gpt-5.5",
            "response_id": "gen-123",
            "error_text": null,
            "request_json": {
                "model": "openai/gpt-5.5",
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "strict": true,
                        "schema": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "capital_plan": {
                                    "type": "object",
                                    "additionalProperties": false
                                }
                            }
                        }
                    }
                }
            },
            "response_json": {"id": "gen-123"}
        });

        let diagnostics = decision_report_diagnostics(&report);
        assert_eq!(diagnostics.provider, "openrouter/json_schema");
        assert_eq!(diagnostics.response_format, "json_schema");
        assert_eq!(diagnostics.strict_schema, "true");
        assert_eq!(diagnostics.root_object, "strict");
        assert_eq!(diagnostics.capital_plan_object, "strict");
        assert_eq!(diagnostics.schema_status, "strict");
        assert_eq!(diagnostics.schema_tone, "good-text");
        assert_eq!(diagnostics.response_present, "yes");
        assert_eq!(diagnostics.error_category, "none");
    }

    #[test]
    fn flags_decision_report_diagnostics_schema_errors() {
        let report = json!({
            "model": "openai/gpt-5.5",
            "error_text": "Invalid schema for response_format 'daytrader_decision_report': In context=('properties', 'capital_plan'), 'additionalProperties' is required to be supplied and to be false.",
            "request_json": {
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "strict": true,
                        "schema": {
                            "type": "object",
                            "additionalProperties": true,
                            "properties": {
                                "capital_plan": {
                                    "type": "object",
                                    "additionalProperties": true
                                }
                            }
                        }
                    }
                }
            }
        });

        let diagnostics = decision_report_diagnostics(&report);
        assert_eq!(diagnostics.root_object, "open");
        assert_eq!(diagnostics.capital_plan_object, "open");
        assert_eq!(diagnostics.schema_status, "needs review");
        assert_eq!(diagnostics.schema_tone, "bad-text");
        assert_eq!(diagnostics.response_present, "no");
        assert_eq!(diagnostics.error_category, "schema");
        assert!(diagnostics.error_excerpt.contains("Invalid schema"));
    }

    #[test]
    fn scores_complete_decision_report_quality_as_ready() {
        let report = json!({"status": "completed"});
        let report_json = json!({
            "market_view": {"bias": "risk-on", "summary": "ok"},
            "capital_plan": {"available_buy_budget_dkk": 1000},
            "selected_assets": [{"symbol": "AMD:xnas", "score": 0.8, "notes": "ok"}],
            "symbol_sentiment": [{"symbol": "AMD:xnas", "sentiment": "BUY", "confidence": 0.7, "rationale": "ok"}],
            "suggested_trades": [{
                "symbol": "AMD:xnas",
                "action": "BUY",
                "quantity": 1,
                "order_type": "Market",
                "estimated_value_dkk": 900,
                "strategy_key": "amd-open"
            }],
            "market_scope_enforcement": {"status": "not_required"}
        });
        let diagnostics = DecisionReportDiagnostics {
            provider: "openrouter/json_schema".to_string(),
            model: "openai/gpt-5.5".to_string(),
            response_format: "json_schema".to_string(),
            strict_schema: "true".to_string(),
            root_object: "strict".to_string(),
            capital_plan_object: "strict".to_string(),
            schema_status: "strict".to_string(),
            schema_tone: "good-text",
            request_bytes: 100,
            response_id: "gen-1".to_string(),
            response_present: "yes".to_string(),
            error_category: "none".to_string(),
            error_excerpt: "No error recorded.".to_string(),
        };

        let quality = decision_report_quality(&report, &report_json, &diagnostics);
        assert_eq!(quality.score, 100);
        assert_eq!(quality.tone, "good-text");
        assert_eq!(quality.status_label, "ready");
        assert!(quality.warnings.is_empty());
    }

    #[test]
    fn decision_report_quality_warns_on_bad_shape_and_scope_filtering() {
        let report = json!({"status": "completed"});
        let report_json = json!({
            "market_view": {"bias": "risk-on", "summary": "ok"},
            "capital_plan": {"available_buy_budget_dkk": 1000},
            "selected_assets": [],
            "symbol_sentiment": [],
            "suggested_trades": [{
                "symbol": "AMD:xnas",
                "action": "BUY",
                "quantity": 1,
                "order_type": "Limit",
                "limit_price_local": null,
                "estimated_value_dkk": 900,
                "strategy_key": "amd-open"
            }],
            "market_scope_enforcement": {
                "status": "enforced",
                "filtered_out_symbols": ["ORSTED:xcse"]
            }
        });
        let diagnostics = DecisionReportDiagnostics {
            provider: "openrouter/json_schema".to_string(),
            model: "openai/gpt-5.5".to_string(),
            response_format: "json_schema".to_string(),
            strict_schema: "true".to_string(),
            root_object: "strict".to_string(),
            capital_plan_object: "strict".to_string(),
            schema_status: "strict".to_string(),
            schema_tone: "good-text",
            request_bytes: 100,
            response_id: "gen-1".to_string(),
            response_present: "yes".to_string(),
            error_category: "none".to_string(),
            error_excerpt: "No error recorded.".to_string(),
        };

        let quality = decision_report_quality(&report, &report_json, &diagnostics);
        assert_eq!(quality.score, 90);
        assert_eq!(quality.status_label, "ready with notes");
        assert!(
            quality
                .warnings
                .iter()
                .any(|warning| warning.contains("incomplete order shape"))
        );
        assert!(
            quality
                .warnings
                .iter()
                .any(|warning| warning.contains("filtered 1 symbol"))
        );
    }

    #[test]
    fn redacts_sensitive_debug_json_fields() {
        let payload = json!({
            "model": "openrouter/fusion",
            "api_key": "sk-test-123456789012345678901234567890",
            "nested": {
                "refresh_token": "refresh-123456789012345678901234567890",
                "messages": [
                    {"content": "Use bearer token Bearer abcdef1234567890abcdef1234567890abcd"}
                ]
            }
        });

        let rendered = compact_json_redacted(Some(&payload), 8_000);
        assert!(rendered.contains("openrouter/fusion"));
        assert!(rendered.contains("\"api_key\": \"[redacted]\""));
        assert!(rendered.contains("\"refresh_token\": \"[redacted]\""));
        assert!(!rendered.contains("sk-test-123456789012345678901234567890"));
        assert!(!rendered.contains("abcdef1234567890abcdef1234567890abcd"));
    }

    #[test]
    fn builds_sanitized_decision_report_debug_payload() {
        let report_json = json!({"suggested_trades": []});
        let report = json!({
            "prompt_text": "Prompt with token sk-live-123456789012345678901234567890",
            "request_json": {
                "model": "openai/gpt-5.5",
                "Authorization": "Bearer abcdef1234567890abcdef1234567890abcd"
            },
            "response_json": {
                "id": "gen-123",
                "client_key": "client-123456789012345678901234567890"
            }
        });

        let debug = decision_report_debug_payload(&report, &report_json);
        assert!(debug.prompt.contains("[redacted]"));
        assert!(debug.request.contains("\"Authorization\": \"[redacted]\""));
        assert!(debug.response.contains("\"client_key\": \"[redacted]\""));
        assert!(debug.normalized.contains("suggested_trades"));
    }

    #[test]
    fn extracts_display_text_from_json() {
        let value = json!({"symbol": "AAPL:xnas", "quantity": 12});
        assert_eq!(text(&value, "symbol"), "AAPL:xnas");
        assert_eq!(number(&value, "quantity", 0), "12");
    }

    #[test]
    fn extracts_scheduler_cycle_nested_status() {
        let row = json!({
            "cycle_json": "{\"operational_notifications\":{\"status\":\"ok\"}}"
        });
        assert_eq!(
            scheduler_cycle_json_status(&row, "operational_notifications"),
            "ok"
        );

        let invalid_row = json!({"cycle_json": "not-json"});
        assert_eq!(
            scheduler_cycle_json_status(&invalid_row, "operational_notifications"),
            "n/a"
        );
    }

    #[test]
    fn extracts_scheduler_cycle_runtime_label() {
        let row = json!({
            "cycle_json": "{\"duration_ms\":65123,\"step_durations\":{\"decision_reports\":{\"duration_ms\":60400}}}"
        });
        assert_eq!(scheduler_cycle_duration(&row), "1m 5s");
        assert_eq!(format_duration_ms(950), "950 ms");
        assert_eq!(format_duration_ms(12_345), "12.3s");

        let invalid_row = json!({"cycle_json": "not-json"});
        assert_eq!(scheduler_cycle_duration(&invalid_row), "n/a");
    }

    #[test]
    fn prefixes_root_relative_urls_for_shared_ngrok_base_path() {
        let html = r#"<a href="/api/health">Health</a><form action="/api/actions/decision-report"><form action="/api/actions/decision-report-dry-run"><input value="/?view=market" /><img src="/favicon.svg" /><a href="https://example.com">External</a>"#;
        let prefixed = prefix_root_relative_urls(html, "/saxo-daytrader");

        assert!(prefixed.contains(r#"href="/saxo-daytrader/api/health""#));
        assert!(prefixed.contains(r#"action="/saxo-daytrader/api/actions/decision-report""#));
        assert!(
            prefixed.contains(r#"action="/saxo-daytrader/api/actions/decision-report-dry-run""#)
        );
        assert!(prefixed.contains(r#"value="/saxo-daytrader/?view=market""#));
        assert!(prefixed.contains(r#"src="/saxo-daytrader/favicon.svg""#));
        assert!(prefixed.contains(r#"href="https://example.com""#));
    }
}
