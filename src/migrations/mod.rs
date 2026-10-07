use crate::retry::{retry_if_retryable, RetryConfig};
use crate::stats::StatsData;
use chrono::Local;
use rusqlite::{params, Connection, Result, Transaction, TransactionBehavior};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One column a schema version requires (see [`REQUIRED_COLUMNS`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequiredColumn {
    /// Schema version that introduced the column.
    pub version: u32,
    /// Table the column lives on.
    pub table: &'static str,
    /// Column name.
    pub name: &'static str,
    /// Column declaration (type plus default), exactly as in `SCHEMA`.
    pub decl: &'static str,
}

/// Single source of truth for the columns each schema version requires. Used by
/// migrations 007/008, the open-time repair, and the tests. Column declarations must
/// match src/database/schema.rs SCHEMA exactly.
///
/// Scope: version 3 (`sessions.sync_timestamp` only; the default-feature migration 003
/// never added it, so the repair closes that drift), 7 and 8. Older columns are covered
/// by the fresh-vs-migrated parity test.
pub(crate) const REQUIRED_COLUMNS: &[RequiredColumn] = &[
    RequiredColumn {
        version: 3,
        table: "sessions",
        name: "sync_timestamp",
        decl: "INTEGER",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "claude_version",
        decl: "TEXT",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "agent_count",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "agent_requests",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "agent_input_tokens",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "agent_output_tokens",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "main_requests",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "main_input_tokens",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 7,
        table: "sessions",
        name: "main_output_tokens",
        decl: "INTEGER DEFAULT 0",
    },
    RequiredColumn {
        version: 8,
        table: "sessions",
        name: "raw_cost",
        decl: "REAL",
    },
    RequiredColumn {
        version: 8,
        table: "sessions",
        name: "raw_lines_added",
        decl: "INTEGER",
    },
    RequiredColumn {
        version: 8,
        table: "sessions",
        name: "raw_lines_removed",
        decl: "INTEGER",
    },
    RequiredColumn {
        version: 8,
        table: "sessions",
        name: "process_start_ms",
        decl: "INTEGER",
    },
    RequiredColumn {
        version: 8,
        table: "session_archive",
        name: "raw_cost",
        decl: "REAL",
    },
    RequiredColumn {
        version: 8,
        table: "session_archive",
        name: "raw_lines_added",
        decl: "INTEGER",
    },
    RequiredColumn {
        version: 8,
        table: "session_archive",
        name: "raw_lines_removed",
        decl: "INTEGER",
    },
    RequiredColumn {
        version: 8,
        table: "session_archive",
        name: "process_start_ms",
        decl: "INTEGER",
    },
];

/// Column names of every distinct table referenced by the registry rows matching
/// `pred`, with exactly one `PRAGMA table_info` per table (an absent table yields an
/// empty set). `Transaction` derefs to `Connection`, so this serves both.
fn registry_table_columns(
    conn: &Connection,
    pred: impl Fn(u32) -> bool,
) -> Result<HashMap<&'static str, HashSet<String>>> {
    let mut map: HashMap<&'static str, HashSet<String>> = HashMap::new();
    for row in REQUIRED_COLUMNS.iter().filter(|r| pred(r.version)) {
        if map.contains_key(row.table) {
            continue;
        }
        // Table names come only from the compile-time registry, never from input.
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", row.table))?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<HashSet<String>>>()?;
        map.insert(row.table, names);
    }
    Ok(map)
}

/// Explicit, non-retryable error for a table that registry columns live on but that
/// does not exist. The message deliberately avoids the words `retry::is_retryable_error`
/// matches, so the retry loop returns it at once.
fn schema_invariant_error(table: &str, version: u32) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(format!(
            "schema invariant violated: table {table} is required by schema version {version} but does not exist"
        )),
    )
}

/// Add every registry column whose version matches `pred` and which is absent, and
/// return the `(table, column)` pairs that were added.
///
/// If a matching row's table does not exist, returns the schema-invariant error
/// before running any ALTER: a missing required table is never skipped.
pub(crate) fn add_missing_columns(
    tx: &Transaction,
    pred: impl Fn(u32) -> bool,
) -> Result<Vec<(&'static str, &'static str)>> {
    let mut existing = registry_table_columns(tx, &pred)?;
    for row in REQUIRED_COLUMNS.iter().filter(|r| pred(r.version)) {
        if existing.get(row.table).is_none_or(|cols| cols.is_empty()) {
            return Err(schema_invariant_error(row.table, row.version));
        }
    }
    let mut added = Vec::new();
    for row in REQUIRED_COLUMNS.iter().filter(|r| pred(r.version)) {
        let cols = existing.entry(row.table).or_default();
        if cols.contains(row.name) {
            continue;
        }
        tx.execute(
            &format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                row.table, row.name, row.decl
            ),
            [],
        )?;
        cols.insert(row.name.to_string());
        added.push((row.table, row.name));
    }
    Ok(added)
}

/// Read-only: every registry column with `version <= max_version` that is absent,
/// including rows whose table does not exist.
pub(crate) fn missing_required_columns(
    conn: &Connection,
    max_version: u32,
) -> Result<Vec<(&'static str, &'static str)>> {
    let existing = registry_table_columns(conn, |v| v <= max_version)?;
    Ok(REQUIRED_COLUMNS
        .iter()
        .filter(|r| r.version <= max_version)
        .filter(|r| !existing.get(r.table).is_some_and(|c| c.contains(r.name)))
        .map(|r| (r.table, r.name))
        .collect())
}

/// sync_meta table (migration 003 under `turso-sync`; same text as `SCHEMA`). The
/// default-feature 003 never created it, so the open-time repair does.
const SYNC_META_DDL: &str = "CREATE TABLE IF NOT EXISTS sync_meta (
                device_id TEXT PRIMARY KEY,
                last_sync_push INTEGER,
                last_sync_pull INTEGER,
                hostname_hash TEXT
            )";

/// session_archive table (migration 005 shape, same declarations as `SCHEMA`). The
/// v8 columns are not listed here: the repair adds them from [`REQUIRED_COLUMNS`]
/// after creating the table, so the result matches `SCHEMA` column for column.
const SESSION_ARCHIVE_DDL: &str = "CREATE TABLE IF NOT EXISTS session_archive (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                start_time TEXT NOT NULL,
                end_time TEXT NOT NULL,
                archived_at TEXT NOT NULL,
                cost REAL NOT NULL,
                lines_added INTEGER NOT NULL,
                lines_removed INTEGER NOT NULL,
                active_time_seconds INTEGER,
                last_activity TEXT,
                model_name TEXT,
                workspace_dir TEXT,
                device_id TEXT
            )";

/// session_archive indexes (migration 005; same text as `SCHEMA`).
const ARCHIVE_SESSION_INDEX_DDL: &str =
    "CREATE INDEX IF NOT EXISTS idx_archive_session ON session_archive(session_id)";
const ARCHIVE_DATE_INDEX_DDL: &str =
    "CREATE INDEX IF NOT EXISTS idx_archive_date ON session_archive(DATE(archived_at))";

