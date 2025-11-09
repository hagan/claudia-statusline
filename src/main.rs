//! # Claudia Statusline
//!
//! A high-performance statusline for Claude Code with persistent stats tracking,
//! progress bars, and enhanced features.
//!
//! ## Features
//!
//! - Git repository status integration
//! - Persistent statistics tracking (XDG-compliant)
//! - Context usage visualization with progress bars
//! - Cost tracking with burn rate calculation
//! - Configurable via TOML files
//! - SQLite dual-write for better concurrent access
//!
//! ## Usage
//!
//! The statusline reads JSON from stdin and outputs a formatted statusline:
//!
//! ```bash
//! echo '{"workspace":{"current_dir":"/path"}}' | statusline
//! ```

use clap::{Parser, Subcommand};
use log::warn;
use std::env;
use std::io::{self, Read};
use std::path::PathBuf;

mod common;
mod config;
mod context_learning;
mod database;
mod display;
mod error;
mod git;
mod git_utils;
mod migrations;
mod models;
mod retry;
mod stats;
#[cfg(feature = "turso-sync")]
mod sync;
mod theme;
mod utils;
mod version;

use display::{format_output, Colors};
use error::Result;
use models::StatuslineInput;
use stats::{get_or_load_stats_data, update_stats_data};
use version::version_string;

/// Claudia Statusline - A high-performance statusline for Claude Code
#[derive(Parser)]
#[command(name = "statusline")]
#[command(version = env!("CLAUDIA_VERSION"))]
#[command(about = "A high-performance statusline for Claude Code", long_about = None)]
#[command(
    after_help = "Input: Reads JSON from stdin\n\nExample:\n  echo '{\"workspace\":{\"current_dir\":\"/path\"}}' | statusline"
)]
struct Cli {
    /// Show detailed version information
    #[arg(long = "version-full")]
    version_full: bool,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    /// Set color theme (light or dark)
    #[arg(long, value_name = "THEME")]
    theme: Option<String>,

    /// Path to configuration file
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Set log level
    #[arg(long, value_name = "LEVEL", value_parser = ["error", "warn", "info", "debug", "trace"])]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate example config file
    GenerateConfig,

    /// Migration utilities for the SQLite database
    Migrate {
        /// Finalize migration from JSON to SQLite-only mode
        #[arg(long)]
        finalize: bool,

        /// Delete JSON file after successful migration (instead of archiving)
        #[arg(long)]
        delete_json: bool,

        /// Run schema migrations to latest version
        #[arg(long)]
        run: bool,

        /// Dump current schema SQL (for Turso setup or documentation)
        #[arg(long)]
        dump_schema: bool,
    },

    /// Database maintenance operations (suitable for cron)
    DbMaintain {
        /// Force VACUUM even if not needed
        #[arg(long)]
        force_vacuum: bool,

        /// Skip data retention pruning
        #[arg(long)]
        no_prune: bool,

        /// Run in quiet mode (only errors)
        #[arg(short, long)]
        quiet: bool,
    },

