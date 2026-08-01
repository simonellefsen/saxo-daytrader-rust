use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

use crate::config::{yaml_i64, yaml_string};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalizationPrefs {
    pub locale: String,
    pub time_zone: String,
    pub hour_cycle: HourCycle,
    pub week_start: WeekStart,
    pub group_separator: String,
    pub decimal_separator: String,
    pub measurement_system: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HourCycle {
    H12,
    H24,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    Monday,
    Sunday,
    Saturday,
}

impl LocalizationPrefs {
    pub fn from_headers_and_config(headers: &HeaderMap, config: &YamlValue) -> Self {
        let locale = first_language(headers)
            .or_else(|| yaml_string(config, &["localization", "locale"]))
            .unwrap_or_else(|| "en-DK".to_string());
        let time_zone = header_value(headers, "x-timezone")
            .or_else(|| header_value(headers, "cf-timezone"))
            .or_else(|| yaml_string(config, &["localization", "time_zone"]))
            .or_else(|| yaml_string(config, &["price_monitor", "timezone"]))
            .unwrap_or_else(|| "Europe/Copenhagen".to_string());
        let hour_cycle = yaml_string(config, &["localization", "hour_cycle"])
            .and_then(|value| parse_hour_cycle(&value))
            .unwrap_or_else(|| default_hour_cycle(&locale));
        let week_start = yaml_string(config, &["localization", "week_start"])
            .and_then(|value| parse_week_start(&value))
            .or_else(|| {
                yaml_i64(config, &["xai", "performance_goals", "week_start_weekday"])
                    .and_then(week_start_from_python_weekday)
            })
            .unwrap_or_else(|| default_week_start(&locale));
        let (group_separator, decimal_separator) = separators_for_locale(&locale);

        Self {
            locale,
            time_zone,
            hour_cycle,
            week_start,
            group_separator,
            decimal_separator,
            measurement_system: yaml_string(config, &["localization", "measurement_system"])
                .unwrap_or_else(|| "metric".to_string()),
        }
    }

    pub fn apply_settings_json(&mut self, settings: &JsonValue) {
        // Runtime settings come from Postgres and override the request-derived
        // defaults. This keeps Kubernetes rollouts stateless while still letting
        // each operator tune formatting from the UI.
        if let Some(locale) = settings.get("locale").and_then(JsonValue::as_str) {
            if !locale.trim().is_empty() {
                self.locale = locale.trim().to_string();
            }
        }
        if let Some(time_zone) = settings.get("time_zone").and_then(JsonValue::as_str) {
            if !time_zone.trim().is_empty() {
                self.time_zone = time_zone.trim().to_string();
            }
        }
        if let Some(hour_cycle) = settings
            .get("hour_cycle")
            .and_then(JsonValue::as_str)
            .and_then(parse_hour_cycle)
        {
            self.hour_cycle = hour_cycle;
        }
        if let Some(week_start) = settings
            .get("week_start")
            .and_then(JsonValue::as_str)
            .and_then(parse_week_start)
        {
            self.week_start = week_start;
        }
        if let Some(group_separator) = settings.get("group_separator").and_then(JsonValue::as_str) {
            self.group_separator = separator_value(group_separator);
        }
        if let Some(decimal_separator) = settings
            .get("decimal_separator")
            .and_then(JsonValue::as_str)
        {
            self.decimal_separator = separator_value(decimal_separator);
        }
        if let Some(measurement_system) = settings
            .get("measurement_system")
            .and_then(JsonValue::as_str)
        {
            if !measurement_system.trim().is_empty() {
                self.measurement_system = measurement_system.trim().to_string();
            }
        }
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn first_language(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::ACCEPT_LANGUAGE)?
        .to_str()
        .ok()?
        .split(',')
        .next()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn format_money(value: f64, currency: &str, prefs: &LocalizationPrefs) -> String {
    format!(
        "{} {}",
        format_number(value, 0, prefs),
        currency.trim().to_uppercase()
    )
}

pub fn format_percent(value: f64, prefs: &LocalizationPrefs) -> String {
    let pct = if value.abs() <= 1.0 {
        value * 100.0
    } else {
        value
    };
    format!("{}%", format_number(pct, 1, prefs))
}

pub fn format_quantity(value: f64, prefs: &LocalizationPrefs) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format_number(value, 0, prefs)
    } else {
        format_number(value, 2, prefs)
    }
}

pub fn format_number(value: f64, decimals: usize, prefs: &LocalizationPrefs) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let absolute = value.abs();
    let rendered = format!("{absolute:.decimals$}");
    let (whole, fraction) = rendered
        .split_once('.')
        .map(|(whole, fraction)| (whole, Some(fraction)))
        .unwrap_or((rendered.as_str(), None));
    let grouped = group_digits(whole, &prefs.group_separator);
    match (decimals, fraction) {
        (0, _) => format!("{sign}{grouped}"),
        (_, Some(fraction)) => format!("{sign}{grouped}{}{fraction}", prefs.decimal_separator),
        _ => format!("{sign}{grouped}"),
    }
}