/// One table or index a schema version requires (see [`REQUIRED_OBJECTS`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequiredObject {
    /// Schema version that introduced the object.
    pub version: u32,
    /// `"table"` or `"index"`, matching `sqlite_master.type`.
    pub kind: &'static str,
    /// Object name in `sqlite_master`.
    pub name: &'static str,
    /// IF NOT EXISTS DDL that creates it (safe to re-run).
    pub ddl: &'static str,
}

/// Tables and indexes each schema version requires that the open-time repair can
/// create with IF NOT EXISTS DDL.
#[rustfmt::skip]
pub(crate) const REQUIRED_OBJECTS: &[RequiredObject] = &[
    RequiredObject { version: 3, kind: "table", name: "sync_meta", ddl: SYNC_META_DDL },
    // An empty archive is valid ("nothing archived yet"), so recreating it loses nothing.
    // Tables come before their indexes: the repair runs these in order.
    RequiredObject { version: 5, kind: "table", name: "session_archive", ddl: SESSION_ARCHIVE_DDL },
    RequiredObject { version: 5, kind: "index", name: "idx_archive_session", ddl: ARCHIVE_SESSION_INDEX_DDL },
    RequiredObject { version: 5, kind: "index", name: "idx_archive_date", ddl: ARCHIVE_DATE_INDEX_DDL },
    RequiredObject { version: 7, kind: "table", name: "transcript_progress", ddl: TRANSCRIPT_PROGRESS_DDL },
    RequiredObject { version: 7, kind: "index", name: "idx_transcript_progress_session", ddl: TRANSCRIPT_PROGRESS_DDL },
];

/// Read-only: every [`REQUIRED_OBJECTS`] entry with `version <= max_version` that is
/// absent from `sqlite_master` (one query).
pub(crate) fn missing_required_objects(
    conn: &Connection,
    max_version: u32,
) -> Result<Vec<&'static str>> {
    let mut stmt =
        conn.prepare("SELECT type, name FROM sqlite_master WHERE type IN ('table', 'index')")?;
    let present = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<HashSet<(String, String)>>>()?;
    Ok(REQUIRED_OBJECTS
        .iter()
        .filter(|o| o.version <= max_version)
        .filter(|o| !present.contains(&(o.kind.to_string(), o.name.to_string())))
        .map(|o| o.name)
        .collect())
}

/// Migration trait for database schema changes
#[allow(dead_code)]
pub trait Migration {
    /// Unique version number (must be sequential)
    fn version(&self) -> u32;

    /// Human-readable description
    fn description(&self) -> &str;

    /// Apply the migration (forward)
    fn up(&self, tx: &Transaction) -> Result<()>;

    /// Rollback the migration (backward)
    fn down(&self, tx: &Transaction) -> Result<()>;
}

/// Migration runner for managing database migrations
#[allow(dead_code)]
pub struct MigrationRunner {
    conn: Connection,
    migrations: Vec<Box<dyn Migration>>,
}

#[allow(dead_code)]
impl MigrationRunner {
    pub fn new(db_path: &Path) -> Result<Self> {
        // Open connection for migrations (don't call SqliteDatabase::new to avoid infinite recursion)
        let conn = Connection::open(db_path)?;

        // busy_timeout first so the WAL switch and the base-table batch wait on a held lock instead of failing at once (mirrors SqliteDatabase::new)
        conn.pragma_update(None, "busy_timeout", 10000)?;
        // Enable WAL for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // Create minimal base schema (WITHOUT migration columns) for testing migrations
        // This allows migrations to add columns without "duplicate column" errors
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

            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                checksum TEXT NOT NULL,
                description TEXT,
                execution_time_ms INTEGER
            );
            "#,
        )?;

        Ok(Self {
            conn,
            migrations: Self::load_all_migrations(),
        })
    }

    /// Load all migration definitions
    fn load_all_migrations() -> Vec<Box<dyn Migration>> {
        vec![
            Box::new(InitialJsonToSqlite),
            Box::new(AddMetaTable),
            Box::new(AddSyncMetadata),
            Box::new(AddAdaptiveLearning),
            Box::new(AddBurnRateTracking),
            Box::new(AddDailyTokenTracking),
            Box::new(AddAgentTracking),
            Box::new(AddAccountingBaseline),
        ]
    }

    /// Get current schema version
    pub fn current_version(&self) -> Result<u32> {
        let version: Option<u32> = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap_or(None);

        Ok(version.unwrap_or(0))
    }

    /// Run all pending migrations.
    ///
    /// Concurrency-safe under concurrent first-run (multiple fresh processes racing on
    /// the same DB file). Each migration step runs in an IMMEDIATE transaction so writers
    /// serialize at BEGIN; the applied version is re-checked INSIDE that transaction to
    /// close the read-then-write gap; the version is recorded with `INSERT OR IGNORE`
    /// so a lost race never raises `UNIQUE constraint failed: schema_migrations.version`;
    /// and a serialized loser that still hits SQLITE_BUSY retries cleanly.
    ///
    /// After the pending steps, [`Self::repair_required_columns`] adds any
    /// [`REQUIRED_COLUMNS`] / [`REQUIRED_OBJECTS`] entry missing for a version that is
    /// already recorded (additive DDL only; no transaction when nothing is missing).
    ///
    /// What the retry covers: the pending migration steps and the repair, both run
    /// inside `retry_if_retryable`. `MigrationRunner::new`'s WAL switch and base-table
    /// batch are NOT retried; they are protected by `busy_timeout` only. A missing
    /// required table is a non-retryable schema-invariant error and is returned at once.
    pub fn migrate(&mut self) -> Result<()> {
        // Mirror the adapter pattern in SqliteDatabase::update_session: run the loop under
        // retry_if_retryable (which works over crate::error::Result/StatuslineError), then
        // map the final StatuslineError back to a rusqlite::Error for our Result signature.
        retry_if_retryable(&RetryConfig::for_db_ops(), || {
            self.migrate_once()
                .and_then(|_| self.repair_required_columns())
                .map_err(crate::error::StatuslineError::Database)
        })
        .map_err(|e| match e {
            crate::error::StatuslineError::Database(db_err) => db_err,
            _ => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(e.to_string()),
            ),
        })
    }

    /// Open-time self-repair: add every registry column and object that a recorded
    /// version requires but the DB lacks (for example a DB stamped v8 by a PR-branch
    /// build, or by an interleaving the fresh-init re-check does not anticipate).
    ///
    /// Additive DDL only (ALTER TABLE ADD COLUMN, CREATE TABLE/INDEX IF NOT EXISTS); it
    /// never changes rows or schema_migrations. It is skipped entirely when the recorded
    /// version is newer than this binary's latest migration. The preflight is read-only, so a healthy DB
    /// opens no transaction. Missing [`REQUIRED_OBJECTS`] (including `session_archive`)
    /// are created before columns are added. A table that registry columns live on, that
    /// is absent and that is not a [`REQUIRED_OBJECTS`] entry, is an explicit
    /// schema-invariant error, and the transaction rolls back.
    fn repair_required_columns(&mut self) -> Result<()> {
        let current = self.current_version()?;
        // A schema recorded by a newer binary may have dropped, renamed or reshaped
        // objects this binary would re-add, so never write to it (forward compat).
        let latest_known = self
            .migrations
            .iter()
            .map(|m| m.version())
            .max()
            .unwrap_or(0);
        if current > latest_known {
            log::debug!(
                "schema v{current} is newer than this binary (v{latest_known}); skipping repair"
            );
            return Ok(());
        }
        if missing_required_columns(&self.conn, current)?.is_empty()
            && missing_required_objects(&self.conn, current)?.is_empty()
        {
            return Ok(());
        }

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-read inside the write lock: a concurrent repairer may have done the work.
        // Objects first, so a recreatable table (session_archive) exists before its
        // registry columns are added; the invariant check in add_missing_columns then
        // fires only for tables the repair cannot create.
        let missing_objects = missing_required_objects(&tx, current)?;
        for name in &missing_objects {
            if let Some(obj) = REQUIRED_OBJECTS.iter().find(|o| o.name == *name) {
                tx.execute_batch(obj.ddl)?;
            }
        }
        let added_columns = add_missing_columns(&tx, |v| v <= current)?;
        tx.commit()?;

        if !added_columns.is_empty() || !missing_objects.is_empty() {
            let cols: Vec<String> = added_columns
                .iter()
                .map(|(t, c)| format!("{t}.{c}"))
                .collect();
            log::warn!(
                "schema repair: version {current} was recorded without columns [{}] and objects [{}]; added them",
                cols.join(", "),
                missing_objects.join(", ")
            );
        }
        Ok(())
    }

    /// Single attempt at running pending migrations. Safe to retry.
    fn migrate_once(&mut self) -> Result<()> {
        let current = self.current_version()?;

        // Collect versions to run
        let versions_to_run: Vec<u32> = self
            .migrations
            .iter()
            .filter(|m| m.version() > current)
            .map(|m| m.version())
            .collect();

        // Run each migration by version
        for version in versions_to_run {
            // Find the migration with this version
            let migration = self
                .migrations
                .iter()
                .find(|m| m.version() == version)
                .expect("Migration should exist");

            let start = std::time::Instant::now();

            // IMMEDIATE transaction: concurrent writers serialize at BEGIN, turning a
            // potential constraint race into a BUSY that retry_if_retryable absorbs.
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;

            // Re-check INSIDE the transaction: another process may have applied this
            // version between our current_version() read and acquiring the write lock.
            let already_applied: i64 = tx.query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                params![migration.version()],
                |row| row.get(0),
            )?;
            if already_applied > 0 {
                // Already recorded by a concurrent process; don't re-run migration.up.
                tx.commit()?;
                continue;
            }

            // Apply migration
            migration.up(&tx)?;

            // Record migration. INSERT OR IGNORE so a lost race can never raise
            // `UNIQUE constraint failed: schema_migrations.version`.
            tx.execute(
                "INSERT OR IGNORE INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    migration.version(),
                    Local::now().to_rfc3339(),
                    "", // Checksum placeholder
                    migration.description(),
                    start.elapsed().as_millis() as i64,
                ],
            )?;

            tx.commit()?;
        }

        Ok(())
    }
}

