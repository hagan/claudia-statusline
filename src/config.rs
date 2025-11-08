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

    // Component visibility toggles
    /// Show current directory path
    pub show_directory: bool,

    /// Show git branch and status
    pub show_git: bool,

    /// Show context usage percentage and progress bar
    pub show_context: bool,

    /// Show Claude model name
    pub show_model: bool,

    /// Show session duration
    pub show_duration: bool,

    /// Show lines added/removed
    pub show_lines_changed: bool,

    /// Show session cost and burn rate
    pub show_cost: bool,
}

/// Context window configuration
///
/// The statusline intelligently detects context window size based on model family and version:
/// - Sonnet 3.5+, 4.5+: 200k tokens
/// - Opus 3.5+: 200k tokens
/// - Older models (Sonnet 3.0, etc.): 160k tokens
/// - Unknown models: Uses `window_size` default (200k)
///
/// Users can override detection for specific models using `model_windows` HashMap.
///
/// **Adaptive Learning (Experimental):**
/// When enabled, the statusline learns actual context window sizes from usage patterns
/// by detecting compaction events and token ceiling observations. This feature is
/// **disabled by default** and requires explicit opt-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Default context window size in tokens (fallback for unknown models)
    ///
    /// Modern Claude models (Sonnet 3.5+, Opus 3.5+, Sonnet 4.5+) use 200k tokens.
    /// This default is used when model-specific detection fails or for unknown models.
    pub window_size: usize,

    /// Optional overrides for specific model display names
    ///
    /// Use this to override intelligent detection for specific models.
    /// Key is the model display name from Claude Code (e.g., "Claude 3.5 Sonnet").
    /// Value is the context window size in tokens.
    ///
    /// Example in config.toml:
    /// ```toml
    /// [context.model_windows]
    /// "Claude 3.5 Sonnet" = 200000
    /// "Claude Sonnet 4.5" = 200000
    /// ```
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub model_windows: std::collections::HashMap<String, usize>,

    /// Enable adaptive learning of context window sizes from usage patterns
    ///
    /// **Default: false (disabled)**
    ///
    /// When enabled, the statusline observes token patterns to learn actual context limits:
    /// - Detects automatic compaction events (sudden token drops)
    /// - Tracks repeated token ceiling observations
    /// - Builds confidence scores based on multiple observations
    ///
    /// **Impact on percentage display (v2.16.5+)**:
    /// Learned values refine BOTH "full" and "working" percentage modes:
    /// - Learned value represents working window where compaction happens (e.g., 156K)
    /// - Total window calculated as working + buffer (e.g., 156K + 40K = 196K)
    /// - "full" mode: tokens / learned_total (e.g., 150K / 196K = 77%)
    /// - "working" mode: tokens / learned_working (e.g., 150K / 156K = 96%)
    ///
    /// Learned values are only used when confidence >= `learning_confidence_threshold`.
    /// User overrides in `model_windows` always take precedence.
    ///
    /// **Experimental feature** - disabled by default for stability.
    pub adaptive_learning: bool,

    /// Minimum confidence score required to use learned context window values
    ///
    /// **Default: 0.7 (70%)**
    ///
    /// Range: 0.0 (0%) to 1.0 (100%)
    ///
    /// Confidence increases with observations:
    /// - 1 observation = ~0.1 confidence
    /// - 3 observations = ~0.4 confidence
    /// - 5+ observations = 0.7+ confidence
    ///
    /// Only applies when `adaptive_learning = true`.
    pub learning_confidence_threshold: f64,

    /// Claude Code buffer reserved for responses (not available for conversation)
    ///
    /// **Default: 40000 tokens (40K)**
    ///
    /// Claude Code reserves approximately 40-45K tokens as a buffer for generating
    /// responses. This buffer is not available for the conversation context.
    ///
    /// This setting is used to:
    /// - Calculate the "working window" (context_window - buffer)
    /// - Determine when to show auto-compact warnings
    /// - Provide accurate estimates of usable context space
    ///
    /// Reference: Claude Code auto-compact triggers when context reaches ~95% capacity
    /// or when you have ~40-45K tokens remaining (the buffer zone).
    pub buffer_size: usize,

    /// Auto-compact warning threshold percentage (mode-aware)
    ///
    /// **Default: 75.0 (mode-aware)**
    ///
    /// Shows warning indicator (⚠) when context percentage exceeds this value.
    ///
    /// **Mode-aware behavior:**
    /// - **"full" mode**: Default 75% = 150K tokens (warns ~6K before compaction at ~156K)
    /// - **"working" mode**: Auto-adjusted to 94% = 150K tokens (same warning point)
    ///
    /// This ensures the warning appears before actual auto-compaction in both display modes.
    ///
    /// **Custom thresholds:**
    /// Set any value between 0.0-100.0 to override the mode-aware defaults.
    /// Custom values are respected as-is without adjustment.
    ///
    /// **Example:**
    /// ```toml
    /// [context]
    /// auto_compact_threshold = 70.0  # Warn earlier (at 140K in "full" mode)
    /// ```
    ///
    /// Range: 0.0 to 100.0
    pub auto_compact_threshold: f64,

    /// Context percentage display mode
    ///
    /// **Default: "full"**
    ///
    /// Controls how the context percentage is calculated and displayed:
    ///
    /// - **"full"**: Percentage of total advertised context window (e.g., 200K)
    ///   - More intuitive: 100% = full 200K context as advertised by Anthropic
    ///   - Example: 150K tokens = 75% of 200K window
    ///   - **Recommended for most users**
    ///
    /// - **"working"**: Percentage of usable working window (context - buffer)
    ///   - More accurate: accounts for Claude's 40K response buffer
    ///   - Example: 150K tokens = 93.75% of 160K working window (200K - 40K)
    ///   - Shows how close you are to actual auto-compact trigger
    ///   - **Useful for power users tracking compaction**
    ///
    /// The buffer_size (default 40K) is only subtracted in "working" mode.
    #[serde(default = "default_percentage_mode")]
    pub percentage_mode: String,
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
            // All components visible by default (backward compatible)
            show_directory: true,
            show_git: true,
            show_context: true,
            show_model: true,
            show_duration: true,
            show_lines_changed: true,
            show_cost: true,
        }
    }
}

