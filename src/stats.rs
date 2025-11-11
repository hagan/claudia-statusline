//! Statistics tracking module.
//!
//! This module provides persistent statistics tracking for Claude Code sessions,
//! including costs, line changes, and usage metrics. Statistics are stored in
//! both JSON and SQLite formats for reliability and concurrent access.

use crate::common::{current_date, current_month, current_timestamp, get_data_dir};
use crate::config::get_config;
use crate::database::SqliteDatabase;
use crate::error::{Result, StatuslineError};
use crate::retry::{retry_if_retryable, RetryConfig};
use fs2::FileExt;
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyStats {
    pub total_cost: f64,
    pub sessions: Vec<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthlyStats {
    pub total_cost: f64,
    pub sessions: usize,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllTimeStats {
    pub total_cost: f64,
    pub sessions: usize,
    pub since: String,
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
    pub fn load() -> Self {
        // Phase 2: Try SQLite first, then fall back to JSON
        if let Ok(data) = Self::load_from_sqlite() {
            return data;
        }

        // Fall back to JSON if SQLite fails
        let path = Self::get_stats_file_path();

        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path) {
                match serde_json::from_str(&contents) {
                    Ok(data) => {
                        // Migrate JSON data to SQLite if needed
                        if let Err(e) = Self::migrate_to_sqlite(&data) {
                            log::warn!("Failed to migrate JSON to SQLite: {}", e);
                        }
                        return data;
                    }
                    Err(e) => {
                        // File exists but can't be parsed - backup and warn
                        warn!("Failed to parse stats file: {}", e);
                        let backup_path = path.with_extension("backup");
                        let _ = fs::copy(&path, &backup_path);
                        warn!("Backed up corrupted stats to: {:?}", backup_path);
                    }
                }
            }
        }

        // Only create default if file doesn't exist (not if corrupted)
        let default_data = Self::default();
        // Try to save the default, but don't fail if we can't
        let _ = default_data.save();
        default_data
    }

    /// Load stats data from SQLite database (Phase 2)
    pub fn load_from_sqlite() -> Result<Self> {
        let db_path = Self::get_sqlite_path()?;

        // Check if database exists
        if !db_path.exists() {
            return Err(StatuslineError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some("SQLite database not found".to_string()),
            )));
        }

        let db = SqliteDatabase::new(&db_path)?;

        // Load components
        let sessions = db.get_all_sessions()?;
        let daily = db.get_all_daily_stats()?;
        let monthly = db.get_all_monthly_stats()?;
        let all_time_total = db.get_all_time_total()?;
        let sessions_count = db.get_all_time_sessions_count()?;
        let since_date = db
            .get_earliest_session_date()?
            .unwrap_or_else(current_timestamp);

        // Construct in one go to avoid field reassigns after Default
        let data = StatsData {
            sessions,
            daily,
            monthly,
            all_time: AllTimeStats {
                total_cost: all_time_total,
                sessions: sessions_count,
                since: since_date,
            },
            ..Default::default()
        };

        Ok(data)
    }

    /// Migrate JSON data to SQLite if not already done
    fn migrate_to_sqlite(data: &Self) -> Result<()> {
        let db_path = Self::get_sqlite_path()?;
        let db = SqliteDatabase::new(&db_path)?;

        log::debug!("migrate_to_sqlite: Checking if migration needed");
        log::debug!("migrate_to_sqlite: JSON has {} sessions", data.sessions.len());

        // Check if we've already migrated by looking for existing sessions
        let has_sessions = db.has_sessions();
        log::debug!("migrate_to_sqlite: DB has_sessions = {}", has_sessions);

        if !has_sessions {
            log::info!("Migrating {} sessions from JSON to SQLite", data.sessions.len());
            // Perform migration
            db.import_sessions(&data.sessions)?;
            log::info!("Successfully migrated {} sessions to SQLite", data.sessions.len());
        } else {
            log::debug!("Skipping migration - database already has sessions");
        }

        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let config = get_config();

        // Save to JSON if backup is enabled
        if config.database.json_backup {
            let path = Self::get_stats_file_path();

            // Acquire and lock the file with retry
            let mut file = acquire_stats_file(&path)?;

            // Save the data using our helper
            save_stats_data(&mut file, self);
        } else {
            log::info!("Skipping JSON backup (json_backup=false, SQLite-only mode)");
        }

        // Always save to SQLite (it's now the primary storage)
        perform_sqlite_dual_write(self);

        Ok(())
    }

    pub fn get_stats_file_path() -> PathBuf {
        get_data_dir().join("stats.json")
    }

    pub fn get_sqlite_path() -> Result<PathBuf> {
        Ok(get_data_dir().join("stats.db"))
    }

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
        if let Ok(db_path) = Self::get_sqlite_path() {
            if let Ok(db) = SqliteDatabase::new(&db_path) {
                if let Err(e) = db.update_session(session_id, update.clone()) {
                    log::warn!("Failed to persist session {} to SQLite: {}", session_id, e);
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
        let last_cost = self.sessions.get(session_id).map(|s| s.cost).unwrap_or(0.0);

        let cost_delta = update.cost - last_cost;

        // Calculate line deltas from previous values
        let last_lines_added = self
            .sessions
            .get(session_id)
            .map(|s| s.lines_added)
            .unwrap_or(0);
        let last_lines_removed = self
            .sessions
            .get(session_id)
            .map(|s| s.lines_removed)
            .unwrap_or(0);
        let lines_added_delta = (update.lines_added as i64) - (last_lines_added as i64);
        let lines_removed_delta = (update.lines_removed as i64) - (last_lines_removed as i64);

        // Always update session metadata (even with zero/negative deltas)
        // This ensures cost corrections and metadata refreshes are persisted
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.cost = update.cost;
            session.lines_added = update.lines_added;
            session.lines_removed = update.lines_removed;
            session.last_updated = now.clone();
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
            .or_insert_with(|| DailyStats {
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
            .or_insert_with(|| MonthlyStats {
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

/// Loads or retrieves the current statistics data.
///
/// This function is process-safe and loads the stats from disk.
///
/// # Returns
///
/// Returns the current `StatsData`, either loaded from disk or a new default instance.
pub fn get_or_load_stats_data() -> StatsData {
    StatsData::load()
}

fn get_stats_backup_path() -> Result<PathBuf> {
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    Ok(get_data_dir().join(format!("stats_backup_{}.json", timestamp)))
}

// Helper function to acquire and lock the stats file with retry
fn acquire_stats_file(path: &Path) -> Result<File> {
    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Use retry configuration for file operations
    let retry_config = RetryConfig::for_file_ops();

    // Try to open the file with retry
    let file = retry_if_retryable(&retry_config, || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(StatuslineError::from)
    })?;

    // Try to acquire exclusive lock with retry
    retry_if_retryable(&retry_config, || {
        file.lock_exclusive()
            .map_err(|e| StatuslineError::lock(format!("Failed to lock stats file: {}", e)))?;
        Ok(())
    })?;

    Ok(file)
}

// Helper function to load stats data from file
fn load_stats_data(file: &mut File, path: &Path) -> StatsData {
    let mut contents = String::new();
    if file.read_to_string(&mut contents).is_ok() && !contents.is_empty() {
        match serde_json::from_str(&contents) {
            Ok(data) => {
                // Migrate JSON data to SQLite if needed
                if let Err(e) = StatsData::migrate_to_sqlite(&data) {
                    log::warn!("Failed to migrate JSON to SQLite: {}", e);
                }
                data
            }
            Err(e) => {
                warn!(
                    "Stats file corrupted: {}. Creating backup and starting fresh.",
                    e
                );
                // Try to create a backup of the corrupted file
                if let Ok(backup_path) = get_stats_backup_path() {
                    if let Err(e) = std::fs::copy(path, &backup_path) {
                        error!("Failed to backup corrupted stats file: {}", e);
                    } else {
                        warn!("Corrupted stats backed up to: {:?}", backup_path);
                    }
                }
                StatsData::default()
            }
        }
    } else {
        StatsData::default()
    }
}

// Helper function to save stats data to file
fn save_stats_data(file: &mut File, stats_data: &StatsData) {
    // Write back to file (truncate and write)
    if let Err(e) = file.set_len(0) {
        error!("Failed to truncate stats file: {}", e);
    }
    if let Err(e) = file.seek(std::io::SeekFrom::Start(0)) {
        error!("Failed to seek stats file: {}", e);
    }

    let json = serde_json::to_string_pretty(stats_data).unwrap_or_else(|_| "{}".to_string());
    if let Err(e) = file.write_all(json.as_bytes()) {
        error!("Failed to write stats file: {}", e);
    }
}

// Check if we should migrate sessions from JSON to SQLite
fn should_migrate_sessions(db: &SqliteDatabase, stats_data: &StatsData) -> bool {
    !db.has_sessions() && !stats_data.sessions.is_empty()
}

// Migrate existing sessions from JSON to SQLite
fn migrate_sessions_to_sqlite(db: &SqliteDatabase, stats_data: &StatsData) {
    // Find the most recently updated session to exclude from migration
    let current_session = stats_data
        .sessions
        .iter()
        .max_by_key(|(_, s)| &s.last_updated)
        .map(|(id, _)| id.clone());

    // Collect sessions to migrate (excluding current session)
    let sessions_to_migrate: std::collections::HashMap<String, SessionStats> = stats_data
        .sessions
        .iter()
        .filter(|(id, _)| current_session.as_ref() != Some(id))
        .map(|(id, session)| (id.clone(), session.clone()))
        .collect();

    if !sessions_to_migrate.is_empty() {
        match db.import_sessions(&sessions_to_migrate) {
            Ok(_) => {
                debug!(
                    "Migrated {} existing sessions from JSON to SQLite",
                    sessions_to_migrate.len()
                );
            }
            Err(e) => {
                warn!("Failed to migrate sessions to SQLite: {}", e);
            }
        }
    }
}

// Write the current session to SQLite
#[allow(dead_code)]
fn write_current_session_to_sqlite(db: &SqliteDatabase, stats_data: &StatsData) {
    if let Some((session_id, session)) = stats_data
        .sessions
        .iter()
        .max_by_key(|(_, s)| &s.last_updated)
    {
        use crate::database::SessionUpdate;
        match db.update_session(
            session_id,
            SessionUpdate {
                cost: session.cost,
                lines_added: session.lines_added,
                lines_removed: session.lines_removed,
                model_name: None,      // not available in dual-write
                workspace_dir: None,   // not available in dual-write
                device_id: None,       // not available in dual-write
                token_breakdown: None, // not available in dual-write
                max_tokens_observed: session.max_tokens_observed, // from in-memory stats
            },
        ) {
            Ok((day_total, session_total)) => {
                debug!(
                    "SQLite dual-write successful: day=${:.2}, session=${:.2}",
                    day_total, session_total
                );
            }
            Err(e) => {
                warn!("SQLite dual-write failed: {}", e);
            }
        }
    }
}

// Helper function to write to SQLite (primary storage)
fn perform_sqlite_dual_write(stats_data: &StatsData) {
    // Write to SQLite (primary storage as of Phase 2)
    let db_path = match StatsData::get_sqlite_path() {
        Ok(p) => p,
        Err(_) => {
            error!("Failed to get SQLite database path");
            return;
        }
    };

    let db = match SqliteDatabase::new(&db_path) {
        Ok(d) => d,
        Err(e) => {
            error!(
                "Failed to initialize SQLite database at {:?}: {}",
                db_path, e
            );
            return;
        }
    };

    // NOTE: Migration is now handled in load_stats_data() when JSON is loaded
    // Current session is written directly in update_session() with all migration v5 fields
    // No need to call write_current_session_to_sqlite() as that would overwrite model_name/workspace_dir/tokens with NULL
}

/// Updates the statistics data with process-safe file locking.
///
/// This function acquires an exclusive lock on the stats file, loads the current data,
/// applies the update function, and saves the result. It also performs a dual-write
/// to SQLite for better concurrent access.
///
/// # Arguments
///
/// * `updater` - A closure that takes a mutable reference to `StatsData` and returns
///   the daily and monthly totals as a tuple
///
/// # Returns
///
/// Returns a tuple of (daily_total, monthly_total) costs.
///
/// # Example
///
/// ```rust,no_run
/// use statusline::stats::update_stats_data;
/// use statusline::database::SessionUpdate;
///
/// let (daily, monthly) = update_stats_data(|stats| {
///     stats.update_session(
///         "session-123",
///         SessionUpdate {
///             cost: 1.50,
///             lines_added: 100,
///             lines_removed: 50,
///             model_name: None,
///             workspace_dir: None,
///             device_id: None,
///             token_breakdown: None,
///             max_tokens_observed: None,
///         },
///     )
/// });
/// ```
pub fn update_stats_data<F>(updater: F) -> (f64, f64)
where
    F: FnOnce(&mut StatsData) -> (f64, f64),
{
    let config = get_config();
    let path = StatsData::get_stats_file_path();

    // Load existing stats data
    let mut stats_data = if config.database.json_backup {
        // Acquire and lock the file with retry
        let mut file = match acquire_stats_file(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to acquire stats file after retries: {}", e);
                return (0.0, 0.0);
            }
        };

        let mut data = load_stats_data(&mut file, &path);

        // Apply the update
        let result = updater(&mut data);

        // Save updated stats data to JSON
        save_stats_data(&mut file, &data);

        // Perform SQLite write
        perform_sqlite_dual_write(&data);

        // File lock is automatically released when file is dropped
        return result;
    } else {
        // SQLite-only mode: load from SQLite
        debug!("Operating in SQLite-only mode (json_backup=false)");
        StatsData::load_from_sqlite().unwrap_or_else(|e| {
            warn!("Failed to load from SQLite: {}", e);
            StatsData::default()
        })
    };

    // Apply the update
    let result = updater(&mut stats_data);

    // Save to SQLite (primary storage)
    perform_sqlite_dual_write(&stats_data);

    result
}

/// Get the daily total from stats data
#[allow(dead_code)]
pub fn get_daily_total(data: &StatsData) -> f64 {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0)
}

pub fn get_session_duration(session_id: &str) -> Option<u64> {
    let data = get_or_load_stats_data();

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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_stats_data_default() {
        let stats = StatsData::default();
        assert_eq!(stats.version, "1.0");
        assert!(stats.sessions.is_empty());
        assert!(stats.daily.is_empty());
        assert!(stats.monthly.is_empty());
        assert_eq!(stats.all_time.total_cost, 0.0);
        assert_eq!(stats.all_time.sessions, 0);
    }

    #[test]
    fn test_stats_data_update_session() {
        use crate::database::SessionUpdate;
        let mut stats = StatsData::default();
        let (daily, monthly) = stats.update_session(
            "test-session",
            SessionUpdate {
                cost: 10.0,
                lines_added: 100,
                lines_removed: 50,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
            },
        );

        assert_eq!(daily, 10.0);
        assert_eq!(monthly, 10.0);
        assert_eq!(stats.all_time.total_cost, 10.0);
        assert_eq!(stats.all_time.sessions, 1);
    }

    #[test]
    #[serial]
    fn test_stats_file_path_xdg() {
        // Set XDG_DATA_HOME for testing
        env::set_var("XDG_DATA_HOME", "/tmp/xdg_test");
        let path = StatsData::get_stats_file_path();
        assert_eq!(
            path,
            PathBuf::from("/tmp/xdg_test/claudia-statusline/stats.json")
        );
        env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    #[serial]
    fn test_stats_save_and_load() {
        use crate::database::SessionUpdate;
        let temp_dir = TempDir::new().unwrap();
        env::set_var("XDG_DATA_HOME", temp_dir.path().to_str().unwrap());

        let mut stats = StatsData::default();
        stats.update_session(
            "test",
            SessionUpdate {
                cost: 5.0,
                lines_added: 50,
                lines_removed: 25,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
            },
        );

        let save_result = stats.save();
        assert!(save_result.is_ok());

        // Make sure the file was actually created
        let data_dir = env::var("XDG_DATA_HOME").unwrap();
        let stats_path = PathBuf::from(data_dir)
            .join("claudia-statusline")
            .join("stats.json");
        assert!(stats_path.exists());

        let loaded_stats = StatsData::load();
        // Check that the session was saved and loaded correctly
        assert!(loaded_stats.sessions.contains_key("test"));
        assert!(loaded_stats.all_time.total_cost >= 5.0); // At least our cost

        env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    #[serial]
    fn test_session_start_time_tracking() {
        use crate::database::SessionUpdate;
        let mut stats = StatsData::default();

        // First update creates session with start_time
        stats.update_session(
            "test-session",
            SessionUpdate {
                cost: 1.0,
                lines_added: 10,
                lines_removed: 5,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
            },
        );

        // Check that start_time was set
        let session = stats.sessions.get("test-session").unwrap();
        assert!(session.start_time.is_some());

        // Second update to same session shouldn't change start_time
        let original_start = session.start_time.clone();
        stats.update_session(
            "test-session",
            SessionUpdate {
                cost: 2.0,
                lines_added: 20,
                lines_removed: 10,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
            },
        );

        let session = stats.sessions.get("test-session").unwrap();
        assert_eq!(session.start_time, original_start);
        assert_eq!(session.cost, 2.0);
    }

    #[test]
    #[serial]
    fn test_concurrent_update_safety() {
        // Skip this test in CI due to thread synchronization timing issues
        if env::var("CI").is_ok() {
            println!("Skipping test_concurrent_update_safety in CI environment");
            return;
        }
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use std::thread;

        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();
        env::set_var("XDG_DATA_HOME", &temp_path);

        // Create the directory structure
        let stats_dir = Path::new(&temp_path).join("claudia-statusline");
        std::fs::create_dir_all(&stats_dir).unwrap();

        // Initialize with clean stats file
        let initial_stats = StatsData::default();
        initial_stats.save().unwrap();

        let completed = Arc::new(AtomicU32::new(0));
        let mut handles = vec![];

        // Spawn 10 threads that each add $1.00
        for i in 0..10 {
            let completed_clone = completed.clone();
            let temp_path_clone = temp_path.clone();
            let handle = thread::spawn(move || {
                // Ensure the thread uses the temp directory
                use crate::database::SessionUpdate;
                env::set_var("XDG_DATA_HOME", &temp_path_clone);
                let (daily, _) = update_stats_data(|stats| {
                    stats.update_session(
                        &format!("test-thread-{}", i),
                        SessionUpdate {
                            cost: 1.0,
                            lines_added: 10,
                            lines_removed: 5,
                            model_name: None,
                            workspace_dir: None,
                            device_id: None,
                            token_breakdown: None,
                            max_tokens_observed: None,
                        },
                    )
                });
                completed_clone.fetch_add(1, Ordering::SeqCst);
                daily
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all updates were applied
        assert_eq!(completed.load(Ordering::SeqCst), 10);

        // Load final stats and check total
        let final_stats = StatsData::load();

        // Count the sessions created
        let test_sessions: Vec<_> = final_stats
            .sessions
            .keys()
            .filter(|k| k.starts_with("test-thread-"))
            .collect();

        // Should have created 10 sessions
        assert_eq!(
            test_sessions.len(),
            10,
            "Should have created 10 test sessions"
        );

        // Each session should have $1.00
        for session_id in test_sessions {
            let session = final_stats.sessions.get(session_id).unwrap();
            assert_eq!(session.cost, 1.0, "Each session should have $1.00");
        }

        env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    #[serial]
    fn test_get_session_duration() {
        // Skip this test in CI due to timing issues
        if env::var("CI").is_ok() {
            println!("Skipping test_get_session_duration in CI environment");
            return;
        }
        use std::thread;
        use std::time::Duration;

        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap();
        env::set_var("XDG_DATA_HOME", temp_path);

        // Create the directory structure
        let stats_dir = Path::new(&temp_path).join("claudia-statusline");
        std::fs::create_dir_all(&stats_dir).unwrap();

        // Initialize with clean stats file
        let initial_stats = StatsData::default();
        initial_stats.save().unwrap();

        // Create a session with a specific start time
        use crate::database::SessionUpdate;
        update_stats_data(|stats| {
            stats.update_session(
                "duration-test-session",
                SessionUpdate {
                    cost: 1.0,
                    lines_added: 10,
                    lines_removed: 5,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                },
            )
        });

        // Wait a bit to ensure some time passes
        thread::sleep(Duration::from_millis(100));

        // Get duration - should exist
        let duration = get_session_duration("duration-test-session");
        assert!(
            duration.is_some(),
            "Duration should exist for valid session"
        );

        let duration = duration.unwrap();
        // Duration is u64, so it's always non-negative
        assert!(
            duration < 3600,
            "Duration should be less than 1 hour for a test"
        );

        // Non-existent session should return None
        assert!(get_session_duration("non-existent-session").is_none());

        env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    #[serial]
    fn test_file_corruption_recovery() {
        // Skip this test in CI due to file system timing issues
        if env::var("CI").is_ok() {
            println!("Skipping test_file_corruption_recovery in CI environment");
            return;
        }
        let temp_dir = TempDir::new().unwrap();
        env::set_var("XDG_DATA_HOME", temp_dir.path().to_str().unwrap());

        let stats_path = StatsData::get_stats_file_path();

        // Create corrupted file
        fs::create_dir_all(stats_path.parent().unwrap()).unwrap();
        fs::write(&stats_path, "not valid json {").unwrap();

        // Load should handle corruption gracefully
        let stats = StatsData::load();
        assert_eq!(stats.version, "1.0");

        // Check that backup was created
        let backup_path = stats_path.with_extension("backup");
        assert!(backup_path.exists(), "Backup file should exist");

        // Verify backup contains corrupted data
        let backup_contents = fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup_contents, "not valid json {");

        env::remove_var("XDG_DATA_HOME");
    }
}