/// Migration 001: Import existing JSON data to SQLite
pub struct InitialJsonToSqlite;

impl Migration for InitialJsonToSqlite {
    fn version(&self) -> u32 {
        1
    }

    fn description(&self) -> &str {
        "Import existing JSON stats data to SQLite"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Load existing JSON data
        let stats_data = StatsData::load();

        // Import sessions
        for (session_id, session) in &stats_data.sessions {
            tx.execute(
                "INSERT OR REPLACE INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    session.start_time.as_ref().unwrap_or(&session.last_updated),
                    &session.last_updated,
                    session.cost,
                    session.lines_added as i64,
                    session.lines_removed as i64,
                ],
            )?;
        }

        // Import daily stats
        for (date, daily) in &stats_data.daily {
            tx.execute(
                "INSERT OR REPLACE INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    date,
                    daily.total_cost,
                    daily.lines_added as i64,
                    daily.lines_removed as i64,
                    daily.sessions.len() as i64,
                ],
            )?;
        }

        // Import monthly stats
        for (month, monthly) in &stats_data.monthly {
            tx.execute(
                "INSERT OR REPLACE INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    month,
                    monthly.total_cost,
                    monthly.lines_added as i64,
                    monthly.lines_removed as i64,
                    monthly.sessions as i64,
                ],
            )?;
        }

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Clear all imported data
        tx.execute("DELETE FROM sessions", [])?;
        tx.execute("DELETE FROM daily_stats", [])?;
        tx.execute("DELETE FROM monthly_stats", [])?;
        Ok(())
    }
}

/// Migration to add meta table for storing maintenance metadata
pub struct AddMetaTable;

impl Migration for AddMetaTable {
    fn version(&self) -> u32 {
        2
    }

    fn description(&self) -> &str {
        "Add meta table for database maintenance metadata"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Create meta table
        tx.execute(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        // Add initial values
        let now = Local::now().to_rfc3339();
        tx.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('created_at', ?1)",
            params![now],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        tx.execute("DROP TABLE IF EXISTS meta", [])?;
        Ok(())
    }
}

/// Migration 003: Add sync metadata for cloud synchronization
#[cfg(feature = "turso-sync")]
pub struct AddSyncMetadata;

#[cfg(feature = "turso-sync")]
impl Migration for AddSyncMetadata {
    fn version(&self) -> u32 {
        3
    }

    fn description(&self) -> &str {
        "Add sync metadata columns and sync_meta table for cloud synchronization"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Add device_id and sync_timestamp columns to existing tables
        // Using ALTER TABLE ADD COLUMN which is safe (adds NULL values to existing rows)

        // Sessions table
        tx.execute("ALTER TABLE sessions ADD COLUMN device_id TEXT", [])?;
        tx.execute("ALTER TABLE sessions ADD COLUMN sync_timestamp INTEGER", [])?;

        // Daily stats table
        tx.execute("ALTER TABLE daily_stats ADD COLUMN device_id TEXT", [])?;

        // Monthly stats table
        tx.execute("ALTER TABLE monthly_stats ADD COLUMN device_id TEXT", [])?;

        // Create sync_meta table for tracking sync state
        tx.execute(
            "CREATE TABLE IF NOT EXISTS sync_meta (
                device_id TEXT PRIMARY KEY,
                last_sync_push INTEGER,
                last_sync_pull INTEGER,
                hostname_hash TEXT
            )",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // SQLite doesn't support DROP COLUMN, so we would need to recreate tables
        // For simplicity, we'll just drop the sync_meta table
        // In production, a proper down migration would recreate tables without sync columns
        tx.execute("DROP TABLE IF EXISTS sync_meta", [])?;

        // Note: device_id and sync_timestamp columns remain in sessions/daily_stats/monthly_stats
        // This is acceptable since they're nullable and don't affect existing functionality

        Ok(())
    }
}

// Migration for when turso-sync feature is disabled - still adds device_id for analytics
#[cfg(not(feature = "turso-sync"))]
pub struct AddSyncMetadata;

#[cfg(not(feature = "turso-sync"))]
impl Migration for AddSyncMetadata {
    fn version(&self) -> u32 {
        3
    }

