//! Saxo's identity for an instrument, and every symbol known to name it.
//!
//! The system trades under its own symbols (`ALK-B:xcse`), and Saxo reports
//! the resulting holding under its compact display symbol (`ALKb:xcse`). Every
//! lookup that compares symbol text therefore sees no holding at all for a
//! share class: a SELL is refused for want of shares, a resting stop is not
//! cleared before the exit, and reservations under the other spelling are not
//! counted. Saxo identifies an instrument by `Uic` and `AssetType` together,
//! and both `broker_position_snapshots` and `daily_indicator_signals` carry
//! the pair, so the two spellings can be joined on it.
//!
//! The mapping only ever widens a lookup to symbols naming the same
//! instrument. When it cannot be built -- no identity recorded, or the read
//! fails -- a lookup falls back to the symbol alone, which is how every caller
//! behaved before it existed.

use std::collections::HashMap;

use serde_json::Value as JsonValue;

use crate::{
    db::sql_escape,
    state::{AppState, saxo_instrument_key, text_value},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InstrumentIdentity {
    pub uic: i64,
    /// Upper-cased, because the tables do not agree on case.
    pub asset_type: String,
}

impl InstrumentIdentity {
    /// For a typed instrument, such as a live Saxo position. The rule is
    /// `state::saxo_instrument_key`'s: `Uic` alone is not enough, because Saxo
    /// guarantees it unique only within an asset type.
    pub(crate) fn new(uic: i64, asset_type: &str) -> Option<Self> {
        saxo_instrument_key(&serde_json::json!({"uic": uic, "asset_type": asset_type}))
            .map(|(uic, asset_type)| Self { uic, asset_type })
    }

    /// For a database row carrying `uic` and `asset_type`.
    pub(crate) fn from_row(row: &JsonValue) -> Option<Self> {
        saxo_instrument_key(row).map(|(uic, asset_type)| Self { uic, asset_type })
    }
}

fn symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

/// The symbols that name one instrument, upper-cased, the requested one first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolAliases {
    pub identity: Option<InstrumentIdentity>,
    pub symbols: Vec<String>,
}

