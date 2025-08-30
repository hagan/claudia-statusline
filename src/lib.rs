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

#![doc(html_root_url = "https://docs.rs/statusline/2.7.0")]

pub mod common;
/// Configuration management module for loading and saving settings
pub mod config;
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
pub mod utils;
pub mod version;

pub use config::Config;
pub use display::format_output;
pub use error::{Result, StatuslineError};
pub use git::get_git_status;
pub use models::StatuslineInput;
pub use stats::{get_or_load_stats_data, update_stats_data, StatsData};
pub use version::{short_version, version_string};
