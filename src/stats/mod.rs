//! Statistics tracking module.
//!
//! Statistics are persisted to SQLite. Legacy stats.json files are read once on
//! startup as a recovery fallback when SQLite is missing or unusable (see BREAK-03);
//! writes are SQLite-only as of v3.0.0.

mod aggregation;
mod cache;
mod persistence;
mod provider;
mod session;

#[cfg(test)]
mod tests;

// Re-exports: public API surface
//
// Items used by both the binary and library targets:
pub use aggregation::{AllTimeStats, DailyStats, MonthlyStats};
pub use cache::{calculate_token_rates_with_db_and_transcript, TokenRateMetrics};
pub use persistence::{get_or_load_stats_data, update_stats_data};
pub use session::{get_session_duration_by_mode, SessionStats};

// Items used only via the library crate (not the binary directly).
// Suppressed to prevent false unused-import warnings when compiling the binary target.
#[allow(unused_imports)]
pub use aggregation::get_daily_total;
#[allow(unused_imports)]
pub use cache::{calculate_cache_metrics, calculate_token_rates, calculate_token_rates_with_db};
#[allow(unused_imports)]
pub use provider::StatsProvider;
#[allow(unused_imports)]
pub use session::get_session_duration;

// Re-export test-only helpers
#[cfg(test)]
pub use cache::calculate_token_rates_from_raw;

use crate::common::{current_timestamp, get_data_dir};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Persistent stats tracking structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsData {
    pub version: String,
    pub created: String,
    pub last_updated: String,
    pub sessions: HashMap<String, SessionStats>,
    pub daily: HashMap<String, DailyStats>,
    pub monthly: HashMap<String, MonthlyStats>,
    pub all_time: AllTimeStats,
}

impl Default for StatsData {
    fn default() -> Self {
        let now = current_timestamp();
        StatsData {
            version: "1.0".to_string(),
            created: now.clone(),
            last_updated: now.clone(),
            sessions: HashMap::new(),
            daily: HashMap::new(),
            monthly: HashMap::new(),
            all_time: AllTimeStats {
                total_cost: 0.0,
                sessions: 0,
                since: now,
            },
        }
    }
}

impl StatsData {
    pub fn get_stats_file_path() -> PathBuf {
        get_data_dir().join("stats.json")
    }

    pub fn get_sqlite_path() -> Result<PathBuf> {
        Ok(get_data_dir().join("stats.db"))
    }
}
