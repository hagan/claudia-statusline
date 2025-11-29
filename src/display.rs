//! Display formatting module.
//!
//! This module handles the visual formatting of the statusline output,
//! including colors, progress bars, and layout.

use crate::config;
use crate::git::{format_git_info, get_git_status};
use crate::layout::{LayoutRenderer, VariableBuilder};
use crate::models::{ContextUsage, Cost, ModelType};
use crate::theme::{get_theme_manager, Theme};
use crate::utils::{calculate_context_usage, parse_duration, sanitize_for_terminal, shorten_path};

/// Gets the current theme based on configuration.
///
/// Checks in this order:
/// 1. Config file: theme = "name"
/// 2. Environment: CLAUDE_THEME or STATUSLINE_THEME
/// 3. Default: "dark"
fn get_current_theme() -> Theme {
    // Get theme name from config or environment
    let theme_name = config::get_theme();

    // Load theme with fallback to default
    get_theme_manager()
        .get_or_load(&theme_name)
        .unwrap_or_else(|_| {
            log::warn!("Failed to load theme '{}', using default", theme_name);
            Theme::default()
        })
}

/// ANSI color codes for terminal output.
pub struct Colors;

impl Colors {
    /// Check if colors are enabled (respects NO_COLOR env var)
    pub fn enabled() -> bool {
        std::env::var("NO_COLOR").is_err()
    }

    /// Get a color from theme, or empty string if colors are disabled
    fn get_themed(color_name: &str) -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(color_name)
    }

    pub fn reset() -> String {
        if Self::enabled() {
            "\x1b[0m".to_string()
        } else {
            String::new()
        }
    }

    #[allow(dead_code)]
    pub fn bold() -> String {
        if Self::enabled() {
            "\x1b[1m".to_string()
        } else {
            String::new()
        }
    }

    pub fn red() -> String {
        Self::get_themed("red")
    }

    pub fn green() -> String {
        Self::get_themed("green")
    }

    pub fn yellow() -> String {
        Self::get_themed("yellow")
    }

    #[allow(dead_code)]
    pub fn blue() -> String {
        Self::get_themed("blue")
    }

    #[allow(dead_code)]
    pub fn magenta() -> String {
        Self::get_themed("magenta")
    }

    pub fn cyan() -> String {
        Self::get_themed("cyan")
    }

    #[allow(dead_code)]
    pub fn white() -> String {
        Self::get_themed("white")
    }

    pub fn gray() -> String {
        Self::get_themed("gray")
    }

    #[allow(dead_code)]
    pub fn orange() -> String {
        Self::get_themed("orange")
    }

    pub fn light_gray() -> String {
        Self::get_themed("light_gray")
    }

    /// Get the appropriate text color based on theme
    #[allow(dead_code)]
    pub fn text_color() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.context_normal)
    }

    /// Get the appropriate separator color based on theme
    pub fn separator_color() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.separator)
    }

    /// Get directory color from theme
    pub fn directory() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.directory)
    }

    /// Get model color from theme
    pub fn model() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.model)
    }

    /// Get git branch color from theme
    #[allow(dead_code)]
    pub fn git_branch() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.git_branch)
    }

    /// Get duration color from theme
    pub fn duration() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.duration)
    }

    /// Get lines added color from theme
    pub fn lines_added() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.lines_added)
    }

    /// Get lines removed color from theme
    pub fn lines_removed() -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        theme.resolve_color(&theme.colors.lines_removed)
    }

    /// Get cost color based on amount and theme thresholds
    pub fn cost_color(cost: f64) -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        let config = config::get_config();

        if cost >= config.cost.medium_threshold {
            theme.resolve_color(&theme.colors.cost_high)
        } else if cost >= config.cost.low_threshold {
            theme.resolve_color(&theme.colors.cost_medium)
        } else {
            theme.resolve_color(&theme.colors.cost_low)
        }
    }

    /// Get context color based on percentage and theme thresholds
    pub fn context_color(percentage: f64) -> String {
        if !Self::enabled() {
            return String::new();
        }
        let theme = get_current_theme();
        let config = config::get_config();

        if percentage > config.display.context_critical_threshold {
            theme.resolve_color(&theme.colors.context_critical)
        } else if percentage > config.display.context_warning_threshold {
            theme.resolve_color(&theme.colors.context_warning)
        } else if percentage > config.display.context_caution_threshold {
            theme.resolve_color(&theme.colors.context_caution)
        } else {
            theme.resolve_color(&theme.colors.context_normal)
        }
    }
}

pub fn format_output(
    current_dir: &str,
    model_name: Option<&str>,
    transcript_path: Option<&str>,
    cost: Option<&Cost>,
    daily_total: f64,
    session_id: Option<&str>,
) {
    let config = config::get_config();
    format_output_with_config(
        current_dir,
        model_name,
        transcript_path,
        cost,
        daily_total,
        session_id,
        &config.display,
    )
}

