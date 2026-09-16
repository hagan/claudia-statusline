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

mod agents;
mod ant;
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
enum SessionsAction {
    /// List sessions newest first with an index for `show` (default)
    List {
        /// Show every session instead of the newest 25
        #[arg(long)]
        all: bool,
        /// Only sessions with attribution data (main_requests > 0)
        #[arg(long)]
        attributed: bool,
        /// Number of sessions to show
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Only sessions whose workspace is this directory or below it
        #[arg(long, value_name = "PATH")]
        workspace: Option<String>,
        /// Shorthand for --workspace $PWD: only this project's sessions
        #[arg(long, conflicts_with = "workspace")]
        here: bool,
    },
    /// Print all parameters of one session: `#` from the list or a session id prefix
    Show {
        /// List index (1-based) or session id prefix
        selector: String,
    },
    /// Interactive: list, read a selection from the terminal, show it
    Pick {
        /// Show every session instead of the newest 25
        #[arg(long)]
        all: bool,
        /// Number of sessions to show
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Only sessions whose workspace is this directory or below it
        #[arg(long, value_name = "PATH")]
        workspace: Option<String>,
        /// Shorthand for --workspace $PWD: only this project's sessions
        #[arg(long, conflicts_with = "workspace")]
        here: bool,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Token attribution per Claude Code version (default) or per session:
    /// main requests, input traffic and output per request, subagent share.
    /// Compare rows across versions to spot a behaviour change after an update.
    Stats {
        /// Group by Claude Code version (default view)
        #[arg(long)]
        by_version: bool,

        /// List the most recent sessions instead
        #[arg(long)]
        sessions: bool,
    },

    /// Browse recorded sessions: list them newest first, pick one, print its
    /// parameters (model, workspace, tokens, agent transcripts, resume line).
    Sessions {
        #[command(subcommand)]
        action: Option<SessionsAction>,
    },

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

    /// Anthropic API enrichment (opt-in, out-of-band)
    Ant {
        #[command(subcommand)]
        action: AntAction,
    },
}

#[derive(Subcommand)]
pub(crate) enum AntAction {
    /// Fetch the Models API and cache context windows
    SyncModels {
        /// Run in quiet mode (suppress the summary; errors still print)
        #[arg(short, long)]
        quiet: bool,
        /// Skip the fetch if the cache is younger than this (e.g. 10m, 24h, 2d).
        /// Omit to always fetch (manual runs are never throttled — ANT-30/D-02).
        #[arg(long, value_name = "DUR")]
        max_age: Option<String>,
    },
    /// Fetch org usage & cost (Admin API) and cache the active account's slice
    SyncUsage {
        /// Run in quiet mode (suppress the summary; errors still print)
        #[arg(short, long)]
        quiet: bool,
        /// Override the active account (default: STATUSLINE_ANT_ACCOUNT)
        #[arg(long)]
        account: Option<String>,
        /// Skip the fetch if the active account's cache is younger than this
        /// (e.g. 10m, 24h, 2d). Omit to always fetch (ANT-30/D-02).
        #[arg(long, value_name = "DUR")]
        max_age: Option<String>,
    },
    /// Diagnose ant enrichment: PATH, config, cache freshness, credential source.
    Doctor {
        /// Output as JSON (mirrors `health --json`).
        #[arg(long)]
        json: bool,
        /// Actively run the active account's credential command + an API
        /// reachability check (the only credential/network site; opt-in).
        #[arg(long)]
        probe: bool,
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
    // Rust ignores SIGPIPE and turns a closed stdout into a panic inside
    // println!. Restore the Unix default so `statusline sessions | head`
    // ends silently (exit 141) like any other CLI. The statusline render path
    // is unaffected: Claude Code reads the whole line.
    #[cfg(unix)]
    // SAFETY: called before any thread exists; SIG_DFL is a valid disposition.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

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
            Commands::Stats {
                by_version,
                sessions,
            } => {
                return commands::stats::show_stats(by_version, sessions);
            }
            Commands::Sessions { action } => {
                let res = match action.unwrap_or(SessionsAction::List {
                    all: false,
                    attributed: false,
                    limit: 25,
                    workspace: None,
                    here: false,
                }) {
                    SessionsAction::List {
                        all,
                        attributed,
                        limit,
                        workspace,
                        here,
                    } => commands::sessions::workspace_filter(workspace, here)
                        .and_then(|ws| commands::sessions::list(all, attributed, limit, ws)),
                    SessionsAction::Show { selector } => commands::sessions::show(&selector),
                    SessionsAction::Pick {
                        all,
                        limit,
                        workspace,
                        here,
                    } => commands::sessions::workspace_filter(workspace, here)
                        .and_then(|ws| commands::sessions::pick(all, limit, ws)),
                };
                // Selection mistakes are user-facing: plain text, not Debug.
                if let Err(e) = res {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
                return Ok(());
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

            Commands::Ant { action } => {
                return commands::ant::handle_ant_command(action);
            }
        }
    }

    // Read JSON from stdin
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    mirror_input(&buffer);

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
    let (daily_total, agents) = render::update_stats_and_daily_total(&input, true);

    // Format and print output
    format_output(
        &current_dir,
        input.model.as_ref().and_then(|m| m.detection_name()),
        input.transcript.as_deref(),
        input.cost.as_ref(),
        daily_total,
        input.session_id.as_deref(),
        display::PayloadExtras {
            context_window: input.context_window.as_ref(),
            rate_limits: input.rate_limits.as_ref(),
            effort: input.effort.as_ref().and_then(|e| e.level.as_deref()),
            exceeds_200k: input.exceeds_200k_tokens,
            version: input.version.as_deref(),
            repo: input.workspace.as_ref().and_then(|w| w.repo.as_ref()),
            agents: agents.as_ref(),
            prompt_cache: input.prompt_cache.as_ref(),
        },
    );

    Ok(())
}

/// Keep a copy of the last raw stdin payload so a Claude Code update that adds,
/// renames or drops a field can be seen with `jq . <file>` instead of guessed.
///
/// Default location: `<data dir>/last-input.json` (owner-only). Set
/// `STATUSLINE_DEBUG_INPUT=<path>` to relocate it or `STATUSLINE_DEBUG_INPUT=off`
/// to disable. Best-effort: a failure here never affects the render.
fn mirror_input(payload: &str) {
    let target = match env::var("STATUSLINE_DEBUG_INPUT") {
        Ok(v) if v.eq_ignore_ascii_case("off") => return,
        Ok(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => common::get_data_dir().join("last-input.json"),
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(&target) {
        let _ = std::io::Write::write_all(&mut f, payload.as_bytes());
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_main_integration_placeholder() {
        // Basic smoke test placeholder to ensure test module links
        assert_eq!(1 + 1, 2);
    }
}
