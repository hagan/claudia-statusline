use crate::config_validation::{FindingKind, Report, SectionContext, Validate};
use crate::error::{Result, StatuslineError};
use crate::gsd::config::GsdConfig;
use log::warn;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Serde default helper: returns `true`.
fn default_true() -> bool {
    true
}

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

    /// Burn rate calculation configuration
    pub burn_rate: BurnRateConfig,

    /// Layout configuration for customizable statusline format
    pub layout: LayoutConfig,

    /// Token rate metrics configuration
    pub token_rate: TokenRateConfig,

    /// GSD project tracking configuration
    pub gsd: GsdConfig,

    /// Ant (opt-in Claude API enrichment) configuration (default disabled, D-08)
    pub ant: crate::ant::config::AntConfig,

    /// Pricing configuration: bundled offline Claude price table + exact-match
    /// lookup aliases (`[pricing]` / `[pricing.aliases]`, PRICE-01/PRICE-05).
    /// Absent section parses to default → byte-identical render (D-11).
    #[serde(default, deserialize_with = "crate::pricing::deserialize_lenient")]
    pub pricing: crate::pricing::PricingConfig,
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

    /// Show token counts in context bar (e.g., "179k/1000k")
    pub show_context_tokens: bool,

    /// Show Claude.ai (Pro/Max) rate-limit windows, e.g. "5h:24% 7d:41%".
    /// Sourced from the payload `rate_limits` object; renders nothing for
    /// API-key usage. Opt-in (default false) to keep existing output unchanged.
    #[serde(default)]
    pub show_rate_limits: bool,

    /// Append a reset countdown to each rate-limit window, e.g.
    /// "5h:24% (2h13m)". Applies to the `show_rate_limits` segment and the
    /// `{rate_limits}` / `{rate_limit_5h}` template variables; the separate
    /// `{rate_limit_5h_reset}` / `{rate_limit_7d_reset}` variables are always
    /// available regardless of this flag. Opt-in (default false).
    #[serde(default)]
    pub rate_limit_reset_countdown: bool,
}

// ---------------------------------------------------------------------------
// Shared semantic-validation helpers (plan 12-05)
//
// None of these performs network IO, spawns a process, or writes to disk. The
// only filesystem access any rule in this file makes is the `.exists()`-guarded
// read the theme manager and the preset loader already perform on the render
// path, and it happens once per `config validate` run, never per render.
// ---------------------------------------------------------------------------

/// Reject a non-finite float BEFORE any range comparison.
///
/// TOML genuinely accepts `nan`, `inf`, `+inf` and `-inf` as float literals, and
/// EVERY comparison against a `nan` is `false` — so a bare `v < 0.0` guard lets
/// `nan` through silently, which is precisely the class of silent fall-through
/// `config validate` exists to end. Every float rule in this file calls this
/// FIRST and skips its range comparison when it returns `false`, so one field
/// never produces both a finiteness error and a bogus range/relation finding.
fn check_finite(cx: &SectionContext, field: &str, v: f64, report: &mut Report) -> bool {
    if v.is_finite() {
        return true;
    }
    report.error(
        FindingKind::InvalidValue,
        cx.key(field),
        format!(
            "`{}` must be a finite number — TOML accepts `nan`, `inf` and `-inf` as float literals, but none of them is usable here",
            field
        ),
    );
    false
}

/// Finiteness gate followed by an inclusive range check.
///
/// Returns `true` only when the value is finite AND within `[min, max]`, so a
/// caller can skip a cross-field relation that a rejected value would make
/// meaningless.
fn check_in_range(
    cx: &SectionContext,
    field: &str,
    v: f64,
    min: f64,
    max: f64,
    report: &mut Report,
) -> bool {
    if !check_finite(cx, field, v, report) {
        return false;
    }
    if v < min || v > max {
        report.error(
            FindingKind::InvalidValue,
            cx.key(field),
            format!("`{}` must be between {} and {} inclusive", field, min, max),
        );
        return false;
    }
    true
}

/// Check an enum-like string against its legal set (D-04).
///
/// The message ALWAYS enumerates the legal set — that is what turns a silent
/// `match … { _ => default }` fall-through into an actionable finding. The
/// offending value is deliberately NOT echoed: `Finding::message` carries no
/// config value text (the key is the locator), which keeps every message safe
/// by construction rather than by per-field review.
fn check_enum(cx: &SectionContext, field: &str, value: &str, legal: &[&str], report: &mut Report) {
    if legal.contains(&value) {
        return;
    }
    report.error(
        FindingKind::InvalidValue,
        cx.key(field),
        format!(
            "invalid value for `{}`; legal values are: {}",
            field,
            legal.join(", ")
        ),
    );
}

/// Semantic rules for `[display]` (plan 12-05).
impl Validate for DisplayConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if self.progress_bar_width == 0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("progress_bar_width"),
                "`progress_bar_width` must be at least 1 — a zero-width bar renders nothing",
            );
        } else if self.progress_bar_width > 100 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("progress_bar_width"),
                "`progress_bar_width` is a fixed character count, not a percentage; a width above 100 will not fit a normal terminal",
            );
        }

        // All three feed the SAME progress bar, so they share a scale and an
        // expected ordering.
        let caution_ok = check_in_range(
            cx,
            "context_caution_threshold",
            self.context_caution_threshold,
            0.0,
            100.0,
            report,
        );
        let warning_ok = check_in_range(
            cx,
            "context_warning_threshold",
            self.context_warning_threshold,
            0.0,
            100.0,
            report,
        );
        let critical_ok = check_in_range(
            cx,
            "context_critical_threshold",
            self.context_critical_threshold,
            0.0,
            100.0,
            report,
        );

        let ascending = self.context_caution_threshold <= self.context_warning_threshold
            && self.context_warning_threshold <= self.context_critical_threshold;
        if caution_ok && warning_ok && critical_ok && !ascending {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("context_warning_threshold"),
                "the three context thresholds colour one progress bar and are expected to ascend: caution <= warning <= critical",
            );
        }

        // Built-ins PLUS the user themes directory — `embedded_themes()` alone
        // would reject a legitimate user theme file.
        let themes = crate::theme::ThemeManager::new().list_themes();
        if !themes.iter().any(|t| t == &self.theme) {
            report.error(
                FindingKind::InvalidValue,
                cx.key("theme"),
                format!("unknown theme; available themes are: {}", themes.join(", ")),
            );
        }
    }
}

/// Context window configuration
///
/// The statusline intelligently detects context window size based on model family and version:
/// - Sonnet 4.5 (1M context): 1M tokens (auto-detected from display name)
/// - Sonnet 3.5+, 4.5: 200k tokens
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
    /// Modern Claude models use varying context windows:
    /// - Sonnet 4.5 (1M context): 1M tokens (auto-detected)
    /// - Sonnet 3.5+, 4.5, Opus 3.5+: 200k tokens (auto-detected)
    ///
    /// This default (200k) is used when model-specific detection fails or for unknown models.
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

/// Semantic rules for `[context]` (plan 12-05).
impl Validate for ContextConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if self.window_size == 0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("window_size"),
                "`window_size` must be greater than 0 — it is the fallback context window for models detection does not recognize",
            );
        }

        // The map KEYS are free-form model display names and are NEVER a
        // finding; only a zero VALUE is.
        for (model, size) in &self.model_windows {
            if *size == 0 {
                report.error(
                    FindingKind::InvalidValue,
                    crate::config_validation::redact_key_path(
                        &cx.key(&format!("model_windows.{}", model)),
                    ),
                    "a `model_windows` override must be greater than 0",
                );
            }
        }

        check_in_range(
            cx,
            "learning_confidence_threshold",
            self.learning_confidence_threshold,
            0.0,
            1.0,
            report,
        );

        if self.window_size > 0 && self.buffer_size >= self.window_size {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("buffer_size"),
                "`buffer_size` is reserved out of `window_size`; a buffer at or above the window leaves no usable context",
            );
        }

        check_in_range(
            cx,
            "auto_compact_threshold",
            self.auto_compact_threshold,
            0.0,
            100.0,
            report,
        );

        // `src/utils.rs` matches this with a `_ =>` arm that silently falls back
        // to "full".
        check_enum(
            cx,
            "percentage_mode",
            &self.percentage_mode,
            &["full", "working"],
            report,
        );
    }
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

/// Semantic rules for `[cost]` (plan 12-05).
impl Validate for CostConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        let low_ok = check_finite(cx, "low_threshold", self.low_threshold, report);
        if low_ok && self.low_threshold < 0.0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("low_threshold"),
                "`low_threshold` is a dollar amount and must not be negative",
            );
        }

        let medium_ok = check_finite(cx, "medium_threshold", self.medium_threshold, report);
        if medium_ok && self.medium_threshold < 0.0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("medium_threshold"),
                "`medium_threshold` is a dollar amount and must not be negative",
            );
        }

        // Only meaningful once both values are real numbers — the finiteness
        // gate short-circuits this relation rather than comparing against `nan`.
        if low_ok && medium_ok && self.low_threshold >= self.medium_threshold {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("low_threshold"),
                "cost colours ascend green -> yellow -> red, so `low_threshold` is expected to be below `medium_threshold`",
            );
        }
    }
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Busy timeout in milliseconds
    pub busy_timeout_ms: u32,

    /// Path to database file (relative to data directory)
    pub path: String,

    /// Legacy in v3.0.0 — JSON writes were removed. The field is retained
    /// for deserialization compatibility so existing v2.x configs still
    /// parse, but its value has no functional effect except for emitting
    /// a one-line stderr deprecation warning when set to true. Default is
    /// now false. Rollback to JSON dual-write requires v2.22.x.
    /// `skip_serializing` keeps it deserialize-only so saved/re-emitted
    /// configs do NOT carry the field forward (D-08).
    #[serde(skip_serializing)]
    pub json_backup: bool,

    /// Retention period for session data in days (0 = keep forever)
    pub retention_days_sessions: Option<u32>,

    /// Retention period for daily stats in days (0 = keep forever)
    pub retention_days_daily: Option<u32>,

    /// Retention period for monthly stats in days (0 = keep forever)
    pub retention_days_monthly: Option<u32>,
}

