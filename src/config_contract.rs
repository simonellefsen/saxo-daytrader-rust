//! Config contract audit.
//!
//! A configuration key that was never wired into the runtime reads exactly like
//! one that is enforced. That ambiguity is how the retired 2026-05-05
//! `strategy.capital.cash_buffer` override survived, and a 2026-07-25 review
//! found the same shape across strategy risk knobs including several
//! `strategy.ladder.*` members and the `taxation.share_income` brackets: all
//! were present in `config.yaml` but absent from the code.
//!
//! This module keeps an explicit table of every audited key and what the runtime
//! actually does with it, then reports three kinds of drift:
//!
//! 1. a key present in config that the table marks as unused,
//! 2. a key the table expects that config no longer supplies,
//! 3. a key present in config that the table does not describe at all.
//!
//! The third case is the one that keeps this honest over time: adding a knob to
//! `config.yaml` without recording whether anything reads it produces a finding
//! on the next startup.
//!
//! The audit is read-only. It reports; it never changes configuration, gates,
//! sizing, or broker behavior.

use serde_yaml::Value as YamlValue;

/// What the runtime does with a configured key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    /// Read by a path that can create, size, or block an order.
    Enforced,
    /// Reaches a prompt, dashboard, analytic, or scheduler cadence only. It
    /// cannot by itself change what is traded.
    Advisory,
    /// Present in configuration and referenced nowhere in `src/`.
    Unused,
}

impl ContractStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforced => "enforced",
            Self::Advisory => "advisory",
            Self::Unused => "unused",
        }
    }
}

/// Whether an entry describes one leaf or a whole data subtree.
///
/// Operator-maintained data maps such as `symbol_aliases` grow new members
/// routinely. Contracting them per member would make every new alias a drift
/// finding, so they are contracted once at the parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContractScope {
    Leaf,
    Subtree,
}

struct ContractEntry {
    path: &'static [&'static str],
    status: ContractStatus,
    scope: ContractScope,
    /// True when an `Unused` key implies a safeguard an operator could
    /// reasonably believe is active. These are the ones worth alerting on.
    risk_surface: bool,
    note: &'static str,
}

/// Config roots this audit covers. Deliberately limited to the sections that
/// describe trading behavior and risk.
pub const AUDITED_ROOTS: &[&str] = &["strategy", "risk", "taxation"];

const fn enforced(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Enforced,
        scope: ContractScope::Leaf,
        risk_surface: false,
        note,
    }
}

const fn advisory(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Advisory,
        scope: ContractScope::Leaf,
        risk_surface: false,
        note,
    }
}

const fn advisory_subtree(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Advisory,
        scope: ContractScope::Subtree,
        risk_surface: false,
        note,
    }
}

const fn enforced_subtree(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Enforced,
        scope: ContractScope::Subtree,
        risk_surface: false,
        note,
    }
}

/// An unused key that does not imply a missing safeguard.
const fn unused(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Unused,
        scope: ContractScope::Leaf,
        risk_surface: false,
        note,
    }
}

