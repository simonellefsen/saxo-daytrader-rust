//! Runtime settings on `AppState`: localization preferences, the cash
//! buffer, and the AI provider settings with the API-key override.
//!
//! The key override is stored as a runtime setting and only ever leaves this
//! module masked: `ai_api_key_status_value` reports whether one is set, its
//! source and a short preview, never the key. `effective_ai_api_key` is read
//! by the provider request path alone.

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value as JsonValue, json};
use tracing::warn;

use crate::{
    config::{yaml_f64, yaml_i64, yaml_string},
    db::{clamp_limit, row_to_json, sql_escape},
    decision_provider_state::ai_provider_capabilities_from_rows,
    localization::LocalizationPrefs,
    models::{AiProviderCapabilityPayload, CashBufferSettings, DashboardAiSettingsPayload},
};

use crate::state::{AppState, dashboard_ai_settings_from_json, validated_ai_model};

const RETIRED_RUNTIME_SETTING_KEYS: &[&str] = &[
    "strategy.capital.cash_buffer",
    "strategy.swing.cash_buffer_pct",
];

/// Short recognizable preview of an API key: first 6 + last 4 characters
/// for long keys, fully redacted for short ones.
pub(crate) fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() >= 16 {
        let head: String = chars[..6].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    } else {
        "•••".to_string()
    }
}