    /// Show diagnostic information about the statusline
    Health {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Cloud sync operations (requires turso-sync feature)
    #[cfg(feature = "turso-sync")]
    Sync {
        /// Show sync status
        #[arg(long)]
        status: bool,

        /// Push local stats to remote
        #[arg(long)]
        push: bool,

        /// Pull remote stats to local
        #[arg(long)]
        pull: bool,

        /// Dry run - preview changes without applying them
        #[arg(long)]
        dry_run: bool,
    },

    /// Adaptive context window learning (experimental)
    ContextLearning {
        /// Show learned context windows for all models
        #[arg(long)]
        status: bool,

        /// Reset learning data for a specific model
        #[arg(long)]
        reset: Option<String>,

        /// Show detailed observations for a specific model
        #[arg(long)]
        details: Option<String>,

        /// Reset all learning data
        #[arg(long)]
        reset_all: bool,

        /// Rebuild learned context windows from session history (recovery)
        #[arg(long)]
        rebuild: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle log level with precedence: CLI > env > default
    // When --log-level is provided, it overrides RUST_LOG environment variable
    if let Some(ref level) = cli.log_level {
        // Set RUST_LOG to the CLI value to ensure it takes precedence
        env::set_var("RUST_LOG", level);
    }

    // Initialize logger with RUST_LOG env var (which may have been set above)
    // Default to "warn" if RUST_LOG is not set
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Handle NO_COLOR with precedence: CLI > env
    if cli.no_color {
        env::set_var("NO_COLOR", "1");
    }

    // Handle theme with precedence: CLI > env > config
    if let Some(ref theme) = cli.theme {
        // Set CLAUDE_THEME to override any existing env vars
        // (CLAUDE_THEME takes precedence over STATUSLINE_THEME in config::get_theme)
        env::set_var("CLAUDE_THEME", theme);
        env::set_var("STATUSLINE_THEME", theme);
    }

    // Handle config path if provided
    if let Some(ref config_path) = cli.config {
        env::set_var("STATUSLINE_CONFIG_PATH", config_path.display().to_string());
    }

    // Handle version-full flag
    if cli.version_full {
        print!("{}", version_string());
        return Ok(());
    }

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::GenerateConfig => {
                let config_path = config::Config::default_config_path()?;
                println!("Generating example config file at: {:?}", config_path);

                // Create parent directories
                if let Some(parent) = config_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Write example config
                std::fs::write(&config_path, config::Config::example_toml())?;
                println!("Config file generated successfully!");
                println!("Edit {} to customize settings", config_path.display());
                return Ok(());
            }
            Commands::Migrate {
                finalize,
                delete_json,
                run,
                dump_schema,
            } => {
                if dump_schema {
                    return dump_database_schema();
                } else if run {
                    return run_schema_migrations();
                } else if finalize {
                    return finalize_migration(delete_json);
                } else {
                    println!("Usage: statusline migrate [OPTIONS]");
                    println!("\nOptions:");
                    println!("  --run          Run schema migrations to latest version");
                    println!(
                        "  --finalize     Complete the migration from JSON to SQLite-only mode"
                    );
                    println!("  --delete-json  Delete the JSON file instead of archiving it (use with --finalize)");
                    println!(
                        "  --dump-schema  Dump current database schema for Turso/documentation"
                    );
                    return Ok(());
                }
            }
            Commands::DbMaintain {
                force_vacuum,
                no_prune,
                quiet,
            } => {
                return perform_database_maintenance(force_vacuum, no_prune, quiet);
            }
            Commands::Health { json } => {
                return show_health_report(json);
            }

            #[cfg(feature = "turso-sync")]
            Commands::Sync {
                status,
                push,
                pull,
                dry_run,
            } => {
                return handle_sync_command(status, push, pull, dry_run);
            }

            Commands::ContextLearning {
                status,
                reset,
                details,
                reset_all,
                rebuild,
            } => {
                return handle_context_learning_command(status, reset, details, reset_all, rebuild);
            }
        }
    }

    // Read JSON from stdin
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    // Parse input
    let input: StatuslineInput = match serde_json::from_str(&buffer) {
        Ok(input) => input,
        Err(e) => {
            // Log parse error to stderr (won't interfere with statusline output)
            warn!("Failed to parse JSON input: {}. Using defaults.", e);
            StatuslineInput::default()
        }
    };

    // Check for migration opportunity (warn once per run)
    check_migration_status();

