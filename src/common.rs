//! Common utilities shared across modules.
//!
//! This module provides shared functionality to reduce code duplication
//! and ensure consistent behavior across the application.

use crate::error::Result;
use chrono::Local;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Gets the application data directory using XDG Base Directory specification.
///
/// Returns `~/.local/share/claudia-statusline/` on Unix-like systems.
///
/// # Example
///
/// ```rust,no_run
/// use statusline::common::get_data_dir;
///
/// let data_dir = get_data_dir();
/// let stats_file = data_dir.join("stats.json");
/// ```
pub fn get_data_dir() -> PathBuf {
    // Use dirs crate for proper XDG handling
    let base_dir = dirs::data_dir().unwrap_or_else(|| {
        // Fallback if dirs crate fails
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".local").join("share")
    });

    base_dir.join("claudia-statusline")
}

/// Gets the current timestamp in ISO 8601 format.
///
/// # Example
///
/// ```rust
/// use statusline::common::current_timestamp;
///
/// let timestamp = current_timestamp();
/// assert!(timestamp.contains("T")); // ISO 8601 format
/// ```
pub fn current_timestamp() -> String {
    Local::now().to_rfc3339()
}

/// Gets the current date in YYYY-MM-DD format.
///
/// # Example
///
/// ```rust
/// use statusline::common::current_date;
///
/// let date = current_date();
/// assert_eq!(date.len(), 10); // YYYY-MM-DD
/// ```
pub fn current_date() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Gets the current month in YYYY-MM format.
///
/// # Example
///
/// ```rust
/// use statusline::common::current_month;
///
/// let month = current_month();
/// assert_eq!(month.len(), 7); // YYYY-MM
/// ```
pub fn current_month() -> String {
    Local::now().format("%Y-%m").to_string()
}

/// Validates a path for security issues.
///
/// Checks for:
/// - Null bytes (prevent injection attacks)
/// - Path traversal attempts
/// - Symbolic link resolution
///
/// # Arguments
///
/// * `path` - The path to validate
///
/// # Returns
///
/// Returns the canonical path if valid, or an error if validation fails.
pub fn validate_path_security(path: &str) -> Result<PathBuf> {
    use crate::error::StatuslineError;
    use std::fs;

    // Check for null bytes (command injection prevention)
    if path.contains('\0') {
        return Err(StatuslineError::invalid_path("Path contains null bytes"));
    }

    // Canonicalize to resolve symlinks and relative paths
    fs::canonicalize(path)
        .map_err(|_| StatuslineError::invalid_path(format!("Cannot canonicalize path: {}", path)))
}

/// Generates a stable device ID from hostname and username.
///
/// The device ID is a hash of the hostname and username, providing:
/// - Stability across reboots (same ID for same machine)
/// - Privacy (doesn't leak actual hostname/username)
/// - Uniqueness (unlikely collisions across different machines)
///
/// # Returns
///
/// A 16-character hexadecimal string representing the device ID.
///
/// # Example
///
/// ```rust
/// use statusline::common::get_device_id;
///
/// let device_id = get_device_id();
/// assert_eq!(device_id.len(), 16); // 64-bit hash in hex
/// ```
pub fn get_device_id() -> String {
    use std::env;

    // Get hostname (fallback to "unknown-host" if unavailable)
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string());

    // Get username (fallback to "unknown-user" if unavailable)
    let username = env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string());

    // Create a stable hash of hostname + username
    let mut hasher = DefaultHasher::new();
    hostname.hash(&mut hasher);
    username.hash(&mut hasher);
    let hash = hasher.finish();

    // Return as 16-character hex string
    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_get_data_dir() {
        let dir = get_data_dir();
        assert!(dir.to_string_lossy().contains("claudia-statusline"));
    }

    #[test]
    fn test_current_timestamp() {
        let ts = current_timestamp();
        assert!(ts.contains("T"));
        assert!(ts.contains(":"));
    }

    #[test]
    fn test_current_date() {
        let date = current_date();
        assert_eq!(date.len(), 10);
        assert!(date.contains("-"));
    }

    #[test]
    fn test_current_month() {
        let month = current_month();
        assert_eq!(month.len(), 7);
        assert!(month.contains("-"));
    }

    #[test]
    fn test_validate_path_security() {
        // Test null byte rejection
        assert!(validate_path_security("path\0injection").is_err());

        // Test valid path
        let temp_dir = TempDir::new().unwrap();
        let result = validate_path_security(temp_dir.path().to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_device_id() {
        let device_id = get_device_id();

        // Should be exactly 16 characters (64-bit hash in hex)
        assert_eq!(device_id.len(), 16);

        // Should only contain hex characters
        assert!(device_id.chars().all(|c| c.is_ascii_hexdigit()));

        // Should be stable (same ID on multiple calls)
        let device_id2 = get_device_id();
        assert_eq!(device_id, device_id2);
    }
}