/// The contract. Statuses were established by reading each key's call sites on
/// 2026-07-25; update an entry in the same change that changes its wiring.
const CONTRACT: &[ContractEntry] = &[
    // ---- strategy (top level) ----
    enforced(
        &["strategy", "enabled"],
        "Stops new scheduled decision-report submission and Trading Manager queueing. Pending provider reports still reach a terminal state; read-only analysis, broker reconciliation, and protective stops continue.",
    ),
    unused(&["strategy", "mode"], "Strategy mode label is not read."),
    unused(
        &["strategy", "selection_interval_minutes"],
        "Cadence comes from the analysis pulses, not this interval.",
    ),
    unused(
        &["strategy", "max_candidates"],
        "Candidate breadth is bounded by the report prompt and trading_manager.max_symbols instead.",
    ),
    enforced(
        &["strategy", "max_selected_assets"],
        "Caps distinct approved BUY symbols per Decision Report after the deterministic gates. SELLs and repeat BUYs for a previously selected symbol remain eligible; zero is unlimited and a negative value blocks BUYs.",
    ),
    enforced(
        &["strategy", "concentration", "max_assets_per_exchange"],
        "Caps distinct held/planned BUY symbols within each canonical exchange-suffix bucket. Zero is explicit unlimited policy; negative values fail BUYs closed, and missing bucket evidence blocks only when a positive cap is enabled.",
    ),
    enforced(
        &["strategy", "concentration", "max_assets_per_currency"],
        "Caps distinct held/planned BUY symbols within each canonical exchange-implied currency bucket. Zero is explicit unlimited policy; negative values fail BUYs closed, and missing bucket evidence blocks only when a positive cap is enabled.",
    ),
    enforced(
        &["strategy", "estimated_slippage_bps"],
        "BUY cost guard uses it as a one-way slippage estimate against database-verified indicator reward.",
    ),
    enforced(
        &["strategy", "cost_guard_multiple"],
        "BUY cost guard multiplies the round-trip exchange-minimum commission hurdle.",
    ),
    // ---- strategy.capital ----
    enforced(
        &["strategy", "capital", "max_deployment_pct"],
        "Bounds the cycle BUY budget.",
    ),
    enforced(
        &["strategy", "capital", "min_cash_buffer_pct"],
        "Bounds the cycle BUY budget.",
    ),
    enforced(
        &["strategy", "capital", "reinvestment_pressure_threshold_pct"],
        "Drives reinvestment pressure in the capital plan.",
    ),
    enforced(
        &["strategy", "capital", "monthly_loss_soft_reduce_dkk"],
        "Soft monthly-loss floor; halves the cycle BUY budget.",
    ),
    enforced(
        &["strategy", "capital", "monthly_loss_soft_buy_multiplier"],
        "Soft-tier BUY budget multiplier.",
    ),
    enforced(
        &["strategy", "capital", "monthly_loss_halt_dkk"],
        "Hard monthly-loss floor; blocks new BUYs.",
    ),
    enforced(
        &["strategy", "capital", "drawdown_lookback_days"],
        "Trailing window the drawdown guardrail measures its peak over.",
    ),
    enforced(
        &["strategy", "capital", "drawdown_soft_reduce_pct"],
        "Soft drawdown band; reduces the cycle BUY budget.",
    ),
    enforced(
        &["strategy", "capital", "drawdown_soft_buy_multiplier"],
        "Soft-tier BUY budget multiplier for the drawdown guardrail.",
    ),
    enforced(
        &["strategy", "capital", "drawdown_halt_pct"],
        "Hard drawdown floor; blocks new BUYs and is the max_drawdown the Hermes goal contract publishes.",
    ),
    // ---- strategy.markov ----
    advisory(&["strategy", "markov", "enabled"], "Markov cycle switch."),
    advisory(&["strategy", "markov", "timezone"], "Markov run cadence."),
    advisory(&["strategy", "markov", "daily_time"], "Markov run cadence."),
    advisory(
        &["strategy", "markov", "run_weekdays_only"],
        "Markov run cadence.",
    ),
    advisory(
        &["strategy", "markov", "window_days"],
        "Markov model parameter.",
    ),
    advisory(
        &["strategy", "markov", "threshold"],
        "Markov labelling threshold.",
    ),
    advisory(
        &["strategy", "markov", "horizon_minutes"],
        "Markov model parameter.",
    ),
    advisory(
        &["strategy", "markov", "sample_count"],
        "Markov chart history depth.",
    ),
    advisory(
        &["strategy", "markov", "min_labeled_days"],
        "Markov minimum sample guard.",
    ),
    advisory(
        &["strategy", "markov", "signal_horizon_days"],
        "Markov signal horizon.",
    ),
    advisory(
        &["strategy", "markov", "forecast_steps"],
        "Markov forecast horizons.",
    ),
    advisory(
        &["strategy", "markov", "max_symbols"],
        "Markov universe cap; 0 means unlimited.",
    ),
    advisory(
        &["strategy", "markov", "instrument_negative_cache_retry_days"],
        "Instrument negative-cache retry window.",
    ),
    advisory_subtree(
        &["strategy", "markov", "symbol_aliases"],
        "Read-only chart-lookup aliases; persisted signals keep the original symbol.",
    ),
    // ---- strategy.quiver ----
    //
    // Read by src/quiver.rs. Both shipped configs declare this policy so local
    // scheduler behavior matches the deployed advisory-data collection cadence.
    advisory(&["strategy", "quiver", "enabled"], "Quiver cycle switch."),
    advisory(&["strategy", "quiver", "timezone"], "Quiver run cadence."),
    advisory(
        &[
            "strategy",
            "quiver",
            "us_open_followup",
            "minutes_after_open",
        ],
        "Quiver's Saxo-calendar-relative US open follow-up cadence.",
    ),
    advisory(
        &["strategy", "quiver", "us_open_followup", "exchange_codes"],
        "US exchanges that define Quiver's Saxo-calendar-relative schedule.",
    ),
    advisory(
        &["strategy", "quiver", "run_weekdays_only"],
        "Quiver run cadence.",
    ),
    advisory(
        &["strategy", "quiver", "lookback_days"],
        "Quiver signal lookback.",
    ),
    advisory(
        &["strategy", "quiver", "max_symbols"],
        "Quiver universe cap.",
    ),
    // ---- strategy.swing ----
    unused(
        &["strategy", "swing", "min_holdings"],
        "No holdings floor is enforced.",
    ),
    enforced(
        &["strategy", "swing", "max_holdings"],
        "Caps concurrent held symbols for new BUYs using persisted positive-quantity positions plus new-symbol BUYs approved earlier in the same scheduler cycle. Adds to existing symbols do not consume a slot; an unavailable position snapshot blocks a new symbol BUY.",
    ),
    advisory(
        &["strategy", "swing", "position_decision_stale_after_days"],
        "Dashboard decision-chip staleness horizon.",
    ),
    enforced(
        &["strategy", "swing", "risk_per_trade_pct"],
        "Caps each BUY's initial estimated loss at the database-verified ATR14 stop distance. The cap uses strategy.ladder.stop_loss_atr_multiple and requires automatic protective stops plus a verified close, ATR14, and DKK share value; missing evidence blocks the BUY.",
    ),
    enforced_subtree(
        &["strategy", "swing", "never_trade_symbols"],
        "Merged with risk.excluded_symbols and blocks candidates.",
    ),
    // ---- strategy.swing.daily_indicators ----
    advisory(
        &["strategy", "swing", "daily_indicators", "enabled"],
        "Daily indicator cycle switch.",
    ),
    advisory(
        &["strategy", "swing", "daily_indicators", "max_symbols"],
        "Indicator universe cap; 0 means unlimited.",
    ),
    advisory(
        &["strategy", "swing", "daily_indicators", "horizon_minutes"],
        "Indicator model parameter.",
    ),
    advisory(
        &["strategy", "swing", "daily_indicators", "sample_count"],
        "Indicator chart history depth.",
    ),
    enforced(
        &["strategy", "swing", "daily_indicators", "min_confluences"],
        "BUY technical gate threshold.",
    ),
    enforced(
        &["strategy", "swing", "daily_indicators", "min_reward_risk"],
        "BUY technical gate threshold.",
    ),
    advisory(
        &["strategy", "swing", "daily_indicators", "daily_time"],
        "Indicator run cadence.",
    ),
    advisory(
        &["strategy", "swing", "daily_indicators", "run_weekdays_only"],
        "Indicator run cadence.",
    ),
    advisory(
        &["strategy", "swing", "daily_indicators", "timezone"],
        "Indicator run cadence.",
    ),
    // ---- strategy.performance_benchmarks ----
    advisory_subtree(
        &["strategy", "performance_benchmarks"],
        "Read-only Saxo-backed benchmark price-return comparison. It cannot affect decisions, Hermes, sizing, or broker behavior.",
    ),
    // ---- strategy.swing.markov_gate ----
    enforced(
        &["strategy", "swing", "markov_gate", "enabled"],
        "Markov BUY gate switch.",
    ),
    enforced(
        &["strategy", "swing", "markov_gate", "min_signed_signal"],
        "Markov BUY gate threshold.",
    ),
    enforced(
        &["strategy", "swing", "markov_gate", "max_position_pct"],
        "Markov starter-position bound.",
    ),
    enforced(
        &["strategy", "swing", "markov_gate", "max_signal_age_days"],
        "Markov signal freshness gate.",
    ),
    // ---- strategy.swing.trading_manager ----
    //
    // Only enabled, max_symbols, and max_report_age_hours affect Rust queue
    // creation. The remaining legacy controls stay explicitly unused.
    enforced(
        &["strategy", "swing", "trading_manager", "enabled"],
        "Stops the Trading Manager from creating new execution orders while preserving report and broker audit paths.",
    ),
    unused(
        &["strategy", "swing", "trading_manager", "use_ai"],
        "AI report use is decided by the decision-report pipeline, not this flag.",
    ),
    unused(
        &["strategy", "swing", "trading_manager", "due_window_minutes"],
        "Report freshness uses max_report_age_hours; this window is not read.",
    ),
    enforced(
        &["strategy", "swing", "trading_manager", "max_symbols"],
        "Bounds distinct symbols evaluated from each Decision Report; excess report symbols are retained as skipped audit rows.",
    ),
    advisory(
        &[
            "strategy",
            "swing",
            "trading_manager",
            "max_report_age_hours",
        ],
        "Scheduled-report freshness window. Reports older than this cannot create new execution orders.",
    ),
    // ---- strategy.swing.journal ----
    advisory(
        &["strategy", "swing", "journal", "enabled"],
        "End-of-day journal switch.",
    ),
    advisory(
        &["strategy", "swing", "journal", "timezone"],
        "Journal cadence.",
    ),
    advisory(
        &["strategy", "swing", "journal", "daily_time"],
        "Journal cadence.",
    ),
    unused(
        &["strategy", "swing", "journal", "weekly_weekday"],
        "Only the daily journal cycle is ported to Rust; the weekly cycle is legacy Python behavior.",
    ),
    unused(
        &["strategy", "swing", "journal", "weekly_time"],
        "Only the daily journal cycle is ported to Rust.",
    ),
    unused(
        &["strategy", "swing", "journal", "monthly_time"],
        "Only the daily journal cycle is ported to Rust.",
    ),
    // ---- strategy.swing.analysis_pulses ----
    unused(
        &["strategy", "swing", "analysis_pulses", "timezone"],
        "Pulse timing comes from exchange calendars, not this timezone.",
    ),
    advisory(
        &["strategy", "swing", "analysis_pulses", "due_window_minutes"],
        "Pulse due window.",
    ),
    enforced(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "europe_open_followup",
            "enabled",
        ],
        "Enables scheduled Nordic/EU open-followup report submission.",
    ),
    advisory(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "europe_open_followup",
            "minutes_after_open",
        ],
        "EU pulse offset.",
    ),
    enforced_subtree(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "europe_open_followup",
            "exchange_codes",
        ],
        "Market-scope filter for the EU pulse.",
    ),
    enforced(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "us_open_followup",
            "enabled",
        ],
        "Enables scheduled US open-followup report submission.",
    ),
    advisory(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "us_open_followup",
            "minutes_after_open",
        ],
        "US pulse offset.",
    ),
    enforced_subtree(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "us_open_followup",
            "exchange_codes",
        ],
        "Market-scope filter for the US pulse.",
    ),
    // ---- strategy.ladder ----
    //
    // Legacy ladder entries remain unimplemented. Protective-stop members are
    // called out individually because they are active downside protection.
    unused(
        &["strategy", "ladder", "rung_count"],
        "Ladder entries are not implemented.",
    ),
    unused(
        &["strategy", "ladder", "min_rung_value_dkk"],
        "Ladder entries are not implemented.",
    ),
    enforced(
        &["strategy", "ladder", "submit_stop_loss_after_fill"],
        "Master switch for the automatic protective-stop sweep. False means no stop is placed, amended, or ratcheted without an operator action.",
    ),
    unused(
        &["strategy", "ladder", "atr_spacing_min"],
        "Ladder entries are not implemented.",
    ),
    unused(
        &["strategy", "ladder", "atr_spacing_factor"],
        "Ladder entries are not implemented.",
    ),
    unused(
        &["strategy", "ladder", "atr_spacing_max"],
        "Ladder entries are not implemented.",
    ),
    enforced(
        &["strategy", "ladder", "stop_loss_atr_multiple"],
        "Distance of a newly placed protective stop below the last close, and the proposed level in the coverage audit.",
    ),
    unused(
        &["strategy", "ladder", "take_profit_rung_multiple"],
        "Take-profit targets are not implemented.",
    ),
    unused(
        &["strategy", "ladder", "max_take_profit_atr_multiple"],
        "Take-profit targets are not implemented.",
    ),
    enforced(
        &["strategy", "ladder", "max_position_weight"],
        "Caps total per-symbol exposure after a BUY using persisted position values plus BUYs already approved in the same scheduler cycle. Missing or invalid exposure evidence blocks the BUY.",
    ),
    enforced(
        &["strategy", "ladder", "trail_stop_atr_multiple"],
        "Distance of the trailing stop below the last close once a position is already protected. Tighter than the initial multiple; the ratchet is monotonic.",
    ),
    enforced(
        &["strategy", "ladder", "min_ratchet_atr_fraction"],
        "How far the trail must advance, as a fraction of ATR14, before the sweep cancels and replaces a resting stop.",
    ),
    // ---- risk ----
    enforced_subtree(
        &["risk", "excluded_symbols"],
        "Blocks candidates in the Trading Manager.",
    ),
    enforced(
        &["risk", "excluded_symbols_csv"],
        "Resolved from RISK_EXCLUDED_SYMBOLS and merged with risk.excluded_symbols plus strategy.swing.never_trade_symbols before the Trading Manager gate.",
    ),
    enforced(
        &["risk", "instrument_quarantine", "enabled"],
        "Derived instrument quarantine switch.",
    ),
    enforced(
        &["risk", "instrument_quarantine", "lookback_days"],
        "Quarantine failure lookback.",
    ),
    enforced(
        &["risk", "instrument_quarantine", "min_failures"],
        "Quarantine activation threshold.",
    ),
    enforced(
        &["risk", "instrument_quarantine", "active_days"],
        "Quarantine duration.",
    ),
    advisory(
        &["risk", "allow_shorting"],
        "Published in the Hermes goal contract; the runtime has no short path regardless.",
    ),
    // ---- taxation ----
    enforced(
        &["taxation", "share_income", "currency"],
        "The after-tax estimate is available only for the configured DKK share-income basis.",
    ),
    enforced_subtree(
        &["taxation", "share_income", "brackets"],
        "Progressive brackets estimate incremental Danish share-income tax on realised gains plus current unrealised P/L; display-only and never posted to the ledger.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// Config supplies a key the contract marks as unused.
    UnusedKeyPresent,
    /// The contract describes a key config no longer supplies.
    ContractedKeyMissing,
    /// Config supplies a key the contract does not describe.
    UncontractedKey,
}