fn default_percentage_mode() -> String {
    "full".to_string()
}

impl ContextConfig {
    /// Get the effective auto-compact threshold based on percentage mode
    ///
    /// The threshold is automatically adjusted based on the display mode to ensure
    /// the warning appears before actual compaction in both modes:
    ///
    /// - "full" mode: Uses the configured threshold directly (default 75%)
    ///   - 75% of 200K = 150K tokens (warning ~6K before compaction at ~156K)
    ///
    /// - "working" mode: Adjusts threshold to account for buffer (default 94%)
    ///   - 94% of 160K = 150K tokens (same warning point as full mode)
    ///
    /// Users can override with custom thresholds that will be respected in both modes.
    pub fn get_effective_threshold(&self) -> f64 {
        // If user has customized the threshold, use it as-is
        // (We detect customization by checking if it's not the default 75% or legacy 80%)
        let is_custom = (self.auto_compact_threshold - 75.0).abs() > 0.1
            && (self.auto_compact_threshold - 80.0).abs() > 0.1;

        if is_custom {
            return self.auto_compact_threshold;
        }

        // Auto-adjust based on mode for default thresholds
        match self.percentage_mode.as_str() {
            "working" => {
                // In working mode, adjust to show warning at same absolute token count
                // Default: 75% of 200K = 150K tokens
                // In working mode: 150K / 160K = 93.75%, round to 94%
                94.0
            }
            _ => {
                // "full" mode (default): use 75% to warn before typical compaction at 78%
                // 75% of 200K = 150K tokens, compaction at ~156K (78%) gives ~6K warning buffer
                75.0
            }
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        ContextConfig {
            window_size: 200_000, // Default for modern Claude models (Sonnet 3.5+, Opus 3.5+, Sonnet 4.5+)
            model_windows: std::collections::HashMap::new(),
            adaptive_learning: false, // Disabled by default (experimental feature)
            learning_confidence_threshold: 0.7, // Require 70% confidence before using learned values
            buffer_size: 40_000, // Claude Code reserves ~40-45K tokens for responses
            auto_compact_threshold: 75.0, // Mode-aware: 75% for "full", auto-adjusted to 94% for "working"
            percentage_mode: default_percentage_mode(), // Default to "full" for user expectations
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
        TranscriptConfig {
            buffer_lines: 50,
        }
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
# Default context window size in tokens (modern Claude models use 200k)
# This is used as fallback when model-specific window is not configured
window_size = 200000

# Model-specific context windows (optional overrides)
# The statusline intelligently detects context window size based on model family/version
# You can override detection here for specific models by display name
# [context.model_windows]
# "Claude 3.5 Sonnet" = 200000
# "Claude Sonnet 4.5" = 200000
# "Claude 3.5 Opus" = 200000
# "Claude 3 Haiku" = 100000

# Adaptive Learning (Experimental) - DISABLED BY DEFAULT
# When enabled, the statusline learns actual context window sizes from usage patterns
# by detecting compaction events and token ceiling observations
adaptive_learning = false

# Minimum confidence threshold (0.0-1.0) required to use learned values
# Only applies when adaptive_learning = true
# Confidence increases with more observations (0.7 = 70% confidence)
learning_confidence_threshold = 0.7

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
# Number of transcript lines to keep in memory (circular buffer)
# For large files, only the last N lines are read (tail-reading optimization)
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
        assert_eq!(config.context.window_size, 200_000); // Updated for modern Claude models
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

    #[test]
    fn test_display_config_defaults() {
        let config = DisplayConfig::default();
        // All components should be visible by default (backward compatible)
        assert!(config.show_directory);
        assert!(config.show_git);
        assert!(config.show_context);
        assert!(config.show_model);
        assert!(config.show_duration);
        assert!(config.show_lines_changed);
        assert!(config.show_cost);
    }

    #[test]
    fn test_display_config_minimal() {
        let toml = r#"
        [display]
        show_directory = true
        show_git = false
        show_context = false
        show_model = false
        show_duration = false
        show_lines_changed = false
        show_cost = true
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.display.show_directory);
        assert!(config.display.show_cost);
        assert!(!config.display.show_git);
        assert!(!config.display.show_context);
        assert!(!config.display.show_model);
        assert!(!config.display.show_duration);
        assert!(!config.display.show_lines_changed);
    }

    #[test]
    fn test_display_config_developer_focus() {
        let toml = r#"
        [display]
        show_directory = true
        show_git = true
        show_context = true
        show_model = false
        show_duration = false
        show_lines_changed = true
        show_cost = false
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.display.show_directory);
        assert!(config.display.show_git);
        assert!(config.display.show_context);
        assert!(config.display.show_lines_changed);
        assert!(!config.display.show_model);
        assert!(!config.display.show_duration);
        assert!(!config.display.show_cost);
    }

    #[test]
    fn test_display_config_partial() {
        // Test that unspecified fields default to true
        let toml = r#"
        [display]
        show_git = false
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.display.show_directory); // Not specified, should default to true
        assert!(!config.display.show_git); // Explicitly set to false
        assert!(config.display.show_model); // Not specified, should default to true
    }

    #[test]
    fn test_display_config_all_disabled() {
        let toml = r#"
        [display]
        show_directory = false
        show_git = false
        show_context = false
        show_model = false
        show_duration = false
        show_lines_changed = false
        show_cost = false
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert!(!config.display.show_directory);
        assert!(!config.display.show_git);
        assert!(!config.display.show_context);
        assert!(!config.display.show_model);
        assert!(!config.display.show_duration);
        assert!(!config.display.show_lines_changed);
        assert!(!config.display.show_cost);
    }

    #[test]
    fn test_display_config_serialization() {
        let config = DisplayConfig::default();
        let serialized = toml::to_string(&config).unwrap();

        // Check that all fields are present in serialized output
        assert!(serialized.contains("show_directory"));
        assert!(serialized.contains("show_git"));
        assert!(serialized.contains("show_context"));
        assert!(serialized.contains("show_model"));
        assert!(serialized.contains("show_duration"));
        assert!(serialized.contains("show_lines_changed"));
        assert!(serialized.contains("show_cost"));
    }
}
