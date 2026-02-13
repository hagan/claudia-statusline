//! Statistics tracking module.
//!
//! This module provides persistent statistics tracking for Claude Code sessions,
//! including costs, line changes, and usage metrics. Statistics are stored in
//! both JSON and SQLite formats for reliability and concurrent access.
//!
//! **Note**: Advanced features (token rates, rolling window) require SQLite.
//! JSON backup mode is deprecated and will be removed in v3.0.

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
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Static flag to ensure deprecation warning is only shown once per process
pub(crate) static JSON_BACKUP_WARNING_SHOWN: OnceLock<bool> = OnceLock::new();

/// Show deprecation warning for json_backup mode (only once per process)
pub(crate) fn warn_json_backup_deprecated() {
    JSON_BACKUP_WARNING_SHOWN.get_or_init(|| {
        warn!(
            "DEPRECATION: json_backup mode is deprecated and will be removed in v3.0. \
             Advanced features (token rates, rolling window, context learning) require SQLite. \
             Run 'statusline migrate --finalize' to migrate to SQLite-only mode."
        );
        // Also print to stderr for visibility (log might be filtered)
        eprintln!(
            "\x1b[33m⚠ DEPRECATION:\x1b[0m json_backup mode is deprecated. \
             Token rates and other advanced features require SQLite. \
             Run 'statusline migrate --finalize' to switch to SQLite-only mode."
        );
        true
    });
}

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