fn localization_settings_key(sso_session: &JsonValue) -> String {
    let user_key = sso_session
        .get("user")
        .and_then(|user| user.get("email"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default");
    format!("localization:{user_key}")
}

impl AppState {
    pub async fn localization_for_user(
        &self,
        mut prefs: LocalizationPrefs,
        sso_session: &JsonValue,
    ) -> LocalizationPrefs {
        let key = localization_settings_key(sso_session);
        match self.runtime_setting(&key).await {
            Ok(Some(value)) => prefs.apply_settings_json(&value),
            Ok(None) => {}
            Err(err) => warn!(key = %key, "localization settings lookup failed: {err:#}"),
        }
        prefs
    }

    pub async fn save_localization_settings(
        &self,
        sso_session: &JsonValue,
        mut value: JsonValue,
    ) -> Result<JsonValue> {
        let key = localization_settings_key(sso_session);
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "updated_at".to_string(),
                JsonValue::from(Utc::now().to_rfc3339()),
            );
        }
        self.save_runtime_setting(&key, &value).await?;
        Ok(value)
    }

    pub fn cash_buffer_settings(&self) -> CashBufferSettings {
        let min_cash_buffer_pct = yaml_f64(
            &self.config,
            &["strategy", "capital", "min_cash_buffer_pct"],
        )
        .unwrap_or(0.10);
        let max_deployment_pct =
            yaml_f64(&self.config, &["strategy", "capital", "max_deployment_pct"]).unwrap_or(0.90);
        let reinvestment_pressure_threshold_pct = yaml_f64(
            &self.config,
            &["strategy", "capital", "reinvestment_pressure_threshold_pct"],
        )
        .unwrap_or(0.05);
        CashBufferSettings {
            min_cash_buffer_pct,
            max_deployment_pct,
            reinvestment_pressure_threshold_pct,
            source: "config".to_string(),
            updated_at: None,
            config_default_min_cash_buffer_pct: min_cash_buffer_pct,
        }
    }

    pub fn cash_buffer_value(&self) -> JsonValue {
        serde_json::to_value(self.cash_buffer_settings())
            .expect("cash buffer settings must serialize")
    }

    pub(crate) fn default_ai_settings_value(&self) -> JsonValue {
        let provider = yaml_string(&self.config, &["xai", "provider"])
            .or_else(|| yaml_string(&self.config, &["xai", "inference_provider"]))
            .unwrap_or_else(|| "openrouter".to_string());
        let config_model = yaml_string(&self.config, &["xai", "model"]).unwrap_or_else(|| {
            if provider == "openrouter" {
                "openai/gpt-5.5".to_string()
            } else {
                "grok-4.3".to_string()
            }
        });
        json!({
            "provider": provider,
            "model": config_model,
            "config_model": config_model,
            "source": "config",
            "updated_at": null,
            "recent_models": []
        })
    }

    /// Models that have actually produced a Decision Report, newest use first.
    ///
    /// The settings menu offered four hardcoded slugs that had drifted out of
    /// date -- none of them was the model that had run the last 93 reports --
    /// so the picker listed choices nobody had made while hiding the ones they
    /// had. `decision_reports.model` already records what ran, which is a
    /// better answer than a list of what someone typed: a model that was tried
    /// and abandoned still appears, and getting back to it costs no retyping.
    ///
    /// Read-only, and deliberately not a validation surface: appearing here
    /// means a report was submitted under that model, not that it still exists
    /// upstream or is a good idea today.
    pub(crate) async fn recently_used_ai_models(&self, limit: i64) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT model
             FROM decision_reports
             WHERE model IS NOT NULL AND TRIM(model) <> ''
             GROUP BY model
             ORDER BY MAX(created_at) DESC
             LIMIT {}",
            clamp_limit(limit, 1, 50)
        );
        let rows = sqlx::query(&sql)
            .fetch_all(&self.pool)
            .await
            .context("reading recently used AI models")?;
        Ok(rows
            .iter()
            .map(row_to_json)
            .filter_map(|row| {
                row.get("model")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
            })
            .collect())
    }

    pub(crate) fn default_dashboard_ai_settings(&self) -> DashboardAiSettingsPayload {
        let mut value = self.default_ai_settings_value();
        value
            .as_object_mut()
            .expect("default AI settings must be an object")
            .insert(
                "api_key".to_string(),
                json!({
                    "configured": false,
                    "source": "missing",
                    "masked": null,
                    "updated_at": null
                }),
            );
        dashboard_ai_settings_from_json(value)
            .expect("default dashboard AI settings must satisfy the sanitized contract")
    }

    pub async fn ai_settings_value(&self) -> Result<JsonValue> {
        let mut value = self.default_ai_settings_value();
        if let Some(saved) = self.runtime_setting("ai_settings").await? {
            let model = saved
                .get("model")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string);
            if let Some(model) = model {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("model".to_string(), JsonValue::from(model));
                    obj.insert("source".to_string(), JsonValue::from("runtime"));
                    obj.insert(
                        "updated_at".to_string(),
                        saved.get("updated_at").cloned().unwrap_or(JsonValue::Null),
                    );
                }
            }
        }
        if let Some(obj) = value.as_object_mut() {
            obj.insert("api_key".to_string(), self.ai_api_key_status_value().await?);
        }
        // The configured default joins the list so reverting a runtime
        // override never depends on the operator remembering the slug. A
        // failed read leaves the picker empty rather than failing the menu:
        // the text field is the real control and it still works.
        let mut recent = self.recently_used_ai_models(12).await.unwrap_or_default();
        let config_model = value
            .get("config_model")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string();
        if !config_model.is_empty() && !recent.iter().any(|model| model == &config_model) {
            recent.push(config_model);
        }
        if let Some(obj) = value.as_object_mut() {
            obj.insert("recent_models".to_string(), JsonValue::from(recent));
        }
        Ok(value)
    }

    /// Bounded local evidence for the provider/model request contract and
    /// outcome reliability. This reads only persisted Decision Report metadata;
    /// it neither exposes the documents selected from nor contacts a provider.
    pub async fn ai_provider_capabilities(
        &self,
        limit: i64,
    ) -> Result<Vec<AiProviderCapabilityPayload>> {
        let rows = self
            .select_json(&format!(
                "SELECT model, status, analysis_pulse_key, request_json, response_json, error_text
                 FROM decision_reports
                 ORDER BY created_at DESC, id DESC
                 LIMIT {}",
                clamp_limit(limit, 1, 1_000)
            ))
            .await?;
        let configured_timeout_seconds = yaml_i64(&self.config, &["xai", "http_timeout_seconds"])
            .or_else(|| yaml_i64(&self.config, &["xai", "deferred_http_timeout_seconds"]))
            .or_else(|| yaml_i64(&self.config, &["xai", "timeout_seconds"]))
            .unwrap_or(30)
            .max(5);
        Ok(ai_provider_capabilities_from_rows(
            rows,
            configured_timeout_seconds,
        ))
    }

    /// Masked status of the AI provider API key. Never contains the key
    /// itself — only whether one is configured, where it comes from, and a
    /// short masked preview so the operator can recognize which key is live.
    pub async fn ai_api_key_status_value(&self) -> Result<JsonValue> {
        let override_entry = self.runtime_setting("ai_api_key").await?;
        let override_key = override_entry
            .as_ref()
            .and_then(|entry| entry.get("api_key"))
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string);
        let config_key = yaml_string(&self.config, &["xai", "api_key"])
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        let (source, key) = if let Some(key) = override_key {
            ("runtime", Some(key))
        } else if let Some(key) = config_key {
            ("config", Some(key))
        } else {
            ("missing", None)
        };
        Ok(json!({
            "configured": key.is_some(),
            "source": source,
            "masked": key.as_deref().map(mask_api_key),
            "updated_at": override_entry
                .as_ref()
                .and_then(|entry| entry.get("updated_at"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        }))
    }

    /// The AI provider API key the process should use right now: the
    /// runtime override saved from Settings wins over the config/env value,
    /// so a rotated key takes effect without a redeploy.
    pub async fn effective_ai_api_key(&self) -> Option<String> {
        if let Ok(Some(saved)) = self.runtime_setting("ai_api_key").await {
            if let Some(key) = saved
                .get("api_key")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty())
            {
                return Some(key.to_string());
            }
        }
        yaml_string(&self.config, &["xai", "api_key"])
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }

    /// Saves (or, for an empty submission, clears) the runtime API-key
    /// override. Returns the masked status only — the key is never echoed.
    pub async fn save_ai_api_key(&self, api_key: &str) -> Result<JsonValue> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            sqlx::query("DELETE FROM runtime_settings WHERE key = 'ai_api_key'")
                .execute(&self.pool)
                .await
                .context("clearing AI API key override")?;
            return self.ai_api_key_status_value().await;
        }
        if api_key.len() > 400 {
            anyhow::bail!("API key is too long");
        }
        if !api_key.chars().all(|ch| ch.is_ascii_graphic()) {
            anyhow::bail!("API key contains whitespace or non-printable characters");
        }
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let value = json!({
            "api_key": api_key,
            "updated_at": updated_at
        });
        self.save_runtime_setting("ai_api_key", &value).await?;
        self.ai_api_key_status_value().await
    }

    pub async fn save_ai_settings(&self, model: &str) -> Result<JsonValue> {
        let model = validated_ai_model(model)?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let value = json!({
            "model": model,
            "updated_at": updated_at
        });
        self.save_runtime_setting("ai_settings", &value).await?;
        self.ai_settings_value().await
    }

    pub async fn effective_xai_model(&self) -> Result<String> {
        Ok(self
            .ai_settings_value()
            .await?
            .get("model")
            .and_then(JsonValue::as_str)
            .unwrap_or("openai/gpt-5.5")
            .to_string())
    }

    pub(crate) async fn runtime_setting(&self, key: &str) -> Result<Option<JsonValue>> {
        let Some(row) = self
            .first_json(&format!(
                "SELECT value_json FROM runtime_settings WHERE key = '{}' LIMIT 1",
                sql_escape(key)
            ))
            .await?
        else {
            return Ok(None);
        };
        let value = row.get("value_json").cloned().unwrap_or(JsonValue::Null);
        if value.is_object() {
            return Ok(Some(value));
        }
        if let Some(text) = value.as_str() {
            return Ok(Some(
                serde_json::from_str(text).context("parsing runtime setting JSON")?,
            ));
        }
        Ok(None)
    }

    /// Removes database settings that were only read by the retired Python
    /// scheduler. Keeping one would make a future compatibility refactor able
    /// to resurrect an unreviewed trading override.
    pub(crate) async fn purge_retired_runtime_settings(&self) -> Result<u64> {
        let keys = RETIRED_RUNTIME_SETTING_KEYS
            .iter()
            .map(|key| format!("'{}'", sql_escape(key)))
            .collect::<Vec<_>>()
            .join(", ");
        let result = sqlx::query(&format!(
            "DELETE FROM runtime_settings WHERE key IN ({keys})"
        ))
        .execute(&self.pool)
        .await
        .context("removing retired legacy runtime settings")?;
        Ok(result.rows_affected())
    }

    pub(crate) async fn save_runtime_setting(&self, key: &str, value: &JsonValue) -> Result<()> {
        let value_text = serde_json::to_string(value).context("serializing runtime setting")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO runtime_settings (key, value_json, updated_at)
             VALUES ('{}', '{}', '{}')
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            sql_escape(key),
            sql_escape(&value_text),
            sql_escape(&updated_at)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("persisting runtime setting")?;
        Ok(())
    }
}
