# Database Migrations Guide

This document explains how database schema migrations work in Claudia Statusline and how to add new migrations.

## Overview

Claudia Statusline uses **two databases**:
1. **Local SQLite** (`~/.local/share/claudia-statusline/stats.db`) - Your device's stats
2. **Remote Turso** (`libsql://...`) - Cloud-synced stats (optional, requires `turso-sync` feature)

Both databases have **independent migration tracking** to ensure schema consistency.

## Migration System Architecture

### Local Database (Automatic)

The local database migrations run **automatically** when the statusline starts:

```rust
// In src/migrations/mod.rs
pub fn run_migrations() {
    if let Ok(db_path) = StatsData::get_sqlite_path() {
        if let Ok(mut runner) = MigrationRunner::new(&db_path) {
            let _ = runner.migrate();
        }
    }
}
```

**Current local migrations:**
- v1: Import JSON data to SQLite
- v2: Add meta table for maintenance metadata
- v3: Add sync columns (device_id, sync_timestamp)

### Remote Turso Database (Manual)

Turso migrations must be run **manually** using the migration tool:

```bash
cargo run --example migrate_turso --features turso-sync --release
```

**Current remote migrations:**
- v1: Initial schema (sessions, daily_stats, monthly_stats tables)

## Schema Version Tracking

Both databases use a `schema_migrations` table:

```sql
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL,
    description TEXT,
    execution_time_ms INTEGER
);
```

This tracks:
- **version**: Sequential migration number (1, 2, 3, ...)
- **applied_at**: When the migration ran
- **description**: Human-readable description
- **execution_time_ms**: Performance tracking

## How to Add a New Migration

### For Local Database

1. **Create a new migration struct** in `src/migrations/mod.rs`:

```rust
pub struct AddNewFeature;

impl Migration for AddNewFeature {
    fn version(&self) -> u32 {
        4  // Next sequential version
    }

    fn description(&self) -> &str {
        "Add new_column to sessions table"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN new_column TEXT",
            [],
        )?;
        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // SQLite doesn't support DROP COLUMN easily
        // Document the manual rollback process or recreate table
        Ok(())
    }
}
```

2. **Register it in `load_all_migrations()`**:

```rust
fn load_all_migrations() -> Vec<Box<dyn Migration>> {
    vec![
        Box::new(InitialJsonToSqlite),
        Box::new(AddMetaTable),
        Box::new(AddSyncMetadata),
        Box::new(AddNewFeature),  // Add your new migration
    ]
}
```

3. **Test it**:

```bash
# The migration will run automatically on next statusline invocation
cargo test
```

### For Remote Turso Database

1. **Add the migration function** in `examples/migrate_turso.rs`:

```rust
async fn add_new_column(conn: &libsql::Connection) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "ALTER TABLE sessions ADD COLUMN new_column TEXT",
        (),
    )
    .await?;
    Ok(())
}
```

2. **Register it in the migrations list**:

```rust
let migrations = vec![
    (1, "Initial schema", initial_schema),
    (2, "Add new column", add_new_column),  // Add your migration
];
```

3. **Run it manually**:

```bash
cargo run --example migrate_turso --features turso-sync --release
```

## Best Practices

### DO ✅

1. **Use sequential version numbers** (1, 2, 3, ...)
2. **Make migrations idempotent** - Safe to run multiple times
3. **Test migrations** with sample data before deploying
4. **Use `IF NOT EXISTS`** for CREATE statements
5. **Use `ADD COLUMN`** instead of recreating tables
6. **Document breaking changes** in migration description
7. **Keep migrations small** - One logical change per migration

### DON'T ❌

1. **Don't skip version numbers** - Always sequential
2. **Don't modify existing migrations** - Create new ones instead
3. **Don't use DROP COLUMN** - SQLite doesn't support it well
4. **Don't delete data** without backup/confirmation
5. **Don't change column types** - Create new column, migrate, drop old

## Migration Patterns

### Adding a Column

```rust
// Safe - uses ALTER TABLE ADD COLUMN
fn up(&self, tx: &Transaction) -> Result<()> {
    tx.execute(
        "ALTER TABLE sessions ADD COLUMN new_field TEXT DEFAULT ''",
        [],
    )?;
    Ok(())
}
```

### Creating a New Table

```rust
fn up(&self, tx: &Transaction) -> Result<()> {
    tx.execute(
        "CREATE TABLE IF NOT EXISTS new_table (
            id INTEGER PRIMARY KEY,
            data TEXT NOT NULL
        )",
        [],
    )?;
    Ok(())
}
```

