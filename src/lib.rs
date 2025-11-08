//! # Claudia Statusline Library
//!
//! A high-performance statusline library for Claude Code with persistent stats tracking.
//!
//! ## Features
//!
//! - **Git Integration**: Automatically detects and displays git repository status
//! - **Stats Tracking**: Persistent tracking of costs and usage across sessions
//! - **Configuration**: TOML-based configuration system with sensible defaults
//! - **Error Handling**: Unified error handling with automatic retries for transient failures
//! - **Database Support**: Dual-write to JSON and SQLite for reliability
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use statusline::models::StatuslineInput;
//!
//! // Parse input from JSON
//! let input: StatuslineInput = serde_json::from_str(r#"
//!     {
//!         "workspace": {"current_dir": "/home/user/project"},
//!         "model": {"display_name": "Claude 3.5 Sonnet"}
//!     }
//! "#).unwrap();
//!
//! // The statusline processes this input and generates formatted output
//! // See the display module for formatting functions
//! ```

// TODO: Re-enable html_root_url once the crate is published on docs.rs
// #![doc(html_root_url = "https://docs.rs/statusline/2.7.0")]

pub mod common;
/// Configuration management module for loading and saving settings
pub mod config;
/// Adaptive context window learning from usage patterns
pub mod context_learning;
/// SQLite database backend for persistent statistics
pub mod database;
pub mod display;
pub mod error;
pub mod git;
pub mod git_utils;
/// Database schema migration system
pub mod migrations;
pub mod models;
/// Retry logic with exponential backoff for transient failures
pub mod retry;
pub mod stats;
/// Cloud synchronization module (requires turso-sync feature)
#[cfg(feature = "turso-sync")]
pub mod sync;
/// Theme system for customizable statusline colors
pub mod theme;
pub mod utils;
pub mod version;

pub use config::Config;
pub use display::{format_output, format_output_to_string};
pub use error::{Result, StatuslineError};
pub use git::get_git_status;
pub use models::{Cost, Model, StatuslineInput, Workspace};
pub use stats::{get_daily_total, get_or_load_stats_data, update_stats_data, StatsData};
pub use theme::{get_theme_manager, Theme, ThemeManager};
pub use version::{short_version, version_string};

// ============================================================================
// Embedding API
// ============================================================================