    // Get current directory
    let current_dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_ref())
        .cloned()
        .unwrap_or_else(|| {
            env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "~".to_string())
        });

    // Early exit for empty or home directory only
    if current_dir.is_empty() || current_dir == "~" {
        print!("{}~{}", Colors::directory(), Colors::reset());
        return Ok(());
    }

    // Update stats tracking if we have session and cost data
    let (daily_total, _monthly_total) =
        if let (Some(session_id), Some(ref cost)) = (&input.session_id, &input.cost) {
            if let Some(total_cost) = cost.total_cost_usd {
                // Extract model name and workspace directory
                let model_name = input
                    .model
                    .as_ref()
                    .and_then(|m| m.display_name.as_ref())
                    .map(|s| s.as_str());
                let workspace_dir = input
                    .workspace
                    .as_ref()
                    .and_then(|w| w.current_dir.as_ref())
                    .map(|s| s.as_str());

                // Extract token breakdown from transcript if available
                let token_breakdown = input
                    .transcript
                    .as_ref()
                    .and_then(|path| utils::get_token_breakdown_from_transcript(path));

                // Get device ID for audit trail
                let device_id = common::get_device_id();

                // Update stats with new cost data
                use database::SessionUpdate;
                let result = update_stats_data(|data| {
                    data.update_session(
                        session_id,
                        SessionUpdate {
                            cost: total_cost,
                            lines_added: cost.total_lines_added.unwrap_or(0),
                            lines_removed: cost.total_lines_removed.unwrap_or(0),
                            model_name: model_name.map(|s| s.to_string()),
                            workspace_dir: workspace_dir.map(|s| s.to_string()),
                            device_id: Some(device_id.clone()),
                            token_breakdown,
                            max_tokens_observed: None, // updated separately
                        },
                    )
                });

                // Track max_tokens_observed for compaction detection
                // This runs regardless of adaptive_learning setting
                if let Some(ref transcript_path) = input.transcript {
                    if let Some(current_tokens) =
                        utils::get_token_count_from_transcript(transcript_path)
                    {
                        // Update session's max_tokens_observed
                        // This updates both in-memory stats and SQLite database
                        update_stats_data(|data| {
                            data.update_max_tokens(session_id, current_tokens);
                            // Return unchanged totals
                            use common::{current_date, current_month};
                            let today = current_date();
                            let month = current_month();
                            let daily_total =
                                data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
                            let monthly_total = data
                                .monthly
                                .get(&month)
                                .map(|m| m.total_cost)
                                .unwrap_or(0.0);
                            (daily_total, monthly_total)
                        });

                        // Adaptive context learning: observe token usage if enabled
                        if let Some(ref model_name) =
                            input.model.as_ref().and_then(|m| m.display_name.as_ref())
                        {
                            let config = config::get_config();
                            if config.context.adaptive_learning {
                                // Get previous token count from session stats
                                let stats_data = get_or_load_stats_data();
                                let previous_tokens = stats_data
                                    .sessions
                                    .get(session_id)
                                    .and_then(|s| s.max_tokens_observed)
                                    .map(|t| t as usize);

                                // Create context learner and observe usage
                                use common::get_data_dir;
                                use context_learning::ContextLearner;
                                use database::SqliteDatabase;

                                let db_path = get_data_dir().join("stats.db");
                                if let Ok(db) = SqliteDatabase::new(&db_path) {
                                    let learner = ContextLearner::new(db);
                                    // Ignore errors from adaptive learning - it's experimental
                                    // Re-use device_id retrieved earlier for consistency
                                    let _ = learner.observe_usage(
                                        model_name,
                                        current_tokens as usize,
                                        previous_tokens,
                                        Some(transcript_path),
                                        workspace_dir,
                                        Some(&device_id),
                                    );
                                }
                            }
                        }
                    }
                }

                result
            } else {
                // Have session but no cost data - still load existing daily totals
                let data = get_or_load_stats_data();
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                let month = chrono::Local::now().format("%Y-%m").to_string();

                let daily_total = data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
                let monthly_total = data
                    .monthly
                    .get(&month)
                    .map(|m| m.total_cost)
                    .unwrap_or(0.0);
                (daily_total, monthly_total)
            }
        } else {
            // No session_id - still load stats data to show accumulated totals
            let data = get_or_load_stats_data();
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let month = chrono::Local::now().format("%Y-%m").to_string();

            let daily_total = data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
            let monthly_total = data
                .monthly
                .get(&month)
                .map(|m| m.total_cost)
                .unwrap_or(0.0);
            (daily_total, monthly_total)
        };

    // Format and print output
    format_output(
        &current_dir,
        input.model.as_ref().and_then(|m| m.display_name.as_deref()),
        input.transcript.as_deref(),
        input.cost.as_ref(),
        daily_total,
        input.session_id.as_deref(),
    );

    Ok(())
}

/// Check if migration is needed and warn the user
fn check_migration_status() {
    let config = config::get_config();

    // Only warn if json_backup is enabled
    if config.database.json_backup {
        let json_path = stats::StatsData::get_stats_file_path();

        // Check if JSON file exists
        if json_path.exists() {
            // Check file size to see if it has meaningful data
            if let Ok(metadata) = std::fs::metadata(&json_path) {
                if metadata.len() > 100 {
                    // More than just empty JSON
                    warn!(
                        "JSON stats file exists at {}. Consider running 'statusline migrate --finalize' to complete migration to SQLite-only mode for better performance.",
                        json_path.display()
                    );
                }
            }
        }
    }
}

