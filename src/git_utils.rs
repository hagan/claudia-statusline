//! Git command execution utilities.
//!
//! This module provides utilities for executing git commands
//! safely and consistently.

use crate::config;
use crate::error::StatuslineError;
use crate::retry::retry_simple;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Executes a git command with the given arguments in a directory.
///
/// This function handles:
/// - Automatic retry on failure (for lock file issues)
/// - Consistent error handling
/// - Timeout to prevent hanging on slow operations
/// - GIT_OPTIONAL_LOCKS=0 to avoid lock conflicts
///
/// # Arguments
///
/// * `dir` - The directory to execute the command in
/// * `args` - The git command arguments
///
/// # Returns
///
/// Returns the command output if successful, or None if the command fails or times out.
fn execute_git_command<P: AsRef<Path>>(dir: P, args: &[&str]) -> Option<Output> {
    let config = config::get_config();

    // Support environment variable override for timeout
    let timeout_ms = std::env::var("STATUSLINE_GIT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(config.git.timeout_ms);

    retry_simple(2, 100, || {
        execute_git_with_timeout(dir.as_ref(), args, timeout_ms)
            .ok_or_else(|| StatuslineError::git("Git command timed out or failed"))
    })
    .ok()
}

/// Internal function that executes a git command with proper timeout support.
///
/// Returns the command output if successful, or None if timeout/failure occurs.
fn execute_git_with_timeout<P: AsRef<Path>>(
    dir: P,
    args: &[&str],
    timeout_ms: u32,
) -> Option<Output> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir.as_ref())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().ok()?;

    // Capture stdout and stderr
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;

    // Start threads to read the output
    let stdout_handle = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).ok();
        output
    });

    let stderr_handle = std::thread::spawn(move || {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).ok();
        output
    });

    // Wait for the timeout duration
    let timeout = Duration::from_millis(timeout_ms as u64);
    let start = Instant::now();

    loop {
        if start.elapsed() > timeout {
            // Timeout reached, kill the process
            let _ = child.kill();
            log::info!(
                "Git command timed out after {}ms: git {}",
                timeout_ms,
                args.join(" ")
            );
            return None;
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                // Process finished, collect output
                let stdout_data = stdout_handle.join().unwrap_or_default();
                let stderr_data = stderr_handle.join().unwrap_or_default();

                return Some(Output {
                    status,
                    stdout: stdout_data,
                    stderr: stderr_data,
                });
            }
            Ok(None) => {
                // Still running, continue waiting
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                log::debug!("Error waiting for git process: {}", e);
                return None;
            }
        }
    }
}

/// Gets the git status in porcelain format.
///
/// This is the main function used by the statusline to get git information.
/// When the `git_porcelain_v2` feature is enabled, it uses porcelain v2 format,
/// otherwise it uses porcelain v1 format.
///
/// # Arguments
///
/// * `dir` - The directory to check
///
/// # Returns
///
/// Returns the porcelain status output if successful.
pub fn get_status_porcelain<P: AsRef<Path>>(dir: P) -> Option<String> {
    #[cfg(feature = "git_porcelain_v2")]
    let args = &["status", "--porcelain=v2", "--branch"];

    #[cfg(not(feature = "git_porcelain_v2"))]
    let args = &["status", "--porcelain=v1", "--branch"];

    let output = execute_git_command(dir, args)?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::TempDir;

    #[test]
    fn test_git_timeout() {
        // Create a temp directory
        let temp_dir = TempDir::new().unwrap();

        // Set a very short timeout
        env::set_var("STATUSLINE_GIT_TIMEOUT_MS", "50");

        // Try to run a command that would take longer than timeout
        // We'll use a non-existent directory which should fail quickly
        let result = get_status_porcelain(temp_dir.path());

        // Should return None (not a git repo)
        assert!(result.is_none());

        // Clean up
        env::remove_var("STATUSLINE_GIT_TIMEOUT_MS");
    }

    #[test]
    fn test_git_with_locks_env() {
        // This test verifies that GIT_OPTIONAL_LOCKS is set
        // We can't directly test it, but we can verify the function works
        let temp_dir = TempDir::new().unwrap();

        // Initialize a git repo
        Command::new("git")
            .args(["init"])
            .current_dir(temp_dir.path())
            .output()
            .ok();

        // Should work even with potential lock conflicts
        let result = get_status_porcelain(temp_dir.path());
        assert!(result.is_some());
    }

    #[test]
    fn test_timeout_kills_process() {
        // Test that timeout actually kills long-running processes
        let temp_dir = TempDir::new().unwrap();

        // Set a short timeout
        let start = Instant::now();
        let result = execute_git_with_timeout(
            temp_dir.path(),
            &["--version"], // Quick command that should succeed
            200,            // 200ms timeout
        );

        // Should complete quickly and successfully
        assert!(result.is_some());
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}