/// Semantic rules for `[database]` (plan 12-05).
impl Validate for DatabaseConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if self.busy_timeout_ms == 0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("busy_timeout_ms"),
                "`busy_timeout_ms` must be greater than 0 — a zero busy timeout fails immediately on any lock contention",
            );
        }

        // A RELATIVE `path` is resolved against the data directory at runtime,
        // so probing its parent from the validating process's CWD would be a
        // false positive. Only an ABSOLUTE path can be checked here, and it is
        // checked by stat alone — the directory is NEVER created.
        if !self.path.is_empty() {
            let path = Path::new(&self.path);
            if path.is_absolute() {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        report.warn(
                            FindingKind::InvalidValue,
                            cx.key("path"),
                            "the parent directory of `path` does not exist; the database cannot be created there",
                        );
                    }
                }
            }
        }

        // A real deserialized field (`skip_serializing`, never an unknown key —
        // D-09), retained only for v2.x compatibility.
        if self.json_backup {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("json_backup"),
                "`json_backup` has had no effect since v3.0.0 — storage is SQLite-only and JSON writes were removed",
            );
        }
    }
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

/// Semantic rules for `[retry]` (plan 12-05).
///
/// Each nested `RetrySettings` is validated through `cx.child(..)`, which
/// narrows the dotted prefix and the present-key set in one call.
impl Validate for RetryConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        self.file_ops.validate(&cx.child("file_ops"), report);
        self.db_ops.validate(&cx.child("db_ops"), report);
        self.git_ops.validate(&cx.child("git_ops"), report);
        self.network_ops.validate(&cx.child("network_ops"), report);
    }
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

/// Semantic rules for one `[retry.*]` block (plan 12-05).
impl Validate for RetrySettings {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if self.max_attempts == 0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("max_attempts"),
                "`max_attempts` must be at least 1 — zero attempts means the operation never runs",
            );
        } else if self.max_attempts > 10 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("max_attempts"),
                "`max_attempts` above 10 multiplies the worst-case latency of an operation the render path waits on",
            );
        }

        if self.initial_delay_ms > self.max_delay_ms {
            report.error(
                FindingKind::InvalidValue,
                cx.key("initial_delay_ms"),
                "`initial_delay_ms` must not exceed `max_delay_ms`",
            );
        }

        // `backoff_factor` is an `f32`; widened only so ONE finiteness gate
        // serves every float rule in this file.
        if check_finite(cx, "backoff_factor", self.backoff_factor as f64, report)
            && self.backoff_factor < 1.0
        {
            report.error(
                FindingKind::InvalidValue,
                cx.key("backoff_factor"),
                "`backoff_factor` must be at least 1.0 — a factor below 1.0 shrinks the delay on every retry",
            );
        }
    }
}

/// Transcript processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptConfig {
    /// Number of lines to keep in memory (circular buffer size)
    pub buffer_lines: usize,
}

/// Semantic rules for `[transcript]` (plan 12-05).
impl Validate for TranscriptConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if self.buffer_lines == 0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("buffer_lines"),
                "`buffer_lines` must be at least 1 — a zero-line buffer discards every transcript line",
            );
        }
    }
}

/// Git configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// Timeout for git operations in milliseconds
    pub timeout_ms: u32,
}

/// Semantic rules for `[git]` (plan 12-05).
impl Validate for GitConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if self.timeout_ms == 0 {
            report.error(
                FindingKind::InvalidValue,
                cx.key("timeout_ms"),
                "`timeout_ms` must be greater than 0 — a zero timeout aborts every git call",
            );
        } else if self.timeout_ms > 5000 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("timeout_ms"),
                "the render path's whole budget is a few milliseconds; a git timeout above 5000ms can stall the status line",
            );
        }
    }
}

/// Burn rate calculation configuration
///
/// Controls how the hourly burn rate ($/hour) is calculated from session costs.
///
/// **Available Modes:**
/// - **"wall_clock" (default)**: Uses total elapsed time from session start to last update
///   - Simple and consistent across sessions
///   - Includes idle time (nights, weekends, breaks)
///   - Results in lower rates for long-running sessions
///   - Example: $8.99 over 22 days = $0.02/hour
///
/// - **"active_time"**: Tracks only active conversation time
///   - Counts time between consecutive messages
///   - Excludes idle periods (>inactivity_threshold)
///   - More accurate representation of actual usage cost
///   - Requires tracking message timestamps in database
///   - Example: $8.99 over 2 hours active = $4.50/hour
///
/// - **"auto_reset"**: Automatically starts new sessions after inactivity
///   - Treats gaps >inactivity_threshold as session boundaries
///   - Each session gets independent cost/duration tracking
///   - Prevents multi-day sessions with inflated durations
///   - Best for realistic burn rate tracking
///   - Example: Session ends after 1 hour idle, new session on next message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BurnRateConfig {
    /// Burn rate calculation mode
    ///
    /// Options: "wall_clock", "active_time", or "auto_reset"
    /// Default: "wall_clock" (backward compatible)
    pub mode: String,

    /// Inactivity threshold in minutes
    ///
    /// Used by "active_time" and "auto_reset" modes:
    /// - **active_time**: Gaps longer than this are excluded from duration
    /// - **auto_reset**: Session is considered ended after this much idle time
    ///
    /// Default: 60 minutes (1 hour)
    /// Reasonable range: 15-120 minutes
    pub inactivity_threshold_minutes: u32,

    /// Minimum session duration in seconds before showing burn rate
    ///
    /// Sessions shorter than this threshold will not display a burn rate,
    /// since very short sessions produce unreliable $/hr estimates.
    ///
    /// Default: 60 seconds (1 minute)
    /// Reasonable range: 30-300 seconds
    #[serde(default = "default_min_duration_seconds")]
    pub min_duration_seconds: u64,
}

/// Semantic rules for `[burn_rate]` are filled in by plan 12-05.
impl Validate for BurnRateConfig {}

/// Layout configuration for customizable statusline format
///
/// Allows users to define their own statusline layout using a template string
/// with variables that get replaced with actual values.
///
/// # Example
///
/// ```toml
/// [layout]
/// # Use a preset
/// preset = "default"
///
/// # Or define custom format
/// format = "{directory} • {git} • {model} • {cost}"
///
/// # Multi-line example
/// format = """
/// {directory} • {git}
/// {context} • {model} • {cost}
/// """
///
/// # Custom separator (default: " • ")
/// separator = " | "
/// ```
///
/// # Available Variables
///
/// | Variable | Example | Description |
/// |----------|---------|-------------|
/// | `{directory}` | `~/projects/app` | Shortened directory path |
/// | `{dir_short}` | `app` | Just the directory name |
/// | `{git}` | `main +2 ~1` | Full git info |
/// | `{git_branch}` | `main` | Branch name only |
/// | `{context}` | `75% [=====>----]` | Context bar with percentage |
/// | `{context_pct}` | `75` | Just the percentage number |
/// | `{context_tokens}` | `150k/200k` | Token counts |
/// | `{model}` | `S4.5` | Model abbreviation |
/// | `{model_full}` | `Claude Sonnet 4.5` | Full model name |
/// | `{model_name}` | `Sonnet` | Model family name |
/// | `{duration}` | `25m` | Session duration |
/// | `{cost}` | `$12.50` | Session cost |
/// | `{burn_rate}` | `$3.50/hr` | Cost per hour |
/// | `{daily_total}` | `$45.00` | Today's total cost |
/// | `{lines}` | `+50 -10` | Lines changed |
/// | `{token_rate}` | `12.5 tok/s` | Token processing rate (combined format) |
/// | `{token_rate_only}` | `12.5 tok/s` | Token rate only |
/// | `{token_session_total}` | `1.5K` | Session token total |
/// | `{token_daily_total}` | `day: 25K` | Daily token total |
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// Layout preset name
    ///
    /// Available presets:
    /// - "default": Standard single-line layout (current behavior)
    /// - "compact": Minimal info, short format
    /// - "detailed": Multi-line with all information
    /// - "minimal": Just directory and model
    ///
    /// If both `preset` and `format` are specified, `format` takes precedence.
    pub preset: String,

    /// Custom format string with variable placeholders
    ///
    /// Use `{variable_name}` syntax. Newlines create multi-line output.
    /// If empty, the preset format is used.
    pub format: String,

    /// Separator between components (default: " • ")
    ///
    /// Used when `{sep}` variable is in the format string,
    /// or when using presets that include separators.
    pub separator: String,

    /// Per-component configuration overrides
    #[serde(default)]
    pub components: ComponentsConfig,

    /// Show unknown template variables as literal `{var}` in output (default: true).
    /// When false, unknown variables render as empty string.
    #[serde(default = "default_true")]
    pub show_unknown_vars: bool,
}

