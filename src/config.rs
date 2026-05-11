use std::{env, path::PathBuf};

use anyhow::Result;
use serde_yaml::Value as YamlValue;

// Keep config access small and explicit. This is similar to reading nested keys
// from a Python dict, except Rust makes the "missing key" case visible with
// `Option<T>`.
pub fn yaml_at<'a>(value: &'a YamlValue, keys: &[&str]) -> Option<&'a YamlValue> {
    let mut current = value;
    for key in keys {
        current = current.get(*key)?;
    }
    Some(current)
}

pub fn yaml_string(value: &YamlValue, keys: &[&str]) -> Option<String> {
    let raw = yaml_at(value, keys)?.as_str()?.to_string();
    if let Some(env_key) = raw.strip_prefix("ENV:") {
        env::var(env_key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    } else {
        Some(raw)
    }
}

pub fn yaml_f64(value: &YamlValue, keys: &[&str]) -> Option<f64> {
    yaml_at(value, keys)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
}

pub fn yaml_i64(value: &YamlValue, keys: &[&str]) -> Option<i64> {
    yaml_at(value, keys).and_then(YamlValue::as_i64)
}

pub fn yaml_bool(value: &YamlValue, keys: &[&str]) -> Option<bool> {
    yaml_at(value, keys).and_then(YamlValue::as_bool)
}

pub fn database_url(config: &YamlValue, config_path: &PathBuf) -> Result<String> {
    if let Ok(database_url) = env::var("DATABASE_URL") {
        if !database_url.trim().is_empty() {
            return Ok(database_url);
        }
    }
    if let Some(database_url) = yaml_string(config, &["portfolio", "database_url"]) {
        if !database_url.trim().is_empty() {
            return Ok(database_url);
        }
    }
    let database_path = yaml_string(config, &["portfolio", "database_path"])
        .unwrap_or_else(|| "ledger.db".to_string());
    let resolved = resolve_config_path(config_path, &database_path);
    Ok(format!("sqlite://{}", resolved.display()))
}

pub fn resolve_config_path(config_path: &PathBuf, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nested_yaml_values() {
        let config: YamlValue = serde_yaml::from_str(
            r#"
app:
  project_name: saxo-rust
execution:
  max_daily_orders: 50
  require_approval_live: false
"#,
        )
        .unwrap();

        assert_eq!(
            yaml_string(&config, &["app", "project_name"]).as_deref(),
            Some("saxo-rust")
        );
        assert_eq!(
            yaml_i64(&config, &["execution", "max_daily_orders"]),
            Some(50)
        );
        assert_eq!(
            yaml_bool(&config, &["execution", "require_approval_live"]),
            Some(false)
        );
    }

    #[test]
    fn resolves_relative_paths_from_config_directory() {
        let config_path = PathBuf::from("/workspace/app/config.yaml");
        assert_eq!(
            resolve_config_path(&config_path, "ledger.db"),
            PathBuf::from("/workspace/app/ledger.db")
        );
    }
}
