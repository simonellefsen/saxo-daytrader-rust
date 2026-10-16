//! Jev read models on `AppState`: the answering version, the operator panel,
//! and `/api/ai/jev`.
//!
//! Observational throughout. Nothing here is read by a path that can admit,
//! size, or block a trade.

use anyhow::Result;
use chrono::Utc;
use serde_json::Value as JsonValue;
use tracing::warn;

use crate::state::AppState;

impl AppState {
    /// Which Jev version answers, and every version that has. Never fails:
    /// an unreadable ledger is reported as unavailable rather than as no
    /// rollover having happened.
    pub async fn jev_models(&self) -> crate::models::JevModelsPayload {
        let configured = crate::jev::configured_model(&self.config);
        match crate::jev_store::model_epochs(&self.pool).await {
            Ok(epochs) => crate::jev_store::models_payload(&configured, epochs),
            Err(err) => {
                warn!("Jev model epochs unavailable: {err:#}");
                crate::models::JevModelsPayload {
                    unavailable: true,
                    ..crate::jev_store::models_payload(&configured, Vec::new())
                }
            }
        }
    }

    /// The Jev operator panel. Never fails: each read that fails is named in
    /// the payload and leaves its section empty.
    pub async fn jev_operator(&self) -> crate::models::JevOperatorPayload {
        let threshold = crate::jev_store::INSTRUCTION_SHAPED_THRESHOLD;
        let reads = crate::jev_store::OperatorReads {
            worker: crate::jev_store::worker_status_rows(&self.pool).await,
            provider: crate::jev_store::provider_state_row(&self.pool).await,
            calls: crate::jev_store::purpose_call_rows(&self.pool).await,
            agreement: crate::jev_store::screening_agreement(&self.pool, threshold).await,
            disagreements: crate::jev_store::screening_disagreements(&self.pool, threshold, 20)
                .await,
        };
        crate::jev_store::operator_payload(
            self.jev_status(),
            Utc::now(),
            crate::jev_review::OBSERVATION_CYCLE_SECONDS,
            crate::jev::RequestLimits::from_yaml(&self.config).payload(),
            reads,
        )
    }

    fn jev_status(&self) -> &'static str {
        match crate::jev::JevConfig::from_yaml(&self.config) {
            crate::jev::JevAvailability::Ready(_) => "ok",
            crate::jev::JevAvailability::Disabled => "disabled",
            crate::jev::JevAvailability::MissingApiKey => "missing_api_key",
        }
    }

    /// What Jev has judged, and where it disagrees with the marker screen.
    ///
    /// Observational throughout. Nothing here is read by a path that can
    /// admit, size, or block a trade -- the rankings in particular are a view
    /// of news standing, not a candidate list.
    pub async fn jev_observations(
        &self,
        limit: i64,
        days: i64,
    ) -> Result<crate::models::JevObservationsPayload> {
        let since = (Utc::now() - chrono::Duration::days(days.clamp(1, 365)))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let threshold = crate::jev_store::INSTRUCTION_SHAPED_THRESHOLD;

        let signals = crate::jev_store::recent_signals(&self.pool, &since, limit).await?;
        let retention = crate::read_model::decode(
            "jev retention",
            crate::jev_signals::retention_summary(
                &signals,
                &Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ),
        )?;
        let screening_agreement = crate::read_model::decode(
            "jev screening agreement",
            crate::jev_store::screening_agreement(&self.pool, threshold).await?,
        )?;
        let results = |rows: Vec<JsonValue>| {
            crate::read_model::decode_each::<crate::models::JevStoredResultPayload>(
                "jev stored results",
                rows,
            )
        };
        Ok(crate::models::JevObservationsPayload {
            status: self.jev_status().to_string(),
            since: Some(since.clone()),
            signal_count: signals.len() as i64,
            retention,
            screening_agreement,
            screening_disagreements: crate::jev_store::disagreements_from_rows(
                &crate::jev_store::screening_disagreements(&self.pool, threshold, 100).await?,
            ),
            symbol_rankings: crate::jev_signals::symbol_rankings(&signals)
                .into_iter()
                .map(|ranking| crate::models::JevSymbolRankingPayload {
                    symbol: ranking.symbol,
                    score: ranking.score,
                    item_count: ranking.item_count,
                    newest_at: ranking.newest_at,
                })
                .collect(),
            models: self.jev_models().await,
            operator: self.jev_operator().await,
            report_grades: results(
                crate::jev_store::results_for(
                    &self.pool,
                    crate::jev_store::PURPOSE_REPORT_GRADING,
                    50,
                )
                .await?,
            )?,
            failure_classifications: results(
                crate::jev_store::results_for(
                    &self.pool,
                    crate::jev_store::PURPOSE_ERROR_CLASSIFICATION,
                    50,
                )
                .await?,
            )?,
            admission: "observational_only".to_string(),
            safety: "Jev judgements are recorded and ranked for measurement only. They \
                     cannot admit an item the marker screen excluded, create a candidate, \
                     size or block a trade, or reach Saxo."
                .to_string(),
            unavailable_reason: None,
        })
    }
}
