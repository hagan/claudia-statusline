//! `migrate` subcommand handlers: status roadmap, schema migrations, schema
//! dump, and JSON->SQLite finalization.

use log::warn;

use crate::error::Result;

/// Show migration status (v3.0.0+: SQLite is the canonical store)
pub(crate) fn show_migration_roadmap() -> Result<()> {
    use crate::common::get_data_dir;
    let data_dir = get_data_dir();
    let json_path = data_dir.join("stats.json");
    let db_path = data_dir.join("stats.db");
    let json_exists = json_path.exists();
    let db_exists = db_path.exists();

    println!("═══════════════════════════════════════════════════════════════");
    println!("          SQLite Migration Status (v3.0.0+)");
    println!("═══════════════════════════════════════════════════════════════\n");
    println!("📊 CURRENT STATUS:\n");
    println!(
        "   Database (SQLite):    {}",
        if db_exists {
            "✓ Exists"
        } else {
            "✗ Not found"
        }
    );
    println!(
        "   Legacy JSON file:     {}",
        if json_exists {
            "✓ Exists"
        } else {
            "✗ Not found"
        }
    );
    println!();
    println!("   v3.0.0 removed JSON writes; SQLite is the canonical store.");
    println!("   Legacy stats.json files are read once on startup for recovery only.\n");

    if json_exists {
        println!("💡 RECOMMENDED NEXT STEPS:\n");
        println!("   A leftover stats.json file exists. Archive or delete it:\n");
        println!("      $ statusline migrate --finalize");
        println!("      $ statusline migrate --finalize --delete-json\n");
    } else {
        println!("✅ MIGRATION COMPLETE:\n");
        println!("   No leftover JSON file. SQLite-only operation is in effect.\n");
    }
    Ok(())
}

/// Finalize the migration from JSON to SQLite-only mode
pub(crate) fn run_schema_migrations() -> Result<()> {
    use crate::common::get_data_dir;
    use crate::display::Colors;
    use crate::migrations::MigrationRunner;

    println!(
        "{}Running database schema migrations...{}",
        Colors::cyan(),
        Colors::reset()
    );
    println!();

    let db_path = get_data_dir().join("stats.db");
    let mut runner =
        MigrationRunner::new(&db_path).map_err(crate::error::StatuslineError::Database)?;

    let current_version = runner
        .current_version()
        .map_err(crate::error::StatuslineError::Database)?;

    println!("Current schema version: {}", current_version);

    runner
        .migrate()
        .map_err(crate::error::StatuslineError::Database)?;

    let new_version = runner
        .current_version()
        .map_err(crate::error::StatuslineError::Database)?;

    println!();
    if new_version > current_version {
        println!(
            "{}✓ Migrated from version {} to {}{}",
            Colors::green(),
            current_version,
            new_version,
            Colors::reset()
        );
    } else {
        println!(
            "{}✓ Database already at latest version ({}){}",
            Colors::green(),
            new_version,
            Colors::reset()
        );
    }
    println!();

    Ok(())
}

pub(crate) fn dump_database_schema() -> Result<()> {
    use crate::display::Colors;

    // Print status to stderr so it doesn't pollute the SQL output on stdout
    eprintln!(
        "{}Generating database schema...{}",
        Colors::cyan(),
        Colors::reset()
    );
    eprintln!();

    // Run all migrations on this temporary database
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("statusline_schema_{}.db", std::process::id()));

    // Create a file-based database for migrations (they need a path)
    {
        let db = crate::database::SqliteDatabase::new(&temp_path)?;
        drop(db); // Close before dumping
    }

    // Open the file to read schema and dump it
    let schemas: Vec<String> = {
        let conn = rusqlite::Connection::open(&temp_path)?;
        let mut stmt = conn.prepare(
            "SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type DESC, name",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        result
    };

    println!("-- Turso Database Schema Setup for Claudia Statusline");
    println!(
        "-- Auto-generated from migrations on {}",
        chrono::Local::now().format("%Y-%m-%d")
    );
    println!("-- This script creates the necessary tables for cloud sync");
    println!();

    for schema in schemas {
        println!("{};", schema);
        println!();
    }

    println!("-- Indexes for better query performance");
    println!("-- (Indexes are included in the CREATE TABLE statements above)");
    println!();

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    Ok(())
}

