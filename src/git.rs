//! Git repository integration module.
//!
//! This module provides functionality to detect git repositories and retrieve
//! their status information, including branch name and file change counts.

use crate::common::validate_path_security;
use crate::display::Colors;
use crate::error::{Result, StatuslineError};
use crate::git_utils;
use crate::utils::sanitize_for_terminal;
use std::path::PathBuf;

/// Git repository status information.
///
/// Contains the current branch name and counts of different types of file changes.
#[derive(Debug, Default)]
pub struct GitStatus {
    pub branch: String,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub untracked: usize,
}

/// Validates that a path is a git repository directory
fn validate_git_directory(dir: &str) -> Result<PathBuf> {
    // Use common validation first
    let canonical_path = validate_path_security(dir)?;

    // Ensure the path is a directory
    if !canonical_path.is_dir() {
        return Err(StatuslineError::invalid_path(format!(
            "Path is not a directory: {}",
            dir
        )));
    }

    // Check if it's a git repository by looking for .git directory
    if !canonical_path.join(".git").exists() {
        return Err(StatuslineError::git("Not a git repository"));
    }

    Ok(canonical_path)
}

/// Gets the git status for the specified directory.
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
/// use statusline::git::get_git_status;
///
/// if let Some(status) = get_git_status("/path/to/repo") {
///     println!("Branch: {}", status.branch);
///     println!("Modified files: {}", status.modified);
/// }
/// ```
pub fn get_git_status(dir: &str) -> Option<GitStatus> {
    // Validate and canonicalize the directory path
    let safe_dir = validate_git_directory(dir).ok()?;

    // Get git status using the utility function
    let status_text = git_utils::get_status_porcelain(&safe_dir)?;

    #[cfg(feature = "git_porcelain_v2")]
    return parse_git_status_v2(&status_text);

    #[cfg(not(feature = "git_porcelain_v2"))]
    return parse_git_status(&status_text);
}

/// Parses git status output in porcelain v1 format.
///
/// This function implements comprehensive parsing of git's porcelain v1 status format,
/// handling all standard XY status codes including special cases like renames, type
/// changes, and unmerged states.
///
/// # Porcelain v1 Format Rules
///
/// ## Branch Line
/// - Lines starting with `## ` indicate branch information
/// - Format: `## <branch>...origin/<branch> [ahead N, behind M]`
/// - Special case: `## HEAD (no branch)` for detached HEAD state
///
/// ## Status Codes (XY format)
/// The first two characters of each line indicate the file status:
/// - **X** (index/staging area status)
/// - **Y** (working tree status)
///
/// ### Standard Status Mappings
/// - **Added**: `A` in X position (staged add)
/// - **Modified**: `M` in either X or Y position
/// - **Deleted**: `D` in either X or Y position  
/// - **Renamed**: `R` in X position (counts as modified)
/// - **Copied**: `C` in X position (counts as modified)
/// - **Type changed**: `T` in either position (counts as modified)
/// - **Untracked**: `??` (both positions are `?`)
/// - **Ignored**: `!!` (both positions are `!`, not counted)
///
/// ### Unmerged/Conflict States
/// All unmerged states are counted as modified:
/// - `DD` - Both deleted
/// - `AU` - Added by us
/// - `UD` - Deleted by them
/// - `UA` - Added by them
/// - `DU` - Deleted by us
/// - `AA` - Both added
/// - `UU` - Both modified
///
/// ### Combined States
/// Some combinations affect multiple counters:
/// - `AM` - Added to index, modified in working tree (counts as both added and modified)
/// - `AD` - Added to index, deleted in working tree (counts as both added and deleted)
/// - `MD` - Modified in index, deleted in working tree (counts as both modified and deleted)
///
/// # Arguments
///
/// * `status_text` - The output from `git status --porcelain=v1 --branch`
///
/// # Returns
///
/// Returns `Some(GitStatus)` with parsed information, or `None` if parsing fails.
fn parse_git_status(status_text: &str) -> Option<GitStatus> {
    let mut status = GitStatus::default();

    for line in status_text.lines() {
        if let Some(branch_info) = line.strip_prefix("## ") {
            // Extract branch name, handling various formats
            if branch_info.starts_with("HEAD (no branch)") {
                // Detached HEAD state - use the full string
                status.branch = branch_info.to_string();
            } else if let Some(branch_end) = branch_info.find("...") {
                // Branch with upstream tracking info
                status.branch = branch_info[..branch_end].to_string();
            } else {
                // Simple branch name without tracking
                status.branch = branch_info.to_string();
            }
        } else if line.len() >= 2 {
            // Parse file status codes
            let chars: Vec<char> = line.chars().collect();
            if chars.len() < 2 {
                continue;
            }

            let x = chars[0]; // Index/staging status
            let y = chars[1]; // Working tree status

            // Handle special two-character codes first
            match (x, y) {
                // Untracked files
                ('?', '?') => status.untracked += 1,
                // Ignored files (don't count)
                ('!', '!') => continue,
                // Unmerged states - all count as modified
                ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U') => status.modified += 1,
                // Regular status codes
                _ => {
                    // Check X (index) status
                    match x {
                        'A' => status.added += 1,    // Added to index
                        'M' => status.modified += 1, // Modified in index
                        'D' => status.deleted += 1,  // Deleted from index
                        'R' => status.modified += 1, // Renamed (counts as modified)
                        'C' => status.modified += 1, // Copied (counts as modified)
                        'T' => status.modified += 1, // Type changed (counts as modified)
                        'U' => status.modified += 1, // Unmerged (counts as modified)
                        _ => {}
                    }

                    // Check Y (working tree) status
                    match y {
                        'M' => status.modified += 1, // Modified in working tree
                        'D' => status.deleted += 1,  // Deleted from working tree
                        'T' => status.modified += 1, // Type changed in working tree
                        'U' => status.modified += 1, // Unmerged (counts as modified)
                        _ => {}
                    }
                }
            }
        }
    }

    Some(status)
}

