use crate::error::{Result, StatuslineError};
use log::warn;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Main configuration structure for the statusline
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Display configuration
    pub display: DisplayConfig,

    /// Context window configuration
    pub context: ContextConfig,

    /// Cost thresholds configuration
    pub cost: CostConfig,

    /// Database configuration
    pub database: DatabaseConfig,

    /// Retry configuration
    pub retry: RetryConfig,

    /// Transcript processing configuration
    pub transcript: TranscriptConfig,

    /// Git configuration
    pub git: GitConfig,

    /// Sync configuration (optional cloud sync)
    #[cfg(feature = "turso-sync")]
    pub sync: SyncConfig,
}

/// Display-related configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Progress bar width in characters
    pub progress_bar_width: usize,

    /// Context usage warning threshold (percentage)
    pub context_warning_threshold: f64,

    /// Context usage critical threshold (percentage)
    pub context_critical_threshold: f64,

    /// Context usage caution threshold (percentage)
    pub context_caution_threshold: f64,

    /// Theme (dark or light)
    pub theme: String,
}

/// Context window configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Default context window size in tokens
    pub window_size: usize,

    /// Context window sizes for specific models (model name -> size)
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub model_windows: std::collections::HashMap<String, usize>,
}

/// Cost threshold configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostConfig {
    /// Low cost threshold (below this is green)
    pub low_threshold: f64,

    /// Medium cost threshold (below this is yellow, above is red)
    pub medium_threshold: f64,
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Maximum connection pool size
    pub max_connections: u32,

    /// Busy timeout in milliseconds
    pub busy_timeout_ms: u32,

    /// Path to database file (relative to data directory)
    pub path: String,

    /// Whether to maintain JSON backup alongside SQLite (default: true for compatibility)
    pub json_backup: bool,

    /// Retention period for session data in days (0 = keep forever)
    pub retention_days_sessions: Option<u32>,

    /// Retention period for daily stats in days (0 = keep forever)
    pub retention_days_daily: Option<u32>,

    /// Retention period for monthly stats in days (0 = keep forever)
    pub retention_days_monthly: Option<u32>,
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// File operation retry configuration
    pub file_ops: RetrySettings,

    /// Database operation retry configuration
    pub db_ops: RetrySettings,

    /// Git operation retry configuration
    pub git_ops: RetrySettings,

    /// Network operation retry configuration
    pub network_ops: RetrySettings,
}

/// Individual retry settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrySettings {
    /// Maximum number of retry attempts
    pub max_attempts: u32,

    /// Initial delay in milliseconds
    pub initial_delay_ms: u64,

    /// Maximum delay in milliseconds
    pub max_delay_ms: u64,

    /// Backoff factor (multiplier for each retry)
    pub backoff_factor: f32,
}

/// Transcript processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptConfig {
    /// Number of lines to keep in memory (circular buffer size)
    pub buffer_lines: usize,
}

/// Git configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// Timeout for git operations in milliseconds
    pub timeout_ms: u32,
}

/// Sync configuration for cloud synchronization
#[cfg(feature = "turso-sync")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// Whether sync is enabled
    pub enabled: bool,

    /// Sync provider (currently only "turso" is supported)
    pub provider: String,

    /// Sync interval in seconds
    pub sync_interval_seconds: u64,

    /// Soft quota warning threshold (0.0 - 1.0)
    /// Warns when usage exceeds this fraction of quota
    pub soft_quota_fraction: f64,

    /// Turso-specific configuration
    pub turso: TursoConfig,
}

/// Turso-specific sync configuration
#[cfg(feature = "turso-sync")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TursoConfig {
    /// Turso database URL (e.g., "libsql://your-db.turso.io")
    pub database_url: String,

    /// Authentication token (or environment variable reference like "${TURSO_AUTH_TOKEN}")
    pub auth_token: String,
}

// Default implementations
// Default is derived above