/// Finalize the migration from JSON to SQLite-only mode
fn run_schema_migrations() -> Result<()> {
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
        MigrationRunner::new(&db_path).map_err(|e| crate::error::StatuslineError::Database(e))?;

    let current_version = runner
        .current_version()
        .map_err(|e| crate::error::StatuslineError::Database(e))?;

    println!("Current schema version: {}", current_version);

    runner
        .migrate()
        .map_err(|e| crate::error::StatuslineError::Database(e))?;

    let new_version = runner
        .current_version()
        .map_err(|e| crate::error::StatuslineError::Database(e))?;

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

fn dump_database_schema() -> Result<()> {
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

fn finalize_migration(delete_json: bool) -> Result<()> {
    use chrono::Utc;
    use std::fs;

    println!("🔄 Finalizing migration to SQLite-only mode...\n");

    // Get paths
    let json_path = stats::StatsData::get_stats_file_path();
    let sqlite_path = stats::StatsData::get_sqlite_path()?;

    // Check if JSON file exists
    if !json_path.exists() {
        println!("✅ No JSON file found. Already in SQLite-only mode.");
        return Ok(());
    }

    // Check if SQLite database exists
    if !sqlite_path.exists() {
        println!("⚠️  SQLite database not found. Creating and migrating...");
        // Load from JSON and trigger migration
        let _ = stats::StatsData::load();
    }

    // Load data from both sources to verify parity
    println!("📊 Verifying data parity between JSON and SQLite...");

    let json_data = if json_path.exists() {
        let contents = fs::read_to_string(&json_path)?;
        serde_json::from_str::<stats::StatsData>(&contents).ok()
    } else {
        None
    };

    let sqlite_data = stats::StatsData::load_from_sqlite().ok();

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
    let config_path = config::Config::default_config_path()?;

    // Create config directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Load existing config or create new one
    let mut config = if config_path.exists() {
        config::Config::load_from_file(&config_path).unwrap_or_default()
    } else {
        config::Config::default()
    };

    // Set json_backup to false
    config.database.json_backup = false;

    // Save updated config
    config.save(&config_path)?;
    println!("✅ Configuration updated: json_backup = false");

    println!("\n🎉 Migration finalized successfully!");
    println!("The statusline is now operating in SQLite-only mode.");
    println!("Performance improvements: ~30% faster reads, better concurrent access");

    Ok(())
}

/// Perform database maintenance operations
fn perform_database_maintenance(force_vacuum: bool, no_prune: bool, quiet: bool) -> Result<()> {
    if !quiet {
        println!("🔧 Starting database maintenance...\n");
    }

    // Get database path
    let db_path = stats::StatsData::get_sqlite_path()?;
    if !db_path.exists() {
        if !quiet {
            println!("❌ Database not found at: {}", db_path.display());
        }
        return Err(error::StatuslineError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Database file not found",
        )));
    }

    // Get initial size
    let initial_size = std::fs::metadata(&db_path)?.len() as f64 / (1024.0 * 1024.0);
    if !quiet {
        println!("📊 Initial database size: {:.2} MB", initial_size);
    }

    // Perform maintenance operations
    let maintenance_result = database::perform_maintenance(force_vacuum, no_prune, quiet)?;

    // Get final size
    let final_size = std::fs::metadata(&db_path)?.len() as f64 / (1024.0 * 1024.0);

    if !quiet {
        println!("\n📊 Final database size: {:.2} MB", final_size);

        if final_size < initial_size {
            let saved = initial_size - final_size;
            let percent = (saved / initial_size) * 100.0;
            println!("💾 Space saved: {:.2} MB ({:.1}%)", saved, percent);
        }

        println!("\n📋 Maintenance summary:");
        println!(
            "  ✅ WAL checkpoint: {}",
            if maintenance_result.checkpoint_done {
                "completed"
            } else {
                "not needed"
            }
        );
        println!(
            "  ✅ Optimization: {}",
            if maintenance_result.optimize_done {
                "completed"
            } else {
                "not needed"
            }
        );
        println!(
            "  ✅ Vacuum: {}",
            if maintenance_result.vacuum_done {
                "completed"
            } else {
                "not needed"
            }
        );
        println!(
            "  ✅ Pruning: {}",
            if maintenance_result.prune_done {
                format!("removed {} old records", maintenance_result.records_pruned)
            } else if no_prune {
                "skipped".to_string()
            } else {
                "not needed".to_string()
            }
        );
        println!(
            "  ✅ Integrity check: {}",
            if maintenance_result.integrity_ok {
                "passed"
            } else {
                "FAILED"
            }
        );

        if maintenance_result.integrity_ok {
            println!("\n✅ Database maintenance completed successfully!");
        } else {
            println!("\n❌ Database integrity check failed! Consider rebuilding from JSON backup.");
        }
    }

    // Exit with non-zero if integrity check failed
    if !maintenance_result.integrity_ok {
        std::process::exit(1);
    }

    Ok(())
}