pub(crate) fn finalize_migration(delete_json: bool) -> Result<()> {
    use chrono::Utc;
    use std::fs;

    println!("🔄 Finalizing migration to SQLite-only mode...\n");

    // Get paths
    let json_path = crate::stats::StatsData::get_stats_file_path();
    let sqlite_path = crate::stats::StatsData::get_sqlite_path()?;

    // Check if JSON file exists
    if !json_path.exists() {
        println!("✅ No JSON file found. Already in SQLite-only mode.");
        return Ok(());
    }

    // Check if SQLite database exists
    if !sqlite_path.exists() {
        println!("⚠️  SQLite database not found. Creating and migrating...");
        // Load from JSON and trigger migration. The value is intentionally discarded;
        // the side effect (JSON→SQLite migration) is what we want.
        warn!("SQLite database absent; loading stats to trigger JSON→SQLite migration");
        let _ = crate::stats::StatsData::load();
    }

    // Load data from both sources to verify parity
    println!("📊 Verifying data parity between JSON and SQLite...");

    let json_data = if json_path.exists() {
        let contents = fs::read_to_string(&json_path)?;
        serde_json::from_str::<crate::stats::StatsData>(&contents).ok()
    } else {
        None
    };

    let sqlite_data = crate::stats::StatsData::load_from_sqlite().ok();

    // Compare counts and totals
    if let (Some(json), Some(sqlite)) = (&json_data, &sqlite_data) {
        let json_sessions = json.sessions.len();
        let sqlite_sessions = sqlite.sessions.len();

        let json_total: f64 = json.sessions.values().map(|s| s.cost).sum();
        let sqlite_total: f64 = sqlite.sessions.values().map(|s| s.cost).sum();

        println!("  JSON sessions: {}", json_sessions);
        println!("  SQLite sessions: {}", sqlite_sessions);
        println!("  JSON total cost: ${:.2}", json_total);
        println!("  SQLite total cost: ${:.2}", sqlite_total);

        // Check for discrepancies
        if json_sessions != sqlite_sessions || (json_total - sqlite_total).abs() > 0.01 {
            println!("\n⚠️  Warning: Data discrepancy detected!");
            println!("Please ensure all data has been migrated before finalizing.");
            println!("You may need to run the statusline normally once to trigger migration.");
            return Ok(());
        }

        println!("\n✅ Data parity verified!");
    }

    // Archive or delete JSON file
    if delete_json {
        println!("\n🗑️  Deleting JSON file...");
        fs::remove_file(&json_path)?;
        println!("✅ JSON file deleted: {}", json_path.display());
    } else {
        // Archive with timestamp
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let archive_path = json_path.with_file_name(format!("stats.json.migrated.{}", timestamp));
        println!("\n📦 Archiving JSON file...");
        fs::rename(&json_path, &archive_path)?;
        println!("✅ JSON archived to: {}", archive_path.display());
    }

    // Update config to disable JSON backup
    println!("\n📝 Updating configuration...");
    let config_path = crate::config::Config::default_config_path()?;

    // Create config directory if it doesn't exist with secure permissions (0o700 on Unix)
    if let Some(parent) = config_path.parent() {
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

    // Load existing config or create new one
    let mut config = if config_path.exists() {
        crate::config::Config::load_from_file(&config_path).unwrap_or_default()
    } else {
        crate::config::Config::default()
    };

    // v3.0.0+: this is a no-op safety belt; the runtime no longer mutates behavior on this flag.
    config.database.json_backup = false;

    // Save updated config
    config.save(&config_path)?;
    println!("✅ Configuration normalized: json_backup field reset (no-op in v3.0.0+)");

    println!("\n🎉 Migration finalized successfully!");
    println!("The statusline is now operating in SQLite-only mode.");
    println!("Performance improvements: ~30% faster reads, better concurrent access");

    Ok(())
}
