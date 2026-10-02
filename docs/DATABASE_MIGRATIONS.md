# Database Migrations Guide

This guide explains how Claudia Statusline manages schema evolution for the local SQLite store and the optional Turso backend used for cloud sync builds.

## Overview
- **Local database** – `stats.db` under the XDG data directory. `SqliteDatabase::new` creates a new database from the full `SCHEMA` (`src/database/schema.rs`) and stamps it with the current schema version in the same transaction. An existing database runs its pending migrations (`src/migrations/mod.rs`) when it is first opened by a process.
- **Remote database (Turso)** – only used when the binary is built with `--features turso-sync`. Examples under `examples/` manage schema creation and simple versioning for the hosted database.

## Local Database
A new database gets the complete schema at once. An existing database is brought up to date by the `Migration` implementations in `src/migrations/mod.rs`, applied in version order and recorded in `schema_migrations`:

1. `InitialJsonToSqlite` – imports legacy `stats.json` data when moving to SQLite.
2. `AddMetaTable` – introduces the `meta` table for maintenance bookkeeping.
3. `AddSyncMetadata` – adds `device_id` columns; with `--features turso-sync` it also adds sync timestamps and the `sync_meta` table.
4. `AddAdaptiveLearning` – adaptive context learning: the `learned_context_windows` table and session metadata.
5. `AddBurnRateTracking` – burn-rate columns (`active_time_seconds`, `last_activity`) and the `session_archive` table used by `auto_reset`.
6. `AddDailyTokenTracking` – token columns on `daily_stats` and `monthly_stats`.
7. `AddAgentTracking` – `claude_version` and agent/main token columns on `sessions`, plus the `transcript_progress` table.
8. `AddAccountingBaseline` – nullable `raw_cost`, `raw_lines_added`, `raw_lines_removed`, `process_start_ms` on `sessions` and `session_archive`; pure DDL, NULL = no baseline yet, readers use `COALESCE(raw_cost, cost)`.

The CLI runs pending migrations automatically when it opens the database. Embedders can also execute them via the helper:

```rust
use statusline::migrations::run_migrations;

fn main() {
    // Best-effort: ignores errors so normal CLI output is unaffected.
    run_migrations();
}
```

For finer control you can construct a `MigrationRunner` manually:
```rust
use statusline::migrations::MigrationRunner;
use statusline::stats::StatsData;

fn apply_migrations() -> rusqlite::Result<()> {
    if let Ok(db_path) = StatsData::get_sqlite_path() {
        let mut runner = MigrationRunner::new(&db_path)?;
        runner.migrate()?;
    }
    Ok(())
}
```

### Adding a New Local Migration
A new database never runs migrations; it is built from `SCHEMA` and stamped with the current version. So `SCHEMA`, the fresh-database version literal and the migration must change together, or fresh and upgraded databases drift apart.

1. Define a struct that implements the `Migration` trait, using the next free version number.
2. Register it in `MigrationRunner::load_all_migrations()`.
3. Add the same columns/tables to `SCHEMA` in `src/database/schema.rs`.
4. In the same change, bump the fresh-database version literal and its description in `SqliteDatabase::new` (`src/database/mod.rs`).
5. Make `up` idempotent: skip columns that already exist (check `PRAGMA table_info`), since a database can be partly migrated.
6. Never backfill or rewrite existing rows unless the change is explicitly a data migration; treat any such change as a data-integrity event and test it on a copy of a populated database.
7. Update the `current_version()` assertions in the `src/migrations/mod.rs` tests and keep the fresh-vs-migrated parity test (`fresh_and_migrated_schemas_match`) passing.

Example skeleton:
```rust
pub struct AddNewColumn;

impl Migration for AddNewColumn {
    fn version(&self) -> u32 { 9 } // the next free version number
    fn description(&self) -> &str { "Add new_column to sessions" }

    fn up(&self, tx: &Transaction) -> rusqlite::Result<()> {
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN new_column TEXT",
            [],
        )?
    }

    fn down(&self, _tx: &Transaction) -> rusqlite::Result<()> {
        // Optional: implement if a proper rollback path exists
        Ok(())
    }
}
```

### Checking Local Migration State
The helper stores applied versions in `schema_migrations`:
```sql
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL,
    checksum TEXT NOT NULL,
    description TEXT,
    execution_time_ms INTEGER
);
```
Inspect via:
```bash
sqlite3 ~/.local/share/claudia-statusline/stats.db 'SELECT * FROM schema_migrations ORDER BY version;'
```

## Remote (Turso) Schema
When the `turso-sync` feature is enabled you are expected to provision the remote schema yourself. The examples in `examples/` and `scripts/setup-turso-schema.sql` cover the basics.

1. **Create the schema**
   ```bash
   cargo run --example setup_schema --features turso-sync --release
   ```
   This creates `sessions`, `daily_stats`, and `monthly_stats` tables (each keyed by `device_id`) plus supporting indexes.

2. **Populate/track migrations**
   ```bash
   cargo run --example migrate_turso --features turso-sync --release
   ```
   The example ensures a `schema_migrations` table exists remotely and records which migrations ran. Out of the box it registers the initial schema as version 1.

3. **Check status**
   ```bash
   cargo run --example check_turso_version --features turso-sync --release
   ```

### Extending Remote Migrations
Remote migrations are expressed as async functions inside `examples/migrate_turso.rs`. Add new entries to the `migrations` vector, bumping the version number sequentially:
```rust
async fn add_notes_column(conn: &libsql::Connection) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "ALTER TABLE sessions ADD COLUMN notes TEXT DEFAULT ''",
        (),
    ).await?;
    Ok(())
}

let migrations = vec![
    (1, "Initial schema", initial_schema),
    (2, "Add notes column", add_notes_column),
];
```
Run `migrate_turso` after updating the list.

## Troubleshooting
- **Local migration fails** – enable debug logs (`RUST_LOG=debug statusline`) and inspect `schema_migrations`. Because migrations are best-effort they will not block CLI output, but you may need to run them manually.
- **Remote migration fails** – check the error from the example binary, verify the Turso connection, or inspect the database using the Turso CLI.
- **JSON/SQLite parity issues** – run the CLI once, then `statusline migrate --finalize` to archive or delete `stats.json` after verifying totals match.

## Future Enhancements
- Expose a CLI entry point to `run_migrations()` (currently only available to embedders/tests).
- Add checksums for migration files to detect drift.
- Provide a dry-run mode for `migration_turso` similar to the sync push/pull commands.