pub fn format_timestamp(value: &str, prefs: &LocalizationPrefs) -> String {
    let parsed = match DateTime::parse_from_rfc3339(value) {
        Ok(value) => value.with_timezone(&Utc),
        Err(_) => return value.to_string(),
    };
    let timezone = prefs.time_zone.parse::<Tz>().unwrap_or(chrono_tz::UTC);
    let local = parsed.with_timezone(&timezone);
    match prefs.hour_cycle {
        HourCycle::H12 => local.format("%Y-%m-%d %-I:%M %p").to_string(),
        HourCycle::H24 => local.format("%Y-%m-%d %H:%M").to_string(),
    }
}

fn group_digits(value: &str, separator: &str) -> String {
    let mut out = String::new();
    for (index, character) in value.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push_str(separator);
        }
        out.push(character);
    }
    out.chars().rev().collect()
}

fn separators_for_locale(locale: &str) -> (String, String) {
    let normalized = locale.to_lowercase();
    if normalized.starts_with("da")
        || normalized.starts_with("de")
        || normalized.starts_with("es")
        || normalized.starts_with("fr")
    {
        (".".to_string(), ",".to_string())
    } else {
        (",".to_string(), ".".to_string())
    }
}

fn default_hour_cycle(locale: &str) -> HourCycle {
    if locale.eq_ignore_ascii_case("en-us") {
        HourCycle::H12
    } else {
        HourCycle::H24
    }
}

fn default_week_start(locale: &str) -> WeekStart {
    if locale.eq_ignore_ascii_case("en-us") {
        WeekStart::Sunday
    } else {
        WeekStart::Monday
    }
}

fn parse_hour_cycle(value: &str) -> Option<HourCycle> {
    match value.trim().to_lowercase().as_str() {
        "12" | "12h" | "h12" => Some(HourCycle::H12),
        "24" | "24h" | "h24" => Some(HourCycle::H24),
        _ => None,
    }
}

fn parse_week_start(value: &str) -> Option<WeekStart> {
    match value.trim().to_lowercase().as_str() {
        "mon" | "monday" | "0" => Some(WeekStart::Monday),
        "sun" | "sunday" | "6" => Some(WeekStart::Sunday),
        "sat" | "saturday" => Some(WeekStart::Saturday),
        _ => None,
    }
}

fn separator_value(value: &str) -> String {
    match value {
        "space" => " ".to_string(),
        "thin_space" => "\u{202f}".to_string(),
        other => other.to_string(),
    }
}

fn week_start_from_python_weekday(value: i64) -> Option<WeekStart> {
    match value {
        0 => Some(WeekStart::Monday),
        5 => Some(WeekStart::Saturday),
        6 => Some(WeekStart::Sunday),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn formats_numbers_with_danish_separators() {
        let prefs = LocalizationPrefs {
            locale: "da-DK".to_string(),
            time_zone: "Europe/Copenhagen".to_string(),
            hour_cycle: HourCycle::H24,
            week_start: WeekStart::Monday,
            group_separator: ".".to_string(),
            decimal_separator: ",".to_string(),
            measurement_system: "metric".to_string(),
        };

        assert_eq!(format_money(351559.2, "DKK", &prefs), "351.559 DKK");
        assert_eq!(format_percent(0.0724, &prefs), "7,2%");
    }

    #[test]
    fn derives_locale_from_accept_language_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("da-DK,da;q=0.9,en-US;q=0.8"),
        );
        let config: YamlValue =
            serde_yaml::from_str("price_monitor:\n  timezone: Europe/Copenhagen\n").unwrap();

        let prefs = LocalizationPrefs::from_headers_and_config(&headers, &config);

        assert_eq!(prefs.locale, "da-DK");
        assert_eq!(prefs.week_start, WeekStart::Monday);
        assert_eq!(prefs.hour_cycle, HourCycle::H24);
    }
}
