//! StatsProvider: DataProvider implementation for stats variables.
//!
//! Wraps the stats system as a DataProvider, producing all layout variables
//! with `stats_` prefixed keys. The provider holds owned, Clone-able data
//! only (no database connections or references) to satisfy Send + Sync.

use crate::provider::{DataProvider, ProviderResult};
use std::collections::HashMap;
use std::time::Duration;

/// Data provider that produces stats-related layout variables.
///
/// All variables are returned with a `stats_` prefix. Every key is always
/// present in the output HashMap; unavailable values use empty strings.
/// Raw numeric values are returned (e.g., "12.50" not "$12.50") so that
/// formatting (currency symbols, colors) stays in the layout engine.
#[allow(dead_code)] // Public API - used by library consumers, not the binary directly
pub struct StatsProvider {
    /// Current session ID (None if no active session).
    session_id: Option<String>,
    /// Total cost for the current session in USD.
    total_cost: f64,
    /// Today's total cost across all sessions in USD.
    daily_total: f64,
    /// Lines added in the current session.
    lines_added: u64,
    /// Lines removed in the current session.
    lines_removed: u64,
    /// Path to the transcript file (for fallback duration parsing).
    transcript_path: Option<String>,
    /// Path to the SQLite database (for token rate queries).
    db_path: Option<String>,
}

#[allow(dead_code)] // Public API - used by library consumers
impl StatsProvider {
    /// Create a new StatsProvider with all data needed for variable production.
    ///
    /// Pass `None` for optional fields when data is unavailable.
    /// The provider will produce empty strings for any variables it cannot compute.
    pub fn new(
        session_id: Option<String>,
        total_cost: f64,
        daily_total: f64,
        lines_added: u64,
        lines_removed: u64,
        transcript_path: Option<String>,
        db_path: Option<String>,
    ) -> Self {
        Self {
            session_id,
            total_cost,
            daily_total,
            lines_added,
            lines_removed,
            transcript_path,
            db_path,
        }
    }

    /// Compute the session duration in seconds using configured burn rate mode.
    ///
    /// Falls back to transcript parsing if session-based duration is unavailable.
    fn get_duration(&self) -> Option<u64> {
        self.session_id
            .as_deref()
            .and_then(super::get_session_duration_by_mode)
            .or_else(|| {
                self.transcript_path
                    .as_deref()
                    .and_then(crate::utils::parse_duration)
            })
    }

    /// Compute burn rate (cost per hour) if duration exceeds the configured minimum.
    ///
    /// Returns `(burn_rate_value, was_reset)` where `was_reset` indicates
    /// whether an auto-reset was detected during this session.
    fn compute_burn_rate(&self, duration: u64) -> Option<f64> {
        let config = crate::config::get_config();
        if duration > config.burn_rate.min_duration_seconds && self.total_cost > 0.0 {
            Some((self.total_cost * 3600.0) / duration as f64)
        } else {
            None
        }
    }

    /// Detect whether the current session was auto-reset.
    ///
    /// In auto_reset mode, the database recreates the session with a new start_time
    /// after an inactivity period. We detect this by comparing the in-memory start_time
    /// with the database start_time.
    fn detect_auto_reset(&self) -> bool {
        let config = crate::config::get_config();
        if config.burn_rate.mode != "auto_reset" {
            return false;
        }

        let session_id = match self.session_id.as_deref() {
            Some(id) => id,
            None => return false,
        };

        let db_path = match self.db_path.as_deref() {
            Some(p) => p,
            None => return false,
        };

        let db_path = std::path::Path::new(db_path);
        if !db_path.exists() {
            return false;
        }

        let db = match crate::database::SqliteDatabase::new(db_path) {
            Ok(db) => db,
            Err(_) => return false,
        };

        // Get DB start_time
        let db_start = match db.get_session_start_time(session_id) {
            Some(t) => t,
            None => return false,
        };

        // Get in-memory start_time
        let stats_data = super::persistence::get_or_load_stats_data();
        let in_memory_start = match stats_data.sessions.get(session_id) {
            Some(session) => match session.start_time.as_ref() {
                Some(t) => t.clone(),
                None => return false,
            },
            None => return false,
        };

        // Compare timestamps -- difference > 1 second means reset occurred
        if let (Some(db_time), Some(mem_time)) = (
            crate::utils::parse_iso8601_to_unix(&db_start),
            crate::utils::parse_iso8601_to_unix(&in_memory_start),
        ) {
            db_time.abs_diff(mem_time) > 1
        } else {
            false
        }
    }

