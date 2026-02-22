//! GitProvider -- wraps existing git module as a DataProvider.
//!
//! Produces the same `git` and `git_branch` variables that
//! `VariableBuilder::git_with_config()` currently produces, preserving
//! backward compatibility. Colors are pre-applied (Phase 6 defers
//! raw-value refactoring).

use crate::git::{format_git_info, get_git_status};
use crate::provider::{DataProvider, ProviderResult};
use crate::utils::sanitize_for_terminal;
use std::collections::HashMap;
use std::time::Duration;

/// Data provider that collects git repository status variables.
///
/// Wraps the existing `get_git_status()` + `format_git_info()` pipeline
/// and returns pre-formatted strings with ANSI color codes embedded.
pub struct GitProvider {
    current_dir: String,
}

impl GitProvider {
    /// Create a new GitProvider for the given directory.
    pub fn new(current_dir: &str) -> Self {
        Self {
            current_dir: current_dir.to_string(),
        }
    }
}

impl DataProvider for GitProvider {
    fn name(&self) -> &str {
        "git"
    }

    fn priority(&self) -> u32 {
        50
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(100)
    }

    fn is_available(&self) -> bool {
        std::path::Path::new(&self.current_dir)
            .join(".git")
            .exists()
    }

    fn collect(&self) -> ProviderResult {
        let mut vars = HashMap::new();

        if let Some(status) = get_git_status(&self.current_dir) {
            let git_info = format_git_info(&status);
            // Trim leading space from format_git_info() legacy format
            vars.insert("git".to_string(), git_info.trim_start().to_string());
            vars.insert(
                "git_branch".to_string(),
                sanitize_for_terminal(&status.branch),
            );
        }

        Ok(vars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::DataProvider;

    #[test]
    fn test_git_provider_trait_contract() {
        let provider = GitProvider::new("/tmp/nonexistent");
        assert_eq!(provider.name(), "git");
        assert_eq!(provider.priority(), 50);
        assert_eq!(provider.timeout(), Duration::from_millis(100));
    }

    #[test]
    fn test_git_provider_unavailable_in_non_git_dir() {
        let provider = GitProvider::new("/tmp");
        assert!(
            !provider.is_available(),
            "Non-git directory should be unavailable"
        );
    }

    #[test]
    fn test_git_provider_collect_non_git() {
        let provider = GitProvider::new("/tmp");
        let result = provider.collect().expect("collect should succeed");
        assert!(
            result.is_empty(),
            "Non-git directory should produce empty vars"
        );
    }
}
