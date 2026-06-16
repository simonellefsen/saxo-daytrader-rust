use serde_json::{Map, Value as JsonValue};
use sqlx::{Column, Row};

// sqlx returns database rows in typed form. This adapter converts a row into a
// generic JSON object so the compatibility API can keep the old Python/Next.js
// shape while the deeper domain model is still being ported.
pub fn row_to_json(row: &sqlx::any::AnyRow) -> JsonValue {
    let mut map = Map::new();
    for column in row.columns() {
        let name = column.name();
        map.insert(name.to_string(), row_value(row, name));
    }
    JsonValue::Object(map)
}

fn row_value(row: &sqlx::any::AnyRow, name: &str) -> JsonValue {
    if prefers_float_column(name) {
        if let Ok(value) = row.try_get::<Option<f64>, _>(name) {
            return value.map(JsonValue::from).unwrap_or(JsonValue::Null);
        }
        if let Ok(value) = row.try_get::<Option<f32>, _>(name) {
            return value
                .map(|value| JsonValue::from(value as f64))
                .unwrap_or(JsonValue::Null);
        }
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(name) {
        return value.map(JsonValue::from).unwrap_or(JsonValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<f64>, _>(name) {
        return value.map(JsonValue::from).unwrap_or(JsonValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<f32>, _>(name) {
        return value
            .map(|value| JsonValue::from(value as f64))
            .unwrap_or(JsonValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<bool>, _>(name) {
        return value.map(JsonValue::from).unwrap_or(JsonValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<String>, _>(name) {
        if let Some(text) = value {
            if name.ends_with("_json") {
                return serde_json::from_str(&text).unwrap_or_else(|_| JsonValue::from(text));
            }
            return JsonValue::from(text);
        }
        return JsonValue::Null;
    }
    JsonValue::Null
}

fn prefers_float_column(name: &str) -> bool {
    matches!(
        name,
        "bull_prob"
            | "sideways_prob"
            | "bear_prob"
            | "signed_signal"
            | "conviction"
            | "rolling_return"
            | "current_close"
            | "threshold"
    )
}

pub fn value_f64(value: &JsonValue, key: &str) -> f64 {
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

pub fn value_i64(value: &JsonValue, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|value| value.as_i64().or_else(|| value.as_f64().map(|v| v as i64)))
        .unwrap_or(0)
}

pub fn json_f64(map: &Map<String, JsonValue>, key: &str) -> f64 {
    map.get(key)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
        .unwrap_or(0.0)
}

pub fn json_i64(map: &Map<String, JsonValue>, key: &str) -> i64 {
    map.get(key)
        .and_then(|value| value.as_i64().or_else(|| value.as_f64().map(|v| v as i64)))
        .unwrap_or(0)
}

pub fn clamp_limit(value: i64, min: i64, max: i64) -> i64 {
    value.max(min).min(max)
}

pub fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

pub fn pct(value: f64, target: f64) -> f64 {
    if target.abs() < f64::EPSILON {
        0.0
    } else {
        value / target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clamps_query_limits() {
        assert_eq!(clamp_limit(0, 1, 250), 1);
        assert_eq!(clamp_limit(12, 1, 250), 12);
        assert_eq!(clamp_limit(999, 1, 250), 250);
    }

    #[test]
    fn escapes_single_quotes_for_inline_sql_fragments() {
        assert_eq!(sql_escape("O'Reilly"), "O''Reilly");
    }

    #[test]
    fn reads_numbers_from_json_objects() {
        let value = json!({"f": 12.5, "i": 7});
        assert_eq!(value_f64(&value, "f"), 12.5);
        assert_eq!(value_i64(&value, "i"), 7);
        assert_eq!(pct(25.0, 100.0), 0.25);
    }

    #[test]
    fn preserves_known_fractional_columns() {
        assert!(prefers_float_column("bull_prob"));
        assert!(prefers_float_column("signed_signal"));
        assert!(!prefers_float_column("sample_count"));

        let value = json!({"signed_signal": "0.63371605"});
        assert_eq!(value_f64(&value, "signed_signal"), 0.63371605);
    }
}
