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

mod commands;
mod common;
mod config;
mod context_learning;
mod database;
mod display;
mod error;
mod git;
#[allow(dead_code)]
mod git_provider;
mod git_utils;
#[allow(dead_code)]
mod gsd;
mod hook_handler;
mod layout;
mod migrations;
mod models;
#[allow(dead_code)]
mod provider;
mod render;
mod retry;
mod session_state;
mod stats;
#[cfg(feature = "turso-sync")]
mod sync;
mod theme;
mod utils;
mod version;

use display::{format_output, Colors};
use error::Result;
use models::StatuslineInput;
use version::version_string;

/// Claudia Statusline - A high-performance statusline for Claude Code
#[derive(Parser)]
#[command(name = "statusline")]
#[command(version = env!("CLAUDIA_VERSION"))]
#[command(about = "A high-performance statusline for Claude Code", long_about = None)]
#[command(
    after_help = "Input: Reads JSON from stdin\n\nExample:\n  echo '{\"workspace\":{\"current_dir\":\"/path\"}}' | statusline"
)]
pub(crate) struct Cli {
    /// Show detailed version information
    #[arg(long = "version-full")]
    version_full: bool,

    /// Disable colored output
    #[arg(long)]
    pub(crate) no_color: bool,

    /// Set color theme (light or dark)
    #[arg(long, value_name = "THEME")]
    theme: Option<String>,

    /// Path to configuration file
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Set log level
    #[arg(long, value_name = "LEVEL", value_parser = ["error", "warn", "info", "debug", "trace"])]
    log_level: Option<String>,

    /// Use test mode (isolated database, adds TEST indicator to output)
    #[arg(long)]
    test_mode: bool,

    /// List all available template variables and their current values
    #[arg(long)]
    list_vars: bool,

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

    /// Hook handlers for Claude Code events (called by hooks)
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand)]
pub(crate) enum HookAction {
    /// PreCompact hook - called when Claude starts compacting
    Precompact {
        /// Session ID from Claude (if not provided, reads from stdin JSON)
        #[arg(long)]
        session_id: Option<String>,

        /// Trigger type: "auto" or "manual" (if not provided, reads from stdin JSON)
        #[arg(long)]
        trigger: Option<String>,
    },

    /// Stop hook - called when Claude session ends
    Stop {
        /// Session ID from Claude (if not provided, reads from stdin JSON)
        #[arg(long)]
        session_id: Option<String>,
    },

    /// PostCompact hook - called after compaction completes (via SessionStart[compact])
    ///
    /// Configure in Claude Code settings with SessionStart hook and matcher "compact":
    /// ```json
    /// "SessionStart": [{"matcher": "compact", "hooks": [{"type": "command", "command": "statusline hook postcompact"}]}]
    /// ```
    Postcompact {
        /// Session ID from Claude (if not provided, reads from stdin JSON)
        #[arg(long)]
        session_id: Option<String>,
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

    // Handle test mode flag - uses isolated database
    if cli.test_mode {
        env::set_var("STATUSLINE_TEST_MODE", "1");
        // Override XDG_DATA_HOME to use test directory
        env::set_var(
            "XDG_DATA_HOME",
            format!(
                "{}/.local/share-test",
                env::var("HOME").unwrap_or_else(|_| String::from("/tmp"))
            ),
        );
    }

    // Handle version-full flag
    if cli.version_full {
        print!("{}", version_string());
        return Ok(());
    }

    // Handle --list-vars flag (runs providers, prints all variables grouped by source)
    if cli.list_vars {
        return commands::list_vars::handle_list_vars(&cli);
    }

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::GenerateConfig => {
                let config_path = config::Config::default_config_path()?;
                println!("Generating example config file at: {:?}", config_path);

                // Create parent directories with secure permissions (0o700 on Unix)
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
                        std::fs::create_dir_all(parent)?;
                    }
                }

                // Write example config with secure permissions (0o600 on Unix)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .mode(0o600)
                        .open(&config_path)?;
                    std::io::Write::write_all(
                        &mut file,
                        config::Config::example_toml().as_bytes(),
                    )?;
                }

                #[cfg(not(unix))]
                {
                    std::fs::write(&config_path, config::Config::example_toml())?;
                }
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
                    return commands::migrate::dump_database_schema();
                } else if run {
                    return commands::migrate::run_schema_migrations();
                } else if finalize {
                    return commands::migrate::finalize_migration(delete_json);
                } else {
                    return commands::migrate::show_migration_roadmap();
                }
            }
            Commands::DbMaintain {
                force_vacuum,
                no_prune,
                quiet,
            } => {
                return commands::maintenance::perform_database_maintenance(
                    force_vacuum,
                    no_prune,
                    quiet,
                );
            }
            Commands::Health { json } => {
                return commands::health::show_health_report(json);
            }

            #[cfg(feature = "turso-sync")]
            Commands::Sync {
                status,
                push,
                pull,
                dry_run,
            } => {
                return commands::sync::handle_sync_command(status, push, pull, dry_run);
            }

            Commands::ContextLearning {
                status,
                reset,
                details,
                reset_all,
                rebuild,
            } => {
                return commands::context_learning::handle_context_learning_command(
                    status, reset, details, reset_all, rebuild,
                );
            }

            Commands::Hook { action } => {
                return commands::hooks::handle_hook_command(action);
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

    // Update stats and resolve today's daily total via the single shared
    // implementation (see src/render.rs). The binary always updates stats.
    let daily_total = render::update_stats_and_daily_total(&input, true);

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
    fn test_main_integration_placeholder() {
        // Basic smoke test placeholder to ensure test module links
        assert_eq!(1 + 1, 2);
    }
}