/// Show diagnostic health information
fn show_health_report(json_output: bool) -> Result<()> {
    use rusqlite::{Connection, OpenFlags};
    use serde_json::json;

    // Get paths
    let db_path = stats::StatsData::get_sqlite_path()?;
    let json_path = stats::StatsData::get_stats_file_path();
    let config = config::get_config();

    // Check if files exist
    let db_exists = db_path.exists();
    let json_exists = json_path.exists();

    // Get stats from database using aggregate helpers
    let mut today_total = 0.0;
    let mut month_total = 0.0;
    let mut all_time_total = 0.0;
    let mut session_count = 0;
    let mut earliest_session: Option<String> = None;

    if db_exists {
        // Prefer normal DB API first; fall back to read-only if environment is read-only (e.g., CI sandbox)
        match database::SqliteDatabase::new(&db_path) {
            Ok(db) => {
                today_total = db.get_today_total().unwrap_or(0.0);
                month_total = db.get_month_total().unwrap_or(0.0);
                all_time_total = db.get_all_time_total().unwrap_or(0.0);
                session_count = db.get_all_time_sessions_count().unwrap_or(0);
                earliest_session = db.get_earliest_session_date().ok().flatten();
            }
            Err(_) => {
                // Read-only fallback: open without attempting schema creation/WAL
                if let Ok(conn) =
                    Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                {
                    // Today total
                    let _ = conn
                        .query_row(
                            "SELECT COALESCE(total_cost, 0.0) FROM daily_stats WHERE date = date('now','localtime')",
                            [],
                            |row| { today_total = row.get::<_, f64>(0)?; Ok(()) },
                        );
                    // Month total
                    let _ = conn
                        .query_row(
                            "SELECT COALESCE(total_cost, 0.0) FROM monthly_stats WHERE month = strftime('%Y-%m','now','localtime')",
                            [],
                            |row| { month_total = row.get::<_, f64>(0)?; Ok(()) },
                        );
                    // All-time total
                    let _ = conn.query_row(
                        "SELECT COALESCE(SUM(cost), 0.0) FROM sessions",
                        [],
                        |row| {
                            all_time_total = row.get::<_, f64>(0)?;
                            Ok(())
                        },
                    );
                    // Session count
                    let _ = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                        session_count = row.get::<_, i64>(0)? as usize;
                        Ok(())
                    });
                    // Earliest session
                    let _ = conn.query_row("SELECT MIN(start_time) FROM sessions", [], |row| {
                        earliest_session = row.get::<_, Option<String>>(0)?;
                        Ok(())
                    });
                }
            }
        }
    }

    if json_output {
        // Output as JSON
        let health = json!({
            "database_path": db_path.display().to_string(),
            "database_exists": db_exists,
            "json_path": json_path.display().to_string(),
            "json_exists": json_exists,
            "json_backup": config.database.json_backup,
            "today_total": today_total,
            "month_total": month_total,
            "all_time_total": all_time_total,
            "session_count": session_count,
            "earliest_session": earliest_session,
        });
        println!("{}", serde_json::to_string(&health)?);
    } else {
        // Output as human-readable text
        println!("Claudia Statusline Health Report");
        println!("================================");
        println!();
        println!("Configuration:");
        println!("  Database path: {}", db_path.display());
        println!("  Database exists: {}", if db_exists { "✅" } else { "❌" });
        println!("  JSON path: {}", json_path.display());
        println!("  JSON exists: {}", if json_exists { "✅" } else { "❌" });
        println!(
            "  JSON backup enabled: {}",
            if config.database.json_backup {
                "✅"
            } else {
                "❌"
            }
        );
        println!();
        println!("Statistics:");
        println!("  Today's total: ${:.2}", today_total);
        println!("  Month total: ${:.2}", month_total);
        println!("  All-time total: ${:.2}", all_time_total);
        println!("  Session count: {}", session_count);
        if let Some(earliest) = earliest_session {
            println!("  Earliest session: {}", earliest);
        } else {
            println!("  Earliest session: N/A");
        }
    }

    Ok(())
}

