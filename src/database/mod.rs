use crate::config;
use rusqlite::{params, Connection, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

mod analytics;
mod context;
mod daily;
mod maintenance;
mod monthly;
mod schema;
mod session;
mod sync;

#[cfg(test)]
mod tests;

// Re-exports: public API surface
//
// Some items are only consumed via the library crate (not the binary directly).
#[allow(unused_imports)]
pub use analytics::SessionWithModel;
pub use maintenance::perform_maintenance;
#[allow(unused_imports)]
pub use maintenance::MaintenanceResult;
pub use schema::{SessionUpdate, SCHEMA};

// Track which database files have been migrated to avoid redundant migration checks
static MIGRATED_DBS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

pub struct SqliteDatabase {
    #[allow(dead_code)]
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl SqliteDatabase {
    pub fn new(db_path: &Path) -> Result<Self> {
        // Ensure parent directory exists with secure permissions (0o700 on Unix)
        if let Some(parent) = db_path.parent() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(parent)
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                            Some(format!("Failed to create directory: {}", e)),
                        )
                    })?;
            }

            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(parent).map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                        Some(format!("Failed to create directory: {}", e)),
                    )
                })?;
            }
        }

        // Get configuration
        let config = config::get_config();

        // Open connection directly - avoids thread-spawning issues on FreeBSD
        // (r2d2's scheduled-thread-pool fails with EAGAIN on FreeBSD)
        let conn = Connection::open(db_path)?;

        // Apply pragmas for WAL mode and concurrency
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", config.database.busy_timeout_ms)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        // Check if this is a new database by looking for existing sessions table
        // A truly new database has no tables at all
        let has_sessions_table: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
                [],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        let is_new_db = !has_sessions_table;

        if is_new_db {
            // NEW DATABASE: Create complete schema with all migration columns
            conn.execute_batch(SCHEMA)?;

            // Mark as fully migrated (v6 includes daily/monthly token tracking)
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
                 VALUES (?1, ?2, '', 'New database with complete schema (v6)', 0)",
                params![6, chrono::Local::now().to_rfc3339()],
            )?;
        } else {
            // OLD DATABASE: Only ensure base tables exist, let migrations add columns/indexes
            // This prevents "no such column" errors when creating indexes
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS sessions (
                    session_id TEXT PRIMARY KEY,
                    start_time TEXT NOT NULL,
                    last_updated TEXT NOT NULL,
                    cost REAL DEFAULT 0.0,
                    lines_added INTEGER DEFAULT 0,
                    lines_removed INTEGER DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS daily_stats (
                    date TEXT PRIMARY KEY,
                    total_cost REAL DEFAULT 0.0,
                    total_lines_added INTEGER DEFAULT 0,
                    total_lines_removed INTEGER DEFAULT 0,
                    session_count INTEGER DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS monthly_stats (
                    month TEXT PRIMARY KEY,
                    total_cost REAL DEFAULT 0.0,
                    total_lines_added INTEGER DEFAULT 0,
                    total_lines_removed INTEGER DEFAULT 0,
                    session_count INTEGER DEFAULT 0
                );
                "#,
            )?;
        }

        // Run migrations only if not already done for this database file
        // This avoids redundant migration checks on hot paths (update_session, etc.)
        let canonical_path = db_path
            .canonicalize()
            .unwrap_or_else(|_| db_path.to_path_buf());
        let migrated_dbs = MIGRATED_DBS.get_or_init(|| Mutex::new(HashSet::new()));

        let needs_migration = {
            let guard = migrated_dbs.lock().unwrap();
            !guard.contains(&canonical_path)
        };

        if needs_migration {
            log::debug!(
                "Running migrations for database: {}",
                canonical_path.display()
            );
            // Run pending migrations and mark as migrated
            crate::migrations::run_migrations_on_db(db_path)?;

            let mut guard = migrated_dbs.lock().unwrap();
            guard.insert(canonical_path.clone());
            log::debug!("Marked database as migrated: {}", canonical_path.display());
        } else {
            log::debug!(
                "Skipping migrations (already migrated): {}",
                canonical_path.display()
            );
        }

        // Set secure file permissions for database file (0o600 on Unix)
        // Do this AFTER creating the pool/schema so first-run databases get secured
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(db_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                // Best effort - log warning but don't fail
                if let Err(e) = std::fs::set_permissions(db_path, perms) {
                    log::warn!("Failed to set database file permissions to 0o600: {}", e);
                }
            }

            // Also fix permissions for WAL and SHM files if they exist
            let wal_path = db_path.with_extension("db-wal");
            if let Ok(metadata) = std::fs::metadata(&wal_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&wal_path, perms);
            }

            let shm_path = db_path.with_extension("db-shm");
            if let Ok(metadata) = std::fs::metadata(&shm_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&shm_path, perms);
            }
        }

        // Create the database wrapper with the connection
        let db = Self {
            path: db_path.to_path_buf(),
            conn: Mutex::new(conn),
        };

        Ok(db)
    }

    fn get_connection(&self) -> Result<MutexGuard<'_, Connection>> {
        // Lock the mutex to get exclusive access to the connection
        // If the mutex is poisoned (previous panic while holding lock), recover the inner value
        self.conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("Connection mutex poisoned".to_string()),
            )
        })
    }
}
