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

use std::env;
use std::io::{self, Read};
use clap::{Parser, Subcommand};

mod common;
mod error;
mod retry;
mod models;
mod git;
mod git_utils;
mod stats;
mod display;
mod utils;
mod version;
mod database;
mod migrations;
mod config;

use error::Result;
use models::StatuslineInput;
use stats::{get_or_load_stats_data, update_stats_data};
use display::{Colors, format_output};
use version::version_string;

/// Claudia Statusline - A high-performance statusline for Claude Code
#[derive(Parser)]
#[command(name = "statusline")]
#[command(version = env!("CLAUDIA_VERSION"))]
#[command(about = "A high-performance statusline for Claude Code", long_about = None)]
#[command(after_help = "Input: Reads JSON from stdin\n\nExample:\n  echo '{\"workspace\":{\"current_dir\":\"/path\"}}' | statusline")]
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
}

fn main() -> Result<()> {
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
            eprintln!("Warning: Failed to parse JSON input: {}. Using defaults.", e);
            StatuslineInput::default()
        }
    };

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
    let (daily_total, _monthly_total) = if let (Some(session_id), Some(ref cost)) = (&input.session_id, &input.cost) {
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
            let monthly_total = data.monthly.get(&month).map(|m| m.total_cost).unwrap_or(0.0);
            (daily_total, monthly_total)
        }
    } else {
        // No session_id - still load stats data to show accumulated totals
        let data = get_or_load_stats_data();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let month = chrono::Local::now().format("%Y-%m").to_string();

        let daily_total = data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
        let monthly_total = data.monthly.get(&month).map(|m| m.total_cost).unwrap_or(0.0);
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

#[cfg(test)]
mod tests {




    #[test]
    fn test_main_integration() {
        // This is a placeholder for integration tests
        // Most tests are now in their respective modules
        assert!(true);
    }
}