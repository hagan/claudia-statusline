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
//! - **Database Support**: SQLite-backed persistence with JSON read fallback for legacy migration
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
/// GitProvider wraps git module as a DataProvider
pub mod git_provider;
pub mod git_utils;
/// GSD project tracking data provider
pub mod gsd;
/// Hook handlers for Claude Code PreCompact and Stop events
pub mod hook_handler;
/// Layout rendering module for customizable statusline format
pub mod layout;
/// Database schema migration system
pub mod migrations;
pub mod models;
/// Data provider system for parallel variable collection
pub mod provider;
/// Shared statusline rendering logic (stats-update flow used by the binary and the embedding API)
pub mod render;
/// Retry logic with exponential backoff for transient failures
pub mod retry;
/// Hook-based state management for real-time event tracking
pub mod state;
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
#[allow(unused_imports)]
pub use git_provider::GitProvider;
pub use gsd::GsdProvider;
pub use models::{Cost, Model, StatuslineInput, Workspace};
pub use stats::{
    get_daily_total, get_or_load_stats_data, update_stats_data, StatsData, StatsProvider,
};
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

    // Load or update stats (single shared implementation; see src/render.rs).
    let daily_total = render::update_stats_and_daily_total(input, update_stats);

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