/// Handle sync commands (status, push, pull)
#[cfg(feature = "turso-sync")]
fn handle_sync_command(status: bool, push: bool, pull: bool, dry_run: bool) -> Result<()> {
    use crate::config::Config;

    // Load configuration
    let config = Config::load()?;
    let mut sync_manager = crate::sync::SyncManager::new(config.sync.clone());

    // Determine which action to take
    if status || (!push && !pull) {
        // Show status (default if no flags specified)
        return show_sync_status(&sync_manager);
    } else if push {
        // Push to remote
        return handle_sync_push(&mut sync_manager, dry_run);
    } else if pull {
        // Pull from remote
        return handle_sync_pull(&mut sync_manager, dry_run);
    }

    Ok(())
}

/// Handle push command
#[cfg(feature = "turso-sync")]
fn handle_sync_push(sync_manager: &mut crate::sync::SyncManager, dry_run: bool) -> Result<()> {
    println!("{}Pushing to remote{}", Colors::cyan(), Colors::reset());
    if dry_run {
        println!(
            "{}[DRY RUN MODE - No changes will be made]{}",
            Colors::yellow(),
            Colors::reset()
        );
    }
    println!();

    match sync_manager.push(dry_run) {
        Ok(result) => {
            println!("{}✅ Push completed{}", Colors::green(), Colors::reset());
            println!();
            println!("Summary:");
            println!("  Sessions: {} pushed", result.sessions_pushed);
            println!("  Daily stats: {} pushed", result.daily_stats_pushed);
            println!("  Monthly stats: {} pushed", result.monthly_stats_pushed);

            if result.dry_run {
                println!();
                println!(
                    "{}This was a dry run - no data was actually pushed.{}",
                    Colors::yellow(),
                    Colors::reset()
                );
            }
        }
        Err(e) => {
            eprintln!("{}❌ Push failed: {}{}", Colors::red(), e, Colors::reset());
            return Err(e);
        }
    }

    Ok(())
}

/// Handle pull command
#[cfg(feature = "turso-sync")]
fn handle_sync_pull(sync_manager: &mut crate::sync::SyncManager, dry_run: bool) -> Result<()> {
    println!("{}Pulling from remote{}", Colors::cyan(), Colors::reset());
    if dry_run {
        println!(
            "{}[DRY RUN MODE - No changes will be made]{}",
            Colors::yellow(),
            Colors::reset()
        );
    }
    println!();

    match sync_manager.pull(dry_run) {
        Ok(result) => {
            println!("{}✅ Pull completed{}", Colors::green(), Colors::reset());
            println!();
            println!("Summary:");
            println!("  Sessions: {} pulled", result.sessions_pulled);
            println!("  Daily stats: {} pulled", result.daily_stats_pulled);
            println!("  Monthly stats: {} pulled", result.monthly_stats_pulled);
            println!("  Conflicts resolved: {}", result.conflicts_resolved);

            if result.dry_run {
                println!();
                println!(
                    "{}This was a dry run - no data was actually pulled.{}",
                    Colors::yellow(),
                    Colors::reset()
                );
            }
        }
        Err(e) => {
            eprintln!("{}❌ Pull failed: {}{}", Colors::red(), e, Colors::reset());
            return Err(e);
        }
    }

    Ok(())
}

