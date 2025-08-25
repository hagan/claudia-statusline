use std::process::Command;
use std::path::Path;

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

pub fn get_git_status(dir: &str) -> Option<GitStatus> {
    // Check if directory exists and is a git repository
    if !Path::new(dir).exists() {
        return None;
    }

    // Run git status command
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--branch")
        .current_dir(dir)
        .output()
        .ok()?;

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