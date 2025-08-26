use std::process::Command;
use std::path::{Path, PathBuf};
use std::fs;
use crate::error::{StatuslineError, Result};
use crate::retry::retry_simple;

#[derive(Debug)]
pub struct GitStatus {
    pub branch: String,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub untracked: usize,
}

impl Default for GitStatus {
    fn default() -> Self {
        GitStatus {
            branch: String::new(),
            added: 0,
            modified: 0,
            deleted: 0,
            untracked: 0,
        }
    }
}

/// Validates a directory path to prevent security vulnerabilities
fn validate_directory_path(dir: &str) -> Result<PathBuf> {
    // Reject paths with null bytes (command injection)
    if dir.contains('\0') {
        return Err(StatuslineError::invalid_path("Path contains null bytes"));
    }

    // Convert to PathBuf and canonicalize to resolve symlinks and relative paths
    let path = Path::new(dir);

    // Get the canonical path, which resolves all symlinks and relative components
    let canonical_path = fs::canonicalize(path)
        .map_err(|_| StatuslineError::invalid_path(format!("Cannot canonicalize path: {}", dir)))?;

    // Ensure the path is a directory
    if !canonical_path.is_dir() {
        return Err(StatuslineError::invalid_path(format!("Path is not a directory: {}", dir)));
    }

    // Check if it's a git repository by looking for .git directory
    if !canonical_path.join(".git").exists() {
        return Err(StatuslineError::git("Not a git repository"));
    }

    Ok(canonical_path)
}

pub fn get_git_status(dir: &str) -> Option<GitStatus> {
    // Validate and canonicalize the directory path
    let safe_dir = validate_directory_path(dir).ok()?;

    // Run git status command with validated path and retry on failure
    // Git operations can fail due to lock files or temporary issues
    let output = retry_simple(2, 100, || {
        Command::new("git")
            .arg("status")
            .arg("--porcelain=v1")
            .arg("--branch")
            .current_dir(&safe_dir)
            .output()
            .map_err(|e| StatuslineError::git(format!("Git command failed: {}", e)))
    }).ok()?;

    if !output.status.success() {
        return None;
    }

    let status_text = String::from_utf8_lossy(&output.stdout);
    parse_git_status(&status_text)
}

fn parse_git_status(status_text: &str) -> Option<GitStatus> {
    let mut status = GitStatus::default();

    for line in status_text.lines() {
        if line.starts_with("## ") {
            // Extract branch name
            let branch_info = &line[3..];
            if let Some(branch_end) = branch_info.find("...") {
                status.branch = branch_info[..branch_end].to_string();
            } else {
                status.branch = branch_info.to_string();
            }
        } else if line.len() > 2 {
            // Parse file status
            let status_code = &line[..2];
            match status_code {
                "A " | "AM" | "AD" => status.added += 1,
                "M " | "MM" | "MD" => status.modified += 1,
                "D " | "DM" => status.deleted += 1,
                "??" => status.untracked += 1,
                _ => {}
            }
        }
    }

    Some(status)
}

pub fn format_git_info(git_status: &GitStatus) -> String {
    let mut parts = Vec::new();

    // Add branch name
    if !git_status.branch.is_empty() {
        parts.push(format!("\x1b[32m{}\x1b[0m", git_status.branch));
    }

    // Add file status counts
    if git_status.added > 0 {
        parts.push(format!("\x1b[32m+{}\x1b[0m", git_status.added));
    }
    if git_status.modified > 0 {
        parts.push(format!("\x1b[33m~{}\x1b[0m", git_status.modified));
    }
    if git_status.deleted > 0 {
        parts.push(format!("\x1b[31m-{}\x1b[0m", git_status.deleted));
    }
    if git_status.untracked > 0 {
        parts.push(format!("\x1b[90m?{}\x1b[0m", git_status.untracked));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_directory_path_security() {
        // Test null byte injection
        assert!(validate_directory_path("/tmp\0/evil").is_err());
        assert!(validate_directory_path("/tmp\0").is_err());

        // Test non-existent paths
        assert!(validate_directory_path("/definitely/does/not/exist").is_err());

        // Test file instead of directory
        let temp_file = std::env::temp_dir().join("test_file.txt");
        std::fs::write(&temp_file, "test").ok();
        assert!(validate_directory_path(temp_file.to_str().unwrap()).is_err());
        std::fs::remove_file(temp_file).ok();

        // Test non-git directory (temp dir usually isn't a git repo)
        let temp_dir = std::env::temp_dir();
        assert!(validate_directory_path(temp_dir.to_str().unwrap()).is_err());
    }

    #[test]
    fn test_malicious_path_inputs() {
        // Directory traversal attempts
        assert!(get_git_status("../../../etc").is_none());
        assert!(get_git_status("../../../../../../").is_none());
        assert!(get_git_status("/etc/passwd").is_none());

        // Command injection attempts
        assert!(get_git_status("/tmp; rm -rf /").is_none());
        assert!(get_git_status("/tmp && echo hacked").is_none());
        assert!(get_git_status("/tmp | cat /etc/passwd").is_none());
        assert!(get_git_status("/tmp`whoami`").is_none());
        assert!(get_git_status("/tmp$(whoami)").is_none());

        // Null byte injection
        assert!(get_git_status("/tmp\0/evil").is_none());

        // Special characters that might cause issues
        assert!(get_git_status("/tmp\n/newline").is_none());
        assert!(get_git_status("/tmp\r/return").is_none());
    }

    #[test]
    fn test_parse_git_status_clean() {
        let status = GitStatus {
            branch: "main".to_string(),
            added: 0,
            modified: 0,
            deleted: 0,
            untracked: 0,
        };
        assert_eq!(status.added, 0);
        assert_eq!(status.modified, 0);
        assert_eq!(status.deleted, 0);
        assert_eq!(status.untracked, 0);
    }

    #[test]
    fn test_parse_git_status_with_changes() {
        let status = GitStatus {
            branch: "feature".to_string(),
            added: 5,
            modified: 3,
            deleted: 2,
            untracked: 1,
        };
        assert_eq!(status.added, 5);
        assert_eq!(status.modified, 3);
        assert_eq!(status.deleted, 2);
        assert_eq!(status.untracked, 1);
    }

    #[test]
    fn test_format_git_info() {
        let status = GitStatus {
            branch: "main".to_string(),
            added: 2,
            modified: 1,
            deleted: 0,
            untracked: 3,
        };
        let formatted = format_git_info(&status);
        assert!(formatted.contains("main"));
        assert!(formatted.contains("+2"));
        assert!(formatted.contains("~1"));
        assert!(formatted.contains("?3"));
    }
}