    /// Fill token rate variables from the database.
    fn fill_token_rates(&self, vars: &mut HashMap<String, String>) {
        let session_id = match self.session_id.as_deref() {
            Some(id) => id,
            None => return,
        };

        let db_path_str = match self.db_path.as_deref() {
            Some(p) => p,
            None => return,
        };

        let db_path = std::path::Path::new(db_path_str);
        if !db_path.exists() {
            return;
        }

        let db = match crate::database::SqliteDatabase::new(db_path) {
            Ok(db) => db,
            Err(_) => return,
        };

        let metrics = super::cache::calculate_token_rates_with_db_and_transcript(
            session_id,
            &db,
            self.transcript_path.as_deref(),
        );

        if let Some(m) = metrics {
            vars.insert("stats_token_rate".into(), format!("{:.1}", m.total_rate));
            vars.insert(
                "stats_token_input_rate".into(),
                format!("{:.1}", m.input_rate),
            );
            vars.insert(
                "stats_token_output_rate".into(),
                format!("{:.1}", m.output_rate),
            );
            vars.insert(
                "stats_token_cache_rate".into(),
                format!("{:.1}", m.cache_read_rate),
            );

            if let Some(hit) = m.cache_hit_ratio {
                vars.insert(
                    "stats_token_cache_hit".into(),
                    format!("{:.1}", hit * 100.0),
                );
            }
            if let Some(roi) = m.cache_roi {
                if roi.is_finite() {
                    vars.insert("stats_token_cache_roi".into(), format!("{:.1}", roi));
                } else {
                    vars.insert("stats_token_cache_roi".into(), "inf".into());
                }
            }

            vars.insert(
                "stats_token_session_total".into(),
                m.session_total_tokens.to_string(),
            );
            vars.insert(
                "stats_token_daily_total".into(),
                m.daily_total_tokens.to_string(),
            );
        }
    }
}

impl DataProvider for StatsProvider {
    fn name(&self) -> &str {
        "stats"
    }

    fn priority(&self) -> u32 {
        50
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(200)
    }

    fn is_available(&self) -> bool {
        true // Stats are always available (empty values for missing data)
    }

    fn collect(&self) -> ProviderResult {
        let mut vars = HashMap::new();

        // -- Cost variables --
        if self.total_cost > 0.0 {
            vars.insert("stats_cost".into(), format!("{:.2}", self.total_cost));
            // Short cost: fewer decimal places for larger values
            let short = if self.total_cost < 10.0 {
                format!("{:.2}", self.total_cost)
            } else {
                format!("{:.1}", self.total_cost)
            };
            vars.insert("stats_cost_short".into(), short);
        } else {
            vars.insert("stats_cost".into(), String::new());
            vars.insert("stats_cost_short".into(), String::new());
        }

        // -- Burn rate --
        let duration = self.get_duration();
        let burn_rate = duration.and_then(|d| self.compute_burn_rate(d));

        if let Some(rate) = burn_rate {
            if rate > 0.0 {
                vars.insert("stats_burn_rate".into(), format!("{:.2}", rate));
            } else {
                vars.insert("stats_burn_rate".into(), String::new());
            }
        } else {
            vars.insert("stats_burn_rate".into(), String::new());
        }

        // -- Burn rate reset detection --
        if self.detect_auto_reset() {
            vars.insert("stats_burn_rate_reset".into(), "reset".into());
        } else {
            vars.insert("stats_burn_rate_reset".into(), String::new());
        }

        // -- Daily total --
        if self.daily_total > 0.0 {
            vars.insert(
                "stats_daily_total".into(),
                format!("{:.2}", self.daily_total),
            );
        } else {
            vars.insert("stats_daily_total".into(), String::new());
        }

        // -- Session time --
        if let Some(d) = duration {
            if d < 60 {
                vars.insert("stats_session_time".into(), format!("{}s", d));
            } else if d < 3600 {
                vars.insert("stats_session_time".into(), format!("{}m", d / 60));
            } else {
                vars.insert(
                    "stats_session_time".into(),
                    format!("{}h{}m", d / 3600, (d % 3600) / 60),
                );
            }
        } else {
            vars.insert("stats_session_time".into(), String::new());
        }

        // -- Lines changed --
        if self.lines_added > 0 || self.lines_removed > 0 {
            vars.insert(
                "stats_lines".into(),
                format!("+{} -{}", self.lines_added, self.lines_removed),
            );
            vars.insert(
                "stats_lines_added".into(),
                format!("+{}", self.lines_added),
            );
            vars.insert(
                "stats_lines_removed".into(),
                format!("-{}", self.lines_removed),
            );
        } else {
            vars.insert("stats_lines".into(), String::new());
            vars.insert("stats_lines_added".into(), String::new());
            vars.insert("stats_lines_removed".into(), String::new());
        }

        // -- Token rates (all default to empty, filled if available) --
        vars.insert("stats_token_rate".into(), String::new());
        vars.insert("stats_token_input_rate".into(), String::new());
        vars.insert("stats_token_output_rate".into(), String::new());
        vars.insert("stats_token_cache_rate".into(), String::new());
        vars.insert("stats_token_cache_hit".into(), String::new());
        vars.insert("stats_token_cache_roi".into(), String::new());
        vars.insert("stats_token_session_total".into(), String::new());
        vars.insert("stats_token_daily_total".into(), String::new());

        // Fill in token rate values if database and session are available
        self.fill_token_rates(&mut vars);

        Ok(vars)
    }
}
