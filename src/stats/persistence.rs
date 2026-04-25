//! Persistence layer: JSON file I/O, SQLite dual-write, and stats orchestration.
//!
//! Contains load/save methods, file locking, migration helpers, and the
//! top-level `update_stats_data` orchestration function.

use super::StatsData;
use crate::common::get_data_dir;
use crate::config::get_config;
use crate::database::SqliteDatabase;
use crate::error::{Result, StatuslineError};
use crate::retry::{retry_if_retryable, RetryConfig};
use fs2::FileExt;
use log::{debug, error, warn};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

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

fn get_stats_backup_path() -> Result<PathBuf> {
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    Ok(get_data_dir().join(format!("stats_backup_{}.json", timestamp)))
}

// Helper function to acquire and lock the stats file with retry
pub(crate) fn acquire_stats_file(path: &Path) -> Result<File> {
    // Ensure directory exists with secure permissions (0o700 on Unix)
    if let Some(parent) = path.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(parent)?;
        }

        #[cfg(not(unix))]
        {
            fs::create_dir_all(parent)?;
        }
    }

    // Use retry configuration for file operations
    let retry_config = RetryConfig::for_file_ops();

    // Try to open the file with retry and secure permissions (0o600 on Unix)
    let file = retry_if_retryable(&retry_config, || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600) // Owner read/write only on Unix (for new files)
                .open(path)
                .map_err(StatuslineError::from)
        }

        #[cfg(not(unix))]
        {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(StatuslineError::from)
        }
    })?;

    // Fix permissions on existing files (mode flag only applies to new files)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = file.metadata() {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(path, perms); // Best effort - don't fail if it doesn't work
        }
    }

    // Try to acquire exclusive lock with retry (non-blocking)
    // CRITICAL: Use try_lock_exclusive() instead of lock_exclusive()
    // lock_exclusive() blocks indefinitely, causing hangs when multiple
    // Claude instances run simultaneously. try_lock_exclusive() returns
    // immediately with WouldBlock error if lock is held, allowing retry.
    retry_if_retryable(&retry_config, || {
        match file.try_lock_exclusive() {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Transient: another process holds the lock, retry is appropriate
                log::debug!("Stats file lock contention (WouldBlock), will retry");
                Err(StatuslineError::lock(
                    "Stats file temporarily locked by another process",
                ))
            }
            Err(e) => {
                // Hard failure: permissions, I/O error, etc.
                log::warn!("Stats file lock failed unexpectedly: {}", e);
                Err(StatuslineError::lock(format!(
                    "Failed to lock stats file: {}",
                    e
                )))
            }
        }
    })?;

    Ok(file)
}

// Helper function to load stats data from file
pub(crate) fn load_stats_data(file: &mut File, path: &Path) -> StatsData {
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
pub(crate) fn save_stats_data(file: &mut File, stats_data: &StatsData) {
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
    let config = get_config();
    let path = StatsData::get_stats_file_path();

    // Load existing stats data
    let mut stats_data = if config.database.json_backup {
        // Show deprecation warning (once per process)
        super::warn_json_backup_deprecated();

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