    fn description(&self) -> &str {
        "Add device_id for analytics (sync features disabled)"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Always add device_id - used by analytics and learning features even without sync
        tx.execute("ALTER TABLE sessions ADD COLUMN device_id TEXT", [])?;
        tx.execute("ALTER TABLE daily_stats ADD COLUMN device_id TEXT", [])?;
        tx.execute("ALTER TABLE monthly_stats ADD COLUMN device_id TEXT", [])?;

        // Note: sync_timestamp and sync_meta table are NOT added (turso-sync disabled)
        Ok(())
    }

    fn down(&self, _tx: &Transaction) -> Result<()> {
        // SQLite doesn't support DROP COLUMN
        // device_id columns remain but are nullable
        Ok(())
    }
}

/// Migration 004: Add adaptive context window learning (consolidated from v4, v5, v6)
pub struct AddAdaptiveLearning;

impl Migration for AddAdaptiveLearning {
    fn version(&self) -> u32 {
        4
    }

    fn description(&self) -> &str {
        "Add adaptive context learning with session metadata and audit trail"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Create learned_context_windows table with ALL fields (including audit trail)
        tx.execute(
            "CREATE TABLE IF NOT EXISTS learned_context_windows (
                model_name TEXT PRIMARY KEY,
                observed_max_tokens INTEGER NOT NULL,
                ceiling_observations INTEGER DEFAULT 0,
                compaction_count INTEGER DEFAULT 0,
                last_observed_max INTEGER NOT NULL,
                last_updated TEXT NOT NULL,
                confidence_score REAL DEFAULT 0.0,
                first_seen TEXT NOT NULL,
                workspace_dir TEXT,
                device_id TEXT
            )",
            [],
        )?;

        // Indexes for learned_context_windows
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_learned_confidence
             ON learned_context_windows(confidence_score DESC)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_learned_workspace_model
             ON learned_context_windows(workspace_dir, model_name)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_learned_device
             ON learned_context_windows(device_id)",
            [],
        )?;

        // Add ALL session columns for adaptive learning and analytics
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN max_tokens_observed INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute("ALTER TABLE sessions ADD COLUMN model_name TEXT", [])?;
        tx.execute("ALTER TABLE sessions ADD COLUMN workspace_dir TEXT", [])?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_input_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_output_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_cache_read_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_cache_creation_tokens INTEGER DEFAULT 0",
            [],
        )?;

        // Indexes for sessions table
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_model_name ON sessions(model_name)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_workspace ON sessions(workspace_dir)",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Drop learned_context_windows table and indexes
        tx.execute("DROP TABLE IF EXISTS learned_context_windows", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_learned_confidence", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_learned_workspace_model", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_learned_device", [])?;

        // Drop session indexes
        tx.execute("DROP INDEX IF EXISTS idx_sessions_model_name", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_sessions_workspace", [])?;

        // Note: SQLite doesn't support DROP COLUMN easily
        // Columns remain but that's acceptable for backward compatibility

        Ok(())
    }
}

/// Migration 005: Add burn rate tracking (active_time, wall_clock, auto_reset modes)
pub struct AddBurnRateTracking;

impl Migration for AddBurnRateTracking {
    fn version(&self) -> u32 {
        5
    }

    fn description(&self) -> &str {
        "Add burn rate tracking columns and session_archive table for all three modes"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Part 1: Add burn rate tracking columns to sessions table
        // Add active_time_seconds for tracking accumulated active conversation time
        // Used by "active_time" burn rate mode
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN active_time_seconds INTEGER DEFAULT 0",
            [],
        )?;

        // Add last_activity timestamp for detecting inactivity gaps
        // Used by both "active_time" and "auto_reset" modes
        tx.execute("ALTER TABLE sessions ADD COLUMN last_activity TEXT", [])?;

        // For existing sessions, set last_activity to last_updated
        tx.execute(
            "UPDATE sessions SET last_activity = last_updated WHERE last_activity IS NULL",
            [],
        )?;

        // Part 2: Create session_archive table for auto_reset mode
        // Stores reset session history to preserve work period data
        tx.execute(
            r#"
            CREATE TABLE IF NOT EXISTS session_archive (
                id INTEGER PRIMARY KEY AUTOINCREMENT,

                -- Session identification
                session_id TEXT NOT NULL,

                -- Time boundaries
                start_time TEXT NOT NULL,
                end_time TEXT NOT NULL,
                archived_at TEXT NOT NULL,

                -- Accumulated values (snapshot at archive time)
                cost REAL NOT NULL,
                lines_added INTEGER NOT NULL,
                lines_removed INTEGER NOT NULL,
                active_time_seconds INTEGER,

                -- Context preservation
                last_activity TEXT,
                model_name TEXT,
                workspace_dir TEXT,
                device_id TEXT
            )
            "#,
            [],
        )?;

        // Create indexes for efficient queries on archived sessions
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_archive_session ON session_archive(session_id)",
            [],
        )?;

        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_archive_date ON session_archive(DATE(archived_at))",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Drop the session_archive table and indexes
        tx.execute("DROP TABLE IF EXISTS session_archive", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_archive_session", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_archive_date", [])?;

        // Note: SQLite doesn't support DROP COLUMN easily
        // active_time_seconds and last_activity columns remain but that's acceptable
        Ok(())
    }
}

/// Migration 006: Add daily/monthly token tracking for aggregate token metrics
pub struct AddDailyTokenTracking;

impl Migration for AddDailyTokenTracking {
    fn version(&self) -> u32 {
        6
    }

    fn description(&self) -> &str {
        "Add token tracking columns to daily_stats and monthly_stats for aggregate metrics"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Add token columns to daily_stats
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_input_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_output_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_cache_read_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_cache_creation_tokens INTEGER DEFAULT 0",
            [],
        )?;

        // Add token columns to monthly_stats
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_input_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_output_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_cache_read_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_cache_creation_tokens INTEGER DEFAULT 0",
            [],
        )?;

        // Create index for efficient daily token queries
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_daily_tokens ON daily_stats(date DESC, total_input_tokens, total_output_tokens)",
            [],
        )?;

        // Create index for efficient monthly token queries
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_monthly_tokens ON monthly_stats(month DESC, total_input_tokens, total_output_tokens)",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Drop the indexes
        tx.execute("DROP INDEX IF EXISTS idx_daily_tokens", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_monthly_tokens", [])?;

        // Note: SQLite doesn't support DROP COLUMN easily
        // Token columns remain but that's acceptable for backward compatibility
        Ok(())
    }
}

