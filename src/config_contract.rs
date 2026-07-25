//! Config contract audit.
//!
//! A configuration key that was never wired into the runtime reads exactly like
//! one that is enforced. That ambiguity is how the retired 2026-05-05
//! `strategy.capital.cash_buffer` override survived, and a 2026-07-25 review
//! found the same shape across the strategy risk knobs: `risk_per_trade_pct`,
//! `max_assets_per_sector`, the whole `strategy.ladder.*` block, and the
//! `taxation.share_income` brackets are all present in `config.yaml` and absent
//! from the code.
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
/// Operator-maintained data maps (`symbol_aliases`, `benchmark_indices`) grow
/// new members routinely. Contracting them per member would make every new alias
/// a drift finding, so they are contracted once at the parent.
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

/// An unused key that reads like an active risk control but is not one.
const fn unused_risk(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Unused,
        scope: ContractScope::Leaf,
        risk_surface: true,
        note,
    }
}

const fn unused_risk_subtree(path: &'static [&'static str], note: &'static str) -> ContractEntry {
    ContractEntry {
        path,
        status: ContractStatus::Unused,
        scope: ContractScope::Subtree,
        risk_surface: true,
        note,
    }
}

/// The contract. Statuses were established by reading each key's call sites on
/// 2026-07-25; update an entry in the same change that changes its wiring.
const CONTRACT: &[ContractEntry] = &[
    // ---- strategy (top level) ----
    unused_risk(
        &["strategy", "enabled"],
        "No master switch is read; setting this to false does not stop the strategy cycle.",
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
    unused_risk(
        &["strategy", "min_selected_assets"],
        "No breadth floor is enforced when queueing.",
    ),
    unused_risk(
        &["strategy", "max_selected_assets"],
        "No breadth ceiling is enforced when queueing.",
    ),
    unused_risk(
        &["strategy", "max_assets_per_sector"],
        "No concentration gate exists; the runtime has no sector metadata source at all.",
    ),
    unused_risk(
        &["strategy", "estimated_slippage_bps"],
        "No cost model consumes it; slippage is not estimated before queueing.",
    ),
    unused_risk(
        &["strategy", "cost_guard_multiple"],
        "No cost model consumes it.",
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
    // Read by src/quiver.rs but supplied only by the Kubernetes config, so a
    // local run silently uses the code defaults.
    advisory(&["strategy", "quiver", "enabled"], "Quiver cycle switch."),
    advisory(&["strategy", "quiver", "timezone"], "Quiver run cadence."),
    advisory(&["strategy", "quiver", "daily_time"], "Quiver run cadence."),
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
    advisory(
        &["strategy", "swing", "max_holdings"],
        "Published to the model and Hermes as max_positions; not a queueing gate.",
    ),
    advisory(
        &["strategy", "swing", "position_decision_stale_after_days"],
        "Dashboard decision-chip staleness horizon.",
    ),
    unused_risk(
        &["strategy", "swing", "min_holding_weight_pct"],
        "No per-position weight floor is enforced.",
    ),
    unused_risk(
        &["strategy", "swing", "max_holding_weight_pct"],
        "No per-position weight ceiling is enforced.",
    ),
    unused_risk(
        &["strategy", "swing", "cash_buffer_pct"],
        "Third cash-buffer path and the second dead one. Only strategy.capital.min_cash_buffer_pct bounds the BUY budget; strategy.capital.cash_buffer was retired 2026-07-22 and this one was never read.",
    ),
    unused_risk(
        &["strategy", "swing", "risk_per_trade_pct"],
        "Position sizing is not risk-based; quantity comes from the model suggestion bounded by budget, minimum trade value, and the commission floor.",
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
    // Only max_report_age_hours is read, and it is absent from both shipped
    // configs. Every knob an operator would reach for here is inert.
    unused_risk(
        &["strategy", "swing", "trading_manager", "enabled"],
        "No switch is read; setting this to false does not stop the Trading Manager.",
    ),
    unused(
        &["strategy", "swing", "trading_manager", "use_ai"],
        "AI report use is decided by the decision-report pipeline, not this flag.",
    ),
    unused(
        &["strategy", "swing", "trading_manager", "due_window_minutes"],
        "Report freshness uses max_report_age_hours; this window is not read.",
    ),
    unused_risk(
        &["strategy", "swing", "trading_manager", "max_symbols"],
        "No per-run candidate cap is applied from configuration.",
    ),
    advisory(
        &[
            "strategy",
            "swing",
            "trading_manager",
            "max_report_age_hours",
        ],
        "Scheduled-report freshness window. Read by the Trading Manager but supplied by neither shipped config, so the code default applies.",
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
    unused_risk_subtree(
        &["strategy", "swing", "journal", "benchmark_indices"],
        "No benchmark comparison is computed in Rust; performance is reported without a benchmark.",
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
    unused_risk(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "europe_open_followup",
            "enabled",
        ],
        "No switch is read; setting this to false does not disable the EU pulse.",
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
    unused_risk(
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            "us_open_followup",
            "enabled",
        ],
        "No switch is read; setting this to false does not disable the US pulse.",
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
    // The entire ladder/bracket feature is configured and unimplemented. The
    // stop-loss members are called out individually because they read as active
    // downside protection and are not.
    unused(
        &["strategy", "ladder", "rung_count"],
        "Ladder entries are not implemented.",
    ),
    unused(
        &["strategy", "ladder", "min_rung_value_dkk"],
        "Ladder entries are not implemented.",
    ),
    unused_risk(
        &["strategy", "ladder", "submit_bracket_with_entry"],
        "No bracket order is ever submitted with an entry.",
    ),
    unused_risk(
        &["strategy", "ladder", "submit_stop_loss_after_fill"],
        "No protective stop is placed after a fill. Automatic stop placement is unbuilt; see wiki/urgent-todo.md U1.",
    ),
    unused_risk(
        &["strategy", "ladder", "submit_take_profit_after_fill"],
        "No take-profit order is placed after a fill.",
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
    advisory(
        &["strategy", "ladder", "stop_loss_atr_multiple"],
        "Sets the protective-stop distance in the read-only coverage audit's proposed stop level. Becomes enforced when automatic stop placement lands.",
    ),
    unused(
        &["strategy", "ladder", "take_profit_rung_multiple"],
        "Take-profit targets are not implemented.",
    ),
    unused(
        &["strategy", "ladder", "max_take_profit_atr_multiple"],
        "Take-profit targets are not implemented.",
    ),
    unused_risk(
        &["strategy", "ladder", "min_position_weight"],
        "No per-position weight bound is enforced.",
    ),
    unused_risk(
        &["strategy", "ladder", "max_position_weight"],
        "No per-position weight bound is enforced.",
    ),
    unused_risk(
        &["strategy", "ladder", "session_flatten_enabled"],
        "No session flatten runs; nothing exits on a schedule.",
    ),
    unused_risk(
        &[
            "strategy",
            "ladder",
            "flatten_minutes_before_tradable_close",
        ],
        "No session flatten runs.",
    ),
    unused_risk(
        &["strategy", "ladder", "trail_stop_atr_multiple"],
        "No trailing stop exists.",
    ),
    // ---- risk ----
    enforced_subtree(
        &["risk", "excluded_symbols"],
        "Blocks candidates in the Trading Manager.",
    ),
    unused_risk(
        &["risk", "excluded_symbols_csv"],
        "Only the list form is read; this ENV pointer is never resolved, so exclusions supplied through RISK_EXCLUDED_SYMBOLS have no effect.",
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
    unused_risk(
        &["risk", "max_position_weight"],
        "No portfolio-level position weight cap is enforced.",
    ),
    advisory(
        &["risk", "allow_shorting"],
        "Published in the Hermes goal contract; the runtime has no short path regardless.",
    ),
    // ---- taxation ----
    unused(
        &["taxation", "share_income", "currency"],
        "Tax estimation is not implemented.",
    ),
    unused_risk_subtree(
        &["taxation", "share_income", "brackets"],
        "estimated_tax_dkk is hardcoded to 0.0, so after-tax P/L equals pre-tax P/L and goal progress is overstated.",
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
    fn unused_risk_key_present_in_config_produces_a_finding() {
        let config = parse("strategy:\n  swing:\n    risk_per_trade_pct: 0.01\n");
        let (summary, findings) = audit_config(&config);
        assert_eq!(summary.unused, 1);
        assert_eq!(summary.unused_risk_surface, 1);
        let finding = findings
            .iter()
            .find(|finding| finding.path == "strategy.swing.risk_per_trade_pct")
            .expect("risk_per_trade_pct is reported");
        assert_eq!(finding.kind, FindingKind::UnusedKeyPresent);
        assert!(finding.risk_surface);
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
    /// The two configs differ on purpose (Kubernetes carries `strategy.quiver`,
    /// local does not), so both are checked.
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
}
