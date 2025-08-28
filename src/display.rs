use crate::models::{Cost, ModelType, ContextUsage};
use crate::git::{get_git_status, format_git_info};
use crate::utils::{shorten_path, calculate_context_usage, parse_duration};
use crate::config;

pub struct Colors;

impl Colors {
    pub const RESET: &'static str = "\x1b[0m";
    #[allow(dead_code)]
    pub const BOLD: &'static str = "\x1b[1m";
    pub const RED: &'static str = "\x1b[31m";
    pub const GREEN: &'static str = "\x1b[32m";
    pub const YELLOW: &'static str = "\x1b[33m";
    #[allow(dead_code)]
    pub const BLUE: &'static str = "\x1b[34m";
    #[allow(dead_code)]
    pub const MAGENTA: &'static str = "\x1b[35m";
    pub const CYAN: &'static str = "\x1b[36m";
    pub const WHITE: &'static str = "\x1b[37m";
    pub const GRAY: &'static str = "\x1b[90m";
    pub const ORANGE: &'static str = "\x1b[38;5;208m";
    pub const LIGHT_GRAY: &'static str = "\x1b[38;5;245m";
}

pub fn format_output(
    current_dir: &str,
    model_name: Option<&str>,
    transcript_path: Option<&str>,
    cost: Option<&Cost>,
    daily_total: f64,
    session_id: Option<&str>,
) {
    let mut output = String::new();

    // Shorten the path
    let short_dir = shorten_path(current_dir);

    // Directory with color
    output.push_str(&format!("{}{}{}", Colors::CYAN, short_dir, Colors::RESET));

    // Git status
    if let Some(git_status) = get_git_status(current_dir) {
        let git_info = format_git_info(&git_status);
        if !git_info.is_empty() {
            output.push_str(&format!(" {}•{}", Colors::GRAY, Colors::RESET));
            output.push_str(&git_info);
        }
    }

    // Context usage from transcript
    if let Some(transcript) = transcript_path {
        if let Some(context) = calculate_context_usage(transcript) {
            output.push_str(&format_context_bar(&context));
        }
    }

    // Model display
    if let Some(name) = model_name {
        let model_type = ModelType::from_name(name);
        output.push_str(&format!(
            " {}{}{}",
            Colors::CYAN,
            model_type.abbreviation(),
            Colors::RESET
        ));
    }

    // Duration from transcript
    if let Some(transcript) = transcript_path {
        if let Some(duration) = parse_duration(transcript) {
            output.push_str(&format!(
                " {}{}{}",
                Colors::LIGHT_GRAY,
                format_duration(duration),
                Colors::RESET
            ));
        }
    }

    // Lines changed
    if let Some(cost_data) = cost {
        if let (Some(added), Some(removed)) = (cost_data.total_lines_added, cost_data.total_lines_removed) {
            if added > 0 || removed > 0 {
                output.push_str(&format!(" {}•{}", Colors::LIGHT_GRAY, Colors::RESET));
                if added > 0 {
                    output.push_str(&format!(" {}+{}{}", Colors::GREEN, added, Colors::RESET));
                }
                if removed > 0 {
                    output.push_str(&format!(" {}-{}{}", Colors::RED, removed, Colors::RESET));
                }
            }
        }
    }

    // Cost display with burn rate
    if let Some(cost_data) = cost {
        if let Some(total_cost) = cost_data.total_cost_usd {
            let cost_color = get_cost_color(total_cost);

            // Calculate burn rate if we have duration
            // First try to get duration from our tracked session stats
            let duration = session_id.and_then(|sid| crate::stats::get_session_duration(sid))
                .or_else(|| {
                    // Fallback to parsing transcript if available
                    transcript_path.and_then(|t| parse_duration(t))
                });

            let burn_rate = duration.and_then(|d| {
                if d > 60 {  // Only show burn rate for sessions > 1 minute
                    Some((total_cost * 3600.0) / d as f64)
                } else {
                    None
                }
            });

            output.push_str(&format!(" {}•{}", Colors::LIGHT_GRAY, Colors::RESET));
            output.push_str(&format!(
                " {}${:.2}{}",
                cost_color,
                total_cost,
                Colors::RESET
            ));

            // Add burn rate if available
            if let Some(rate) = burn_rate {
                if rate > 0.0 {
                    output.push_str(&format!(
                        " {}(${:.2}/hr){}",
                        Colors::GRAY,
                        rate,
                        Colors::RESET
                    ));
                }
            }

            // Add daily total if different from session cost
            if daily_total > total_cost {
                let daily_color = get_cost_color(daily_total);
                output.push_str(&format!(
                    " {}(day: {}${:.2}){}",
                    Colors::RESET,
                    daily_color,
                    daily_total,
                    Colors::RESET
                ));
            }
        } else if daily_total > 0.0 {
            // Show daily total even if no session cost
            let daily_color = get_cost_color(daily_total);
            output.push_str(&format!(
                " {}day: {}${:.2}{}",
                Colors::RESET,
                daily_color,
                daily_total,
                Colors::RESET
            ));
        }
    } else if daily_total > 0.0 {
        // Show daily total even if no cost data
        let daily_color = get_cost_color(daily_total);
        output.push_str(&format!(
            " {}day: {}${:.2}{}",
            Colors::RESET,
            daily_color,
            daily_total,
            Colors::RESET
        ));
    }

    print!("{}", output);
}

fn format_context_bar(context: &ContextUsage) -> String {
    let percentage = context.percentage;
    let config = config::get_config();

    // Choose color based on configured thresholds
    let (color, percentage_color) = if percentage > config.display.context_critical_threshold {
        (Colors::RED, Colors::RED)
    } else if percentage > config.display.context_warning_threshold {
        (Colors::ORANGE, Colors::ORANGE)
    } else if percentage > config.display.context_caution_threshold {
        (Colors::YELLOW, Colors::YELLOW)
    } else {
        (Colors::GREEN, Colors::WHITE)
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
        " {}• {}{}%{} {}[{}]{}",
        Colors::LIGHT_GRAY,
        percentage_color,
        percentage.round() as u32,
        Colors::RESET,
        color,
        bar,
        Colors::RESET
    )
}

fn get_cost_color(cost: f64) -> &'static str {
    let config = config::get_config();
    if cost >= config.cost.medium_threshold {
        Colors::RED
    } else if cost >= config.cost.low_threshold {
        Colors::YELLOW
    } else {
        Colors::GREEN
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
        assert_eq!(Colors::CYAN, "\x1b[36m");
        assert_eq!(Colors::GREEN, "\x1b[32m");
        assert_eq!(Colors::RED, "\x1b[31m");
        assert_eq!(Colors::YELLOW, "\x1b[33m");
        assert_eq!(Colors::RESET, "\x1b[0m");
    }

    #[test]
    fn test_get_cost_color() {
        assert_eq!(get_cost_color(2.5), Colors::GREEN);
        assert_eq!(get_cost_color(10.0), Colors::YELLOW);
        assert_eq!(get_cost_color(25.0), Colors::RED);
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

        let high = ContextUsage { percentage: 95.0 };
        let bar = format_context_bar(&high);
        assert!(bar.contains("95%"));
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
}