impl Default for DisplayConfig {
    fn default() -> Self {
        DisplayConfig {
            progress_bar_width: 10,
            context_warning_threshold: 70.0,
            context_critical_threshold: 90.0,
            context_caution_threshold: 50.0,
            theme: "dark".to_string(),
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        ContextConfig {
            window_size: 160_000,
            model_windows: std::collections::HashMap::new(),
        }
    }
}

impl Default for CostConfig {
    fn default() -> Self {
        CostConfig {
            low_threshold: 5.0,
            medium_threshold: 20.0,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            max_connections: 5,
            busy_timeout_ms: 10000,
            path: "stats.db".to_string(),
            json_backup: true, // Default to true for backward compatibility
            retention_days_sessions: None, // None means use default (90 days)
            retention_days_daily: None, // None means use default (365 days)
            retention_days_monthly: None, // None means use default (0 = forever)
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        RetryConfig {
            file_ops: RetrySettings {
                max_attempts: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_factor: 2.0,
            },
            db_ops: RetrySettings {
                max_attempts: 5,
                initial_delay_ms: 50,
                max_delay_ms: 2000,
                backoff_factor: 1.5,
            },
            git_ops: RetrySettings {
                max_attempts: 3,
                initial_delay_ms: 100,
                max_delay_ms: 3000,
                backoff_factor: 2.0,
            },
            network_ops: RetrySettings {
                max_attempts: 2,
                initial_delay_ms: 200,
                max_delay_ms: 1000,
                backoff_factor: 2.0,
            },
        }
    }
}

impl Default for RetrySettings {
    fn default() -> Self {
        RetrySettings {
            max_attempts: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_factor: 2.0,
        }
    }
}

impl Default for TranscriptConfig {
    fn default() -> Self {
        TranscriptConfig { buffer_lines: 50 }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        GitConfig {
            timeout_ms: 200, // 200ms default timeout for git operations
        }
    }
}

#[cfg(feature = "turso-sync")]
impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            enabled: false, // Disabled by default
            provider: "turso".to_string(),
            sync_interval_seconds: 60,
            soft_quota_fraction: 0.75, // Warn at 75% of quota
            turso: TursoConfig::default(),
        }
    }
}

// From trait implementations for better ergonomics
impl From<PathBuf> for Config {
    fn from(path: PathBuf) -> Self {
        Config::load_from_file(&path).unwrap_or_default()
    }
}

impl From<&Path> for Config {
    fn from(path: &Path) -> Self {
        Config::load_from_file(path).unwrap_or_default()
    }
}

impl From<String> for Config {
    fn from(path: String) -> Self {
        Config::load_from_file(Path::new(&path)).unwrap_or_default()
    }
}

impl From<&str> for Config {
    fn from(path: &str) -> Self {
        Config::load_from_file(Path::new(path)).unwrap_or_default()
    }
}

// Configuration loading
impl Config {
    /// Load configuration from file, or use defaults
    pub fn load() -> Result<Self> {
        // Try to find config file in standard locations
        if let Some(config_path) = Self::find_config_file() {
            Self::load_from_file(&config_path)
        } else {
            // No config file found, use defaults
            Ok(Config::default())
        }
    }

    /// Load configuration from a specific file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| StatuslineError::Config(format!("Failed to read config file: {}", e)))?;

        let config: Config = toml::from_str(&contents)
            .map_err(|e| StatuslineError::Config(format!("Failed to parse config file: {}", e)))?;

        Ok(config)
    }

    /// Save configuration to file
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        let toml_string = toml::to_string_pretty(self)
            .map_err(|e| StatuslineError::Config(format!("Failed to serialize config: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                StatuslineError::Config(format!("Failed to create config directory: {}", e))
            })?;
        }

        fs::write(path, toml_string)
            .map_err(|e| StatuslineError::Config(format!("Failed to write config file: {}", e)))?;

        Ok(())
    }

    /// Find config file in standard locations
    fn find_config_file() -> Option<PathBuf> {
        // Check in order of priority:
        // 1. Environment variable from CLI flag
        if let Ok(path) = std::env::var("STATUSLINE_CONFIG_PATH") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }

        // 2. Environment variable
        if let Ok(path) = std::env::var("STATUSLINE_CONFIG") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }

        // 3. XDG config directory
        if let Some(config_dir) = dirs::config_dir() {
            let path = config_dir.join("claudia-statusline").join("config.toml");
            if path.exists() {
                return Some(path);
            }
        }

        // 4. Home directory
        if let Some(home_dir) = dirs::home_dir() {
            let path = home_dir.join(".claudia-statusline.toml");
            if path.exists() {
                return Some(path);
            }
        }

        None
    }

    /// Get default config file path (for creating new config)
    pub fn default_config_path() -> Result<PathBuf> {
        if let Some(config_dir) = dirs::config_dir() {
            Ok(config_dir.join("claudia-statusline").join("config.toml"))
        } else {
            Err(StatuslineError::Config(
                "Could not determine config directory".into(),
            ))
        }
    }

    /// Generate example config file content
    pub fn example_toml() -> &'static str {
        r#"# Claudia Statusline Configuration File