/// Parses git status output in porcelain v2 format.
///
/// This function is only available when the `git_porcelain_v2` feature is enabled.
/// Porcelain v2 format provides machine-readable output with more structured information.
///
/// # Porcelain v2 Format
///
/// ## Header Lines
/// - `# branch.oid <commit>` - Current commit SHA
/// - `# branch.head <branch>` - Current branch name
/// - `# branch.upstream <upstream>` - Upstream branch
/// - `# branch.ab +<ahead> -<behind>` - Ahead/behind counts
///
/// ## File Status Lines
/// - `1 <xy> <sub> <mH> <mI> <mW> <hH> <hI> <path>` - Ordinary changed file
/// - `2 <xy> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path><sep><origPath>` - Renamed/copied
/// - `u <xy> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>` - Unmerged file
/// - `? <path>` - Untracked file
/// - `! <path>` - Ignored file
///
/// The XY status codes are the same as porcelain v1.
#[cfg(feature = "git_porcelain_v2")]
fn parse_git_status_v2(status_text: &str) -> Option<GitStatus> {
    let mut status = GitStatus::default();

    for line in status_text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            // Parse header lines
            if let Some(branch_name) = header.strip_prefix("branch.head ") {
                status.branch = branch_name.to_string();
            }
        } else if let Some(first_char) = line.chars().next() {
            match first_char {
                '1' => {
                    // Ordinary changed file: 1 <xy> ...
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let xy = parts[1];
                        if let Some((x, y)) = parse_xy_status(xy) {
                            apply_status_codes(&mut status, x, y);
                        }
                    }
                }
                '2' => {
                    // Renamed or copied file: 2 <xy> ...
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let xy = parts[1];
                        if let Some((x, y)) = parse_xy_status(xy) {
                            apply_status_codes(&mut status, x, y);
                        }
                    }
                }
                'u' => {
                    // Unmerged file: u <xy> ...
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let xy = parts[1];
                        if let Some((x, y)) = parse_xy_status(xy) {
                            apply_status_codes(&mut status, x, y);
                        }
                    }
                }
                '?' => {
                    // Untracked file
                    status.untracked += 1;
                }
                '!' => {
                    // Ignored file - don't count
                }
                _ => {}
            }
        }
    }

    Some(status)
}

/// Helper function to parse XY status string into two characters
#[cfg(feature = "git_porcelain_v2")]
fn parse_xy_status(xy: &str) -> Option<(char, char)> {
    let chars: Vec<char> = xy.chars().collect();
    if chars.len() >= 2 {
        Some((chars[0], chars[1]))
    } else {
        None
    }
}

/// Helper function to apply status codes to the GitStatus struct
#[cfg(feature = "git_porcelain_v2")]
fn apply_status_codes(status: &mut GitStatus, x: char, y: char) {
    // Handle special two-character codes first
    match (x, y) {
        // Untracked files (shouldn't happen here, handled separately)
        ('?', '?') => status.untracked += 1,
        // Ignored files (don't count)
        ('!', '!') => {}
        // Unmerged states - all count as modified
        ('D', 'D')
        | ('A', 'U')
        | ('U', 'D')
        | ('U', 'A')
        | ('D', 'U')
        | ('A', 'A')
        | ('U', 'U') => status.modified += 1,
        // Regular status codes
        _ => {
            // Check X (index) status
            match x {
                'A' => status.added += 1,    // Added to index
                'M' => status.modified += 1, // Modified in index
                'D' => status.deleted += 1,  // Deleted from index
                'R' => status.modified += 1, // Renamed (counts as modified)
                'C' => status.modified += 1, // Copied (counts as modified)
                'T' => status.modified += 1, // Type changed (counts as modified)
                'U' => status.modified += 1, // Unmerged (counts as modified)
                _ => {}
            }

            // Check Y (working tree) status
            match y {
                'M' => status.modified += 1, // Modified in working tree
                'D' => status.deleted += 1,  // Deleted from working tree
                'T' => status.modified += 1, // Type changed in working tree
                'U' => status.modified += 1, // Unmerged (counts as modified)
                _ => {}
            }
        }
    }
}