/// Semantic rules for `[layout]` are filled in by plan 12-05.
impl Validate for LayoutConfig {}

/// Per-component configuration for fine-grained customization
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ComponentsConfig {
    /// Directory component settings
    pub directory: DirectoryComponentConfig,

    /// Git component settings
    pub git: GitComponentConfig,

    /// Context component settings
    pub context: ContextComponentConfig,

    /// Cost component settings
    pub cost: CostComponentConfig,

    /// Model component settings
    pub model: ModelComponentConfig,

    /// Token rate component settings
    pub token_rate: TokenRateComponentConfig,
}

/// Directory component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectoryComponentConfig {
    /// Format: "short" (default), "full", "basename"
    pub format: String,

    /// Maximum length before truncation (0 = no limit)
    pub max_length: usize,

    /// Override theme color (empty = use theme)
    pub color: String,
}

/// Git component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitComponentConfig {
    /// Format: "full" (default), "branch", "status"
    pub format: String,

    /// When to show: "always" (default), "dirty", "never"
    pub show_when: String,

    /// Override theme color (empty = use theme)
    pub color: String,
}

/// Context component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextComponentConfig {
    /// Format: "full" (default), "bar", "percent", "tokens"
    pub format: String,

    /// Progress bar width (default from display config)
    pub bar_width: Option<usize>,

    /// Show token counts
    pub show_tokens: bool,
}

/// Cost component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostComponentConfig {
    /// Format: "full" (default), "cost_only", "rate_only", "with_daily"
    pub format: String,

    /// Override theme color (empty = use theme)
    pub color: String,
}

/// Model component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelComponentConfig {
    /// Format: "abbreviation" (default), "full", "name", "version"
    ///
    /// - "abbreviation": Short form like "S4.5", "O4.5", "H4.5"
    /// - "full": Full display name like "Claude Sonnet 4.5"
    /// - "name": Just the model family like "Sonnet", "Opus", "Haiku"
    /// - "version": Just the version number like "4.5"
    pub format: String,

    /// Override theme color (empty = use theme)
    pub color: String,
}

/// Token rate component configuration
///
/// Controls how token rate metrics are displayed in the statusline.
/// Works in conjunction with `[token_rate]` config for enabling the feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenRateComponentConfig {
    /// Format: "rate_only" (default), "with_session", "with_daily", "full"
    ///
    /// - "rate_only": Just the rate (e.g., "13.9 tok/s")
    /// - "with_session": Rate + session total (e.g., "13.9 tok/s • 150K")
    /// - "with_daily": Rate + daily total (e.g., "13.9 tok/s (day: 2.5M)")
    /// - "full": Rate + session + daily (e.g., "13.9 tok/s • 150K (day: 2.5M)")
    pub format: String,

    /// Time unit for rate calculation: "second" (default), "minute", "hour"
    ///
    /// - "second": Tokens per second (e.g., "13.9 tok/s")
    /// - "minute": Tokens per minute (e.g., "834 tok/min")
    /// - "hour": Tokens per hour (e.g., "50.1K tok/hr")
    pub time_unit: String,

    /// Show session total token count (e.g., "150K")
    ///
    /// When true, shows aggregate tokens for current session.
    /// Overridden by format if format specifies session display.
    pub show_session_total: bool,

    /// Show daily total token count (e.g., "(day: 2.5M)")
    ///
    /// When true, shows aggregate tokens for today across all sessions.
    /// Similar to how cost shows "(day: $X.XX)".
    pub show_daily_total: bool,

    /// Override theme color (empty = use theme)
    pub color: String,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            preset: "default".to_string(),
            format: String::new(),               // Empty = use preset
            separator: " \u{2022} ".to_string(), // " • "
            components: ComponentsConfig::default(),
            show_unknown_vars: true,
        }
    }
}

impl Default for DirectoryComponentConfig {
    fn default() -> Self {
        Self {
            format: "short".to_string(),
            max_length: 0,
            color: String::new(),
        }
    }
}

impl Default for GitComponentConfig {
    fn default() -> Self {
        Self {
            format: "full".to_string(),
            show_when: "always".to_string(),
            color: String::new(),
        }
    }
}

impl Default for ContextComponentConfig {
    fn default() -> Self {
        Self {
            format: "full".to_string(),
            bar_width: None,
            show_tokens: true,
        }
    }
}

impl Default for CostComponentConfig {
    fn default() -> Self {
        Self {
            format: "full".to_string(),
            color: String::new(),
        }
    }
}

impl Default for ModelComponentConfig {
    fn default() -> Self {
        Self {
            format: "abbreviation".to_string(),
            color: String::new(),
        }
    }
}

impl Default for TokenRateComponentConfig {
    fn default() -> Self {
        Self {
            format: "rate_only".to_string(),
            time_unit: "second".to_string(),
            show_session_total: false,
            show_daily_total: false,
            color: String::new(),
        }
    }
}

/// Token rate metrics configuration
///
/// Controls display of token usage rates in tokens per second (tok/s).
///
/// **Available Display Modes:**
/// - **"summary"**: Single total token rate (e.g., "13.9 tok/s")
/// - **"detailed"**: Breakdown by token type (e.g., "In:5.2 Out:8.7 tok/s • Cache:85%")
/// - **"cache_only"**: Cache-focused view (e.g., "Cache:85% (12x ROI) • 41.7 tok/s")
///
/// **Duration Modes:**
/// By default, token rates inherit the duration mode from burn_rate.mode:
/// - **"wall_clock"**: Total elapsed time (includes idle periods)
/// - **"active_time"**: Only active conversation time (excludes gaps)
/// - **"auto_reset"**: Resets after inactivity threshold
///
/// **Example Calculations:**
/// - Input tokens: 18,750 / 3600s = 5.2 tok/s
/// - Output tokens: 31,250 / 3600s = 8.7 tok/s
/// - Cache read: 150,000 / 3600s = 41.7 tok/s
/// - Total tokens: 50,000 / 3600s = 13.9 tok/s
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenRateConfig {
    /// Enable token rate metrics display
    ///
    /// **Default: false (opt-in feature)**
    ///
    /// When enabled, shows token usage rates alongside burn rate.
    /// Useful for understanding token consumption patterns during intensive sessions.
    pub enabled: bool,

    /// Token rate display mode
    ///
    /// **Default: "summary"**
    ///
    /// Options:
    /// - **"summary"**: Simple total rate (e.g., "13.9 tok/s")
    /// - **"detailed"**: Token type breakdown (e.g., "In:5.2 Out:8.7 tok/s • Cache:85%")
    /// - **"cache_only"**: Cache-focused (e.g., "Cache:85% (12x ROI) • 41.7 tok/s")
    pub display_mode: String,

    /// Show cache efficiency metrics (hit ratio, ROI)
    ///
    /// **Default: true**
    ///
    /// When enabled, displays cache hit ratio and return on investment (ROI).
    /// Only shown in "detailed" and "cache_only" modes.
    ///
    /// Example: "Cache:85% (12x ROI)" means 85% cache hits with 12x token savings.
    pub cache_metrics: bool,

    /// Inherit duration mode from burn_rate configuration
    ///
    /// **Default: true**
    ///
    /// When true, uses the same duration mode as burn_rate (wall_clock, active_time, or auto_reset).
    /// When false, always uses wall_clock mode for token rate calculations.
    ///
    /// Recommended to keep true for consistency between cost and token metrics.
    pub inherit_duration_mode: bool,

    /// Rolling window for rate calculation in seconds
    ///
    /// **Default: 0 (disabled, uses session average)**
    ///
    /// When set to a positive value (e.g., 60, 120), calculates token rate based on
    /// messages within the last N seconds instead of the entire session average.
    ///
    /// This makes the displayed rate more responsive to current activity:
    /// - 0: Session average (total_tokens / session_duration) - stable but slow to react
    /// - 60: Last minute of activity - responsive to current pace
    /// - 120: Last 2 minutes - balance between responsiveness and stability
    ///
    /// Note: Daily totals remain accurate (from database); only the displayed rate changes.
    pub rate_window_seconds: u64,

    /// Which token rates to display
    ///
    /// **Default: "both"**
    ///
    /// Options:
    /// - **"both"**: Show both input and output rates (e.g., "In:5.2K Out:8.7K tok/s")
    /// - **"output_only"**: Show only output rate (e.g., "Out:8.7K tok/s")
    /// - **"input_only"**: Show only input rate (e.g., "In:5.2K tok/s")
    ///
    /// Useful when you only care about generation speed (output) or context size (input).
    pub rate_display: String,
}

/// Semantic rules for `[token_rate]` are filled in by plan 12-05.
impl Validate for TokenRateConfig {}

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