impl FindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnusedKeyPresent => "unused_key_present",
            Self::ContractedKeyMissing => "contracted_key_missing",
            Self::UncontractedKey => "uncontracted_key",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContractFinding {
    pub path: String,
    pub kind: FindingKind,
    pub risk_surface: bool,
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContractSummary {
    pub enforced: usize,
    pub advisory: usize,
    pub unused: usize,
    pub unused_risk_surface: usize,
    pub uncontracted: usize,
    pub missing: usize,
}

/// The contract status recorded for an exact key path, if the path is covered.
///
/// Used to prove that anything offered to Hermes as a tunable variable is
/// actually read by the runtime. Returns `None` for paths outside
/// `AUDITED_ROOTS`, which are simply not described by this table.
// Currently consulted only by the guard test that proves every variable offered
// to Hermes is one the runtime reads. Kept available for runtime callers.
#[allow(dead_code)]
pub fn status_for_path(path: &str) -> Option<ContractStatus> {
    let segments = path.split('.').map(str::to_string).collect::<Vec<_>>();
    match_entry(&segments).map(|entry| entry.status)
}

/// Audit `config` against the contract table.
///
/// Findings are returned in a stable order: unused risk keys first, then other
/// unused keys, then uncontracted keys, then missing keys.
pub fn audit_config(config: &YamlValue) -> (ContractSummary, Vec<ContractFinding>) {
    let mut summary = ContractSummary::default();
    let mut findings = Vec::new();

    for leaf in config_leaf_paths(config) {
        match match_entry(&leaf) {
            Some(entry) => match entry.status {
                ContractStatus::Enforced => summary.enforced += 1,
                ContractStatus::Advisory => summary.advisory += 1,
                ContractStatus::Unused => {
                    summary.unused += 1;
                    if entry.risk_surface {
                        summary.unused_risk_surface += 1;
                    }
                    findings.push(ContractFinding {
                        path: leaf.join("."),
                        kind: FindingKind::UnusedKeyPresent,
                        risk_surface: entry.risk_surface,
                        note: entry.note.to_string(),
                    });
                }
            },
            None => {
                summary.uncontracted += 1;
                findings.push(ContractFinding {
                    path: leaf.join("."),
                    kind: FindingKind::UncontractedKey,
                    // An unclassified key under an audited root is treated as a
                    // risk surface until someone contracts it. Failing loud is
                    // the whole point of this audit.
                    risk_surface: true,
                    note: "Key is not described by the config contract. Add it to CONTRACT in src/config_contract.rs with its real status.".to_string(),
                });
            }
        }
    }

    for entry in CONTRACT {
        if crate::config::yaml_at(config, entry.path).is_none() {
            summary.missing += 1;
            findings.push(ContractFinding {
                path: entry.path.join("."),
                kind: FindingKind::ContractedKeyMissing,
                risk_surface: entry.status == ContractStatus::Enforced,
                note: format!(
                    "Contract expects this key ({}) but config does not supply it. {}",
                    entry.status.as_str(),
                    entry.note
                ),
            });
        }
    }

    findings.sort_by_key(|finding| {
        let rank = match (finding.kind, finding.risk_surface) {
            (FindingKind::UnusedKeyPresent, true) => 0,
            (FindingKind::UnusedKeyPresent, false) => 1,
            (FindingKind::UncontractedKey, _) => 2,
            (FindingKind::ContractedKeyMissing, _) => 3,
        };
        (rank, finding.path.clone())
    });

    (summary, findings)
}

/// Enumerate leaf paths under the audited roots.
///
/// Sequences are leaves: a list of symbols or exchange codes is one setting, not
/// one setting per member.
fn config_leaf_paths(config: &YamlValue) -> Vec<Vec<String>> {
    let mut leaves = Vec::new();
    for root in AUDITED_ROOTS {
        if let Some(node) = config.get(*root) {
            collect_leaves(node, vec![(*root).to_string()], &mut leaves);
        }
    }
    leaves
}

fn collect_leaves(node: &YamlValue, path: Vec<String>, out: &mut Vec<Vec<String>>) {
    match node {
        YamlValue::Mapping(map) if !map.is_empty() => {
            // A subtree contract stops the descent so operator-maintained data
            // maps do not produce one finding per member.
            if matches!(
                match_entry(&path),
                Some(entry) if entry.scope == ContractScope::Subtree
            ) {
                out.push(path);
                return;
            }
            for (key, value) in map {
                let Some(key) = key.as_str() else { continue };
                let mut child = path.clone();
                child.push(key.to_string());
                collect_leaves(value, child, out);
            }
        }
        _ => out.push(path),
    }
}

/// Exact leaf match wins; otherwise the longest matching subtree prefix.
fn match_entry(path: &[String]) -> Option<&'static ContractEntry> {
    let mut best: Option<&'static ContractEntry> = None;
    for entry in CONTRACT {
        if entry.path.len() == path.len()
            && entry
                .path
                .iter()
                .zip(path.iter())
                .all(|(left, right)| left == right)
        {
            return Some(entry);
        }
        if entry.scope == ContractScope::Subtree
            && entry.path.len() < path.len()
            && entry
                .path
                .iter()
                .zip(path.iter())
                .all(|(left, right)| left == right)
            && best.is_none_or(|current| current.path.len() < entry.path.len())
        {
            best = Some(entry);
        }
    }
    best
}