impl SymbolAliases {
    /// A quoted, escaped list for `UPPER(symbol) IN (...)`.
    pub(crate) fn sql_list(&self) -> String {
        self.symbols
            .iter()
            .map(|symbol| format!("'{}'", sql_escape(symbol)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Symbol-to-instrument map built from the two tables that record both.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InstrumentBook {
    identities: HashMap<String, InstrumentIdentity>,
}

impl InstrumentBook {
    pub(crate) async fn load(state: &AppState) -> Self {
        let mut book = Self::default();
        // Each symbol keeps the identity of its newest indicator run. A symbol
        // whose resolution changed must not stay joined to its old instrument.
        let indicators = state
            .select_json(
                "SELECT UPPER(symbol) AS symbol, uic, UPPER(asset_type) AS asset_type,
                        MAX(run_date) AS run_date
                 FROM daily_indicator_signals
                 WHERE uic > 0 AND asset_type IS NOT NULL AND asset_type <> ''
                 GROUP BY UPPER(symbol), uic, UPPER(asset_type)",
            )
            .await
            .unwrap_or_default();
        let mut newest: HashMap<String, String> = HashMap::new();
        for row in &indicators {
            let (symbol, run_date) = (text_value(row, "symbol"), text_value(row, "run_date"));
            let Some(identity) = InstrumentIdentity::from_row(row) else {
                continue;
            };
            if newest.get(&symbol).is_some_and(|seen| *seen >= run_date) {
                continue;
            }
            newest.insert(symbol.clone(), run_date);
            book.identities.insert(symbol, identity);
        }
        // The broker's own record wins for the symbol it reports.
        for row in &state
            .select_json(
                "SELECT symbol, uic, asset_type FROM broker_position_snapshots WHERE uic > 0",
            )
            .await
            .unwrap_or_default()
        {
            if let Some(identity) = InstrumentIdentity::from_row(row) {
                book.identities
                    .insert(symbol_key(&text_value(row, "symbol")), identity);
            }
        }
        book
    }

    pub(crate) fn identity(&self, symbol: &str) -> Option<&InstrumentIdentity> {
        self.identities.get(&symbol_key(symbol))
    }

    pub(crate) fn aliases(&self, symbol: &str) -> SymbolAliases {
        let requested = symbol_key(symbol);
        let identity = self.identity(&requested).cloned();
        let mut others = identity
            .as_ref()
            .map(|identity| {
                self.identities
                    .iter()
                    .filter(|(candidate, known)| **candidate != requested && *known == identity)
                    .map(|(candidate, _)| candidate.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        others.sort();
        let mut symbols = vec![requested];
        symbols.extend(others);
        SymbolAliases { identity, symbols }
    }
}

/// Convenience for a single lookup.
pub(crate) async fn symbol_aliases(state: &AppState, symbol: &str) -> SymbolAliases {
    InstrumentBook::load(state).await.aliases(symbol)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The 2026-10-23 state: ALK-B bought under the system symbol and held by
    /// Saxo as `ALKb:xcse`, with a resting stop under the broker symbol.
    pub(crate) async fn share_class_state(extra: &[&str]) -> AppState {
        use std::{path::PathBuf, sync::Once};

        use sqlx::any::AnyPoolOptions;

        static INSTALL_DRIVERS: Once = Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory instrument test database");
        for statement in [
            "CREATE TABLE broker_position_snapshots (
                symbol TEXT PRIMARY KEY, uic INTEGER, asset_type TEXT, quantity REAL NOT NULL,
                open_price_local REAL, can_be_closed INTEGER
            )",
            "CREATE TABLE daily_indicator_signals (
                id TEXT PRIMARY KEY, run_date TEXT NOT NULL, symbol TEXT NOT NULL,
                uic BIGINT, asset_type TEXT, close DOUBLE PRECISION, atr14 DOUBLE PRECISION
            )",
            "INSERT INTO broker_position_snapshots
                (symbol, uic, asset_type, quantity, open_price_local, can_be_closed) VALUES
                ('ALKb:xcse', 19782, 'Stock', 28, 245.0, 1),
                ('NOVOb:xcse', 15629, 'Stock', 10, 400.0, 1)",
            "INSERT INTO daily_indicator_signals (id, run_date, symbol, uic, asset_type, close, atr14) VALUES
                ('a', '2026-10-22', 'ALK-B:xcse', 19782, 'Stock', 240.6, 6.0),
                ('b', '2026-10-22', 'NOVO-B:xcse', 15629, 'CfdOnStock', 400.0, 10.0),
                ('c', '2026-10-01', 'MOVED:xcse', 19782, 'Stock', 1.0, 1.0),
                ('d', '2026-10-22', 'MOVED:xcse', 99999, 'Stock', 1.0, 1.0)",
        ]
        .into_iter()
        .chain(extra.iter().copied())
        {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("seed instrument test database");
        }
        AppState {
            config_path: PathBuf::from("instrument-identity-test.yaml"),
            config: serde_yaml::from_str("saxo:\n  environment: sim\n")
                .expect("parse instrument test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    #[tokio::test]
    async fn both_spellings_of_a_share_class_name_one_instrument() {
        let book = InstrumentBook::load(&share_class_state(&[]).await).await;
        let expected = Some(InstrumentIdentity::new(19782, "Stock").unwrap());
        assert_eq!(
            book.aliases("ALK-B:xcse"),
            SymbolAliases {
                identity: expected.clone(),
                symbols: vec!["ALK-B:XCSE".to_string(), "ALKB:XCSE".to_string()]
            }
        );
        assert_eq!(
            book.aliases("ALKb:xcse"),
            SymbolAliases {
                identity: expected,
                symbols: vec!["ALKB:XCSE".to_string(), "ALK-B:XCSE".to_string()]
            },
            "the broker spelling reaches orders recorded under the system one"
        );
    }

    #[tokio::test]
    async fn an_alias_needs_the_same_asset_type_and_the_newest_resolution() {
        let book = InstrumentBook::load(&share_class_state(&[]).await).await;
        assert_eq!(
            book.aliases("NOVO-B:xcse").symbols,
            vec!["NOVO-B:XCSE".to_string()],
            "a shared uic in another asset type is a different instrument"
        );
        assert_eq!(
            book.aliases("MOVED:xcse").symbols,
            vec!["MOVED:XCSE".to_string()],
            "a symbol that once resolved to ALK-B's uic is not joined to it now"
        );
        assert_eq!(
            book.aliases("UNKNOWN:xcse"),
            SymbolAliases {
                identity: None,
                symbols: vec!["UNKNOWN:XCSE".to_string()]
            }
        );
    }

    #[tokio::test]
    async fn a_missing_table_falls_back_to_the_symbol_alone() {
        use sqlx::any::AnyPoolOptions;

        let state = share_class_state(&["DROP TABLE daily_indicator_signals"]).await;
        let book = InstrumentBook::load(&state).await;
        assert_eq!(
            book.aliases("ALK-B:xcse").symbols,
            vec!["ALK-B:XCSE".to_string()]
        );
        let empty = AppState {
            pool: AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("open empty database"),
            ..state
        };
        assert_eq!(
            symbol_aliases(&empty, "ALK-B:xcse").await.sql_list(),
            "'ALK-B:XCSE'"
        );
    }
}