/// Semantic rules for `[sync]` (plan 12-05). Exists only under
/// `--features turso-sync`, the only build where `SyncConfig` is a field.
#[cfg(feature = "turso-sync")]
impl Validate for SyncConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        // The struct's own doc says turso is the only supported provider.
        check_enum(cx, "provider", &self.provider, &["turso"], report);

        check_in_range(
            cx,
            "soft_quota_fraction",
            self.soft_quota_fraction,
            0.0,
            1.0,
            report,
        );

        if self.sync_interval_seconds == 0 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("sync_interval_seconds"),
                "`sync_interval_seconds` of 0 removes every interval guard between sync attempts",
            );
        }

        let turso = cx.child("turso");
        if self.enabled && self.turso.database_url.is_empty() {
            report.warn(
                FindingKind::InvalidValue,
                turso.key("database_url"),
                "sync is enabled but `database_url` is empty; nothing can be synced",
            );
        }
        // `turso.auth_token` deliberately has NO rule: it is a credential, and
        // no finding may carry it or reference its content. Pinned by
        // `validate_sync_never_echoes_auth_token`.
    }
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
            // Token counts opt-in (new feature, default off for minimal statusline)
            show_context_tokens: false,
            // Rate limits opt-in (Pro/Max only; default off to keep output unchanged)
            show_rate_limits: false,
            // Reset countdown opt-in (default off; *_reset template vars always available)
            rate_limit_reset_countdown: false,
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
            buffer_size: 40_000,                // Claude Code reserves ~40-45K tokens for responses
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
            busy_timeout_ms: 10000,
            path: "stats.db".to_string(),
            json_backup: false, // v3.0.0+: JSON writes removed; default is now false (D-03)
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
                max_attempts: 5,
                initial_delay_ms: 50,
                max_delay_ms: 2000,
                backoff_factor: 1.5,
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
        // Increased from 50 to 500 for better token accumulation in long sessions
        // Each line is ~2KB, so 500 lines ≈ 1MB memory usage (acceptable for statusline)
        // For sessions with >500 messages, MAX(new, old) in DB preserves cumulative totals
        TranscriptConfig { buffer_lines: 500 }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        GitConfig {
            timeout_ms: 200, // 200ms default timeout for git operations
        }
    }
}

fn default_min_duration_seconds() -> u64 {
    60
}

impl Default for BurnRateConfig {
    fn default() -> Self {
        BurnRateConfig {
            mode: "wall_clock".to_string(), // Default to wall_clock for backward compatibility
            inactivity_threshold_minutes: 60, // 1 hour default
            min_duration_seconds: default_min_duration_seconds(), // 60 seconds minimum
        }
    }
}

impl Default for TokenRateConfig {
    fn default() -> Self {
        TokenRateConfig {
            enabled: false,                      // Opt-in feature, disabled by default
            display_mode: "summary".to_string(), // Simple display mode by default
            cache_metrics: true,                 // Show cache efficiency by default
            inherit_duration_mode: true,         // Use burn_rate.mode for consistency
            rate_window_seconds: 0,              // 0 = use session average (disabled)
            rate_display: "both".to_string(),    // Show both input and output rates
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

// ---------------------------------------------------------------------------
// Config source resolution: ONE candidate list, TWO traversals
// ---------------------------------------------------------------------------

/// Process-global count of `exists()` evaluations performed by
/// [`Config::find_config_file`].
///
/// **Observability instrumentation, NOT a supported API** — the same shape and
/// the same caveat as `crate::pricing::cache::price_cache_reads`. It exists so
/// the render path's short-circuit (stop at the FIRST existing candidate) is an
/// OBSERVED fact rather than a claim about the source's shape: a regression that
/// routed `find_config_file` through the diagnostic resolver would make the cold
/// config load pay four filesystem probes, one of which may sit on a slow or
/// unavailable mount (T-12-54). It is compiled into every profile on purpose.
static CONFIG_EXISTS_PROBES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Number of `exists()` probes [`Config::find_config_file`] has made this
/// process. See [`CONFIG_EXISTS_PROBES`]; not a supported API.
// Genuinely uncalled by the BINARY in Wave 1: read from the library's own tests
// (and from `src/commands/config.rs` once plan 12-09 lands).
#[allow(dead_code)]
#[doc(hidden)]
pub fn config_exists_probes() -> usize {
    CONFIG_EXISTS_PROBES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset the probe counter. See [`CONFIG_EXISTS_PROBES`]; not a supported API.
#[allow(dead_code)]
#[doc(hidden)]
pub fn reset_config_exists_probes() {
    CONFIG_EXISTS_PROBES.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// One entry of the config search order, with its hit/miss verdict (D-08).
// Constructed only by `resolve_config_source`; the BINARY gains a caller in 12-08/12-09.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigCandidate {
    /// Stable machine token: `env_statusline_config_path`,
    /// `env_statusline_config`, `xdg_config_dir`, `home_dotfile`.
    pub source: &'static str,
    /// The path this candidate resolves to, or `None` when it cannot be
    /// computed at all (the env var is unset, or `dirs::home_dir()` is `None`).
    pub path: Option<PathBuf>,
    /// Whether that path exists. Always `false` for an uncomputable candidate.
    pub exists: bool,
}

/// The full, walked search order plus the winner (D-08).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    /// Every candidate, in search order, each carrying a computed `exists`.
    pub candidates: Vec<ConfigCandidate>,
    /// The first existing candidate — the file `Config::load()` would read.
    pub active: Option<PathBuf>,
    /// That candidate's `source` token.
    pub active_source: Option<&'static str>,
}

/// The SINGLE definition of the config search order — pure path construction.
///
/// It performs **no filesystem access**: no `exists`, no `metadata`, no read.
/// Both traversals consume it, which is what keeps them from drifting apart
/// (T-12-15), while leaving each free to decide how far to walk.
///
/// On macOS `crate::common::get_config_dir()` resolves through
/// `dirs::config_dir()` to `$HOME/Library/Application Support/...`, NOT
/// `$HOME/.config/...`. This list reports what the code actually does; D-10's
/// misplaced-config warning is what covers the difference.
fn config_candidate_paths() -> Vec<(&'static str, Option<PathBuf>)> {
    vec![
        // 1. Environment variable set by the `--config` CLI flag.
        (
            "env_statusline_config_path",
            env::var("STATUSLINE_CONFIG_PATH").ok().map(PathBuf::from),
        ),
        // 2. Environment variable.
        (
            "env_statusline_config",
            env::var("STATUSLINE_CONFIG").ok().map(PathBuf::from),
        ),
        // 3. XDG / platform config directory.
        (
            "xdg_config_dir",
            Some(crate::common::get_config_dir().join("config.toml")),
        ),
        // 4. Home directory dotfile.
        (
            "home_dotfile",
            dirs::home_dir().map(|home| home.join(".claudia-statusline.toml")),
        ),
    ]
}

/// DIAGNOSTIC config-source resolution: walk the WHOLE candidate list.
///
/// Every candidate gets an `exists` verdict, so `config path` (plan 12-08) and
/// `config validate` (plan 12-09) can show hit/miss for each. This is
/// deliberately NOT what the render path uses — see [`CONFIG_EXISTS_PROBES`].
#[allow(dead_code)]
pub fn resolve_config_source() -> ResolvedConfig {
    let mut candidates = Vec::new();
    let mut active: Option<PathBuf> = None;
    let mut active_source: Option<&'static str> = None;

    for (source, path) in config_candidate_paths() {
        let exists = path.as_ref().map(|p| p.exists()).unwrap_or(false);
        if exists && active.is_none() {
            active = path.clone();
            active_source = Some(source);
        }
        candidates.push(ConfigCandidate {
            source,
            path,
            exists,
        });
    }

    ResolvedConfig {
        candidates,
        active,
        active_source,
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

        // The `toml` diagnostic is REDACTED before it reaches this message.
        // `Display`ing the error echoes the offending source line verbatim AND
        // quotes the offending value inline, and this message reaches the RENDER
        // path at the default log level via `build_config`'s `warn!` — so an
        // `[ant.accounts.*] admin_key_command` typo used to print an API key.
        // `sanitize_for_terminal` is not a mitigation: it preserves printable
        // bytes. See `crate::config_validation::redact_toml_error` (T-12-53).
        let config: Config = toml::from_str(&contents).map_err(|e| {
            StatuslineError::Config(format!(
                "Failed to parse config file: {}",
                crate::config_validation::redact_toml_error(&contents, &e)
            ))
        })?;

        Ok(config)
    }

    /// Save configuration to file
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        let toml_string = toml::to_string_pretty(self)
            .map_err(|e| StatuslineError::Config(format!("Failed to serialize config: {}", e)))?;

        // Ensure parent directory exists with secure permissions (0o700 on Unix)
        if let Some(parent) = path.parent() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(parent)
                    .map_err(|e| {
                        StatuslineError::Config(format!("Failed to create config directory: {}", e))
                    })?;
            }

            #[cfg(not(unix))]
            {
                fs::create_dir_all(parent).map_err(|e| {
                    StatuslineError::Config(format!("Failed to create config directory: {}", e))
                })?;
            }
        }

        // Write config file with secure permissions (0o600 on Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .map_err(|e| {
                    StatuslineError::Config(format!("Failed to write config file: {}", e))
                })?;
            std::io::Write::write_all(&mut file, toml_string.as_bytes()).map_err(|e| {
                StatuslineError::Config(format!("Failed to write config file: {}", e))
            })?;
        }

        #[cfg(not(unix))]
        {
            fs::write(path, toml_string).map_err(|e| {
                StatuslineError::Config(format!("Failed to write config file: {}", e))
            })?;
        }

        Ok(())
    }

    /// Find config file in standard locations — the RENDER path's traversal.
    ///
    /// Shares the candidate LIST with [`resolve_config_source`] but NOT the
    /// traversal: this returns on the FIRST candidate that exists, exactly as it
    /// did before the diagnostic resolver existed, so a cold config load still
    /// costs one `exists()` probe when `STATUSLINE_CONFIG_PATH` wins. It must
    /// never call `resolve_config_source`, which walks all four (T-12-54); the
    /// difference is observed by `config_exists_probes`, not asserted.
    fn find_config_file() -> Option<PathBuf> {
        for (_source, candidate) in config_candidate_paths() {
            let Some(path) = candidate else {
                continue;
            };
            CONFIG_EXISTS_PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if path.exists() {
                return Some(path);
            }
        }

        None
    }

    /// Get default config file path (for creating new config)
    pub fn default_config_path() -> Result<PathBuf> {
        let config_dir = crate::common::get_config_dir();
        Ok(config_dir.join("config.toml"))
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

# Component visibility toggles (all default to true except show_context_tokens)
# show_directory = true
# show_git = true
# show_context = true
# show_model = true
# show_duration = true
# show_lines_changed = true
# show_cost = true
# show_rate_limits = false           # Pro/Max only: "5h:24% 7d:41%" (absent for API keys)
# rate_limit_reset_countdown = false # Append reset countdown: "5h:24% (2h13m)"

# Show token counts in context bar (e.g., "179k/1000k")
# show_context_tokens = false

[context]
# Default context window size in tokens (fallback for unknown models)
# Auto-detection: Sonnet 4.5 (1M context) uses 1M, Sonnet 3.5+/4.5/Opus 3.5+ use 200k
# This fallback is used when model-specific detection fails
window_size = 200000

# Model-specific context windows (optional overrides)
# The statusline intelligently detects context window size based on model family/version
# and display name patterns (e.g., "(1M context)" suffix)
# You can override detection here for specific models by display name
# [context.model_windows]
# "Claude 3.5 Sonnet" = 200000
# "Claude Sonnet 4.5" = 200000
# "Sonnet 4.5 (1M context)" = 1000000  # Auto-detected, override not needed
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
busy_timeout_ms = 10000
path = "stats.db"  # Relative to data directory

# Data retention settings (for db-maintain command)
retention_days_sessions = 90    # Keep session data for N days
retention_days_daily = 365      # Keep daily aggregates for N days
retention_days_monthly = 0      # Keep monthly aggregates for N days (0 = forever)

[transcript]
# Number of transcript lines to keep in memory (circular buffer)
# For large files, only the last N lines are read (tail-reading optimization)
buffer_lines = 50

[retry.file_ops]
# File operation retry settings (tuned for concurrent access)
max_attempts = 5
initial_delay_ms = 50
max_delay_ms = 2000
backoff_factor = 1.5

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

[burn_rate]
# Burn rate calculation mode
# Options: "wall_clock", "active_time", or "auto_reset"
#
# - "wall_clock" (default): Uses total elapsed time from session start to last update
#   Simple and backward compatible. Includes idle time (nights, weekends).
#   Example: $8.99 over 22 days = $0.02/hour
#
# - "active_time": Tracks only active conversation time (excludes idle periods)
#   More accurate representation of actual usage cost.
#   Example: $8.99 over 2 hours active = $4.50/hour
#
# - "auto_reset": Automatically starts new sessions after inactivity
#   Each session gets independent cost/duration tracking.
#   Best for realistic burn rate tracking.
mode = "wall_clock"

# Inactivity threshold in minutes (used by "active_time" and "auto_reset" modes)
# Default: 60 minutes (1 hour)
inactivity_threshold_minutes = 60

# Minimum session duration (seconds) before showing burn rate
# Sessions shorter than this produce unreliable $/hr estimates
# Default: 60 seconds
# min_duration_seconds = 60

[token_rate]
# Enable token rate metrics display (tokens per second)
# Default: false (opt-in feature)
enabled = false

# Display mode: "summary", "detailed", or "cache_only"
# - "summary": Simple total rate (e.g., "13.9 tok/s")
# - "detailed": Token type breakdown (e.g., "In:5.2 Out:8.7 tok/s • Cache:85%")
# - "cache_only": Cache-focused (e.g., "Cache:85% (12x ROI) • 41.7 tok/s")
# Default: "summary"
display_mode = "summary"

# Show cache efficiency metrics (hit ratio, ROI)
# Default: true
cache_metrics = true

# Inherit duration mode from burn_rate configuration
# When true, uses same duration mode as burn_rate (wall_clock, active_time, auto_reset)
# When false, always uses wall_clock mode for token rate calculations
# Default: true (recommended for consistency)
inherit_duration_mode = true

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
use std::sync::{OnceLock, RwLock};

/// Cached global configuration.
///
/// Stored as a leaked `&'static Config` so callers keep the ergonomic `&'static`
/// return type. A `RwLock` (rather than `OnceLock`) backs it so tests can reset the
/// cache via [`reset_config`]: the env vars consulted in [`build_config`] are read once
/// at first access, and without a reset an env var set by an earlier test (or left over
/// from one) would be frozen in for the rest of the process, polluting later
/// config-dependent assertions (issue #34).
static CONFIG: RwLock<Option<&'static Config>> = RwLock::new(None);

/// Get the global configuration instance.
///
/// Built once on first call (from the config file plus environment overrides) and cached
/// for the lifetime of the process. Tests may call [`reset_config`] to force a rebuild.
pub fn get_config() -> &'static Config {
    // Fast path: already initialized.
    if let Some(cfg) = *CONFIG.read().expect("config lock poisoned") {
        return cfg;
    }
    // Slow path: build once and cache. Re-check under the write lock in case another
    // thread initialized while we were waiting for it.
    let mut guard = CONFIG.write().expect("config lock poisoned");
    if let Some(cfg) = *guard {
        return cfg;
    }
    let leaked: &'static Config = Box::leak(Box::new(build_config()));
    *guard = Some(leaked);
    leaked
}