### Creating an Index

```rust
fn up(&self, tx: &Transaction) -> Result<()> {
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_device
         ON sessions(device_id, last_updated DESC)",
        [],
    )?;
    Ok(())
}
```

### Changing Column Types (Complex)

Since SQLite doesn't support `ALTER COLUMN`, you need to:

```rust
fn up(&self, tx: &Transaction) -> Result<()> {
    // 1. Create new table with correct schema
    tx.execute(
        "CREATE TABLE sessions_new (
            session_id TEXT PRIMARY KEY,
            cost REAL NOT NULL  -- Changed from INTEGER
        )",
        [],
    )?;

    // 2. Copy data with type conversion
    tx.execute(
        "INSERT INTO sessions_new SELECT session_id, CAST(cost AS REAL) FROM sessions",
        [],
    )?;

    // 3. Drop old table
    tx.execute("DROP TABLE sessions", [])?;

    // 4. Rename new table
    tx.execute("ALTER TABLE sessions_new RENAME TO sessions", [])?;

    Ok(())
}
```

## Checking Migration Status

### Local Database

```bash
# Check current version (automatically migrates on run)
statusline health
```

### Remote Turso Database

```bash
# Check Turso schema version
cargo run --example check_turso_version --features turso-sync --release
```

## Troubleshooting

### Migration Failed on Local Database

The local migration system is **best-effort** - it won't crash the statusline if it fails:

```rust
pub fn run_migrations() {
    if let Ok(db_path) = StatsData::get_sqlite_path() {
        if let Ok(mut runner) = MigrationRunner::new(&db_path) {
            let _ = runner.migrate();  // Ignores errors
        }
    }
}
```

If a migration fails:
1. Check logs: `RUST_LOG=debug statusline`
2. Manually inspect database: `sqlite3 ~/.local/share/claudia-statusline/stats.db`
3. Check `schema_migrations` table: `SELECT * FROM schema_migrations;`

### Migration Failed on Turso

If Turso migration fails:
1. Check error message from `migrate_turso`
2. Verify Turso connection: `statusline sync --status`
3. Manually inspect: Use Turso CLI or web dashboard
4. Rollback if needed (see next section)

### Rolling Back Migrations

**Local database:**
```bash
# Not yet implemented - migrations are forward-only
# If needed, restore from JSON backup or recreate database
```

**Turso database:**
```bash
# Manually delete from schema_migrations
# Then manually undo schema changes via Turso CLI
```

## Future Enhancements

Potential improvements to the migration system:

1. **Automatic Turso migrations** - Run on `statusline sync` command
2. **Migration checksums** - Verify migrations haven't been modified
3. **Rollback support** - Implement `down()` migrations properly
4. **Migration testing** - Unit tests for each migration
5. **Schema diff tool** - Compare local vs remote schemas
6. **Dry-run mode** - Preview migrations before applying

## Examples

### Example: Adding a "notes" field to sessions

**Local migration (`src/migrations/mod.rs`):**
```rust
pub struct AddSessionNotes;

impl Migration for AddSessionNotes {
    fn version(&self) -> u32 { 4 }

    fn description(&self) -> &str {
        "Add notes field to sessions table"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN notes TEXT",
            [],
        )?;
        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // SQLite can't drop columns - document manual process
        Ok(())
    }
}
```

**Turso migration (`examples/migrate_turso.rs`):**
```rust
async fn add_session_notes(conn: &libsql::Connection) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "ALTER TABLE sessions ADD COLUMN notes TEXT",
        (),
    )
    .await?;
    Ok(())
}

// In main():
let migrations = vec![
    (1, "Initial schema", initial_schema),
    (2, "Add session notes", add_session_notes),
];
```

**Apply:**
```bash
# Local: Automatic on next run
cargo build --release

# Turso: Manual
cargo run --example migrate_turso --features turso-sync --release
```

## Summary

- ✅ **Local migrations**: Automatic, tracked in `schema_migrations`
- ✅ **Turso migrations**: Manual, tracked in `schema_migrations`
- ✅ **Version tracking**: Both databases independently versioned
- ✅ **Forward-only**: Migrations go forward, rollback is manual
- ✅ **Idempotent**: Safe to run multiple times
- ✅ **Easy to extend**: Add new struct + register it

Your database schema changes are **safe, tracked, and easy to manage**! 🎉