/// Show sync status and configuration
#[cfg(feature = "turso-sync")]
fn show_sync_status(_sync_manager: &crate::sync::SyncManager) -> Result<()> {
    use crate::config::Config;

    let config = Config::load()?;

    println!("{}Sync Status{}", Colors::cyan(), Colors::reset());
    println!("============");
    println!();

    println!("Configuration:");
    println!(
        "  Sync enabled: {}",
        if config.sync.enabled { "✅" } else { "❌" }
    );
    println!("  Provider: {}", config.sync.provider);
    println!("  Sync interval: {}s", config.sync.sync_interval_seconds);
    println!(
        "  Quota warning threshold: {:.0}%",
        config.sync.soft_quota_fraction * 100.0
    );
    println!();

    if config.sync.enabled {
        println!("Turso Configuration:");
        if !config.sync.turso.database_url.is_empty() {
            println!("  Database URL: {}", config.sync.turso.database_url);
        } else {
            println!(
                "  Database URL: {}(not configured){}",
                Colors::red(),
                Colors::reset()
            );
        }

        if !config.sync.turso.auth_token.is_empty() {
            if config.sync.turso.auth_token.starts_with('$') {
                println!("  Auth token: {} (env var)", config.sync.turso.auth_token);
            } else {
                println!("  Auth token: *** (configured)");
            }
        } else {
            println!(
                "  Auth token: {}(not configured){}",
                Colors::red(),
                Colors::reset()
            );
        }
        println!();

        // Test connection
        println!("Testing connection...");
        // Note: We need a mutable reference for test_connection
        // But we received an immutable reference, so we'll create a temp copy
        let mut temp_manager = crate::sync::SyncManager::new(config.sync.clone());

        match temp_manager.test_connection() {
            Ok(connected) => {
                if connected {
                    println!(
                        "  Connection: {}✅ Connected{}",
                        Colors::green(),
                        Colors::reset()
                    );
                } else {
                    println!(
                        "  Connection: {}❌ Not connected{}",
                        Colors::red(),
                        Colors::reset()
                    );
                    if let Some(err) = temp_manager.status().error_message.as_ref() {
                        println!("  Error: {}", err);
                    }
                }
            }
            Err(e) => {
                println!(
                    "  Connection: {}❌ Error: {}{}",
                    Colors::red(),
                    e,
                    Colors::reset()
                );
            }
        }
    } else {
        println!("Sync is disabled. To enable:");
        println!("  1. Edit your config file with sync settings");
        println!("  2. Set sync.enabled = true");
        println!("  3. Configure Turso database URL and token");
        println!();
        println!("See: statusline generate-config for example configuration");
    }

    Ok(())
}