/// Reset the cached configuration so the next [`get_config`] rebuilds from the current
/// environment and config file.
///
/// Exists so tests can obtain deterministic, env-dependent config despite the
/// process-global cache; production code never calls it. Resetting leaks the previously
/// cached `Config`, which is harmless for the bounded number of resets in a test run.
#[doc(hidden)]
#[allow(dead_code)] // Used by lib/integration tests; not called from the binary.
pub fn reset_config() {
    *CONFIG.write().expect("config lock poisoned") = None;
}

/// Build a fully-resolved [`Config`] from the config file plus environment overrides.
fn build_config() -> Config {
    {
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

        // Override show_context_tokens from environment if set (for testing)
        if let Ok(val) = env::var("STATUSLINE_SHOW_CONTEXT_TOKENS") {
            config.display.show_context_tokens = val == "true" || val == "1";
        }

        // Override burn_rate.mode from environment if set (for testing)
        if let Ok(mode) = env::var("STATUSLINE_BURN_RATE_MODE") {
            config.burn_rate.mode = mode;
        }

        // Override burn_rate.inactivity_threshold_minutes from environment if set (for testing)
        if let Ok(val) = env::var("STATUSLINE_BURN_RATE_THRESHOLD") {
            if let Ok(threshold) = val.parse::<u32>() {
                config.burn_rate.inactivity_threshold_minutes = threshold;
            }
        }

        // Override burn_rate.min_duration_seconds from environment if set (for testing)
        if let Ok(val) = env::var("STATUSLINE_BURN_RATE_MIN_DURATION") {
            if let Ok(seconds) = val.parse::<u64>() {
                config.burn_rate.min_duration_seconds = seconds;
            }
        }

        // Override token_rate.enabled from environment if set (for testing)
        if let Ok(val) = env::var("STATUSLINE_TOKEN_RATE_ENABLED") {
            config.token_rate.enabled = val == "true" || val == "1";
        }

        // Override token_rate.display_mode from environment if set (for testing)
        if let Ok(mode) = env::var("STATUSLINE_TOKEN_RATE_MODE") {
            config.token_rate.display_mode = mode;
        }

        // Override token_rate.cache_metrics from environment if set (for testing)
        if let Ok(val) = env::var("STATUSLINE_TOKEN_RATE_CACHE_METRICS") {
            config.token_rate.cache_metrics = val == "true" || val == "1";
        }

        // Override token_rate.inherit_duration_mode from environment if set (for testing)
        if let Ok(val) = env::var("STATUSLINE_TOKEN_RATE_INHERIT_DURATION") {
            config.token_rate.inherit_duration_mode = val == "true" || val == "1";
        }

        // v3.0.0+: emit a one-line stderr deprecation note (once per process) when a
        // legacy `json_backup = true` is detected. This is the canonical fire-site:
        // the fully-resolved Config is finalized here, once per process.
        warn_json_backup_legacy_if_set(&config);

        config
    }
}

static JSON_BACKUP_DEPRECATION_WARNED: OnceLock<()> = OnceLock::new();