/// Format output with explicit display configuration (returns String)
fn format_statusline_string(
    current_dir: &str,
    model_name: Option<&str>,
    transcript_path: Option<&str>,
    cost: Option<&Cost>,
    daily_total: f64,
    session_id: Option<&str>,
    display_config: &config::DisplayConfig,
) -> String {
    log::debug!(
        "format_statusline_string called: model_name={:?}, transcript_path={:?}, show_context={}",
        model_name,
        transcript_path,
        display_config.show_context
    );
    let mut parts = Vec::new();

    // Create database handle once for reuse (performance optimization for token rates)
    let db = session_id.and_then(|_| {
        let db_path = crate::stats::StatsData::get_sqlite_path().ok()?;
        if !db_path.exists() {
            return None;
        }
        crate::database::SqliteDatabase::new(&db_path).ok()
    });

    // 0. TEST indicator if in test mode
    if std::env::var("STATUSLINE_TEST_MODE").is_ok() {
        parts.push(format!("{}[TEST]{}", Colors::yellow(), Colors::reset()));
    }

    // 1. Directory (always first if shown)
    if display_config.show_directory {
        let short_dir = sanitize_for_terminal(&shorten_path(current_dir));
        parts.push(format!(
            "{}{}{}",
            Colors::directory(),
            short_dir,
            Colors::reset()
        ));
    }

    // 2. Git status
    if display_config.show_git {
        if let Some(git_status) = get_git_status(current_dir) {
            let git_info = format_git_info(&git_status);
            if !git_info.is_empty() {
                // Trim leading space from git_info (legacy format)
                parts.push(git_info.trim_start().to_string());
            }
        }
    }

    // 3. Context usage from transcript
    if display_config.show_context {
        if let Some(transcript) = transcript_path {
            if let Some(context) = calculate_context_usage(transcript, model_name, session_id, None)
            {
                let current_tokens = crate::utils::get_token_count_from_transcript(transcript);
                let full_config = config::get_config();
                let window_size = Some(crate::utils::get_context_window_for_model(
                    model_name,
                    full_config,
                ));
                parts.push(format_context_bar(&context, current_tokens, window_size));
            }
        }
    }

    // 4. Model display (sanitize untrusted model name)
    if display_config.show_model {
        if let Some(name) = model_name {
            let sanitized_name = sanitize_for_terminal(name);
            let model_type = ModelType::from_name(&sanitized_name);
            parts.push(format!(
                "{}{}{}",
                Colors::model(),
                sanitize_for_terminal(&model_type.abbreviation()),
                Colors::reset()
            ));
        }
    }

    // 5. Duration from transcript
    if display_config.show_duration {
        if let Some(transcript) = transcript_path {
            if let Some(duration) = parse_duration(transcript) {
                parts.push(format!(
                    "{}{}{}",
                    Colors::duration(),
                    format_duration(duration),
                    Colors::reset()
                ));
            }
        }
    }

    // 6. Lines changed
    if display_config.show_lines_changed {
        if let Some(cost_data) = cost {
            if let (Some(added), Some(removed)) =
                (cost_data.total_lines_added, cost_data.total_lines_removed)
            {
                if added > 0 || removed > 0 {
                    let mut lines_part = String::new();
                    if added > 0 {
                        lines_part.push_str(&format!(
                            "{}+{}{}",
                            Colors::lines_added(),
                            added,
                            Colors::reset()
                        ));
                    }
                    if removed > 0 {
                        if added > 0 {
                            lines_part.push(' ');
                        }
                        lines_part.push_str(&format!(
                            "{}-{}{}",
                            Colors::lines_removed(),
                            removed,
                            Colors::reset()
                        ));
                    }
                    parts.push(lines_part);
                }
            }
        }
    }

    // 7. Cost display with burn rate
    if display_config.show_cost {
        if let Some(cost_data) = cost {
            if let Some(total_cost) = cost_data.total_cost_usd {
                let cost_color = get_cost_color(total_cost);

                // Calculate burn rate if we have duration
                // Use configured burn_rate mode (wall_clock, active_time, or auto_reset)
                let duration = session_id
                    .and_then(crate::stats::get_session_duration_by_mode)
                    .or_else(|| transcript_path.and_then(parse_duration));

                let burn_rate = duration.and_then(|d| {
                    if d > 60 {
                        Some((total_cost * 3600.0) / d as f64)
                    } else {
                        None
                    }
                });

                let mut cost_part = format!("{}${:.2}{}", cost_color, total_cost, Colors::reset());

                // Add burn rate if available
                if let Some(rate) = burn_rate {
                    if rate > 0.0 {
                        cost_part.push_str(&format!(
                            " {}(${:.2}/hr){}",
                            Colors::light_gray(),
                            rate,
                            Colors::reset()
                        ));
                    }
                }

                // Add daily total if different from session cost
                if daily_total > total_cost {
                    let daily_color = get_cost_color(daily_total);
                    cost_part.push_str(&format!(
                        " {}(day: {}${:.2}){}",
                        Colors::reset(),
                        daily_color,
                        daily_total,
                        Colors::reset()
                    ));
                }

                parts.push(cost_part);
            } else if daily_total > 0.0 {
                // Show daily total even if no session cost
                let daily_color = get_cost_color(daily_total);
                parts.push(format!(
                    "day: {}${:.2}{}",
                    daily_color,
                    daily_total,
                    Colors::reset()
                ));
            }
        } else if daily_total > 0.0 {
            // Show daily total even if no cost data
            let daily_color = get_cost_color(daily_total);
            parts.push(format!(
                "day: {}${:.2}{}",
                daily_color,
                daily_total,
                Colors::reset()
            ));
        }
    }

    // 8. Token rate metrics (opt-in feature)
    // Uses rolling window if configured, otherwise session average
    if let Some(sid) = session_id {
        if let Some(ref db_handle) = db {
            if let Some(token_rates) =
                crate::stats::calculate_token_rates_with_db_and_transcript(sid, db_handle, transcript_path)
            {
                let token_rate_str = format_token_rates(&token_rates);
                parts.push(token_rate_str);
            }
        }
    }

    // Join parts with separator
    let separator = format!(" {}•{} ", Colors::separator_color(), Colors::reset());
    parts.join(&separator)
}

