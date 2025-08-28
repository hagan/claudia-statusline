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
//! use statusline::{models::StatuslineInput, display::format_output};
//!
//! // Parse input from JSON
//! let input: StatuslineInput = serde_json::from_str(r#"
//!     {
//!         "workspace": {"current_dir": "/home/user/project"},
//!         "model": {"display_name": "Claude 3.5 Sonnet"}
//!     }
//! "#).unwrap();
//!
//! // Format and display the statusline
//! let output = format_output(&input);
//! println!("{}", output);
//! ```

#![warn(missing_docs)]
#![doc(html_root_url = "https://docs.rs/statusline/2.6.0")]

pub mod error;
pub mod models;
pub mod git;
#[cfg(feature = "async")]
pub mod git_async;
pub mod stats;
pub mod display;
pub mod utils;
pub mod version;
pub mod config;
pub mod retry;
pub mod database;
pub mod migrations;

pub use error::{StatuslineError, Result};
pub use models::StatuslineInput;
pub use display::format_output;
pub use stats::{StatsData, get_or_load_stats_data, update_stats_data};
pub use git::get_git_status;
pub use version::{version_string, short_version};
pub use config::Config;