/// transcript_progress table and its session index (migration 007). IF NOT EXISTS,
/// so the open-time repair can re-run it safely.
const TRANSCRIPT_PROGRESS_DDL: &str = "CREATE TABLE IF NOT EXISTS transcript_progress (
                path TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                is_agent INTEGER NOT NULL DEFAULT 0,
                agent_type TEXT,
                size INTEGER NOT NULL DEFAULT 0,
                mtime INTEGER NOT NULL DEFAULT 0,
                offset INTEGER NOT NULL DEFAULT 0,
                last_request_id TEXT,
                requests INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_transcript_progress_session
                ON transcript_progress(session_id);";

/// Migration 007: per-session subagent/main token attribution, the Claude Code
/// version that produced the session, and the incremental transcript parse
/// state that makes the subagent scan cheap on every render.
pub struct AddAgentTracking;

impl Migration for AddAgentTracking {
    fn version(&self) -> u32 {
        7
    }

    fn description(&self) -> &str {
        "Add claude_version + agent/main token columns to sessions and the transcript_progress table"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        add_missing_columns(tx, |v| v == 7)?;
        tx.execute_batch(TRANSCRIPT_PROGRESS_DDL)?;
        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        tx.execute("DROP INDEX IF EXISTS idx_transcript_progress_session", [])?;
        tx.execute("DROP TABLE IF EXISTS transcript_progress", [])?;
        // The added sessions columns stay (SQLite DROP COLUMN caveat, as above).
        Ok(())
    }
}

/// Migration 008: per-session delta baseline for accounting correctness.
///
/// Stores the last raw payload counters (cost, lines added/removed) and the
/// inferred process start on both `sessions` and `session_archive`, so the
/// restart/correction classifier can compute deltas against what the payload
/// last reported rather than against the stored cumulative totals.
///
/// Pure additive DDL (D-15): no UPDATE, no backfill. The columns are nullable
/// with no default; NULL means "no baseline yet" and readers use
/// `COALESCE(raw_cost, cost)` (correct for every pre-008 row, whose `cost`
/// always held the last raw payload value).
///
/// Columns that already exist are skipped, so a DB created from the full v8
/// `SCHEMA`, or a concurrent opener that lost the version-record race, never
/// hits "duplicate column name". 007 and 008 share the registry-driven idempotent
/// helper (`add_missing_columns` over `REQUIRED_COLUMNS`) and the open-time repair
/// (Phase 16). During the 7 -> 8 upgrade a missing `session_archive` is an explicit
/// schema-invariant error; once 8 is recorded, the open-time repair recreates it.
pub struct AddAccountingBaseline;

impl Migration for AddAccountingBaseline {
    fn version(&self) -> u32 {
        8
    }

    fn description(&self) -> &str {
        "Add per-session raw counter baseline + inferred process start (sessions, session_archive)"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        add_missing_columns(tx, |v| v == 8)?;
        Ok(())
    }

    fn down(&self, _tx: &Transaction) -> Result<()> {
        // SQLite doesn't support DROP COLUMN easily; the nullable baseline
        // columns stay, which is harmless for older readers.
        Ok(())
    }
}

/// Run migrations on a specific database path
/// Returns Err only on critical failures that prevent migrations from running
pub fn run_migrations_on_db(db_path: &Path) -> Result<()> {
    let mut runner = MigrationRunner::new(db_path)?;
    runner.migrate()
}

