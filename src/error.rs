//! Error handling module for Claudia Statusline.
//!
//! This module provides a unified error type using the `thiserror` crate,
//! consolidating all error types from various operations into a single enum.

use std::io;
use thiserror::Error;

/// Unified error type for the Claudia Statusline application.
///
/// This enum represents all possible errors that can occur in the application,
/// providing automatic conversions from underlying error types.
#[derive(Error, Debug)]
pub enum StatuslineError {
    /// I/O operation errors
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON parsing errors
    #[error("JSON parsing error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// Database operation errors
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Git operation errors
    #[error("Git operation failed: {0}")]
    GitOperation(String),

    /// File validation errors
    #[error("Invalid file path: {0}")]
    InvalidPath(String),

    /// Stats file errors
    #[error("Stats file error: {0}")]
    #[allow(dead_code)]
    StatsFile(String),

    /// Lock acquisition errors
    #[error("Failed to acquire lock: {0}")]
    LockFailed(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    #[allow(dead_code)]
    Config(String),

    /// Sync operation errors
    #[cfg(feature = "turso-sync")]
    #[error("Sync error: {0}")]
    #[allow(dead_code)]
    Sync(String),

    /// Generic operation errors
    #[error("{0}")]
    Other(String),
}

/// Result type alias for Statusline operations
pub type Result<T> = std::result::Result<T, StatuslineError>;

// Helper implementations for common conversions
impl StatuslineError {
    /// Create a git operation error
    pub fn git(msg: impl Into<String>) -> Self {
        StatuslineError::GitOperation(msg.into())
    }

    /// Create an invalid path error
    pub fn invalid_path(msg: impl Into<String>) -> Self {
        StatuslineError::InvalidPath(msg.into())
    }

    /// Create a stats file error
    #[allow(dead_code)]
    pub fn stats(msg: impl Into<String>) -> Self {
        StatuslineError::StatsFile(msg.into())
    }

    /// Create a lock failure error
    pub fn lock(msg: impl Into<String>) -> Self {
        StatuslineError::LockFailed(msg.into())
    }

    /// Create a generic other error
    pub fn other(msg: impl Into<String>) -> Self {
        StatuslineError::Other(msg.into())
    }
}

// Allow conversion from string for convenience
impl From<String> for StatuslineError {
    fn from(s: String) -> Self {
        StatuslineError::Other(s)
    }
}

impl From<&str> for StatuslineError {
    fn from(s: &str) -> Self {
        StatuslineError::Other(s.to_string())
    }
}
