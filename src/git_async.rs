//! Async git operations module.
//!
//! This module provides asynchronous versions of git operations for better
//! performance with large repositories. It's only available when the `async`
//! feature is enabled.

#[cfg(feature = "async")]
use tokio::process::Command;
use std::path::{Path, PathBuf};
use crate::error::{StatuslineError, Result};
use crate::git::GitStatus;

/// Validates a directory path to prevent security vulnerabilities (async version)
async fn validate_directory_path_async(dir: &str) -> Result<PathBuf> {
    // Reject paths with null bytes (command injection)
    if dir.contains('\0') {
        return Err(StatuslineError::invalid_path("Path contains null bytes"));
    }

    // Convert to PathBuf and canonicalize to resolve symlinks and relative paths
    let path = Path::new(dir);
    
    // Use tokio's async fs operations
    let canonical_path = tokio::fs::canonicalize(path).await
        .map_err(|_| StatuslineError::invalid_path(format!("Cannot canonicalize path: {}", dir)))?;
    
    // Ensure the path is a directory
    let metadata = tokio::fs::metadata(&canonical_path).await
        .map_err(|_| StatuslineError::invalid_path("Cannot read path metadata"))?;
        
    if !metadata.is_dir() {
        return Err(StatuslineError::invalid_path(format!("Path is not a directory: {}", dir)));
    }
    
    // Check if it's a git repository by looking for .git directory
    let git_dir = canonical_path.join(".git");
    match tokio::fs::metadata(&git_dir).await {
        Ok(m) if m.is_dir() => Ok(canonical_path),
        _ => Err(StatuslineError::git("Not a git repository")),
    }
}

/// Gets the git status for the specified directory asynchronously.
///
/// This function uses async I/O to run git commands, which can be more efficient
/// for large repositories or when checking multiple repositories concurrently.
///
/// # Arguments
///
/// * `dir` - The directory path to check
///
/// # Returns
///
/// Returns `Some(GitStatus)` if the directory is a git repository,
/// or `None` if it's not a git repository or an error occurs.
///
/// # Example
///
/// ```rust,no_run
/// use statusline::git_async::get_git_status_async;
///
/// #[tokio::main]
/// async fn main() {
///     if let Some(status) = get_git_status_async("/path/to/repo").await {
///         println!("Branch: {}", status.branch);
///         println!("Modified files: {}", status.modified);
///     }
/// }
/// ```
#[cfg(feature = "async")]
pub async fn get_git_status_async(dir: &str) -> Option<GitStatus> {
    // Validate and canonicalize the directory path
    let safe_dir = validate_directory_path_async(dir).await.ok()?;
    
    // Run git status command asynchronously
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--branch")
        .current_dir(&safe_dir)
        .output()
        .await
        .ok()?;
    
    if !output.status.success() {
        return None;
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_git_status(&stdout)
}

/// Gets the current git branch name asynchronously.
///
/// # Arguments
///
/// * `dir` - The directory path to check
///
/// # Returns
///
/// Returns `Some(String)` with the branch name, or `None` if not a git repository.
#[cfg(feature = "async")]
pub async fn get_git_branch_async(dir: &str) -> Option<String> {
    let safe_dir = validate_directory_path_async(dir).await.ok()?;
    
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .current_dir(&safe_dir)
        .output()
        .await
        .ok()?;
    
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Checks if there are uncommitted changes asynchronously.
///
/// # Arguments
///
/// * `dir` - The directory path to check
///
/// # Returns
///
/// Returns `true` if there are uncommitted changes, `false` otherwise.
#[cfg(feature = "async")]
pub async fn has_uncommitted_changes_async(dir: &str) -> bool {
    let safe_dir = match validate_directory_path_async(dir).await {
        Ok(p) => p,
        Err(_) => return false,
    };
    
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(&safe_dir)
        .output()
        .await
        .ok();
    
    match output {
        Some(o) if o.status.success() => !o.stdout.is_empty(),
        _ => false,
    }
}

/// Parses git status output into a GitStatus struct.
///
/// This is the same parsing logic as the sync version, extracted for reuse.
fn parse_git_status(stdout: &str) -> Option<GitStatus> {
    let mut status = GitStatus::default();
    
    for line in stdout.lines() {
        if line.starts_with("## ") {
            // Extract branch name
            let branch_info = &line[3..];
            if let Some(branch) = branch_info.split("...").next() {
                status.branch = branch.to_string();
            } else {
                status.branch = branch_info.to_string();
            }
        } else if line.len() > 3 {
            // Parse file status
            let status_code = &line[0..2];
            match status_code {
                "A " | " A" | "AM" => status.added += 1,
                "M " | " M" | "MM" => status.modified += 1,
                "D " | " D" => status.deleted += 1,
                "??" => status.untracked += 1,
                _ => {}
            }
        }
    }
    
    // Return None if we didn't find any git info
    if status.branch.is_empty() {
        None
    } else {
        Some(status)
    }
}

#[cfg(all(test, feature = "async"))]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::process::Command as StdCommand;
    
    #[tokio::test]
    async fn test_get_git_status_async() {
        // Create a temporary git repository
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();
        
        // Initialize git repo
        StdCommand::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .expect("Failed to init git repo");
        
        // Configure git user for the test repo
        StdCommand::new("git")
            .args(&["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
            
        StdCommand::new("git")
            .args(&["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        
        // Create a test file
        std::fs::write(repo_path.join("test.txt"), "test content").unwrap();
        
        // Get status before adding
        let status = get_git_status_async(repo_path.to_str().unwrap()).await;
        assert!(status.is_some());
        let status = status.unwrap();
        assert_eq!(status.untracked, 1);
        // Branch might be "No commits yet on main" or similar for new repos
        assert!(status.branch.contains("main") || status.branch.contains("master") || status.branch.is_empty());
    }
    
    #[tokio::test]
    async fn test_get_git_branch_async() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();
        
        // Initialize git repo
        StdCommand::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .expect("Failed to init git repo");
        
        // Configure git user
        StdCommand::new("git")
            .args(&["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
            
        StdCommand::new("git")
            .args(&["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        
        // Create initial commit to establish branch
        std::fs::write(repo_path.join("README.md"), "test").unwrap();
        StdCommand::new("git")
            .args(&["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(&["commit", "-m", "Initial commit"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        
        // Get branch name
        let branch = get_git_branch_async(repo_path.to_str().unwrap()).await;
        assert!(branch.is_some());
        let branch_name = branch.unwrap();
        assert!(branch_name == "main" || branch_name == "master");
    }
    
    #[tokio::test]
    async fn test_non_git_directory_async() {
        let temp_dir = TempDir::new().unwrap();
        let status = get_git_status_async(temp_dir.path().to_str().unwrap()).await;
        assert!(status.is_none());
    }
}