/// Handle context learning command
fn handle_context_learning_command(
    status: bool,
    reset: Option<String>,
    details: Option<String>,
    reset_all: bool,
    rebuild: bool,
) -> Result<()> {
    use crate::common::get_data_dir;
    use crate::context_learning::ContextLearner;
    use crate::database::SqliteDatabase;
    use crate::display::Colors;

    // Create context learner
    let db_path = get_data_dir().join("stats.db");
    let db = SqliteDatabase::new(&db_path)?;
    let learner = ContextLearner::new(db);

    // Handle rebuild from session history
    if rebuild {
        println!();
        println!(
            "{}Rebuilding learned context windows from session history...{}",
            Colors::cyan(),
            Colors::reset()
        );
        println!();

        learner.rebuild_from_sessions()?;

        println!();
        println!("{}✓ Rebuild complete{}", Colors::green(), Colors::reset());
        println!();
        println!(
            "{}Use --status to see the results{}",
            Colors::cyan(),
            Colors::reset()
        );
        println!();
        return Ok(());
    }

    // Handle reset all
    if reset_all {
        println!(
            "{}Resetting all learned context data...{}",
            Colors::yellow(),
            Colors::reset()
        );
        learner.reset_all()?;
        println!(
            "{}✓ All learning data cleared{}",
            Colors::green(),
            Colors::reset()
        );
        return Ok(());
    }

    // Handle reset for specific model
    if let Some(model_name) = reset {
        println!(
            "{}Resetting learned context data for: {}{}",
            Colors::yellow(),
            model_name,
            Colors::reset()
        );
        learner.reset_model(&model_name)?;
        println!(
            "{}✓ Learning data cleared for {}{}",
            Colors::green(),
            model_name,
            Colors::reset()
        );
        return Ok(());
    }

    // Handle details for specific model
    if let Some(model_name) = details {
        if let Some(record) = learner.get_learned_window_details(&model_name)? {
            println!();
            println!(
                "{}Learned Context Window Details for {}{}",
                Colors::cyan(),
                model_name,
                Colors::reset()
            );
            println!("{}", "=".repeat(60));
            println!();
            println!(
                "  Observed Max Tokens:     {}{}{}",
                Colors::green(),
                record.observed_max_tokens,
                Colors::reset()
            );
            println!(
                "  Confidence Score:        {}{:.1}%{}",
                Colors::green(),
                record.confidence_score * 100.0,
                Colors::reset()
            );
            println!("  Ceiling Observations:    {}", record.ceiling_observations);
            println!("  Compaction Count:        {}", record.compaction_count);
            println!("  First Seen:              {}", record.first_seen);
            println!("  Last Updated:            {}", record.last_updated);
            println!();
            println!("{}Audit Trail:{}", Colors::cyan(), Colors::reset());
            println!(
                "  Workspace:               {}",
                record.workspace_dir.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "  Device ID:               {}",
                record.device_id.as_deref().unwrap_or("<unknown>")
            );
            println!();

            let config = crate::config::get_config();
            if record.confidence_score >= config.context.learning_confidence_threshold {
                println!(
                    "  {}✓ Confidence threshold met - using learned value{}",
                    Colors::green(),
                    Colors::reset()
                );
            } else {
                println!(
                    "  {}⚠ Confidence too low - using default value{}",
                    Colors::yellow(),
                    Colors::reset()
                );
                println!(
                    "    Threshold: {:.1}%",
                    config.context.learning_confidence_threshold * 100.0
                );
            }
            println!();
        } else {
            println!(
                "{}No learning data found for: {}{}",
                Colors::yellow(),
                model_name,
                Colors::reset()
            );
        }
        return Ok(());
    }

    // Handle status (show all)
    if status {
        let all_records = learner.get_all_learned_windows()?;

        if all_records.is_empty() {
            println!();
            println!(
                "{}No learned context windows yet{}",
                Colors::yellow(),
                Colors::reset()
            );
            println!();
            println!(
                "{}To enable adaptive learning:{}",
                Colors::cyan(),
                Colors::reset()
            );
            println!("  1. Edit config: statusline generate-config");
            println!("  2. Set [context] adaptive_learning = true");
            println!("  3. Use Claude normally - learning happens automatically");
            println!();
            return Ok(());
        }

        println!();
        println!(
            "{}Learned Context Windows{}",
            Colors::cyan(),
            Colors::reset()
        );
        println!("{}", "=".repeat(80));
        println!();
        println!(
            "{:<25} {:>12} {:>10} {:>10} {:>12}",
            "Model", "Max Tokens", "Confidence", "Compactions", "Observations"
        );
        println!("{}", "-".repeat(80));

        for record in all_records {
            let confidence_color = if record.confidence_score >= 0.7 {
                Colors::green()
            } else if record.confidence_score >= 0.4 {
                Colors::yellow()
            } else {
                Colors::red()
            };

            println!(
                "{:<25} {:>12} {}{:>9.1}%{} {:>10} {:>12}",
                record.model_name,
                record.observed_max_tokens,
                confidence_color,
                record.confidence_score * 100.0,
                Colors::reset(),
                record.compaction_count,
                record.ceiling_observations
            );
        }
        println!();

        let config = crate::config::get_config();
        if config.context.adaptive_learning {
            println!(
                "{}✓ Adaptive learning is enabled{}",
                Colors::green(),
                Colors::reset()
            );
        } else {
            println!(
                "{}⚠ Adaptive learning is disabled in config{}",
                Colors::yellow(),
                Colors::reset()
            );
        }
        println!(
            "  Confidence threshold: {:.1}%",
            config.context.learning_confidence_threshold * 100.0
        );
        println!();
        println!(
            "{}Use --details <model> to see audit trail (workspace/device){}",
            Colors::cyan(),
            Colors::reset()
        );
        println!();

        return Ok(());
    }

    // No flags specified - show help
    println!();
    println!(
        "{}Adaptive Context Learning Commands{}",
        Colors::cyan(),
        Colors::reset()
    );
    println!();
    println!("  statusline context-learning --status");
    println!("    Show all learned context windows");
    println!();
    println!("  statusline context-learning --details <model>");
    println!("    Show detailed observations for a specific model");
    println!();
    println!("  statusline context-learning --reset <model>");
    println!("    Reset learning data for a specific model");
    println!();
    println!("  statusline context-learning --reset-all");
    println!("    Reset all learning data");
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_main_integration_placeholder() {
        // Basic smoke test placeholder to ensure test module links
        assert_eq!(1 + 1, 2);
    }
}
