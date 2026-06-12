//! SQLite persistence and stats orchestration.
//!
//! JSON read fallback (`StatsData::load`) is retained for v2.x recovery only —
//! it fires when SQLite is missing or unusable; writes are SQLite-only as of v3.0.0.
//!
//! Contains load/save methods, migration helpers, and the top-level
//! `update_stats_data` orchestration function.

use super::StatsData;
use crate::database::SqliteDatabase;
use crate::error::{Result, StatuslineError};
use log::{error, warn};
use std::fs;

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

                        // Fix permissions on backup (fs::copy preserves source permissions)
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(metadata) = fs::metadata(&backup_path) {
                                let mut perms = metadata.permissions();
                                perms.set_mode(0o600);
                                let _ = fs::set_permissions(&backup_path, perms);
                            }
                        }

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
            .unwrap_or_else(crate::common::current_timestamp);

        // Construct in one go to avoid field reassigns after Default
        let data = StatsData {
            sessions,
            daily,
            monthly,
            all_time: super::aggregation::AllTimeStats {
                total_cost: all_time_total,
                sessions: sessions_count,
                since: since_date,
            },
            ..Default::default()
        };

        Ok(data)
    }

    /// Migrate JSON data to SQLite if not already done
    pub(crate) fn migrate_to_sqlite(data: &Self) -> Result<()> {
        let db_path = Self::get_sqlite_path()?;
        let db = SqliteDatabase::new(&db_path)?;

        log::debug!("migrate_to_sqlite: Checking if migration needed");
        log::debug!(
            "migrate_to_sqlite: JSON has {} sessions",
            data.sessions.len()
        );

        // Check if we've already migrated by looking for existing sessions
        let has_sessions = db.has_sessions();
        log::debug!("migrate_to_sqlite: DB has_sessions = {}", has_sessions);

        if !has_sessions {
            log::info!(
                "Migrating {} sessions from JSON to SQLite",
                data.sessions.len()
            );
            // Perform migration
            db.import_sessions(&data.sessions)?;
            log::info!(
                "Successfully migrated {} sessions to SQLite",
                data.sessions.len()
            );
        } else {
            log::debug!("Skipping migration - database already has sessions");
        }

        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        // v3.0.0+: writes are SQLite-only. The JSON backup write path was removed.
        perform_sqlite_dual_write(self);
        Ok(())
    }
}

// ── Free functions ───────────────────────────────────────────────────────

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

// Helper function to write to SQLite (primary storage)
fn perform_sqlite_dual_write(_stats_data: &StatsData) {
    // Write to SQLite (primary storage as of Phase 2)
    let db_path = match StatsData::get_sqlite_path() {
        Ok(p) => p,
        Err(_) => {
            error!("Failed to get SQLite database path");
            return;
        }
    };

    let _db = match SqliteDatabase::new(&db_path) {
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

/// Updates the statistics data, persisting the result to SQLite.
///
/// This function loads the current data, applies the update function, and writes the
/// result to SQLite. As of v3.0.0 the JSON backup write path (and its `fs2` file lock)
/// was removed; concurrency safety now rests on the SQLite transaction in the write path.
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
///             active_time_seconds: None,
///             last_activity: None,
///         },
///     )
/// });
/// ```
pub fn update_stats_data<F>(updater: F) -> (f64, f64)
where
    F: FnOnce(&mut StatsData) -> (f64, f64),
{
    // v3.0.0+: SQLite-only. Load from SQLite, apply the update, write back to SQLite.
    let mut stats_data = StatsData::load_from_sqlite().unwrap_or_else(|e| {
        warn!("Failed to load from SQLite: {}", e);
        StatsData::default()
    });

    // Apply the update
    let result = updater(&mut stats_data);

    // Save to SQLite (primary storage)
    perform_sqlite_dual_write(&stats_data);

    result
}