/// Log the audit once during startup.
///
/// One summary line always, plus one warning per risk-surface finding. Ordinary
/// unused keys stay at debug so the startup log does not become noise.
pub fn log_config_contract_audit(config: &YamlValue) {
    let (summary, findings) = audit_config(config);
    tracing::info!(
        enforced = summary.enforced,
        advisory = summary.advisory,
        unused = summary.unused,
        unused_risk_surface = summary.unused_risk_surface,
        uncontracted = summary.uncontracted,
        missing = summary.missing,
        "config contract audit complete"
    );
    for finding in &findings {
        if finding.risk_surface {
            tracing::warn!(
                key = %finding.path,
                kind = finding.kind.as_str(),
                "config contract: {}",
                finding.note
            );
        } else {
            tracing::debug!(
                key = %finding.path,
                kind = finding.kind.as_str(),
                "config contract: {}",
                finding.note
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> YamlValue {
        serde_yaml::from_str(text).expect("test config parses")
    }

    #[test]
    fn retired_sector_cap_is_reported_as_uncontracted() {
        let config = parse("strategy:\n  max_assets_per_sector: 2\n");
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.uncontracted, 1);
        let finding = findings
            .iter()
            .find(|finding| finding.path == "strategy.max_assets_per_sector")
            .expect("max_assets_per_sector is reported");
        assert_eq!(finding.kind, FindingKind::UncontractedKey);
        assert!(finding.risk_surface);
    }

    #[test]
    fn retired_position_weight_keys_are_reported_as_uncontracted() {
        let config = parse(
            "strategy:\n  swing:\n    min_holding_weight_pct: 0.05\n    max_holding_weight_pct: 0.25\n  ladder:\n    min_position_weight: 0.02\nrisk:\n  max_position_weight: 0.25\n",
        );
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.uncontracted, 4);
        for path in [
            "strategy.swing.min_holding_weight_pct",
            "strategy.swing.max_holding_weight_pct",
            "strategy.ladder.min_position_weight",
            "risk.max_position_weight",
        ] {
            let finding = findings
                .iter()
                .find(|finding| finding.path == path)
                .unwrap_or_else(|| panic!("{path} is reported"));
            assert_eq!(finding.kind, FindingKind::UncontractedKey);
            assert!(finding.risk_surface);
        }
    }

    #[test]
    fn retired_session_flatten_keys_are_reported_as_uncontracted() {
        let config = parse(
            "strategy:\n  ladder:\n    session_flatten_enabled: false\n    flatten_minutes_before_tradable_close: 15\n",
        );
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.uncontracted, 2);
        for path in [
            "strategy.ladder.session_flatten_enabled",
            "strategy.ladder.flatten_minutes_before_tradable_close",
        ] {
            let finding = findings
                .iter()
                .find(|finding| finding.path == path)
                .unwrap_or_else(|| panic!("{path} is reported"));
            assert_eq!(finding.kind, FindingKind::UncontractedKey);
            assert!(finding.risk_surface);
        }
    }

    #[test]
    fn retired_legacy_benchmark_indices_are_reported_as_uncontracted() {
        let config = parse(
            "strategy:\n  swing:\n    journal:\n      benchmark_indices:\n        US:\n          S&P 500: '^GSPC'\n",
        );
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.uncontracted, 1);
        let finding = findings
            .iter()
            .find(|finding| finding.path == "strategy.swing.journal.benchmark_indices.US.S&P 500")
            .expect("retired benchmark key is reported");
        assert_eq!(finding.kind, FindingKind::UncontractedKey);
        assert!(finding.risk_surface);
    }

    #[test]
    fn retired_ladder_bracket_and_take_profit_keys_are_reported_as_uncontracted() {
        let config = parse(
            "strategy:\n  ladder:\n    submit_bracket_with_entry: false\n    submit_take_profit_after_fill: false\n",
        );
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.uncontracted, 2);
        for path in [
            "strategy.ladder.submit_bracket_with_entry",
            "strategy.ladder.submit_take_profit_after_fill",
        ] {
            let finding = findings
                .iter()
                .find(|finding| finding.path == path)
                .unwrap_or_else(|| panic!("{path} is reported"));
            assert_eq!(finding.kind, FindingKind::UncontractedKey);
            assert!(finding.risk_surface);
        }
    }

    #[test]
    fn enforced_key_present_in_config_produces_no_finding() {
        let config = parse("strategy:\n  capital:\n    monthly_loss_halt_dkk: -50000\n");
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.enforced, 1);
        assert!(
            !findings
                .iter()
                .any(|finding| finding.path == "strategy.capital.monthly_loss_halt_dkk")
        );
    }

    #[test]
    fn concentration_limits_are_enforced_contract_keys() {
        let config = parse(
            "strategy:\n  concentration:\n    max_assets_per_exchange: 0\n    max_assets_per_currency: 0\n",
        );
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.enforced, 2);
        assert!(!findings.iter().any(|finding| {
            matches!(
                finding.path.as_str(),
                "strategy.concentration.max_assets_per_exchange"
                    | "strategy.concentration.max_assets_per_currency"
            )
        }));
    }

    #[test]
    fn key_missing_from_the_contract_is_reported_as_uncontracted() {
        let config = parse("strategy:\n  a_brand_new_knob: 3\n");
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.uncontracted, 1);
        let finding = findings
            .iter()
            .find(|finding| finding.path == "strategy.a_brand_new_knob")
            .expect("uncontracted key is reported");
        assert_eq!(finding.kind, FindingKind::UncontractedKey);
        assert!(
            finding.risk_surface,
            "an unclassified key under an audited root must fail loud"
        );
    }

    #[test]
    fn contracted_key_absent_from_config_is_reported_as_missing() {
        let config = parse("strategy:\n  enabled: true\n");
        let (summary, _) = audit_config(&config);
        assert!(summary.missing > 0);
    }

    #[test]
    fn subtree_contract_collapses_operator_maintained_data_maps() {
        let config = parse(
            "strategy:\n  markov:\n    symbol_aliases:\n      \"COST:xnys\": \"COST:xnas\"\n      \"HON:xnys\": \"HON:xnas\"\n",
        );
        let (summary, findings) = audit_config(&config);
        assert_eq!(
            summary.advisory, 1,
            "the alias map counts once, not once per alias"
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.path.starts_with("strategy.markov.symbol_aliases.")),
            "individual aliases must not produce findings"
        );
    }

    #[test]
    fn sequence_values_are_treated_as_a_single_setting() {
        let config = parse("strategy:\n  markov:\n    forecast_steps: [1, 2, 3, 5, 10]\n");
        let (summary, _) = audit_config(&config);
        assert_eq!(summary.advisory, 1);
    }

    /// Guards the contract against silent drift: adding a key to either shipped
    /// config without contracting it fails here rather than only at runtime.
    ///
    /// Both shipped configs are checked independently because environment
    /// injection may still differ without weakening contract coverage.
    fn assert_shipped_config_is_contracted(relative_path: &str) {
        let path = format!("{}/{relative_path}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("shipped config {relative_path} is readable: {err}"));
        let config = parse(&text);
        let (_, findings) = audit_config(&config);
        let uncontracted = findings
            .iter()
            .filter(|finding| finding.kind == FindingKind::UncontractedKey)
            .map(|finding| finding.path.clone())
            .collect::<Vec<_>>();
        assert!(
            uncontracted.is_empty(),
            "{relative_path} has uncontracted keys: {uncontracted:?}"
        );
        // An enforced key falling back to a code default is the drift that
        // actually changes trading behavior; absent advisory keys are fine.
        let missing_enforced = findings
            .iter()
            .filter(|finding| {
                finding.kind == FindingKind::ContractedKeyMissing && finding.risk_surface
            })
            .map(|finding| finding.path.clone())
            .collect::<Vec<_>>();
        assert!(
            missing_enforced.is_empty(),
            "{relative_path} omits enforced keys: {missing_enforced:?}"
        );
    }

    #[test]
    fn local_shipped_config_is_fully_contracted() {
        assert_shipped_config_is_contracted("config.yaml");
    }

    #[test]
    fn kubernetes_shipped_config_is_fully_contracted() {
        assert_shipped_config_is_contracted("deploy/k8s/base/config.k8s.yaml");
    }

    #[test]
    fn shipped_configs_share_quiver_scheduler_policy() {
        let local = parse(
            &std::fs::read_to_string(format!("{}/config.yaml", env!("CARGO_MANIFEST_DIR")))
                .expect("local config is readable"),
        );
        let kubernetes = parse(
            &std::fs::read_to_string(format!(
                "{}/deploy/k8s/base/config.k8s.yaml",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("Kubernetes config is readable"),
        );
        let path = ["strategy", "quiver"];

        assert_eq!(
            crate::config::yaml_at(&local, &path),
            crate::config::yaml_at(&kubernetes, &path),
            "local and Kubernetes Quiver policy must stay aligned"
        );
    }

    #[test]
    fn shipped_configs_share_daily_indicator_policy() {
        let local = parse(
            &std::fs::read_to_string(format!("{}/config.yaml", env!("CARGO_MANIFEST_DIR")))
                .expect("local config is readable"),
        );
        let kubernetes = parse(
            &std::fs::read_to_string(format!(
                "{}/deploy/k8s/base/config.k8s.yaml",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("Kubernetes config is readable"),
        );
        let path = ["strategy", "swing", "daily_indicators"];

        assert_eq!(
            crate::config::yaml_at(&local, &path),
            crate::config::yaml_at(&kubernetes, &path),
            "local and Kubernetes daily-indicator policy must stay aligned"
        );
    }

    #[test]
    fn shipped_configs_share_performance_benchmark_policy() {
        let local = parse(
            &std::fs::read_to_string(format!("{}/config.yaml", env!("CARGO_MANIFEST_DIR")))
                .expect("local config is readable"),
        );
        let kubernetes = parse(
            &std::fs::read_to_string(format!(
                "{}/deploy/k8s/base/config.k8s.yaml",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("Kubernetes config is readable"),
        );
        let path = ["strategy", "performance_benchmarks"];

        assert_eq!(
            crate::config::yaml_at(&local, &path),
            crate::config::yaml_at(&kubernetes, &path),
            "local and Kubernetes performance benchmark policy must stay aligned"
        );
    }
}
