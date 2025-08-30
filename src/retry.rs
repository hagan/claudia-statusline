//! Retry logic module.
//!
//! This module provides retry functionality with exponential backoff
//! for handling transient failures in various operations.

use std::thread;
use std::time::Duration;
use log::debug;
use crate::error::{StatuslineError, Result};
use crate::config;

/// Configuration for retry behavior with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial delay between retries in milliseconds
    pub initial_delay_ms: u64,
    /// Maximum delay between retries in milliseconds
    pub max_delay_ms: u64,
    /// Factor to multiply delay by after each attempt (for exponential backoff)
    pub backoff_factor: f32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_factor: 2.0,
        }
    }
}

impl From<&config::RetrySettings> for RetryConfig {
    fn from(settings: &config::RetrySettings) -> Self {
        RetryConfig {
            max_attempts: settings.max_attempts,
            initial_delay_ms: settings.initial_delay_ms,
            max_delay_ms: settings.max_delay_ms,
            backoff_factor: settings.backoff_factor,
        }
    }
}

impl From<config::RetrySettings> for RetryConfig {
    fn from(settings: config::RetrySettings) -> Self {
        RetryConfig {
            max_attempts: settings.max_attempts,
            initial_delay_ms: settings.initial_delay_ms,
            max_delay_ms: settings.max_delay_ms,
            backoff_factor: settings.backoff_factor,
        }
    }
}

impl RetryConfig {
    /// Quick configuration for file operations (from config)
    pub fn for_file_ops() -> Self {
        let app_config = config::get_config();
        Self::from(&app_config.retry.file_ops)
    }

    /// Quick configuration for database operations (from config)
    pub fn for_db_ops() -> Self {
        let app_config = config::get_config();
        Self::from(&app_config.retry.db_ops)
    }

    /// Quick configuration for git operations (from config)
    #[allow(dead_code)]
    pub fn for_git_ops() -> Self {
        let app_config = config::get_config();
        Self::from(&app_config.retry.git_ops)
    }

    /// Quick configuration for network operations (from config)
    #[allow(dead_code)]
    pub fn for_network_ops() -> Self {
        let app_config = config::get_config();
        Self::from(&app_config.retry.network_ops)
    }
}

/// Retry a fallible operation with exponential backoff
pub fn retry_with_backoff<F, T>(config: &RetryConfig, mut operation: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut current_delay = config.initial_delay_ms;
    let mut last_error = None;

    for attempt in 1..=config.max_attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(e) => {
                last_error = Some(e);

                // Don't sleep after the last attempt
                if attempt < config.max_attempts {
                    // Log the retry attempt
                    debug!(
                        "Attempt {}/{} failed, retrying in {}ms...",
                        attempt, config.max_attempts, current_delay
                    );

                    thread::sleep(Duration::from_millis(current_delay));

                    // Calculate next delay with exponential backoff
                    current_delay = ((current_delay as f32 * config.backoff_factor) as u64)
                        .min(config.max_delay_ms);
                }
            }
        }
    }

    // All attempts failed, return the last error
    Err(last_error.unwrap_or_else(|| {
        StatuslineError::other("Retry failed with no error information")
    }))
}

/// Retry a fallible operation with simple fixed delay
pub fn retry_simple<F, T>(max_attempts: u32, delay_ms: u64, operation: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let config = RetryConfig {
        max_attempts,
        initial_delay_ms: delay_ms,
        max_delay_ms: delay_ms,
        backoff_factor: 1.0, // No backoff, fixed delay
    };

    retry_with_backoff(&config, operation)
}

/// Check if an error is retryable
pub fn is_retryable_error(error: &StatuslineError) -> bool {
    match error {
        // I/O errors are often transient
        StatuslineError::Io(_) => true,
        // Database busy errors are retryable
        StatuslineError::Database(e) => {
            let error_string = e.to_string().to_lowercase();
            error_string.contains("busy") ||
            error_string.contains("locked") ||
            error_string.contains("timeout")
        },
        // Lock failures are retryable
        StatuslineError::LockFailed(_) => true,
        // Git operations might be retryable if repository is busy
        StatuslineError::GitOperation(msg) => {
            let msg_lower = msg.to_lowercase();
            msg_lower.contains("lock") ||
            msg_lower.contains("busy") ||
            msg_lower.contains("timeout")
        },
        // Other errors are generally not retryable
        _ => false,
    }
}

/// Retry only if the error is retryable
pub fn retry_if_retryable<F, T>(config: &RetryConfig, mut operation: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut current_delay = config.initial_delay_ms;
    let mut last_error = None;

    for attempt in 1..=config.max_attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(e) => {
                // Check if the error is retryable
                if !is_retryable_error(&e) {
                    // Not retryable, return immediately
                    return Err(e);
                }

                last_error = Some(e);

                // Don't sleep after the last attempt
                if attempt < config.max_attempts {
                    debug!(
                        "Retryable error on attempt {}/{}, retrying in {}ms...",
                        attempt, config.max_attempts, current_delay
                    );

                    thread::sleep(Duration::from_millis(current_delay));

                    // Calculate next delay with exponential backoff
                    current_delay = ((current_delay as f32 * config.backoff_factor) as u64)
                        .min(config.max_delay_ms);
                }
            }
        }
    }

    // All attempts failed, return the last error
    Err(last_error.unwrap_or_else(|| {
        StatuslineError::other("Retry failed with no error information")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_retry_success_on_third_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let config = RetryConfig {
            max_attempts: 5,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            backoff_factor: 2.0,
        };

        let result = retry_with_backoff(&config, || {
            let count = attempts_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(StatuslineError::other("Temporary failure"))
            } else {
                Ok(42)
            }
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_retry_all_attempts_fail() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            backoff_factor: 2.0,
        };

        let result = retry_with_backoff(&config, || -> Result<i32> {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err(StatuslineError::other("Permanent failure"))
        });

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_is_retryable_error() {
        use std::io;

        // I/O errors are retryable
        let io_error = StatuslineError::Io(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
        assert!(is_retryable_error(&io_error));

        // Lock failures are retryable
        let lock_error = StatuslineError::LockFailed("couldn't acquire lock".to_string());
        assert!(is_retryable_error(&lock_error));

        // Invalid path errors are not retryable
        let path_error = StatuslineError::InvalidPath("bad path".to_string());
        assert!(!is_retryable_error(&path_error));
    }

    #[test]
    fn test_retry_if_retryable_non_retryable_error() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let config = RetryConfig::default();

        let result = retry_if_retryable(&config, || -> Result<i32> {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            // Return a non-retryable error
            Err(StatuslineError::InvalidPath("not a valid path".to_string()))
        });

        assert!(result.is_err());
        // Should only try once since the error is not retryable
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_exponential_backoff_timing() {
        let start = std::time::Instant::now();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 50,
            max_delay_ms: 200,
            backoff_factor: 2.0,
        };

        let _result = retry_with_backoff(&config, || -> Result<i32> {
            Err(StatuslineError::other("Always fails"))
        });

        let elapsed = start.elapsed().as_millis();
        // Should have delays of 50ms and 100ms (total 150ms minimum)
        assert!(elapsed >= 150);
        // But shouldn't exceed 50 + 100 + some overhead (say 250ms max)
        assert!(elapsed < 250);
    }
}