pub fn format_git_info(git_status: &GitStatus) -> String {
    let mut parts = Vec::new();

    // Add branch name (sanitized for terminal safety)
    if !git_status.branch.is_empty() {
        parts.push(format!(
            "{}{}{}",
            Colors::green(),
            sanitize_for_terminal(&git_status.branch),
            Colors::reset()
        ));
    }

    // Add file status counts
    if git_status.added > 0 {
        parts.push(format!(
            "{}+{}{}",
            Colors::green(),
            git_status.added,
            Colors::reset()
        ));
    }
    if git_status.modified > 0 {
        parts.push(format!(
            "{}~{}{}",
            Colors::yellow(),
            git_status.modified,
            Colors::reset()
        ));
    }
    if git_status.deleted > 0 {
        parts.push(format!(
            "{}-{}{}",
            Colors::red(),
            git_status.deleted,
            Colors::reset()
        ));
    }
    if git_status.untracked > 0 {
        parts.push(format!(
            "{}?{}{}",
            Colors::gray(),
            git_status.untracked,
            Colors::reset()
        ));
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
    fn test_validate_git_directory_security() {
        // Test null byte injection
        assert!(validate_git_directory("/tmp\0/evil").is_err());
        assert!(validate_git_directory("/tmp\0").is_err());

        // Test non-existent paths
        assert!(validate_git_directory("/definitely/does/not/exist").is_err());

        // Test file instead of directory
        let temp_file = std::env::temp_dir().join("test_file.txt");
        std::fs::write(&temp_file, "test").ok();
        assert!(validate_git_directory(temp_file.to_str().unwrap()).is_err());
        std::fs::remove_file(temp_file).ok();

        // Test non-git directory (temp dir usually isn't a git repo)
        let temp_dir = std::env::temp_dir();
        assert!(validate_git_directory(temp_dir.to_str().unwrap()).is_err());
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
    fn test_parse_git_status_branch_formats() {
        // Test simple branch name
        let status_text = "## main\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.branch, "main");

        // Test branch with upstream tracking
        let status_text = "## main...origin/main [ahead 1, behind 2]\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.branch, "main");

        // Test feature branch
        let status_text = "## feature/cool\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.branch, "feature/cool");

        // Test detached HEAD
        let status_text = "## HEAD (no branch)\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.branch, "HEAD (no branch)");
    }

    #[test]
    fn test_parse_git_status_added_files() {
        // Simple added file
        let status_text = "## main\nA  file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.added, 1);
        assert_eq!(status.modified, 0);

        // Added and modified
        let status_text = "## main\nAM file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.added, 1);
        assert_eq!(status.modified, 1);

        // Added and deleted (rare but possible)
        let status_text = "## main\nAD file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.added, 1);
        assert_eq!(status.deleted, 1);
    }

    #[test]
    fn test_parse_git_status_modified_files() {
        // Modified in working tree only
        let status_text = "## main\n M file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Modified in index only
        let status_text = "## main\nM  file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Modified in both
        let status_text = "## main\nMM file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 2); // Counts as 2 modifications

        // Type changed
        let status_text = "## main\nT  file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);
    }

    #[test]
    fn test_parse_git_status_deleted_files() {
        // Deleted from index
        let status_text = "## main\nD  file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.deleted, 1);

        // Deleted from working tree
        let status_text = "## main\n D file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.deleted, 1);

        // Modified then deleted
        let status_text = "## main\nMD file.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);
        assert_eq!(status.deleted, 1);
    }

    #[test]
    fn test_parse_git_status_renamed_copied() {
        // Renamed file (counts as modified)
        let status_text = "## main\nR  old.txt -> new.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);
        assert_eq!(status.added, 0);

        // Copied file (counts as modified)
        let status_text = "## main\nC  original.txt -> copy.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);
    }

    #[test]
    fn test_parse_git_status_unmerged_conflicts() {
        // Both deleted
        let status_text = "## main\nDD conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Added by us
        let status_text = "## main\nAU conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Deleted by them
        let status_text = "## main\nUD conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Added by them
        let status_text = "## main\nUA conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Deleted by us
        let status_text = "## main\nDU conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Both added
        let status_text = "## main\nAA conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);

        // Both modified
        let status_text = "## main\nUU conflict.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.modified, 1);
    }

    #[test]
    fn test_parse_git_status_untracked_ignored() {
        // Untracked file
        let status_text = "## main\n?? new.txt\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.untracked, 1);

        // Ignored file (should not be counted)
        let status_text = "## main\n!! target/\n";
        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.untracked, 0);
        assert_eq!(status.modified, 0);
        assert_eq!(status.added, 0);
        assert_eq!(status.deleted, 0);
    }

    #[test]
    fn test_parse_git_status_complex_scenario() {
        // A realistic complex status output
        let status_text = "## feature/new-feature...origin/feature/new-feature [ahead 3]\n\
                          A  new_file.rs\n\
                          AM modified_new.rs\n\
                          M  existing.rs\n\
                           M working_only.rs\n\
                          MM both_modified.rs\n\
                          D  deleted.rs\n\
                          R  old_name.rs -> new_name.rs\n\
                          ?? untracked.txt\n\
                          ?? another_untracked.md\n\
                          !! .DS_Store\n\
                          UU conflict.rs\n";

        let status = parse_git_status(status_text).unwrap();
        assert_eq!(status.branch, "feature/new-feature");
        assert_eq!(status.added, 2); // A and AM
        assert_eq!(status.modified, 7); // AM, M, M, MM (2x), R, UU
        assert_eq!(status.deleted, 1); // D
        assert_eq!(status.untracked, 2); // ?? files
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

    #[cfg(feature = "git_porcelain_v2")]
    #[test]
    fn test_parse_git_status_v2_branch() {
        // Test branch name parsing
        let status_text = "# branch.oid 1234567890abcdef\n# branch.head main\n";
        let status = parse_git_status_v2(status_text).unwrap();
        assert_eq!(status.branch, "main");

        // Test detached HEAD
        let status_text = "# branch.oid 1234567890abcdef\n# branch.head (detached)\n";
        let status = parse_git_status_v2(status_text).unwrap();
        assert_eq!(status.branch, "(detached)");
    }

    #[cfg(feature = "git_porcelain_v2")]
    #[test]
    fn test_parse_git_status_v2_files() {
        // Test various file statuses
        let status_text = "# branch.head main\n\
                          1 A. N... 100644 100644 000000 1234567 1234567 new.txt\n\
                          1 .M N... 100644 100644 100644 1234567 1234567 modified.txt\n\
                          1 M. N... 100644 100644 100644 1234567 1234567 staged.txt\n\
                          1 MM N... 100644 100644 100644 1234567 1234567 both.txt\n\
                          1 D. N... 100644 000000 000000 1234567 0000000 deleted.txt\n\
                          2 R. N... 100644 100644 100644 1234567 1234567 R100 renamed.txt\told.txt\n\
                          ? untracked.txt\n\
                          ! ignored.txt\n";

        let status = parse_git_status_v2(status_text).unwrap();
        assert_eq!(status.branch, "main");
        assert_eq!(status.added, 1); // A.
        assert_eq!(status.modified, 5); // .M, M., MM (2x), R.
        assert_eq!(status.deleted, 1); // D.
        assert_eq!(status.untracked, 1); // ?
    }

    #[cfg(feature = "git_porcelain_v2")]
    #[test]
    fn test_parse_git_status_v2_unmerged() {
        // Test unmerged conflict states
        let status_text = "# branch.head main\n\
                          u UU N... 100644 100644 100644 100644 1234567 1234567 1234567 conflict.txt\n\
                          u AA N... 100644 100644 100644 100644 1234567 1234567 1234567 both_added.txt\n";

        let status = parse_git_status_v2(status_text).unwrap();
        assert_eq!(status.modified, 2); // All unmerged states count as modified
    }

    #[test]
    fn test_format_git_info_sanitizes_branch_name() {
        // Disable colors for this test to check sanitization only
        std::env::set_var("NO_COLOR", "1");

        // Test that malicious branch names are sanitized
        let status = GitStatus {
            branch: "feature/\x1b[31mdanger\x1b[0m\x00\x07".to_string(),
            added: 0,
            modified: 0,
            deleted: 0,
            untracked: 0,
        };
        let formatted = format_git_info(&status);
        // Should not contain control characters (the escape codes from the malicious input)
        assert!(!formatted.contains('\x00'));
        assert!(!formatted.contains('\x07'));
        // Should contain the sanitized branch name
        assert!(formatted.contains("feature/danger"));

        // Clean up
        std::env::remove_var("NO_COLOR");
    }
}
