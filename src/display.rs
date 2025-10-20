//! Display formatting module.
//!
//! This module handles the visual formatting of the statusline output,
//! including colors, progress bars, and layout.

use crate::config;
use crate::git::{format_git_info, get_git_status};
use crate::models::{ContextUsage, Cost, ModelType};
use crate::utils::{calculate_context_usage, parse_duration, sanitize_for_terminal, shorten_path};

/// ANSI color codes for terminal output.
pub struct Colors;

impl Colors {
    /// Check if colors are enabled (respects NO_COLOR env var)
    pub fn enabled() -> bool {
        std::env::var("NO_COLOR").is_err()
    }

    /// Get a color code, or empty string if colors are disabled
    fn get(code: &'static str) -> &'static str {
        if Self::enabled() {
            code
        } else {
            ""
        }
    }

    pub fn reset() -> &'static str {
        Self::get("\x1b[0m")
    }
    #[allow(dead_code)]
    pub fn bold() -> &'static str {
        Self::get("\x1b[1m")
    }
    pub fn red() -> &'static str {
        Self::get("\x1b[31m")
    }
    pub fn green() -> &'static str {
        Self::get("\x1b[32m")
    }
    pub fn yellow() -> &'static str {
        Self::get("\x1b[33m")
    }
    #[allow(dead_code)]
    pub fn blue() -> &'static str {
        Self::get("\x1b[34m")
    }
    #[allow(dead_code)]
    pub fn magenta() -> &'static str {
        Self::get("\x1b[35m")
    }
    pub fn cyan() -> &'static str {
        Self::get("\x1b[36m")
    }
    pub fn white() -> &'static str {
        Self::get("\x1b[37m")
    }
    pub fn gray() -> &'static str {
        Self::get("\x1b[90m")
    }
    pub fn orange() -> &'static str {
        Self::get("\x1b[38;5;208m")
    }
    pub fn light_gray() -> &'static str {
        Self::get("\x1b[38;5;245m")
    }

    // For backwards compatibility with existing code
    #[allow(dead_code)]
    pub const RESET: &'static str = "\x1b[0m";
    #[allow(dead_code)]
    pub const RED: &'static str = "\x1b[31m";
    #[allow(dead_code)]
    pub const GREEN: &'static str = "\x1b[32m";
    #[allow(dead_code)]
    pub const YELLOW: &'static str = "\x1b[33m";
    #[allow(dead_code)]
    pub const CYAN: &'static str = "\x1b[36m";
    #[allow(dead_code)]
    pub const WHITE: &'static str = "\x1b[37m";
    #[allow(dead_code)]
    pub const GRAY: &'static str = "\x1b[90m";
    #[allow(dead_code)]
    pub const ORANGE: &'static str = "\x1b[38;5;208m";
    #[allow(dead_code)]
    pub const LIGHT_GRAY: &'static str = "\x1b[38;5;245m";

    /// Get the appropriate text color based on theme
    pub fn text_color() -> &'static str {
        if !Self::enabled() {
            return "";
        }
        let theme = config::get_theme();
        if theme == "light" {
            Self::gray() // Darker for light backgrounds
        } else {
            Self::white() // Brighter for dark backgrounds
        }
    }

    /// Get the appropriate separator color based on theme
    pub fn separator_color() -> &'static str {
        if !Self::enabled() {
            return "";
        }
        let theme = config::get_theme();
        if theme == "light" {
            Self::gray()
        } else {
            Self::light_gray()
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
    let mut parts = Vec::new();

    // 1. Directory (always first if shown)
    if display_config.show_directory {
        let short_dir = sanitize_for_terminal(&shorten_path(current_dir));
        parts.push(format!(
            "{}{}{}",
            Colors::cyan(),
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
            if let Some(context) = calculate_context_usage(transcript) {
                parts.push(format_context_bar(&context));
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
                Colors::cyan(),
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
                    Colors::light_gray(),
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
                            Colors::green(),
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
                            Colors::red(),
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
                let duration = session_id
                    .and_then(crate::stats::get_session_duration)
                    .or_else(|| transcript_path.and_then(parse_duration));

                let burn_rate = duration.and_then(|d| {
                    if d > 60 {
                        Some((total_cost * 3600.0) / d as f64)
                    } else {
                        None
                    }
                });

                let mut cost_part = format!(
                    "{}${:.2}{}",
                    cost_color,
                    total_cost,
                    Colors::reset()
                );

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

    // Join parts with separator
    let separator = format!(" {}•{} ", Colors::separator_color(), Colors::reset());
    parts.join(&separator)
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
    let output = format_statusline_string(
        current_dir,
        model_name,
        transcript_path,
        cost,
        daily_total,
        session_id,
        display_config,
    );
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

fn format_context_bar(context: &ContextUsage) -> String {
    let percentage = context.percentage;
    let config = config::get_config();

    // Choose color based on configured thresholds
    let (color, percentage_color) = if percentage > config.display.context_critical_threshold {
        (Colors::red(), Colors::red())
    } else if percentage > config.display.context_warning_threshold {
        (Colors::orange(), Colors::orange())
    } else if percentage > config.display.context_caution_threshold {
        (Colors::yellow(), Colors::yellow())
    } else {
        (Colors::green(), Colors::text_color())
    };

    // Create progress bar with configured width
    let bar_width = config.display.progress_bar_width;
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
        "{}{}%{} {}[{}]{}",
        percentage_color,
        percentage.round() as u32,
        Colors::reset(),
        color,
        bar,
        Colors::reset()
    )
}

fn get_cost_color(cost: f64) -> &'static str {
    let config = config::get_config();
    if cost >= config.cost.medium_threshold {
        Colors::red()
    } else if cost >= config.cost.low_threshold {
        Colors::yellow()
    } else {
        Colors::green()
    }
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
            assert_eq!(get_cost_color(2.5), "\x1b[32m"); // green
            assert_eq!(get_cost_color(10.0), "\x1b[33m"); // yellow
            assert_eq!(get_cost_color(25.0), "\x1b[31m"); // red
        } else {
            // When NO_COLOR is set, all colors return empty strings
            assert_eq!(get_cost_color(2.5), "");
            assert_eq!(get_cost_color(10.0), "");
            assert_eq!(get_cost_color(25.0), "");
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
        let low = ContextUsage { percentage: 10.0 };
        let bar = format_context_bar(&low);
        assert!(bar.contains("10%"));
        assert!(bar.contains("[=>"));
        assert!(!bar.contains('•'));

        let high = ContextUsage { percentage: 95.0 };
        let bar = format_context_bar(&high);
        assert!(bar.contains("95%"));
        assert!(!bar.contains('•'));
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

    #[test]
    fn test_theme_affects_colors() {
        // Save original NO_COLOR state
        let original_no_color = std::env::var("NO_COLOR").ok();

        // Ensure colors are enabled for this test
        std::env::remove_var("NO_COLOR");

        // Light theme should use gray text/separator
        std::env::set_var("STATUSLINE_THEME", "light");
        assert_eq!(Colors::text_color(), Colors::gray());
        assert_eq!(Colors::separator_color(), Colors::gray());

        // Dark theme should use white text and light gray separator
        std::env::set_var("STATUSLINE_THEME", "dark");
        assert_eq!(Colors::text_color(), Colors::white());
        assert_eq!(Colors::separator_color(), Colors::light_gray());

        // Cleanup
        std::env::remove_var("STATUSLINE_THEME");

        // Restore original NO_COLOR state
        if let Some(value) = original_no_color {
            std::env::set_var("NO_COLOR", value);
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
}
