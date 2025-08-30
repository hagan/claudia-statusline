//! Git command execution utilities.
//!
//! This module provides utilities for executing git commands
//! safely and consistently.

use crate::error::StatuslineError;
use crate::retry::retry_simple;
use std::path::Path;
use std::process::{Command, Output};

/// Executes a git command with the given arguments in a directory.
///
/// This function handles:
/// - Automatic retry on failure (for lock file issues)
/// - Consistent error handling
///
/// # Arguments
///
/// * `dir` - The directory to execute the command in
/// * `args` - The git command arguments
///
/// # Returns
///
/// Returns the command output if successful, or None if the command fails.
fn execute_git_command<P: AsRef<Path>>(dir: P, args: &[&str]) -> Option<Output> {
    retry_simple(2, 100, || {
        Command::new("git")
            .args(args)
            .current_dir(dir.as_ref())
            .output()
            .map_err(|e| StatuslineError::git(format!("Git command failed: {}", e)))
    })
    .ok()
}

/// Gets the git status in porcelain format.
///
/// This is the main function used by the statusline to get git information.
///
/// # Arguments
///
/// * `dir` - The directory to check
///
/// # Returns
///
/// Returns the porcelain status output if successful.
pub fn get_status_porcelain<P: AsRef<Path>>(dir: P) -> Option<String> {
    let output = execute_git_command(dir, &["status", "--porcelain=v1", "--branch"])?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}