#
# This file configures various aspects of the statusline behavior.
# All values shown are the defaults - you can override only what you need.

[display]
# Width of the progress bar in characters
progress_bar_width = 10

# Context usage thresholds (percentage)
context_warning_threshold = 70.0     # Orange color above this
context_critical_threshold = 90.0    # Red color above this
context_caution_threshold = 50.0     # Yellow color above this

# Theme: "dark" or "light"
theme = "dark"

[context]
# Default context window size in tokens
window_size = 160000

# Model-specific context windows (optional)
# [context.model_windows]
# "claude-3-opus" = 200000
# "claude-3.5-sonnet" = 200000

[cost]
# Cost thresholds for color coding
low_threshold = 5.0      # Green below this
medium_threshold = 20.0  # Yellow between low and medium, red above

[database]
# Database connection settings
max_connections = 5
busy_timeout_ms = 10000
path = "stats.db"  # Relative to data directory
json_backup = true  # Maintain JSON backup alongside SQLite (set to false for SQLite-only mode)

# Data retention settings (for db-maintain command)
retention_days_sessions = 90    # Keep session data for N days
retention_days_daily = 365      # Keep daily aggregates for N days
retention_days_monthly = 0      # Keep monthly aggregates for N days (0 = forever)

[transcript]
# Number of transcript lines to keep in memory
buffer_lines = 50

[retry.file_ops]
# File operation retry settings
max_attempts = 3
initial_delay_ms = 100
max_delay_ms = 5000
backoff_factor = 2.0

[retry.db_ops]
# Database operation retry settings
max_attempts = 5
initial_delay_ms = 50
max_delay_ms = 2000
backoff_factor = 1.5

[retry.git_ops]
# Git operation retry settings
max_attempts = 3
initial_delay_ms = 100
max_delay_ms = 3000
backoff_factor = 2.0

[retry.network_ops]
# Network operation retry settings
max_attempts = 2
initial_delay_ms = 200
max_delay_ms = 1000
backoff_factor = 2.0

[git]
# Git operation settings
timeout_ms = 200  # Timeout for git operations

# Optional cloud sync configuration
# Requires building with --features turso-sync
# [sync]
# enabled = false
# provider = "turso"
# sync_interval_seconds = 60
# soft_quota_fraction = 0.75  # Warn when usage exceeds 75% of quota
#
# [sync.turso]
# database_url = "libsql://claude-stats.turso.io"
# auth_token = "${TURSO_AUTH_TOKEN}"  # Or paste token directly
"#
    }
}

// Global configuration instance
use std::sync::OnceLock;

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Get the global configuration instance
pub fn get_config() -> &'static Config {
    CONFIG.get_or_init(|| {
        let mut config = Config::load().unwrap_or_else(|e| {
            warn!("Failed to load config: {}. Using defaults.", e);
            Config::default()
        });

        // Override theme from environment if set
        if let Ok(theme) = env::var("CLAUDE_THEME") {
            config.display.theme = theme;
        } else if let Ok(theme) = env::var("STATUSLINE_THEME") {
            config.display.theme = theme;
        }

        config
    })
}

/// Get the current theme (with environment override support)
pub fn get_theme() -> String {
    env::var("CLAUDE_THEME")
        .or_else(|_| env::var("STATUSLINE_THEME"))
        .unwrap_or_else(|_| get_config().display.theme.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.display.progress_bar_width, 10);
        assert_eq!(config.context.window_size, 160_000);
        assert_eq!(config.cost.low_threshold, 5.0);
    }

    #[test]
    fn test_save_and_load_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let config = Config::default();
        config.save(&config_path).unwrap();

        let loaded_config = Config::load_from_file(&config_path).unwrap();
        assert_eq!(
            loaded_config.display.progress_bar_width,
            config.display.progress_bar_width
        );
    }

    #[test]
    fn test_example_config() {
        let example = Config::example_toml();
        assert!(example.contains("Claudia Statusline Configuration"));
        assert!(example.contains("progress_bar_width"));
        assert!(example.contains("window_size"));
    }
}
