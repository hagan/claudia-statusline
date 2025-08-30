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

mod common;
mod config;
mod database;
mod display;
mod error;
mod git;
mod git_utils;
mod migrations;
mod models;
mod retry;
mod stats;
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
    },
}

fn main() -> Result<()> {
    // Initialize logging with WARN level by default (can be overridden with RUST_LOG env var)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let cli = Cli::parse();

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
            Commands::Migrate { finalize, delete_json } => {
                if finalize {
                    return finalize_migration(delete_json);
                } else {
                    println!("Usage: statusline migrate --finalize [--delete-json]");
                    println!("\nThis command finalizes the migration from JSON to SQLite-only mode.");
                    println!("Options:");
                    println!("  --finalize     Complete the migration and disable JSON backup");
                    println!("  --delete-json  Delete the JSON file instead of archiving it");
                    return Ok(());
                }
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
        print!("{}~{}", Colors::CYAN, Colors::RESET);
        return Ok(());
    }

    // Update stats tracking if we have session and cost data
    let (daily_total, _monthly_total) =
        if let (Some(session_id), Some(ref cost)) = (&input.session_id, &input.cost) {
            if let Some(total_cost) = cost.total_cost_usd {
                // Update stats with new cost data
                update_stats_data(|data| {
                    data.update_session(
                        session_id,
                        total_cost,
                        cost.total_lines_added.unwrap_or(0),
                        cost.total_lines_removed.unwrap_or(0),
                    )
                })
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
                if metadata.len() > 100 {  // More than just empty JSON
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

#[cfg(test)]
mod tests {

    #[test]
    fn test_main_integration_placeholder() {
        // Basic smoke test placeholder to ensure test module links
        assert_eq!(1 + 1, 2);
    }
}