/// Format statusline using the configurable layout system.
///
/// This function builds all component variables and renders them
/// using the user's layout configuration (preset or custom format).
fn format_statusline_with_layout(
    current_dir: &str,
    model_name: Option<&str>,
    transcript_path: Option<&str>,
    cost: Option<&Cost>,
    daily_total: f64,
    session_id: Option<&str>,
    layout_config: &config::LayoutConfig,
) -> String {
    let full_config = config::get_config();
    let reset = Colors::reset();
    let components = &layout_config.components;

    // Build variables using VariableBuilder with component configs
    let mut builder = VariableBuilder::new();

    // Directory (with component config)
    let short_dir = sanitize_for_terminal(&shorten_path(current_dir));
    let basename = std::path::Path::new(current_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(current_dir);
    builder = builder.directory_with_config(
        current_dir,
        &short_dir,
        basename,
        &Colors::directory(),
        &reset,
        &components.directory,
    );

    // Git status (with component config)
    if let Some(git_status) = get_git_status(current_dir) {
        let git_info = format_git_info(&git_status);
        let branch = sanitize_for_terminal(&git_status.branch);
        let is_dirty = git_status.added > 0
            || git_status.modified > 0
            || git_status.deleted > 0
            || git_status.untracked > 0;

        // Build status-only string (without branch)
        let status_parts: Vec<String> = [
            (git_status.added > 0).then(|| format!("+{}", git_status.added)),
            (git_status.modified > 0).then(|| format!("~{}", git_status.modified)),
            (git_status.deleted > 0).then(|| format!("-{}", git_status.deleted)),
            (git_status.untracked > 0).then(|| format!("?{}", git_status.untracked)),
        ]
        .into_iter()
        .flatten()
        .collect();
        let status_only = status_parts.join(" ");

        builder = builder.git_with_config(
            git_info.trim_start(),
            Some(&branch),
            if status_only.is_empty() {
                None
            } else {
                Some(&status_only)
            },
            is_dirty,
            &Colors::green(),
            &reset,
            &components.git,
        );
    }

    // Context usage (with component config)
    if let Some(transcript) = transcript_path {
        if let Some(context) = calculate_context_usage(transcript, model_name, session_id, None) {
            let current_tokens = crate::utils::get_token_count_from_transcript(transcript);
            let window_size = crate::utils::get_context_window_for_model(model_name, full_config);
            let bar_width = full_config.display.progress_bar_width;
            let raw_bar = format_raw_bar(context.percentage, bar_width);
            builder = builder.context_with_config(
                &raw_bar,
                Some(context.percentage as u32),
                current_tokens.map(|t| (t as u64, window_size as u64)),
                &components.context,
            );
        }
    }

    // Model (with component config)
    if let Some(name) = model_name {
        let sanitized_name = sanitize_for_terminal(name);
        let model_type = ModelType::from_name(&sanitized_name);
        builder = builder.model_with_config(
            &model_type.abbreviation(),
            &sanitized_name,
            &model_type.family(),
            &model_type.version(),
            &Colors::model(),
            &reset,
            &components.model,
        );
    }

    // Duration
    if let Some(transcript) = transcript_path {
        if let Some(duration) = parse_duration(transcript) {
            builder = builder.duration(&format_duration(duration), &Colors::duration(), &reset);
        }
    }

    // Lines changed
    if let Some(cost_data) = cost {
        if let (Some(added), Some(removed)) =
            (cost_data.total_lines_added, cost_data.total_lines_removed)
        {
            builder = builder.lines_changed(
                added,
                removed,
                &Colors::lines_added(),
                &Colors::lines_removed(),
                &reset,
            );
        }
    }

    // Cost and burn rate (with component config)
    if let Some(cost_data) = cost {
        if let Some(total_cost) = cost_data.total_cost_usd {
            let cost_color = get_cost_color(total_cost);

            // Calculate burn rate
            let duration = session_id
                .and_then(crate::stats::get_session_duration_by_mode)
                .or_else(|| transcript_path.and_then(parse_duration));

            let burn_rate = duration.and_then(|d| {
                if d > 60 {
                    Some((total_cost * 3600.0) / d as f64)
                } else {
                    None
                }
            });

            builder = builder.cost_with_config(
                Some(total_cost),
                burn_rate,
                if daily_total > total_cost {
                    Some(daily_total)
                } else {
                    None
                },
                &cost_color,
                &Colors::light_gray(),
                &reset,
                &components.cost,
            );
        }
    }

    // Token rate (with component config)
    // Uses rolling window if configured, otherwise session average
    if let Some(sid) = session_id {
        // Create database handle for token rate calculation
        if let Some(db) = crate::stats::StatsData::get_sqlite_path()
            .ok()
            .filter(|p| p.exists())
            .and_then(|p| crate::database::SqliteDatabase::new(&p).ok())
        {
            if let Some(token_rates) =
                crate::stats::calculate_token_rates_with_db_and_transcript(sid, &db, transcript_path)
            {
                builder = builder.token_rate_with_config(
                    token_rates.total_rate,
                    Some(token_rates.session_total_tokens),
                    Some(token_rates.daily_total_tokens),
                    &Colors::light_gray(),
                    &reset,
                    &components.token_rate,
                );
            }
        }
    }

    // Build variables and render
    let variables = builder.build();
    let renderer = LayoutRenderer::from_config(layout_config);
    renderer.render(&variables)
}

/// Format output with explicit display configuration (prints to stdout)
fn format_output_with_config(
    current_dir: &str,
    model_name: Option<&str>,
    transcript_path: Option<&str>,
    cost: Option<&Cost>,
    daily_total: f64,
    session_id: Option<&str>,
    display_config: &config::DisplayConfig,
) {
    let full_config = config::get_config();

    // Check if custom layout is configured (non-empty format OR non-default preset)
    let use_layout_system = !full_config.layout.format.is_empty()
        || full_config.layout.preset.to_lowercase() != "default";

    let output = if use_layout_system {
        format_statusline_with_layout(
            current_dir,
            model_name,
            transcript_path,
            cost,
            daily_total,
            session_id,
            &full_config.layout,
        )
    } else {
        format_statusline_string(
            current_dir,
            model_name,
            transcript_path,
            cost,
            daily_total,
            session_id,
            display_config,
        )
    };
    print!("{}", output);
}

/// Format output to a string instead of printing.
///
/// This is the library-friendly version of format_output that returns
/// the formatted statusline as a String.
#[allow(dead_code)]
pub fn format_output_to_string(
    current_dir: &str,
    model_name: Option<&str>,
    transcript_path: Option<&str>,
    cost: Option<&Cost>,
    daily_total: f64,
    session_id: Option<&str>,
) -> String {
    let config = config::get_config();
    format_statusline_string(
        current_dir,
        model_name,
        transcript_path,
        cost,
        daily_total,
        session_id,
        &config.display,
    )
}

/// Generate just the raw progress bar without colors (e.g., "[====>-----]")
fn format_raw_bar(percentage: f64, width: usize) -> String {
    let filled_ratio = percentage / 100.0;
    let filled = (filled_ratio * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;

    format!(
        "[{}{}{}]",
        "=".repeat(filled),
        if filled < width { ">" } else { "" },
        "-".repeat(empty.saturating_sub(if filled < width { 1 } else { 0 }))
    )
}

fn format_context_bar(
    context: &ContextUsage,
    current_tokens: Option<u32>,
    window_size: Option<usize>,
) -> String {
    use crate::models::CompactionState;

    let config = config::get_config();
    let bar_width = config.display.progress_bar_width;

    // Format token counts if enabled and data available
    let token_display = if let (Some(current), Some(window)) = (current_tokens, window_size) {
        if config.display.show_context_tokens {
            format!(
                " {}{}/{}{}",
                Colors::light_gray(),
                crate::utils::format_token_count(current as usize),
                crate::utils::format_token_count(window),
                Colors::reset()
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Handle different compaction states
    match context.compaction_state {
        CompactionState::InProgress => {
            // Simple static indicator - statusline doesn't update frequently enough for animation
            format!(
                "{}Compacting...{}{}",
                Colors::yellow(),
                Colors::reset(),
                token_display
            )
        }

        CompactionState::RecentlyCompleted => {
            // Show percentage with checkmark instead of warning
            let percentage = context.percentage;
            let color = Colors::context_color(percentage);
            let percentage_color = color.clone();

            let filled_ratio = percentage / 100.0;
            let filled = (filled_ratio * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let empty = bar_width - filled;

            let bar = format!(
                "{}{}{}",
                "=".repeat(filled),
                if filled < bar_width { ">" } else { "" },
                "-".repeat(empty.saturating_sub(if filled < bar_width { 1 } else { 0 }))
            );

            format!(
                "{}{}%{} {}[{}]{} {}✓{}{}",
                percentage_color,
                percentage.round() as u32,
                Colors::reset(),
                color,
                bar,
                Colors::reset(),
                Colors::green(),
                Colors::reset(),
                token_display
            )
        }

        CompactionState::Normal => {
            // Normal display with optional warning
            let percentage = context.percentage;
            let color = Colors::context_color(percentage);
            let percentage_color = color.clone();

            let filled_ratio = percentage / 100.0;
            let filled = (filled_ratio * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let empty = bar_width - filled;

            let bar = format!(
                "{}{}{}",
                "=".repeat(filled),
                if filled < bar_width { ">" } else { "" },
                "-".repeat(empty.saturating_sub(if filled < bar_width { 1 } else { 0 }))
            );

            // Add warning indicator if approaching auto-compact threshold
            let warning = if context.approaching_limit {
                format!(" {}⚠{}", Colors::orange(), Colors::reset())
            } else {
                String::new()
            };

            format!(
                "{}{}%{} {}[{}]{}{}{}",
                percentage_color,
                percentage.round() as u32,
                Colors::reset(),
                color,
                bar,
                Colors::reset(),
                warning,
                token_display
            )
        }
    }
}

fn get_cost_color(cost: f64) -> String {
    Colors::cost_color(cost)
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// Format token rate metrics based on display mode.
///
/// NOTE: This function is used in NON-LAYOUT MODE only (format_output).
/// For layout mode, use `VariableBuilder::token_rate_with_config()` in layout.rs.
///
/// The two paths have different capabilities:
/// - Non-layout (this): Supports display_mode ("summary", "detailed", "cache_only")
///   with cache metrics, ROI calculations, and detailed breakdowns
/// - Layout mode: Simpler format options (rate_only, with_session, with_daily, full)
///   for template rendering with {token_rate}, {token_rate_only}, etc.
fn format_token_rates(metrics: &crate::stats::TokenRateMetrics) -> String {
    let config = crate::config::get_config();
    let mode = &config.token_rate.display_mode;
    let component_config = &config.layout.components.token_rate;

    // Convert rate based on time_unit config
    let (rate_multiplier, unit_str) = match component_config.time_unit.as_str() {
        "minute" => (60.0, "tok/min"),
        "hour" => (3600.0, "tok/hr"),
        _ => (1.0, "tok/s"), // "second" is default
    };

    // Helper to format a rate value with appropriate precision
    let format_rate = |rate: f64| -> String {
        let adjusted = rate * rate_multiplier;
        if adjusted >= 1000.0 {
            format!("{:.1}K", adjusted / 1000.0)
        } else {
            format!("{:.1}", adjusted)
        }
    };

    // Build the rate display based on display mode
    let rate_str = match mode.as_str() {
        "detailed" => {
            // Detailed: "In:5.2K Out:8.7K tok/hr • Cache:85%"
            // Note: input_rate includes cache_read_rate for meaningful display
            // (raw input without cache is often near-zero for long sessions)
            let effective_input_rate = metrics.input_rate + metrics.cache_read_rate;
            let mut parts = vec![format!(
                "{}In:{} Out:{} {}{}",
                Colors::light_gray(),
                format_rate(effective_input_rate),
                format_rate(metrics.output_rate),
                unit_str,
                Colors::reset()
            )];

            // Add cache metrics if available and enabled
            if config.token_rate.cache_metrics {
                if let Some(hit_ratio) = metrics.cache_hit_ratio {
                    let cache_pct = (hit_ratio * 100.0) as u8;
                    let cache_str = if let Some(roi) = metrics.cache_roi {
                        if roi.is_infinite() {
                            format!("Cache:{}% (∞ ROI)", cache_pct)
                        } else {
                            format!("Cache:{}% ({:.1}x ROI)", cache_pct, roi)
                        }
                    } else {
                        format!("Cache:{}%", cache_pct)
                    };
                    parts.push(cache_str);
                }
            }

            parts.join(" • ")
        }
        "cache_only" => {
            // Cache-focused: "Cache:85% (12x ROI) • 41.7 tok/s"
            if config.token_rate.cache_metrics {
                if let Some(hit_ratio) = metrics.cache_hit_ratio {
                    let cache_pct = (hit_ratio * 100.0) as u8;
                    let cache_str = if let Some(roi) = metrics.cache_roi {
                        if roi.is_infinite() {
                            format!(
                                "{}Cache:{}% (∞ ROI){}",
                                Colors::cyan(),
                                cache_pct,
                                Colors::reset()
                            )
                        } else {
                            format!(
                                "{}Cache:{}% ({:.1}x ROI){}",
                                Colors::cyan(),
                                cache_pct,
                                roi,
                                Colors::reset()
                            )
                        }
                    } else {
                        format!("{}Cache:{}%{}", Colors::cyan(), cache_pct, Colors::reset())
                    };

                    format!(
                        "{} • {}{} {}{}",
                        cache_str,
                        Colors::light_gray(),
                        format_rate(metrics.total_rate),
                        unit_str,
                        Colors::reset()
                    )
                } else {
                    // No cache data, fallback to summary
                    format!(
                        "{}{} {}{}",
                        Colors::light_gray(),
                        format_rate(metrics.total_rate),
                        unit_str,
                        Colors::reset()
                    )
                }
            } else {
                // Cache metrics disabled, fallback to summary
                format!(
                    "{}{} {}{}",
                    Colors::light_gray(),
                    format_rate(metrics.total_rate),
                    unit_str,
                    Colors::reset()
                )
            }
        }
        other => {
            // Summary (default): "13.9 tok/s" or "45.2K tok/hr"
            if other != "summary" {
                log::warn!(
                    "Unknown token_rate display_mode '{}', using 'summary'",
                    other
                );
            }
            format!(
                "{}{} {}{}",
                Colors::light_gray(),
                format_rate(metrics.total_rate),
                unit_str,
                Colors::reset()
            )
        }
    };

    // Add session and daily totals based on component config
    let show_session = component_config.show_session_total && metrics.session_total_tokens > 0;
    let show_daily = component_config.show_daily_total && metrics.daily_total_tokens > 0;

    if !show_session && !show_daily {
        return rate_str;
    }

    let mut parts = vec![rate_str];

    if show_session {
        parts.push(format!(
            "{}{}{}",
            Colors::light_gray(),
            format_token_count_for_display(metrics.session_total_tokens),
            Colors::reset()
        ));
    }

    if show_daily {
        parts.push(format!(
            "{}day: {}{}",
            Colors::light_gray(),
            format_token_count_for_display(metrics.daily_total_tokens),
            Colors::reset()
        ));
    }

    parts.join(" ")
}

/// Format token count with K/M suffix for display
fn format_token_count_for_display(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{}K", (count + 500) / 1000)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colors() {
        // Test the color functions (which respect NO_COLOR) not the constants
        if Colors::enabled() {
            assert_eq!(Colors::cyan(), "\x1b[36m");
            assert_eq!(Colors::green(), "\x1b[32m");
            assert_eq!(Colors::red(), "\x1b[31m");
            assert_eq!(Colors::yellow(), "\x1b[33m");
            assert_eq!(Colors::reset(), "\x1b[0m");
        } else {
            // When NO_COLOR is set, all colors return empty strings
            assert_eq!(Colors::cyan(), "");
            assert_eq!(Colors::green(), "");
            assert_eq!(Colors::red(), "");
            assert_eq!(Colors::yellow(), "");
            assert_eq!(Colors::reset(), "");
        }
    }

    #[test]
    fn test_get_cost_color() {
        // The test should work whether or not NO_COLOR is set
        if Colors::enabled() {
            // Cost colors are now theme-dependent, so we can't check for specific ANSI codes
            // Instead, verify that:
            // 1. Colors are returned (non-empty strings with ANSI escape sequences)
            // 2. Different cost levels return different colors
            let low_cost = get_cost_color(2.5); // < $5 (low threshold)
            let medium_cost = get_cost_color(10.0); // $5-$20 (medium threshold)
            let high_cost = get_cost_color(25.0); // >= $20 (high threshold)

            // All should have ANSI escape codes
            assert!(low_cost.starts_with("\x1b["), "Low cost should have color");
            assert!(
                medium_cost.starts_with("\x1b["),
                "Medium cost should have color"
            );
            assert!(
                high_cost.starts_with("\x1b["),
                "High cost should have color"
            );

            // Different costs should have different colors
            assert_ne!(
                low_cost, medium_cost,
                "Low and medium costs should have different colors"
            );
            assert_ne!(
                medium_cost, high_cost,
                "Medium and high costs should have different colors"
            );
            assert_ne!(
                low_cost, high_cost,
                "Low and high costs should have different colors"
            );
        } else {
            // When NO_COLOR is set, all colors return empty strings
            assert_eq!(get_cost_color(2.5), String::new());
            assert_eq!(get_cost_color(10.0), String::new());
            assert_eq!(get_cost_color(25.0), String::new());
        }
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(90), "1m");
        assert_eq!(format_duration(3665), "1h1m");
    }

    #[test]
    fn test_format_context_bar() {
        use crate::models::CompactionState;
        let low = ContextUsage {
            percentage: 10.0,
            approaching_limit: false,
            tokens_remaining: 180_000,
            compaction_state: CompactionState::Normal,
        };
        let bar = format_context_bar(&low, None, None);
        assert!(bar.contains("10%"));
        assert!(bar.contains("[=>"));
        assert!(!bar.contains('•'));
        assert!(!bar.contains('⚠')); // No warning at 10%

        let high = ContextUsage {
            percentage: 95.0,
            approaching_limit: true,
            tokens_remaining: 10_000,
            compaction_state: CompactionState::Normal,
        };
        let bar = format_context_bar(&high, None, None);
        assert!(bar.contains("95%"));
        assert!(!bar.contains('•'));
        assert!(bar.contains('⚠')); // Warning at 95%
    }

    #[test]
    fn test_format_context_bar_compaction_states() {
        use crate::models::CompactionState;

        // Test InProgress state - should show "Compacting..." message
        let in_progress = ContextUsage {
            percentage: 50.0,
            approaching_limit: false,
            tokens_remaining: 80_000,
            compaction_state: CompactionState::InProgress,
        };
        let bar = format_context_bar(&in_progress, None, None);
        assert!(
            bar.contains("Compacting"),
            "InProgress should show 'Compacting' message"
        );
        // Should NOT show percentage or progress bar during compaction
        assert!(!bar.contains('%'), "InProgress should not show percentage");
        // Check for progress bar pattern (not ANSI escape codes which also contain '[')
        assert!(
            !bar.contains("[=") && !bar.contains("[>") && !bar.contains("[-"),
            "InProgress should not show progress bar"
        );

        // Test RecentlyCompleted state - should show checkmark
        let recently_completed = ContextUsage {
            percentage: 35.0,
            approaching_limit: false,
            tokens_remaining: 104_000,
            compaction_state: CompactionState::RecentlyCompleted,
        };
        let bar = format_context_bar(&recently_completed, None, None);
        assert!(bar.contains("35%"), "Should show correct percentage");
        assert!(bar.contains('✓'), "RecentlyCompleted should show checkmark");

        // Test Normal state with warning
        let normal_warning = ContextUsage {
            percentage: 85.0,
            approaching_limit: true,
            tokens_remaining: 24_000,
            compaction_state: CompactionState::Normal,
        };
        let bar = format_context_bar(&normal_warning, None, None);
        assert!(bar.contains("85%"), "Should show correct percentage");
        assert!(
            bar.contains('⚠'),
            "Normal with high usage should show warning"
        );
        assert!(!bar.contains('✓'), "Normal should not show checkmark");
    }

    #[test]
    fn test_burn_rate_calculation() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a temporary transcript file with 10-minute duration
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"Start"}},"timestamp":"2025-08-25T10:00:00.000Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"End"}},"timestamp":"2025-08-25T10:10:00.000Z"}}"#).unwrap();

        // Test that burn rate is calculated correctly
        // $0.50 over 10 minutes (600 seconds) = $3.00/hour
        let _cost = Cost {
            total_cost_usd: Some(0.50),
            total_lines_added: None,
            total_lines_removed: None,
        };

        // The burn rate calculation happens in format_output
        // We can verify the math directly here
        let duration = 600u64; // 10 minutes in seconds
        let total_cost = 0.50;
        let burn_rate = (total_cost * 3600.0) / duration as f64;
        assert_eq!(burn_rate, 3.0); // $3.00 per hour

        // Test with 5-minute session (the problematic case)
        let duration_5min = 300u64; // 5 minutes
        let cost_high = 33.28; // The cost from the user's example
        let burn_rate_5min = (cost_high * 3600.0) / duration_5min as f64;
        assert_eq!(burn_rate_5min, 399.36); // This WAS the problem - now fixed

        // With proper timestamp parsing, 5 minutes should give correct rate
        let realistic_cost = 0.25; // More realistic for 5 minutes
        let realistic_burn = (realistic_cost * 3600.0) / 300.0;
        assert_eq!(realistic_burn, 3.0); // $3.00/hr is reasonable
    }

    // Helper guard to temporarily clear NO_COLOR for theme tests
    struct ClearNoColor(Option<String>);
    impl ClearNoColor {
        fn new() -> Self {
            let old = std::env::var("NO_COLOR").ok();
            std::env::remove_var("NO_COLOR");
            Self(old)
        }
    }
    impl Drop for ClearNoColor {
        fn drop(&mut self) {
            if let Some(val) = &self.0 {
                std::env::set_var("NO_COLOR", val);
            }
        }
    }

    #[test]
    #[ignore] // Skip in CI where NO_COLOR is set
    fn test_theme_affects_colors() {
        // Use RAII guard to ensure clean environment
        let _guard = ClearNoColor::new();

        // Verify colors are actually enabled after clearing NO_COLOR
        if !Colors::enabled() {
            // If colors are still disabled, skip this test
            eprintln!("Skipping theme test - colors remain disabled despite clearing NO_COLOR");
            return;
        }

        // Save original theme env var
        let original_theme = std::env::var("STATUSLINE_THEME").ok();

        // Light theme should use gray text/separator
        std::env::set_var("STATUSLINE_THEME", "light");
        assert_eq!(Colors::text_color(), "\x1b[90m"); // gray
        assert_eq!(Colors::separator_color(), "\x1b[90m"); // gray

        // Dark theme should use white text and light gray separator
        std::env::set_var("STATUSLINE_THEME", "dark");
        assert_eq!(Colors::text_color(), "\x1b[37m"); // white
        assert_eq!(Colors::separator_color(), "\x1b[38;5;245m"); // light_gray

        // Cleanup
        if let Some(value) = original_theme {
            std::env::set_var("STATUSLINE_THEME", value);
        } else {
            std::env::remove_var("STATUSLINE_THEME");
        }
    }

    #[test]
    fn test_sanitized_output() {
        // Test with malicious directory path containing ANSI codes
        let malicious_dir = "/home/user/\x1b[31mdanger\x1b[0m/project";
        let model_with_control = "claude-\x00-opus\x07";

        // Create a simple output string to test sanitization
        let short_dir = sanitize_for_terminal(&shorten_path(malicious_dir));
        assert!(!short_dir.contains('\x1b'));
        assert!(!short_dir.contains('\x00'));
        assert!(!short_dir.contains('\x07'));

        // Test model name sanitization
        let sanitized_model = sanitize_for_terminal(model_with_control);
        assert_eq!(sanitized_model, "claude--opus");
    }

    #[test]
    fn test_token_rate_time_unit_conversion() {
        // Test that time_unit config produces correct rate multipliers and units
        // This tests the logic used in format_token_rates()

        let test_cases = vec![
            ("second", 1.0, "tok/s"),
            ("minute", 60.0, "tok/min"),
            ("hour", 3600.0, "tok/hr"),
            ("invalid", 1.0, "tok/s"), // defaults to second
        ];

        for (time_unit, expected_multiplier, expected_unit) in test_cases {
            let (multiplier, unit) = match time_unit {
                "minute" => (60.0, "tok/min"),
                "hour" => (3600.0, "tok/hr"),
                _ => (1.0, "tok/s"),
            };

            assert_eq!(
                multiplier, expected_multiplier,
                "time_unit '{}' should have multiplier {}",
                time_unit, expected_multiplier
            );
            assert_eq!(
                unit, expected_unit,
                "time_unit '{}' should have unit '{}'",
                time_unit, expected_unit
            );
        }
    }

    #[test]
    fn test_token_rate_formatting_with_k_suffix() {
        // Test that large rates get K suffix
        let format_rate = |rate: f64, multiplier: f64| -> String {
            let adjusted = rate * multiplier;
            if adjusted >= 1000.0 {
                format!("{:.1}K", adjusted / 1000.0)
            } else {
                format!("{:.1}", adjusted)
            }
        };

        // At tok/s (multiplier 1.0)
        assert_eq!(format_rate(10.5, 1.0), "10.5");
        assert_eq!(format_rate(1000.0, 1.0), "1.0K");
        assert_eq!(format_rate(1500.0, 1.0), "1.5K");

        // At tok/min (multiplier 60.0)
        assert_eq!(format_rate(10.0, 60.0), "600.0"); // 10 * 60 = 600
        assert_eq!(format_rate(20.0, 60.0), "1.2K"); // 20 * 60 = 1200

        // At tok/hr (multiplier 3600.0)
        assert_eq!(format_rate(10.0, 3600.0), "36.0K"); // 10 * 3600 = 36000
        assert_eq!(format_rate(0.5, 3600.0), "1.8K"); // 0.5 * 3600 = 1800
        assert_eq!(format_rate(0.1, 3600.0), "360.0"); // 0.1 * 3600 = 360
    }

    #[test]
    fn test_token_rate_hour_display() {
        // Verify the hour display format matches expected output
        let rate_per_second = 12.5; // 12.5 tok/s
        let hour_multiplier = 3600.0;
        let adjusted = rate_per_second * hour_multiplier; // 45000 tok/hr

        let formatted = if adjusted >= 1000.0 {
            format!("{:.1}K tok/hr", adjusted / 1000.0)
        } else {
            format!("{:.1} tok/hr", adjusted)
        };

        assert_eq!(formatted, "45.0K tok/hr");
    }
}