/// Run migrations on startup (best effort)
#[allow(dead_code)]
pub fn run_migrations() {
    if let Ok(db_path) = StatsData::get_sqlite_path() {
        let _ = run_migrations_on_db(&db_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_migration_runner() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        assert_eq!(runner.current_version().unwrap(), 0);

        runner.migrate().unwrap();
        // We now have 8 migrations: InitialJsonToSqlite (v1), AddMetaTable (v2), AddSyncMetadata (v3),
        // AddAdaptiveLearning (v4), AddBurnRateTracking (v5), AddDailyTokenTracking (v6),
        // AddAgentTracking (v7), AddAccountingBaseline (v8)
        assert_eq!(runner.current_version().unwrap(), 8);
    }

    #[test]
    fn test_agent_tracking_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_agents.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();
        assert_eq!(runner.current_version().unwrap(), 8);

        let session_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for col in [
            "claude_version",
            "agent_count",
            "agent_requests",
            "agent_input_tokens",
            "agent_output_tokens",
            "main_requests",
            "main_input_tokens",
            "main_output_tokens",
        ] {
            assert!(session_columns.iter().any(|c| c == col), "missing {col}");
        }

        let has_progress: i64 = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transcript_progress'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_progress, 1);
    }

    #[test]
    #[cfg(feature = "turso-sync")]
    fn test_sync_metadata_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_sync.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        // Verify sync_meta table exists
        let table_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sync_meta'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(table_exists, "sync_meta table should exist");

        // Verify device_id column was added to sessions
        let sessions_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            sessions_columns.contains(&"device_id".to_string()),
            "sessions table should have device_id column"
        );
        assert!(
            sessions_columns.contains(&"sync_timestamp".to_string()),
            "sessions table should have sync_timestamp column"
        );
    }

    #[test]
    fn test_learned_context_windows_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_learned.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        // Verify learned_context_windows table exists
        let table_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='learned_context_windows'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(table_exists, "learned_context_windows table should exist");

        // Verify index exists
        let index_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_learned_confidence'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(index_exists, "idx_learned_confidence index should exist");

        // Verify max_tokens_observed column was added to sessions
        let sessions_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            sessions_columns.contains(&"max_tokens_observed".to_string()),
            "sessions table should have max_tokens_observed column"
        );
    }

    #[test]
    fn test_daily_token_tracking_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_tokens.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        // Verify the runner is at the latest version (v7 and v8 follow the token migration)
        assert_eq!(runner.current_version().unwrap(), 8);

        // Verify token columns were added to daily_stats
        let daily_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(daily_stats)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            daily_columns.contains(&"total_input_tokens".to_string()),
            "daily_stats should have total_input_tokens column"
        );
        assert!(
            daily_columns.contains(&"total_output_tokens".to_string()),
            "daily_stats should have total_output_tokens column"
        );
        assert!(
            daily_columns.contains(&"total_cache_read_tokens".to_string()),
            "daily_stats should have total_cache_read_tokens column"
        );
        assert!(
            daily_columns.contains(&"total_cache_creation_tokens".to_string()),
            "daily_stats should have total_cache_creation_tokens column"
        );

        // Verify token columns were added to monthly_stats
        let monthly_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(monthly_stats)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            monthly_columns.contains(&"total_input_tokens".to_string()),
            "monthly_stats should have total_input_tokens column"
        );
        assert!(
            monthly_columns.contains(&"total_output_tokens".to_string()),
            "monthly_stats should have total_output_tokens column"
        );

        // Verify indexes were created
        let daily_index_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_daily_tokens'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(daily_index_exists, "idx_daily_tokens index should exist");

        let monthly_index_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_monthly_tokens'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(
            monthly_index_exists,
            "idx_monthly_tokens index should exist"
        );
    }

    const BASELINE_COLUMNS: [&str; 4] = [
        "raw_cost",
        "raw_lines_added",
        "raw_lines_removed",
        "process_start_ms",
    ];

    fn column_names(conn: &Connection, table: &str) -> Vec<String> {
        conn.prepare(&format!("PRAGMA table_info({})", table))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// Frozen v3.3.0 (schema v6) SCHEMA text, no stamp.
    const V6_SCHEMA: &str = include_str!("../../tests/fixtures/schema_v6_v3.3.0.sql");
    /// Synthetic v6 seed rows, ending with the v6 stamp.
    const V6_SEED: &str = include_str!("../../tests/fixtures/seed_v6.sql");

    const AGENT_COLUMNS: [&str; 8] = [
        "claude_version",
        "agent_count",
        "agent_requests",
        "agent_input_tokens",
        "agent_output_tokens",
        "main_requests",
        "main_input_tokens",
        "main_output_tokens",
    ];

    /// Create a DB at `dir/name` exactly as a v3.3.0 binary leaves it: v6 SCHEMA,
    /// synthetic seed rows, and the v6 stamp.
    fn v6_fixture_db(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(V6_SCHEMA).unwrap();
        conn.execute_batch(V6_SEED).unwrap();
        path
    }

    fn recorded_versions(conn: &Connection) -> Vec<u32> {
        conn.prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| row.get::<_, u32>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn stamp(conn: &Connection, version: u32) {
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
             VALUES (?1, '2026-01-01T00:00:00+00:00', '', 'test stamp', 0)",
            params![version],
        )
        .unwrap();
    }

    /// (name, type, notnull, dflt_value, pk) for every column of `table`.
    type ColumnMeta = (String, String, i64, Option<String>, i64);

    fn column_meta(conn: &Connection, table: &str) -> Vec<ColumnMeta> {
        conn.prepare(&format!(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('{}')",
            table
        ))
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn table_names(conn: &Connection) -> std::collections::BTreeSet<String> {
        conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    #[test]
    fn test_agent_tracking_migration_skips_existing_columns() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_agents_fresh.db");

        // Fresh path: full SCHEMA already has every 007 column.
        drop(crate::database::SqliteDatabase::new(&db_path).unwrap());
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("DELETE FROM schema_migrations WHERE version IN (7, 8)", [])
                .unwrap();
            stamp(&conn, 6);
        }

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner
            .migrate()
            .expect("migration 007 must skip columns that already exist");
        let cols = column_names(&runner.conn, "sessions");
        for col in AGENT_COLUMNS {
            assert_eq!(
                cols.iter().filter(|c| *c == col).count(),
                1,
                "sessions.{col} must exist exactly once"
            );
        }
        assert_eq!(recorded_versions(&runner.conn), vec![6, 7, 8]);
    }

    #[test]
    fn test_agent_tracking_migration_adds_partially_missing_columns() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = v6_fixture_db(&temp_dir, "partial.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("ALTER TABLE sessions ADD COLUMN claude_version TEXT", [])
                .unwrap();
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN agent_count INTEGER DEFAULT 0",
                [],
            )
            .unwrap();
        }

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner
            .migrate()
            .expect("migration 007 must add only the missing columns");
        let cols = column_names(&runner.conn, "sessions");
        for col in AGENT_COLUMNS {
            assert_eq!(
                cols.iter().filter(|c| *c == col).count(),
                1,
                "sessions.{col} must exist exactly once"
            );
        }
        assert_eq!(recorded_versions(&runner.conn), vec![6, 7, 8]);
    }

    #[test]
    fn required_columns_match_schema_minus_v6() {
        let full = Connection::open_in_memory().unwrap();
        full.execute_batch(crate::database::SCHEMA).unwrap();
        let v6 = Connection::open_in_memory().unwrap();
        v6.execute_batch(V6_SCHEMA).unwrap();

        let full_tables = table_names(&full);
        let v6_tables = table_names(&v6);
        let new_tables: Vec<&String> = full_tables.difference(&v6_tables).collect();
        assert_eq!(new_tables, vec!["transcript_progress"]);
        assert!(
            v6_tables.is_subset(&full_tables),
            "SCHEMA dropped a v6 table: {:?}",
            v6_tables.difference(&full_tables).collect::<Vec<_>>()
        );

        // For every table in both: SCHEMA columns minus v6 columns == registry rows >= 7.
        for table in full_tables.intersection(&v6_tables) {
            let full_cols: HashSet<String> = column_names(&full, table).into_iter().collect();
            let v6_cols: HashSet<String> = column_names(&v6, table).into_iter().collect();
            let added: HashSet<String> = full_cols.difference(&v6_cols).cloned().collect();
            let registry: HashSet<String> = REQUIRED_COLUMNS
                .iter()
                .filter(|r| r.version >= 7 && r.table == table)
                .map(|r| r.name.to_string())
                .collect();
            assert_eq!(
                added, registry,
                "{table}: SCHEMA minus v6 must equal the registry's v7+ columns"
            );
        }

        // Every registry row exists in SCHEMA with exactly the declared metadata.
        for row in REQUIRED_COLUMNS {
            let schema_meta = column_meta(&full, row.table)
                .into_iter()
                .find(|m| m.0 == row.name)
                .unwrap_or_else(|| panic!("SCHEMA lacks {}.{}", row.table, row.name));

            let probe = Connection::open_in_memory().unwrap();
            probe
                .execute_batch("CREATE TABLE probe (id INTEGER)")
                .unwrap();
            probe
                .execute(
                    &format!("ALTER TABLE probe ADD COLUMN {} {}", row.name, row.decl),
                    [],
                )
                .unwrap();
            let probe_meta = column_meta(&probe, "probe")
                .into_iter()
                .find(|m| m.0 == row.name)
                .unwrap();
            assert_eq!(
                (probe_meta.1, probe_meta.2, probe_meta.3, probe_meta.4),
                (schema_meta.1, schema_meta.2, schema_meta.3, schema_meta.4),
                "registry decl for {}.{} ({:?}) does not match SCHEMA",
                row.table,
                row.name,
                row.decl
            );
        }
    }

    #[test]
    fn migration_008_errors_when_session_archive_missing() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = v6_fixture_db(&temp_dir, "no_archive.db");
        Connection::open(&db_path)
            .unwrap()
            .execute("DROP TABLE session_archive", [])
            .unwrap();

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        let err = runner
            .migrate()
            .expect_err("008 must fail when session_archive is missing");
        assert!(
            err.to_string().contains("session_archive"),
            "error must name the missing table: {err}"
        );
        assert!(!recorded_versions(&runner.conn).contains(&8));
        assert!(
            !column_names(&runner.conn, "sessions")
                .iter()
                .any(|c| c == "raw_cost"),
            "008's transaction must roll back"
        );
    }

    #[test]
    fn missing_required_columns_reports_absent_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V6_SCHEMA).unwrap();
        conn.execute("DROP TABLE session_archive", []).unwrap();
        let missing = missing_required_columns(&conn, 8).unwrap();
        for col in BASELINE_COLUMNS {
            assert!(
                missing.contains(&("session_archive", col)),
                "missing_required_columns must report session_archive.{col}: {missing:?}"
            );
        }
    }

    #[test]
    fn test_accounting_baseline_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_baseline.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        // Bring the DB to v7 first so session_archive exists, then seed rows that
        // predate migration 008.
        runner.migrations.retain(|m| m.version() <= 7);
        runner.migrate().unwrap();
        assert_eq!(runner.current_version().unwrap(), 7);
        runner
            .conn
            .execute(
                "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
                 VALUES ('s1', '2026-01-01T00:00:00Z', '2026-01-01T01:00:00Z', 12.34, 100, 20)",
                [],
            )
            .unwrap();
        runner
            .conn
            .execute(
                "INSERT INTO session_archive (session_id, start_time, end_time, archived_at, cost, lines_added, lines_removed)
                 VALUES ('s0', '2025-12-31T00:00:00Z', '2025-12-31T01:00:00Z', '2025-12-31T02:00:00Z', 5.5, 7, 3)",
                [],
            )
            .unwrap();

        // Now run the full migration set (008 is pending).
        runner.migrations = MigrationRunner::load_all_migrations();
        runner.migrate().unwrap();
        assert_eq!(runner.current_version().unwrap(), 8);

        // Scope to the seeded rows: migration 001 imports whatever StatsData::load()
        // returns, so the table may hold other rows depending on the environment.
        for (table, sid) in [("sessions", "s1"), ("session_archive", "s0")] {
            let cols = column_names(&runner.conn, table);
            for col in BASELINE_COLUMNS {
                assert!(cols.iter().any(|c| c == col), "{table} missing {col}");
            }
            let nulls: i64 = runner
                .conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM {} WHERE session_id = '{}' AND raw_cost IS NULL
                         AND raw_lines_added IS NULL AND raw_lines_removed IS NULL
                         AND process_start_ms IS NULL",
                        table, sid
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                nulls, 1,
                "{table}: pre-existing row must have NULL baseline"
            );
        }

        let (cost, added, removed): (f64, i64, i64) = runner
            .conn
            .query_row(
                "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = 's1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(cost.to_bits(), 12.34f64.to_bits());
        assert_eq!((added, removed), (100, 20));

        let (cost, added, removed): (f64, i64, i64) = runner
            .conn
            .query_row(
                "SELECT cost, lines_added, lines_removed FROM session_archive WHERE session_id = 's0'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(cost.to_bits(), 5.5f64.to_bits());
        assert_eq!((added, removed), (7, 3));
    }

    #[test]
    fn test_accounting_baseline_migration_skips_existing_columns() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_baseline_fresh.db");

        // Fresh path: full SCHEMA (already has the 008 columns) + version 8 recorded.
        drop(crate::database::SqliteDatabase::new(&db_path).unwrap());

        // Simulate a concurrent opener / lost race that sees v7 and re-runs 008:
        // drop the v8 record and mark the DB as v7 so only migration 008 is pending.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("DELETE FROM schema_migrations WHERE version = 8", [])
                .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
                 VALUES (7, '2026-01-01T00:00:00Z', '', 'test', 0)",
                [],
            )
            .unwrap();
        }

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        assert_eq!(runner.current_version().unwrap(), 7);
        runner
            .migrate()
            .expect("migration 008 must skip columns that already exist");
        assert_eq!(runner.current_version().unwrap(), 8);
        for table in ["sessions", "session_archive"] {
            let cols = column_names(&runner.conn, table);
            for col in BASELINE_COLUMNS {
                assert_eq!(
                    cols.iter().filter(|c| *c == col).count(),
                    1,
                    "{table}.{col} must exist exactly once"
                );
            }
        }
    }

    #[test]
    fn fresh_and_migrated_schemas_match() {
        let fresh_dir = TempDir::new().unwrap();
        let fresh_path = fresh_dir.path().join("fresh.db");
        drop(crate::database::SqliteDatabase::new(&fresh_path).unwrap());
        let fresh = Connection::open(&fresh_path).unwrap();

        let migrated_dir = TempDir::new().unwrap();
        let migrated_path = migrated_dir.path().join("migrated.db");
        let mut runner = MigrationRunner::new(&migrated_path).unwrap();
        runner.migrate().unwrap();

        for table in ["sessions", "session_archive"] {
            let a = column_names(&fresh, table);
            let b = column_names(&runner.conn, table);
            for col in BASELINE_COLUMNS {
                assert!(a.iter().any(|c| c == col), "fresh {table} missing {col}");
                assert!(b.iter().any(|c| c == col), "migrated {table} missing {col}");
            }
        }

        // Every table, full column metadata, no allowlist. Order-independent: migrations
        // append columns in a different order than SCHEMA declares them.
        let fresh_tables = table_names(&fresh);
        let migrated_tables = table_names(&runner.conn);
        let all: std::collections::BTreeSet<&String> =
            fresh_tables.union(&migrated_tables).collect();
        for table in all {
            assert!(fresh_tables.contains(table), "fresh DB lacks table {table}");
            assert!(
                migrated_tables.contains(table),
                "migrated DB lacks table {table}"
            );
            let a: std::collections::BTreeSet<ColumnMeta> =
                column_meta(&fresh, table).into_iter().collect();
            let b: std::collections::BTreeSet<ColumnMeta> =
                column_meta(&runner.conn, table).into_iter().collect();
            assert_eq!(
                a,
                b,
                "{table}: fresh SCHEMA and migrated column metadata differ\n  only fresh: {:?}\n  only migrated: {:?}",
                a.difference(&b).collect::<Vec<_>>(),
                b.difference(&a).collect::<Vec<_>>()
            );
        }

        // Indexes: only the phase-owned idx_transcript_progress_session is compared. Known
        // pre-existing index drift, intentionally not compared here (exists only on fresh
        // DBs since migrations 001-006, outside D-05): idx_sessions_start_time,
        // idx_sessions_last_updated, idx_sessions_cost, idx_sessions_device,
        // idx_daily_date_cost, idx_daily_device, idx_monthly_device.
        for conn in [&fresh, &runner.conn] {
            let tbl: String = conn
                .query_row(
                    "SELECT tbl_name FROM sqlite_master WHERE type='index' AND name='idx_transcript_progress_session'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(tbl, "transcript_progress");
            let cols: Vec<String> = conn
                .prepare("SELECT name FROM pragma_index_info('idx_transcript_progress_session')")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(cols, vec!["session_id"]);
        }
    }

    /// Snapshot every seeded v6 row: the v6 column list, ordered by primary key.
    fn seeded_rows(conn: &Connection) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
        let v6 = Connection::open_in_memory().unwrap();
        v6.execute_batch(V6_SCHEMA).unwrap();
        [
            ("sessions", "session_id"),
            ("daily_stats", "date"),
            ("monthly_stats", "month"),
            ("session_archive", "id"),
            ("learned_context_windows", "model_name"),
        ]
        .iter()
        .map(|(table, pk)| {
            let cols = column_names(&v6, table);
            let sql = format!("SELECT {} FROM {} ORDER BY {}", cols.join(", "), table, pk);
            let mut stmt = conn.prepare(&sql).unwrap();
            let rows = stmt
                .query_map([], |row| {
                    (0..cols.len())
                        .map(|i| row.get::<_, rusqlite::types::Value>(i))
                        .collect::<Result<Vec<_>>>()
                })
                .unwrap()
                .map(|r| r.unwrap())
                .collect::<Vec<_>>();
            assert!(!rows.is_empty(), "seed must populate {table}");
            (table.to_string(), rows)
        })
        .collect()
    }

    /// v6 fixture DB stamped 6, 7 and 8 by hand, without any 007/008 column, table or index.
    fn stamped_v8_without_columns(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let path = v6_fixture_db(dir, name);
        let conn = Connection::open(&path).unwrap();
        stamp(&conn, 7);
        stamp(&conn, 8);
        path
    }

    #[test]
    fn repair_adds_missing_columns_when_stamped_v8() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = stamped_v8_without_columns(&temp_dir, "stamped_v8.db");
        let before = seeded_rows(&Connection::open(&db_path).unwrap());

        drop(crate::database::SqliteDatabase::new(&db_path).expect("open must repair, not fail"));

        let conn = Connection::open(&db_path).unwrap();
        assert_eq!(missing_required_columns(&conn, 8).unwrap(), vec![]);
        assert_eq!(
            missing_required_objects(&conn, 8).unwrap(),
            Vec::<&str>::new()
        );
        assert_eq!(recorded_versions(&conn), vec![6, 7, 8]);
        assert_eq!(seeded_rows(&conn), before, "repair must not change any row");
    }

    #[test]
    fn repair_recreates_missing_session_archive() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = stamped_v8_without_columns(&temp_dir, "no_archive_v8.db");
        Connection::open(&db_path)
            .unwrap()
            .execute("DROP TABLE session_archive", [])
            .unwrap();

        drop(
            crate::database::SqliteDatabase::new(&db_path)
                .expect("a missing session_archive must be recreated, not fail every open"),
        );

        let conn = Connection::open(&db_path).unwrap();
        assert_eq!(missing_required_columns(&conn, 8).unwrap(), vec![]);
        assert_eq!(
            missing_required_objects(&conn, 8).unwrap(),
            Vec::<&str>::new()
        );
        assert_eq!(recorded_versions(&conn), vec![6, 7, 8]);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_archive", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "a recreated archive starts empty");

        // The recreated table matches a fresh SCHEMA table column for column.
        let fresh_dir = TempDir::new().unwrap();
        let fresh_path = fresh_dir.path().join("fresh.db");
        drop(crate::database::SqliteDatabase::new(&fresh_path).unwrap());
        let fresh = Connection::open(&fresh_path).unwrap();
        assert_eq!(
            column_meta(&conn, "session_archive"),
            column_meta(&fresh, "session_archive")
        );
    }

    #[test]
    fn add_missing_columns_errors_and_rolls_back_when_required_table_missing() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = stamped_v8_without_columns(&temp_dir, "no_archive_tx.db");
        let mut conn = Connection::open(&db_path).unwrap();
        conn.execute("DROP TABLE session_archive", []).unwrap();

        let tx = conn.transaction().unwrap();
        let err = add_missing_columns(&tx, |v| v <= 8)
            .expect_err("a missing registry table must be an explicit error");
        drop(tx);
        assert!(
            err.to_string().contains("session_archive"),
            "error must name the missing table: {err}"
        );
        assert!(!crate::retry::is_retryable_error(
            &crate::error::StatuslineError::Database(err)
        ));
        assert!(
            !column_names(&conn, "sessions")
                .iter()
                .any(|c| c == "claude_version"),
            "nothing half-applied"
        );
    }

    #[test]
    fn repair_skips_schema_newer_than_binary() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = stamped_v8_without_columns(&temp_dir, "newer_v9.db");
        let conn = Connection::open(&db_path).unwrap();
        stamp(&conn, 9);
        let before = column_names(&conn, "sessions");
        drop(conn);

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner
            .migrate()
            .expect("a newer schema must open without repair");

        assert_eq!(
            column_names(&runner.conn, "sessions"),
            before,
            "an older binary must not add columns to a newer schema"
        );
        assert!(!missing_required_columns(&runner.conn, 8)
            .unwrap()
            .is_empty());
        assert_eq!(recorded_versions(&runner.conn), vec![6, 7, 8, 9]);
    }

    #[test]
    fn repair_recreates_missing_transcript_index() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("no_index.db");
        drop(crate::database::SqliteDatabase::new(&db_path).unwrap());
        let read_row = |conn: &Connection| -> (String, String, i64, i64) {
            conn.query_row(
                "SELECT path, session_id, offset, output_tokens FROM transcript_progress",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
        };
        let before = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO transcript_progress (path, session_id, offset, output_tokens, updated_at)
                 VALUES ('/tmp/synthetic/t.jsonl', 'synthetic-session', 4096, 77, '2026-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
            conn.execute("DROP INDEX idx_transcript_progress_session", [])
                .unwrap();
            read_row(&conn)
        };

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        let has_index: i64 = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_transcript_progress_session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_index, 1);
        assert_eq!(read_row(&runner.conn), before);
    }

    #[test]
    fn repair_healthy_path_takes_no_write_lock() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("healthy.db");
        drop(crate::database::SqliteDatabase::new(&db_path).unwrap());
        let schema_version = |conn: &Connection| -> i64 {
            conn.query_row("PRAGMA schema_version", [], |r| r.get(0))
                .unwrap()
        };
        let before = schema_version(&Connection::open(&db_path).unwrap());

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        let blocker = Connection::open(&db_path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let result = runner.migrate();
        blocker.execute_batch("ROLLBACK").unwrap();
        result.expect("healthy path must not need the write lock");

        assert_eq!(schema_version(&runner.conn), before);
        assert_eq!(missing_required_columns(&runner.conn, 8).unwrap(), vec![]);
        assert_eq!(
            missing_required_objects(&runner.conn, 8).unwrap(),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn concurrent_repairers_add_each_column_once() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = stamped_v8_without_columns(&temp_dir, "concurrent.db");
        // Switch to WAL up front so the two constructors do not race the journal-mode change.
        Connection::open(&db_path)
            .unwrap()
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let path = db_path.clone();
                std::thread::spawn(move || -> Result<()> {
                    barrier.wait();
                    MigrationRunner::new(&path)?.migrate()
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().expect("each repairer must succeed");
        }

        let conn = Connection::open(&db_path).unwrap();
        for row in REQUIRED_COLUMNS {
            assert_eq!(
                column_names(&conn, row.table)
                    .iter()
                    .filter(|c| *c == row.name)
                    .count(),
                1,
                "{}.{} must exist exactly once",
                row.table,
                row.name
            );
        }
    }
}
