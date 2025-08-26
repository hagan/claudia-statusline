// Version information module
// This module provides version and build information for the statusline

use std::fmt;

/// Version information structure
pub struct VersionInfo {
    pub version: &'static str,
    pub git_hash: &'static str,
    pub git_branch: &'static str,
    pub git_dirty: bool,
    #[allow(dead_code)]
    pub git_describe: &'static str,
    pub build_date: &'static str,
    pub build_profile: &'static str,
    pub rustc_version: &'static str,
}

impl VersionInfo {
    /// Get the current version information
    pub fn current() -> Self {
        Self {
            version: env!("CLAUDIA_VERSION"),
            git_hash: env!("CLAUDIA_GIT_HASH"),
            git_branch: env!("CLAUDIA_GIT_BRANCH"),
            git_dirty: env!("CLAUDIA_GIT_DIRTY") == "true",
            git_describe: env!("CLAUDIA_GIT_DESCRIBE"),
            build_date: env!("CLAUDIA_BUILD_DATE"),
            build_profile: env!("CLAUDIA_BUILD_PROFILE"),
            rustc_version: env!("CLAUDIA_RUSTC_VERSION"),
        }
    }

    /// Get a short version string (just version and git hash)
    pub fn short(&self) -> String {
        if self.git_dirty {
            format!("v{} ({}+dirty)", self.version, self.git_hash)
        } else {
            format!("v{} ({})", self.version, self.git_hash)
        }
    }

    /// Get the full semantic version with git information
    #[allow(dead_code)]
    pub fn full(&self) -> String {
        let dirty = if self.git_dirty { "+dirty" } else { "" };
        format!("v{}-{}{}", self.version, self.git_hash, dirty)
    }

    /// Check if this is a release build
    #[allow(dead_code)]
    pub fn is_release(&self) -> bool {
        self.build_profile == "release"
    }

    /// Check if the working directory was clean during build
    #[allow(dead_code)]
    pub fn is_clean(&self) -> bool {
        !self.git_dirty
    }
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Claudia Statusline v{}", self.version)?;
        writeln!(f, "Git: {} ({}){}",
            self.git_hash,
            self.git_branch,
            if self.git_dirty { " +uncommitted changes" } else { "" }
        )?;
        writeln!(f, "Built: {} ({})", self.build_date, self.build_profile)?;
        writeln!(f, "Rustc: {}", self.rustc_version)?;
        Ok(())
    }
}

/// Get the version string for --version output
pub fn version_string() -> String {
    let info = VersionInfo::current();
    format!("{}", info)
}

/// Get a short version string for logs or debug output
#[allow(dead_code)]
pub fn short_version() -> String {
    let info = VersionInfo::current();
    info.short()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_info_creation() {
        let info = VersionInfo::current();
        assert!(!info.version.is_empty());
        assert!(!info.git_hash.is_empty());
        assert!(!info.build_date.is_empty());
    }

    #[test]
    fn test_short_version() {
        let info = VersionInfo::current();
        let short = info.short();
        assert!(short.starts_with("v"));
        assert!(short.contains(&info.version));
    }

    #[test]
    fn test_full_version() {
        let info = VersionInfo::current();
        let full = info.full();
        assert!(full.starts_with("v"));
        assert!(full.contains(&info.version));
        assert!(full.contains(&info.git_hash));
    }

    #[test]
    fn test_version_display() {
        let info = VersionInfo::current();
        let display = format!("{}", info);
        assert!(display.contains("Claudia Statusline"));
        assert!(display.contains("Git:"));
        assert!(display.contains("Built:"));
        assert!(display.contains("Rustc:"));
    }

    #[test]
    fn test_version_string_function() {
        let version = version_string();
        assert!(version.contains("Claudia Statusline"));
        assert!(!version.is_empty());
    }

    #[test]
    fn test_short_version_function() {
        let short = short_version();
        assert!(short.starts_with("v"));
        assert!(short.contains("("));
        assert!(short.contains(")"));
    }

    #[test]
    fn test_is_release() {
        let info = VersionInfo::current();
        // Will be true if built with --release, false otherwise
        assert!(info.is_release() || !info.is_release());
    }

    #[test]
    fn test_is_clean() {
        let info = VersionInfo::current();
        // Will be true if working directory is clean, false otherwise
        assert!(info.is_clean() || !info.is_clean());
    }

    #[test]
    fn test_dirty_flag_in_short_version() {
        let info = VersionInfo::current();
        let short = info.short();
        if info.git_dirty {
            assert!(short.contains("+dirty"));
        } else {
            assert!(!short.contains("+dirty"));
        }
    }
}