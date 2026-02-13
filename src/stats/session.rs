//! Session statistics tracking and burn rate duration logic.
//!
//! Contains the `SessionStats` struct and `StatsData` methods for session
//! updates, max token tracking, and duration calculation by mode.

use super::StatsData;
use crate::common::{current_date, current_month, current_timestamp};
use crate::database::SqliteDatabase;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub last_updated: String,
    pub cost: f64,
    pub lines_added: u64,
    pub lines_removed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>, // ISO 8601 timestamp of session start
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens_observed: Option<u32>, // For adaptive context learning
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_time_seconds: Option<u64>, // Accumulated active time (for burn_rate.mode = "active_time")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>, // ISO 8601 timestamp of last activity (for active_time tracking)
}

impl StatsData {
    pub fn update_session(
        &mut self,
        session_id: &str,
        update: crate::database::SessionUpdate,
    ) -> (f64, f64) {
        let today = current_date();
        let month = current_month();
        let now = current_timestamp();

        // Update SQLite database directly with all parameters including new migration v5 fields
        // This ensures model_name, workspace_dir, device_id, and token breakdown are persisted immediately
        // SqliteDatabase::new() will create the database if it doesn't exist
        // Note: max_tokens_observed will be updated separately from main.rs/lib.rs
        //
        // IMPORTANT: Auto-reset mode deletes then RECREATES the session in the same transaction,
        // so we can't check existence. Instead, compare start_time to detect if session was reset.
        let mut session_was_reset = false;
        let config = crate::config::get_config();

        if let Ok(db_path) = Self::get_sqlite_path() {
            if let Ok(db) = SqliteDatabase::new(&db_path) {
                if let Err(e) = db.update_session(session_id, update.clone()) {
                    log::warn!("Failed to persist session {} to SQLite: {}", session_id, e);
                } else if config.burn_rate.mode == "auto_reset" {
                    // Only check for reset if we're in auto_reset mode
                    // Check if session was reset by comparing start_time
                    // Auto-reset deletes+recreates session, giving it a new start_time
                    if let Some(db_start_time) = db.get_session_start_time(session_id) {
                        // Compare with in-memory start_time
                        if let Some(in_memory_session) = self.sessions.get(session_id) {
                            if let Some(ref in_memory_start) = in_memory_session.start_time {
                                // If start times differ by more than 1 second, session was reset
                                // (allow small differences due to timestamp precision)
                                if let (Some(db_time), Some(mem_time)) = (
                                    crate::utils::parse_iso8601_to_unix(&db_start_time),
                                    crate::utils::parse_iso8601_to_unix(in_memory_start),
                                ) {
                                    let time_diff = db_time.abs_diff(mem_time);

                                    if time_diff > 1 {
                                        // More than 1 second difference = reset
                                        session_was_reset = true;
                                        log::info!(
                                            "Session {} was auto-reset (start_time changed: {} -> {})",
                                            session_id,
                                            in_memory_start,
                                            db_start_time
                                        );
                                        self.sessions.remove(session_id);
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                log::warn!(
                    "Failed to open SQLite database at {:?} for session update",
                    db_path
                );
            }
        } else {
            log::warn!("Failed to get SQLite path for session update");
        }

        // Calculate delta from last known session cost
        // If session was reset, treat as new session (delta = full value)
        let last_cost = if session_was_reset {
            0.0
        } else {
            self.sessions.get(session_id).map(|s| s.cost).unwrap_or(0.0)
        };

        let cost_delta = update.cost - last_cost;

        // Calculate line deltas from previous values
        // If session was reset, treat as new session (delta = full value)
        let last_lines_added = if session_was_reset {
            0
        } else {
            self.sessions
                .get(session_id)
                .map(|s| s.lines_added)
                .unwrap_or(0)
        };
        let last_lines_removed = if session_was_reset {
            0
        } else {
            self.sessions
                .get(session_id)
                .map(|s| s.lines_removed)
                .unwrap_or(0)
        };
        let lines_added_delta = (update.lines_added as i64) - (last_lines_added as i64);
        let lines_removed_delta = (update.lines_removed as i64) - (last_lines_removed as i64);

        // Query active_time_seconds and last_activity from SQLite for JSON backup persistence
        let (active_time_seconds, last_activity) = if let Ok(db_path) = Self::get_sqlite_path() {
            if let Ok(db) = SqliteDatabase::new(&db_path) {
                db.get_session_active_time(session_id)
                    .unwrap_or((None, None))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Always update session metadata (even with zero/negative deltas)
        // This ensures cost corrections and metadata refreshes are persisted
        // IMPORTANT: Also populate active_time_seconds and last_activity for JSON backup
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.cost = update.cost;
            session.lines_added = update.lines_added;
            session.lines_removed = update.lines_removed;
            session.last_updated = now.clone();
            session.active_time_seconds = active_time_seconds;
            session.last_activity = last_activity.clone();
        } else {
            self.sessions.insert(
                session_id.to_string(),
                SessionStats {
                    last_updated: now.clone(),
                    cost: update.cost,
                    lines_added: update.lines_added,
                    lines_removed: update.lines_removed,
                    start_time: Some(now.clone()), // Track when session started
                    max_tokens_observed: None,     // Will be updated by adaptive learning
                    active_time_seconds,           // Populated from SQLite
                    last_activity,                 // Populated from SQLite
                },
            );
            self.all_time.sessions += 1;
        }

        // IMPORTANT: Check if this session exists for this month BEFORE modifying daily.sessions
        // We must query SQLite for the authoritative answer, since daily.sessions vectors
        // are not persisted and will be empty after a restart (see database.rs:462)
        let mut session_seen_this_month = false;

        // Try to check SQLite first (authoritative source)
        if let Ok(db_path) = Self::get_sqlite_path() {
            if db_path.exists() {
                if let Ok(db) = SqliteDatabase::new(&db_path) {
                    session_seen_this_month = db
                        .session_active_in_month(session_id, &month)
                        .unwrap_or(false);
                }
            }
        }

        // Fallback: If SQLite check failed, check in-memory daily.sessions (works for non-restarted sessions)
        if !session_seen_this_month {
            for (date_key, daily_stats) in &self.daily {
                if date_key.starts_with(&month)
                    && daily_stats.sessions.contains(&session_id.to_string())
                {
                    session_seen_this_month = true;
                    break;
                }
            }
        }

        // Update daily stats
        let daily = self
            .daily
            .entry(today.clone())
            .or_insert_with(|| super::aggregation::DailyStats {
                total_cost: 0.0,
                sessions: Vec::new(),
                lines_added: 0,
                lines_removed: 0,
            });

        let is_new_session = !daily.sessions.contains(&session_id.to_string());
        if is_new_session {
            daily.sessions.push(session_id.to_string());
        }
        daily.total_cost += cost_delta;
        // Use deltas instead of absolute totals to avoid double-counting
        daily.lines_added = (daily.lines_added as i64 + lines_added_delta).max(0) as u64;
        daily.lines_removed = (daily.lines_removed as i64 + lines_removed_delta).max(0) as u64;

        // Update monthly stats
        let monthly = self
            .monthly
            .entry(month.clone())
            .or_insert_with(|| super::aggregation::MonthlyStats {
                total_cost: 0.0,
                sessions: 0,
                lines_added: 0,
                lines_removed: 0,
            });

        // Increment monthly session count only if this is a new session for the month
        // Note: When loading from SQLite, daily.sessions vectors are empty (we don't persist them),
        // so we rely on the loaded monthly.sessions value and only increment when we see a truly new session
        // We checked session_seen_this_month BEFORE modifying daily.sessions to avoid false positives
        if !session_seen_this_month && is_new_session {
            monthly.sessions += 1;
        }

        monthly.total_cost += cost_delta;
        // Use deltas instead of absolute totals to avoid double-counting
        monthly.lines_added = (monthly.lines_added as i64 + lines_added_delta).max(0) as u64;
        monthly.lines_removed = (monthly.lines_removed as i64 + lines_removed_delta).max(0) as u64;

        // Update all-time stats
        self.all_time.total_cost += cost_delta;

        // Update last modified
        self.last_updated = now;

        // No need to save here - the caller (update_stats_data) handles saving
        // with proper file locking

        // Return current daily and monthly totals
        let daily_total = self.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
        let monthly_total = self
            .monthly
            .get(&month)
            .map(|m| m.total_cost)
            .unwrap_or(0.0);

        (daily_total, monthly_total)
    }

    /// Update max_tokens_observed for a session (adaptive learning)
    /// This should be called after update_session when context usage is calculated
    pub fn update_max_tokens(&mut self, session_id: &str, current_tokens: u32) {
        // Update in-memory stats
        if let Some(session) = self.sessions.get_mut(session_id) {
            let new_max = session.max_tokens_observed.unwrap_or(0).max(current_tokens);
            session.max_tokens_observed = Some(new_max);
        }

        // Persist to SQLite database using dedicated method
        if let Ok(db_path) = Self::get_sqlite_path() {
            if let Ok(db) = SqliteDatabase::new(&db_path) {
                if let Err(e) = db.update_max_tokens_observed(session_id, current_tokens) {
                    log::warn!(
                        "Failed to update max_tokens_observed for session {} in SQLite: {}",
                        session_id,
                        e
                    );
                }
            } else {
                log::warn!(
                    "Failed to open SQLite database at {:?} for max_tokens update",
                    db_path
                );
            }
        } else {
            log::warn!("Failed to get SQLite path for max_tokens update");
        }
    }
}

// ── Burn rate duration functions ──────────────────────────────────────────

pub fn get_session_duration(session_id: &str) -> Option<u64> {
    let data = super::persistence::get_or_load_stats_data();

    data.sessions.get(session_id).and_then(|session| {
        session.start_time.as_ref().and_then(|start_time| {
            // Parse start time as ISO 8601
            crate::utils::parse_iso8601_to_unix(start_time).and_then(|start_unix| {
                // Get current time
                let now_unix = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()?
                    .as_secs();

                // Return duration in seconds
                Some(now_unix.saturating_sub(start_unix))
            })
        })
    })
}

/// Get session duration in seconds based on configured burn_rate mode
///
/// Respects the `burn_rate.mode` configuration setting:
/// - "wall_clock": Total elapsed time from session start to now (default)
/// - "active_time": Only time spent actively conversing (excludes idle gaps)
/// - "auto_reset": Wall-clock time within current session (resets after inactivity)
pub fn get_session_duration_by_mode(session_id: &str) -> Option<u64> {
    let config = crate::config::get_config();
    let mode = &config.burn_rate.mode;

    match mode.as_str() {
        "active_time" => {
            // Query database for active_time_seconds via SqliteDatabase
            if let Ok(db_path) = StatsData::get_sqlite_path() {
                if db_path.exists() {
                    if let Ok(db) = SqliteDatabase::new(&db_path) {
                        if let Some((Some(t), _last_activity)) =
                            db.get_session_active_time(session_id)
                        {
                            return Some(t);
                        }
                    }
                }
            }
            // Fallback to wall_clock if database query fails
            get_session_duration(session_id)
        }
        "auto_reset" => {
            // Auto-reset mode: Session is archived and recreated after inactivity threshold
            // start_time is reset to current time when recreated, so wall-clock duration
            // represents the current work period (time since last reset)
            get_session_duration(session_id)
        }
        _ => {
            // Wall-clock mode (default): Duration from session start to now (includes idle time)
            get_session_duration(session_id)
        }
    }
}