/// Emits a one-line stderr deprecation note when a legacy `json_backup = true`
/// is detected. Fires at most once per process. Per D-01/D-02 (CONTEXT.md):
/// json_backup is ignored legacy in v3.0.0 — this is informational only,
/// the binary continues rendering normally.
fn warn_json_backup_legacy_if_set(cfg: &Config) {
    if cfg.database.json_backup {
        JSON_BACKUP_DEPRECATION_WARNED.get_or_init(|| {
            eprintln!(
                "note: 'json_backup' is ignored in v3.0.0 (writes removed); see MIGRATION_GUIDE.md"
            );
        });
    }
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
    #[serial_test::serial]
    fn test_get_config_reset_picks_up_env_changes() {
        // Proves the anti-pollution guarantee from issue #34: the process-global config
        // cache must not freeze env-derived values across a reset.
        let original_theme = env::var("STATUSLINE_THEME").ok();
        let original_claude_theme = env::var("CLAUDE_THEME").ok();
        // CLAUDE_THEME takes precedence over STATUSLINE_THEME in build_config; clear it so
        // this test controls the resolved theme deterministically.
        env::remove_var("CLAUDE_THEME");

        env::set_var("STATUSLINE_THEME", "dark");
        reset_config();
        assert_eq!(
            get_config().display.theme,
            "dark",
            "config should reflect the environment after a reset"
        );

        // Without a reset the cache holds the previously built value.
        env::set_var("STATUSLINE_THEME", "light");
        assert_eq!(
            get_config().display.theme,
            "dark",
            "cached config should not change until reset"
        );

        // After a reset it must pick up the new value (no stale cache pollution).
        reset_config();
        assert_eq!(
            get_config().display.theme,
            "light",
            "config must reflect the current environment after reset"
        );

        // Restore prior environment and rebuild so later tests see a clean cache.
        match original_theme {
            Some(v) => env::set_var("STATUSLINE_THEME", v),
            None => env::remove_var("STATUSLINE_THEME"),
        }
        if let Some(v) = original_claude_theme {
            env::set_var("CLAUDE_THEME", v);
        }
        reset_config();
    }

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
    fn test_database_config_default_json_backup_false() {
        // D-03: the v3.0.0 default for json_backup is false.
        let db = DatabaseConfig::default();
        assert!(
            !db.json_backup,
            "v3.0.0 default for json_backup must be false"
        );
    }

    #[test]
    fn test_config_serialization_omits_json_backup() {
        // FIX 3 / D-08: #[serde(skip_serializing)] on json_backup means the
        // serialized TOML (the same path Config::save() uses) must not contain it,
        // even when the in-memory value is true.
        let mut config = Config::default();
        config.database.json_backup = true;
        let serialized = toml::to_string_pretty(&config).expect("config should serialize");
        assert!(
            !serialized.contains("json_backup"),
            "serialized config must omit json_backup (skip_serializing).\n{}",
            serialized
        );
    }

    #[test]
    fn test_config_still_deserializes_legacy_json_backup() {
        // v2.x configs that still carry json_backup must parse (Deserialize retained).
        let toml = "[database]\njson_backup = true\n";
        let config: Config = toml::from_str(toml).expect("legacy config should parse");
        assert!(
            config.database.json_backup,
            "json_backup must still deserialize from legacy v2.x configs"
        );
    }

    #[test]
    fn test_absent_pricing_section_yields_default() {
        // An empty config (no [pricing] table) must parse to the default
        // PricingConfig: empty aliases + source = Auto (D-11, byte-identical
        // default render). Config carries #[serde(default)], so this holds.
        let config: Config = toml::from_str("").expect("empty config should parse");
        assert!(
            config.pricing.aliases.is_empty(),
            "absent [pricing] must yield empty aliases"
        );
        assert_eq!(
            config.pricing.source,
            crate::pricing::PricingSource::Auto,
            "absent [pricing] must yield Auto source"
        );
    }

    #[test]
    fn test_pricing_aliases_round_trip() {
        // A [pricing.aliases] table deserializes into the aliases map.
        let toml = "[pricing.aliases]\n\"my-proxy-opus\" = \"claude-opus-4-8\"\n";
        let config: Config = toml::from_str(toml).expect("pricing aliases should parse");
        assert_eq!(
            config
                .pricing
                .aliases
                .get("my-proxy-opus")
                .map(String::as_str),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn test_json_backup_legacy_warning_gates_to_once() {
        // FIX 6: prove the once-per-process gating semantics deterministically,
        // using a LOCAL OnceLock + counter (so the assertion does not race the
        // process-global static used by warn_json_backup_legacy_if_set, which may
        // already be initialized by other tests in the same binary).
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gate: OnceLock<()> = OnceLock::new();
        let init_count = AtomicUsize::new(0);

        // Mirror the gating shape: call the initializer N times against the same
        // OnceLock and prove the side effect fires exactly once.
        for _ in 0..5 {
            gate.get_or_init(|| {
                init_count.fetch_add(1, Ordering::SeqCst);
            });
        }

        assert!(gate.get().is_some(), "gate should be initialized");
        assert_eq!(
            init_count.load(Ordering::SeqCst),
            1,
            "OnceLock initializer must run exactly once across N calls"
        );

        // And the production helper must not panic / re-init when called repeatedly
        // with json_backup = true within this process.
        let mut cfg = Config::default();
        cfg.database.json_backup = true;
        warn_json_backup_legacy_if_set(&cfg);
        warn_json_backup_legacy_if_set(&cfg);
        warn_json_backup_legacy_if_set(&cfg);
        assert!(
            JSON_BACKUP_DEPRECATION_WARNED.get().is_some(),
            "process-global gate should be initialized after a true config"
        );
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

    // -----------------------------------------------------------------------
    // Config source resolution (plan 12-04 Task 2)
    // -----------------------------------------------------------------------

    /// Panic-safe save/restore for the environment variables the resolver reads.
    struct ResolverEnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl ResolverEnvGuard {
        const KEYS: [&'static str; 4] = [
            "STATUSLINE_CONFIG_PATH",
            "STATUSLINE_CONFIG",
            "XDG_CONFIG_HOME",
            "HOME",
        ];

        fn new() -> Self {
            Self {
                saved: Self::KEYS.iter().map(|k| (*k, env::var(k).ok())).collect(),
            }
        }

        fn set(&self, key: &str, value: &Path) {
            env::set_var(key, value);
        }

        fn unset(&self, key: &str) {
            env::remove_var(key);
        }
    }

    impl Drop for ResolverEnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(v) => env::set_var(key, v),
                    None => env::remove_var(key),
                }
            }
        }
    }

    /// Fixture layout: one file per candidate, each with a DISTINCT
    /// `display.progress_bar_width` so the winner is identifiable from the
    /// loaded `Config` alone.
    struct ResolverFixture {
        _dir: TempDir,
        env_path_file: PathBuf,
        env_config_file: PathBuf,
        xdg_home: PathBuf,
        xdg_file: PathBuf,
        fake_home: PathBuf,
        home_file: PathBuf,
    }

    impl ResolverFixture {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let root = dir.path().to_path_buf();

            let env_path_file = root.join("from_env_path.toml");
            std::fs::write(&env_path_file, "[display]\nprogress_bar_width = 11\n").unwrap();

            let env_config_file = root.join("from_env_config.toml");
            std::fs::write(&env_config_file, "[display]\nprogress_bar_width = 12\n").unwrap();

            let xdg_home = root.join("xdg");
            let xdg_dir = xdg_home.join("claudia-statusline");
            std::fs::create_dir_all(&xdg_dir).unwrap();
            let xdg_file = xdg_dir.join("config.toml");
            std::fs::write(&xdg_file, "[display]\nprogress_bar_width = 13\n").unwrap();

            let fake_home = root.join("home");
            std::fs::create_dir_all(&fake_home).unwrap();
            let home_file = fake_home.join(".claudia-statusline.toml");
            std::fs::write(&home_file, "[display]\nprogress_bar_width = 14\n").unwrap();

            Self {
                _dir: dir,
                env_path_file,
                env_config_file,
                xdg_home,
                xdg_file,
                fake_home,
                home_file,
            }
        }

        /// Point all four candidates at this fixture's files.
        fn point_all(&self, guard: &ResolverEnvGuard) {
            guard.set("STATUSLINE_CONFIG_PATH", &self.env_path_file);
            guard.set("STATUSLINE_CONFIG", &self.env_config_file);
            guard.set("XDG_CONFIG_HOME", &self.xdg_home);
            guard.set("HOME", &self.fake_home);
        }
    }

    #[test]
    #[serial_test::serial]
    fn resolver_reports_all_four_candidates_in_order() {
        let guard = ResolverEnvGuard::new();
        let fixture = ResolverFixture::new();
        fixture.point_all(&guard);

        let resolved = resolve_config_source();
        assert_eq!(
            resolved.candidates.len(),
            4,
            "the search order has exactly four candidates"
        );
        let tokens: Vec<&str> = resolved.candidates.iter().map(|c| c.source).collect();
        assert_eq!(
            tokens,
            vec![
                "env_statusline_config_path",
                "env_statusline_config",
                "xdg_config_dir",
                "home_dotfile",
            ],
            "source tokens must appear in the documented search order"
        );
    }

    #[test]
    #[serial_test::serial]
    fn resolver_active_matches_independently_expected_winner() {
        let guard = ResolverEnvGuard::new();
        let fixture = ResolverFixture::new();

        // Each row carries its expected winner LITERALLY. The oracle is the row,
        // never the other traversal — comparing the resolver against
        // `find_config_file` would be a wrapper checked against its own delegate.
        //
        // (label, setup, expected active path, expected progress_bar_width)
        type Row = (&'static str, fn(&ResolverEnvGuard, &ResolverFixture), usize);
        let rows: Vec<Row> = vec![
            (
                "env_statusline_config_path wins over everything",
                |g, f| f.point_all(g),
                11,
            ),
            (
                "env_statusline_config wins when candidate 1 is unset",
                |g, f| {
                    f.point_all(g);
                    g.unset("STATUSLINE_CONFIG_PATH");
                },
                12,
            ),
            (
                "xdg_config_dir wins when neither env var is set",
                |g, f| {
                    f.point_all(g);
                    g.unset("STATUSLINE_CONFIG_PATH");
                    g.unset("STATUSLINE_CONFIG");
                },
                13,
            ),
            (
                "home_dotfile wins when the xdg file is absent",
                |g, f| {
                    f.point_all(g);
                    g.unset("STATUSLINE_CONFIG_PATH");
                    g.unset("STATUSLINE_CONFIG");
                    let _ = std::fs::remove_file(&f.xdg_file);
                },
                14,
            ),
        ];

        let expected_paths: Vec<PathBuf> = vec![
            fixture.env_path_file.clone(),
            fixture.env_config_file.clone(),
            fixture.xdg_file.clone(),
            fixture.home_file.clone(),
        ];

        for ((label, setup, expected_width), expected_path) in rows.into_iter().zip(expected_paths)
        {
            setup(&guard, &fixture);

            let resolved = resolve_config_source();
            assert_eq!(
                resolved.active.as_deref(),
                Some(expected_path.as_path()),
                "{label}: resolver picked the wrong active config"
            );

            // Independent cross-check: the file `Config::load()` ACTUALLY read,
            // observed through a value unique to that fixture file.
            let loaded = Config::load().expect("fixture configs must load");
            assert_eq!(
                loaded.display.progress_bar_width, expected_width,
                "{label}: Config::load() did not read the resolver's active file"
            );
        }

        // Total miss: every candidate computable, none existing.
        let empty = TempDir::new().unwrap();
        guard.set("STATUSLINE_CONFIG_PATH", &empty.path().join("nope1.toml"));
        guard.set("STATUSLINE_CONFIG", &empty.path().join("nope2.toml"));
        guard.set("XDG_CONFIG_HOME", empty.path());
        guard.set("HOME", empty.path());
        let resolved = resolve_config_source();
        assert_eq!(
            resolved.active, None,
            "total miss must resolve to no config"
        );
        assert_eq!(resolved.active_source, None);
        assert_eq!(
            Config::load().unwrap().display.progress_bar_width,
            Config::default().display.progress_bar_width,
            "a total miss must load defaults"
        );
    }

    #[test]
    #[serial_test::serial]
    fn find_config_file_short_circuits_on_first_hit() {
        let guard = ResolverEnvGuard::new();
        let fixture = ResolverFixture::new();
        fixture.point_all(&guard);

        // Candidate 1 exists -> the render path must stop there.
        reset_config_exists_probes();
        let loaded = Config::load().expect("candidate 1 must load");
        assert_eq!(loaded.display.progress_bar_width, 11);
        assert_eq!(
            config_exists_probes(),
            1,
            "a first-candidate hit must cost exactly ONE exists() probe"
        );

        // No candidate exists, all four computable -> four probes, then defaults.
        let empty = TempDir::new().unwrap();
        guard.set("STATUSLINE_CONFIG_PATH", &empty.path().join("nope1.toml"));
        guard.set("STATUSLINE_CONFIG", &empty.path().join("nope2.toml"));
        guard.set("XDG_CONFIG_HOME", empty.path());
        guard.set("HOME", empty.path());
        reset_config_exists_probes();
        let _ = Config::load().expect("a total miss must fall back to defaults");
        assert_eq!(
            config_exists_probes(),
            4,
            "a total miss must probe every computable candidate"
        );

        // The DIAGNOSTIC traversal, with candidate 1 winning again, must still
        // evaluate all four — the other direction of the same difference.
        fixture.point_all(&guard);
        reset_config_exists_probes();
        let resolved = resolve_config_source();
        assert_eq!(
            resolved
                .candidates
                .iter()
                .filter(|c| c.path.is_some())
                .count(),
            4,
            "the resolver must compute every candidate path"
        );
        assert_eq!(resolved.active_source, Some("env_statusline_config_path"));
        // Later candidates carry a COMPUTED exists flag, not a default `false`:
        // candidate 3 exists in this fixture and the resolver says so, even
        // though candidate 1 already won.
        assert!(
            resolved.candidates[2].exists,
            "the resolver must evaluate candidates after the winner"
        );
        assert!(resolved.candidates[3].exists);
        assert_eq!(
            config_exists_probes(),
            0,
            "the diagnostic resolver must not charge the render path's probe counter"
        );
    }

    #[test]
    fn candidate_paths_perform_no_filesystem_access() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/config.rs"))
            .expect("read own source");

        let mut body = String::new();
        let mut inside = false;
        for line in source.lines() {
            if !inside {
                if line.starts_with("fn config_candidate_paths") {
                    inside = true;
                }
                continue;
            }
            if line == "}" {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
        assert!(
            !body.is_empty(),
            "failed to extract the body of fn config_candidate_paths"
        );

        for forbidden in ["exists(", "metadata(", "read_to_string(", "read_dir("] {
            assert!(
                !body.contains(forbidden),
                "config_candidate_paths must perform NO filesystem access, found `{forbidden}` in:\n{body}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Semantic validation rules (plan 12-05 Task 1)
    // -----------------------------------------------------------------------

    use crate::config_validation::{present_keys_for_section, Severity};

    /// Build a `SectionContext` over an inline document and run ONE section's
    /// rules. The document is the TARGET file, exactly as the engine supplies
    /// it, so cross-section lookups (`display.theme`) resolve from it.
    fn validate_section<V: Validate>(section: &V, name: &str, document: &str) -> Report {
        let doc: toml::Value = toml::from_str(document).expect("fixture document parses");
        let keys = present_keys_for_section(&doc, name);
        let cx = SectionContext::new(name, &keys, &doc);
        let mut report = Report::new();
        section.validate(&cx, &mut report);
        report
    }

    fn findings_for<'a>(
        report: &'a Report,
        key: &str,
    ) -> Vec<&'a crate::config_validation::Finding> {
        report.findings.iter().filter(|f| f.key == key).collect()
    }

    fn severities_at(report: &Report, key: &str) -> Vec<Severity> {
        findings_for(report, key)
            .iter()
            .map(|f| f.severity)
            .collect()
    }

    #[test]
    fn validate_display_rejects_unknown_theme_and_names_the_alternatives() {
        let display = DisplayConfig {
            theme: "nonexistent".to_string(),
            ..DisplayConfig::default()
        };
        let report = validate_section(&display, "display", "[display]\ntheme = \"nonexistent\"\n");

        let theme_findings = findings_for(&report, "display.theme");
        assert_eq!(
            theme_findings.len(),
            1,
            "exactly one finding for an unknown theme: {:?}",
            report.findings
        );
        assert_eq!(theme_findings[0].severity, Severity::Error);
        assert_eq!(theme_findings[0].kind, FindingKind::InvalidValue);
        let msg = &theme_findings[0].message;
        for name in ["dark", "light", "gruvbox"] {
            assert!(
                msg.contains(name),
                "the message must enumerate real theme names, missing `{name}`: {msg}"
            );
        }
    }

    #[test]
    fn validate_display_accepts_every_embedded_theme() {
        for name in crate::theme::Theme::embedded_themes() {
            let display = DisplayConfig {
                theme: name.to_string(),
                ..DisplayConfig::default()
            };
            let report = validate_section(&display, "display", "[display]\n");
            assert!(
                findings_for(&report, "display.theme").is_empty(),
                "embedded theme `{name}` must validate: {:?}",
                report.findings
            );
        }
    }

    #[test]
    fn validate_display_bar_width_and_threshold_order() {
        let display = DisplayConfig {
            progress_bar_width: 0,
            ..DisplayConfig::default()
        };
        let report = validate_section(&display, "display", "[display]\n");
        assert_eq!(
            severities_at(&report, "display.progress_bar_width"),
            vec![Severity::Error]
        );

        let wide = DisplayConfig {
            progress_bar_width: 400,
            ..DisplayConfig::default()
        };
        let report = validate_section(&wide, "display", "[display]\n");
        assert_eq!(
            severities_at(&report, "display.progress_bar_width"),
            vec![Severity::Warning]
        );

        // Out of range is an ERROR; merely out of ORDER is a warning.
        let out_of_range = DisplayConfig {
            context_warning_threshold: 140.0,
            ..DisplayConfig::default()
        };
        let report = validate_section(&out_of_range, "display", "[display]\n");
        assert_eq!(
            severities_at(&report, "display.context_warning_threshold"),
            vec![Severity::Error],
            "an out-of-range threshold must not ALSO produce the ordering warning: {:?}",
            report.findings
        );

        let unordered = DisplayConfig {
            context_caution_threshold: 95.0,
            ..DisplayConfig::default()
        };
        let report = validate_section(&unordered, "display", "[display]\n");
        assert_eq!(
            severities_at(&report, "display.context_warning_threshold"),
            vec![Severity::Warning]
        );
    }

    #[test]
    fn validate_display_default_is_clean() {
        let report = validate_section(&DisplayConfig::default(), "display", "[display]\n");
        assert!(
            report.findings.is_empty(),
            "the shipped default must validate cleanly: {:?}",
            report.findings
        );
    }

    #[test]
    fn validate_context_section_rules() {
        let context = ContextConfig {
            window_size: 0,
            learning_confidence_threshold: 1.7,
            auto_compact_threshold: -1.0,
            percentage_mode: "partial".to_string(),
            ..ContextConfig::default()
        };
        let report = validate_section(&context, "context", "[context]\n");

        for key in [
            "context.window_size",
            "context.learning_confidence_threshold",
            "context.auto_compact_threshold",
            "context.percentage_mode",
        ] {
            assert_eq!(
                severities_at(&report, key),
                vec![Severity::Error],
                "expected one error at {key}: {:?}",
                report.findings
            );
        }

        let mode = findings_for(&report, "context.percentage_mode")[0]
            .message
            .clone();
        assert!(
            mode.contains("full") && mode.contains("working"),
            "the enum message must name the legal set: {mode}"
        );

        // Cross-field: a buffer at or above the window leaves nothing usable.
        let crowded = ContextConfig {
            buffer_size: ContextConfig::default().window_size,
            ..ContextConfig::default()
        };
        let report = validate_section(&crowded, "context", "[context]\n");
        assert_eq!(
            severities_at(&report, "context.buffer_size"),
            vec![Severity::Warning]
        );
    }

    #[test]
    fn validate_context_model_windows_keys_are_never_findings() {
        let mut context = ContextConfig::default();
        context
            .model_windows
            .insert("Claude 3.5 Sonnet".to_string(), 200_000);
        context.model_windows.insert("zero-model".to_string(), 0);
        let report = validate_section(&context, "context", "[context]\n");

        assert!(
            report
                .findings
                .iter()
                .all(|f| !f.key.contains("Claude 3.5 Sonnet")),
            "a legitimate free-form model key must produce NO finding: {:?}",
            report.findings
        );
        assert_eq!(
            severities_at(&report, "context.model_windows.zero-model"),
            vec![Severity::Error],
            "but a zero VALUE is an error: {:?}",
            report.findings
        );
    }

    #[test]
    fn validate_cost_rejects_non_finite() {
        // TOML accepts `nan` and `inf` literals; a bare `v < 0.0` guard passes
        // both silently. Confirm the document shape is real, not hypothetical.
        let doc = "[cost]\nlow_threshold = nan\nmedium_threshold = inf\n";
        let parsed: CostConfig = toml::from_str::<toml::Value>(doc)
            .expect("nan/inf are valid TOML floats")
            .get("cost")
            .unwrap()
            .clone()
            .try_into()
            .expect("and deserialize into f64 fields");
        assert!(parsed.low_threshold.is_nan());
        assert!(parsed.medium_threshold.is_infinite());

        let report = validate_section(&parsed, "cost", doc);

        assert_eq!(
            severities_at(&report, "cost.low_threshold"),
            vec![Severity::Error],
            "exactly one error, and NO range-relation warning, for a non-finite value: {:?}",
            report.findings
        );
        assert_eq!(
            severities_at(&report, "cost.medium_threshold"),
            vec![Severity::Error],
            "{:?}",
            report.findings
        );
        assert_eq!(report.findings.len(), 2, "{:?}", report.findings);
        assert!(!report.has_warnings());
    }

    #[test]
    fn validate_cost_ordering_is_a_warning_not_an_error() {
        let cost = CostConfig {
            low_threshold: 5.0,
            medium_threshold: 1.0,
        };
        let report = validate_section(&cost, "cost", "[cost]\n");
        assert_eq!(
            severities_at(&report, "cost.low_threshold"),
            vec![Severity::Warning],
            "an inverted (but valid) pair is advisory only: {:?}",
            report.findings
        );
        assert!(!report.has_errors());

        let negative = CostConfig {
            low_threshold: -1.0,
            medium_threshold: 10.0,
        };
        let report = validate_section(&negative, "cost", "[cost]\n");
        assert_eq!(
            severities_at(&report, "cost.low_threshold"),
            vec![Severity::Error]
        );
    }

    #[test]
    fn validate_database_section_rules() {
        let db = DatabaseConfig {
            busy_timeout_ms: 0,
            json_backup: true,
            ..DatabaseConfig::default()
        };
        let report = validate_section(&db, "database", "[database]\n");
        assert_eq!(
            severities_at(&report, "database.busy_timeout_ms"),
            vec![Severity::Error]
        );
        assert_eq!(
            severities_at(&report, "database.json_backup"),
            vec![Severity::Warning],
            "json_backup is a real deserialized field, never an unknown key (D-09)"
        );

        // A relative path is resolved against the data directory at runtime and
        // must NOT be probed from the validating process's CWD.
        let relative = DatabaseConfig {
            path: "stats.db".to_string(),
            ..DatabaseConfig::default()
        };
        let report = validate_section(&relative, "database", "[database]\n");
        assert!(
            findings_for(&report, "database.path").is_empty(),
            "a relative path must not be a false positive: {:?}",
            report.findings
        );

        let absent = std::env::temp_dir().join("claudia-statusline-no-such-dir-12-05");
        assert!(!absent.exists(), "fixture precondition");
        let missing_parent = DatabaseConfig {
            path: absent.join("stats.db").to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        };
        let report = validate_section(&missing_parent, "database", "[database]\n");
        assert_eq!(
            severities_at(&report, "database.path"),
            vec![Severity::Warning]
        );
        assert!(
            !absent.exists(),
            "validation must STAT only — it may never create the directory"
        );
    }

    #[test]
    fn validate_retry_reports_nested_keys_per_block() {
        let mut retry = RetryConfig::default();
        retry.file_ops.max_attempts = 0;
        retry.db_ops.initial_delay_ms = 10_000;
        retry.db_ops.max_delay_ms = 100;
        retry.git_ops.backoff_factor = 0.5;
        retry.network_ops.max_attempts = 50;

        let report = validate_section(&retry, "retry", "[retry]\n");
        assert_eq!(
            severities_at(&report, "retry.file_ops.max_attempts"),
            vec![Severity::Error]
        );
        assert_eq!(
            severities_at(&report, "retry.db_ops.initial_delay_ms"),
            vec![Severity::Error]
        );
        assert_eq!(
            severities_at(&report, "retry.git_ops.backoff_factor"),
            vec![Severity::Error]
        );
        assert_eq!(
            severities_at(&report, "retry.network_ops.max_attempts"),
            vec![Severity::Warning]
        );

        let mut nonfinite = RetryConfig::default();
        nonfinite.file_ops.backoff_factor = f32::NAN;
        let report = validate_section(&nonfinite, "retry", "[retry]\n");
        assert_eq!(
            severities_at(&report, "retry.file_ops.backoff_factor"),
            vec![Severity::Error],
            "the finiteness gate covers the f32 field too: {:?}",
            report.findings
        );
    }

    #[test]
    fn validate_transcript_and_git_sections() {
        let report = validate_section(
            &TranscriptConfig { buffer_lines: 0 },
            "transcript",
            "[transcript]\n",
        );
        assert_eq!(
            severities_at(&report, "transcript.buffer_lines"),
            vec![Severity::Error]
        );

        let report = validate_section(&GitConfig { timeout_ms: 0 }, "git", "[git]\n");
        assert_eq!(
            severities_at(&report, "git.timeout_ms"),
            vec![Severity::Error]
        );

        let report = validate_section(&GitConfig { timeout_ms: 30_000 }, "git", "[git]\n");
        assert_eq!(
            severities_at(&report, "git.timeout_ms"),
            vec![Severity::Warning]
        );

        let report = validate_section(&GitConfig::default(), "git", "[git]\n");
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[cfg(feature = "turso-sync")]
    #[test]
    fn validate_sync_provider_and_quota() {
        let sync = SyncConfig {
            provider: "libsql".to_string(),
            soft_quota_fraction: 1.5,
            ..SyncConfig::default()
        };
        let report = validate_section(&sync, "sync", "[sync]\n");

        assert_eq!(
            severities_at(&report, "sync.provider"),
            vec![Severity::Error],
            "{:?}",
            report.findings
        );
        assert!(
            findings_for(&report, "sync.provider")[0]
                .message
                .contains("turso"),
            "the message must name the only supported provider"
        );
        assert_eq!(
            severities_at(&report, "sync.soft_quota_fraction"),
            vec![Severity::Error],
            "{:?}",
            report.findings
        );

        let report = validate_section(&SyncConfig::default(), "sync", "[sync]\n");
        assert!(
            report.findings.is_empty(),
            "the shipped sync default must validate cleanly: {:?}",
            report.findings
        );
    }

    #[cfg(feature = "turso-sync")]
    #[test]
    fn validate_sync_never_echoes_auth_token() {
        let sync = SyncConfig {
            enabled: true,
            provider: "libsql".to_string(),
            soft_quota_fraction: 9.0,
            sync_interval_seconds: 0,
            turso: TursoConfig {
                auth_token: "sk-ant-SENTINEL-EEE".to_string(),
                ..TursoConfig::default()
            },
        };
        let report = validate_section(
            &sync,
            "sync",
            "[sync]\n[sync.turso]\nauth_token = \"sk-ant-SENTINEL-EEE\"\n",
        );

        assert!(
            !report.findings.is_empty(),
            "the fixture must actually produce findings"
        );
        for finding in &report.findings {
            assert!(
                !finding.key.contains("SENTINEL") && !finding.message.contains("SENTINEL"),
                "a credential must never reach a finding: {:?}",
                finding
            );
        }
        assert!(
            findings_for(&report, "sync.turso.auth_token").is_empty(),
            "`auth_token` has no rule at all: {:?}",
            report.findings
        );
    }
}