/// Render a statusline from structured input data.
///
/// This is the primary API for embedding the statusline in other tools.
/// It handles all the formatting, git detection, and stats tracking internally.
///
/// # Arguments
///
/// * `input` - The statusline input containing workspace and model information
/// * `update_stats` - Whether to update persistent statistics (set to false for preview)
///
/// # Returns
///
/// A formatted statusline string ready for display.
///
/// # Example
///
/// ```rust,no_run
/// use statusline::{render_statusline, StatuslineInput};
/// use statusline::models::{Workspace, Model};
///
/// let input = StatuslineInput {
///     workspace: Some(Workspace {
///         current_dir: Some("/home/user/project".to_string()),
///     }),
///     model: Some(Model {
///         display_name: Some("Claude 3.5 Sonnet".to_string()),
///     }),
///     ..Default::default()
/// };
///
/// let output = render_statusline(&input, false).unwrap();
/// println!("{}", output);
/// ```
pub fn render_statusline(input: &StatuslineInput, update_stats: bool) -> Result<String> {
    // Get workspace directory
    let current_dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_deref())
        .unwrap_or("~");

    // Get model name
    let model_name = input.model.as_ref().and_then(|m| m.display_name.as_deref());

    // Get transcript path
    let transcript_path = input.transcript.as_deref();

    // Get cost data
    let cost = input.cost.as_ref();

    // Get session ID
    let session_id = input.session_id.as_deref();

    // Load or update stats
    let daily_total = if update_stats && session_id.is_some() {
        // Update stats with new data
        if let Some(ref cost) = input.cost {
            if let Some(total_cost) = cost.total_cost_usd {
                // Extract model name and workspace directory
                let model_name = input.model.as_ref().and_then(|m| m.display_name.as_ref()).map(|s| s.as_str());
                let workspace_dir = input.workspace.as_ref().and_then(|w| w.current_dir.as_ref()).map(|s| s.as_str());

                // Extract token breakdown from transcript if available
                let token_breakdown = input.transcript.as_ref()
                    .and_then(|path| utils::get_token_breakdown_from_transcript(path));

                // Get device ID for audit trail
                let device_id = common::get_device_id();

                let (daily_total, _monthly_total) = stats::update_stats_data(|data| {
                    data.update_session(
                        session_id.unwrap(),
                        total_cost,
                        cost.total_lines_added.unwrap_or(0),
                        cost.total_lines_removed.unwrap_or(0),
                        model_name,
                        workspace_dir,
                        Some(&device_id),
                        token_breakdown.as_ref(),
                    )
                });
                daily_total
            } else {
                // Have session but no cost data - still load existing daily totals
                let data = stats::get_or_load_stats_data();
                stats::get_daily_total(&data)
            }
        } else {
            // No cost data - just get current daily total
            let data = stats::get_or_load_stats_data();
            stats::get_daily_total(&data)
        }
    } else {
        // Just get current daily total without updating
        let stats_data = stats::get_or_load_stats_data();
        stats::get_daily_total(&stats_data)
    };

    // Adaptive context learning: observe token usage if enabled
    if update_stats && transcript_path.is_some() && model_name.is_some() && session_id.is_some() {
        let config = config::get_config();
        if config.context.adaptive_learning {
            if let Some(current_tokens) = utils::get_token_count_from_transcript(transcript_path.unwrap()) {
                // Get previous token count from session stats
                let stats_data = stats::get_or_load_stats_data();
                let previous_tokens = stats_data
                    .sessions
                    .get(session_id.unwrap())
                    .and_then(|s| s.max_tokens_observed)
                    .map(|t| t as usize);

                // Create context learner and observe usage
                use crate::common::get_data_dir;
                use crate::context_learning::ContextLearner;
                use crate::database::SqliteDatabase;

                let db_path = get_data_dir().join("stats.db");
                if let Ok(db) = SqliteDatabase::new(&db_path) {
                    let learner = ContextLearner::new(db);
                    // Extract workspace_dir and device_id for audit trail
                    let workspace_dir = input.workspace.as_ref().and_then(|w| w.current_dir.as_deref());
                    let device_id = crate::common::get_device_id();
                    // Ignore errors from adaptive learning - it's experimental and shouldn't block statusline
                    let _ = learner.observe_usage(
                        model_name.unwrap(),
                        current_tokens as usize,
                        previous_tokens,
                        transcript_path,
                        workspace_dir,
                        Some(&device_id),
                    );

                    // Update session's max_tokens_observed (adaptive learning)
                    // This updates both in-memory stats and SQLite database
                    stats::update_stats_data(|data| {
                        data.update_max_tokens(session_id.unwrap(), current_tokens);
                        // Return unchanged totals (we're just updating token tracking)
                        use crate::common::{current_date, current_month};
                        let today = current_date();
                        let month = current_month();
                        let daily_total = data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
                        let monthly_total = data.monthly.get(&month).map(|m| m.total_cost).unwrap_or(0.0);
                        (daily_total, monthly_total)
                    });
                }
            }
        }
    }

    // Format the output to string
    let output = display::format_output_to_string(
        current_dir,
        model_name,
        transcript_path,
        cost,
        daily_total,
        session_id,
    );

    Ok(output)
}

/// Render a statusline from a JSON string.
///
/// This is a convenience function that parses JSON input and calls `render_statusline`.
///
/// # Arguments
///
/// * `json` - A JSON string containing the statusline input
/// * `update_stats` - Whether to update persistent statistics
///
/// # Returns
///
/// A formatted statusline string ready for display.
///
/// # Example
///
/// ```rust,no_run
/// use statusline::render_from_json;
///
/// let json = r#"{
///     "workspace": {"current_dir": "/home/user/project"},
///     "model": {"display_name": "Claude 3.5 Sonnet"}
/// }"#;
///
/// let output = render_from_json(json, false).unwrap();
/// println!("{}", output);
/// ```
pub fn render_from_json(json: &str, update_stats: bool) -> Result<String> {
    let input: StatuslineInput = serde_json::from_str(json)
        .map_err(|e| StatuslineError::other(format!("Failed to parse JSON: {}", e)))?;
    render_statusline(&input, update_stats)
}
