//! Color resolution, token formatting, and separator cleanup helpers.

/// Resolve a color override string to an ANSI code
///
/// Supports:
/// - Named colors: "red", "green", "blue", "cyan", "yellow", "magenta", "white", "gray"
/// - Hex colors: "#FF5733" or "#F53"
/// - ANSI codes: "\x1b[32m" (passthrough)
/// - 256 colors: "38;5;123" (wrapped in \x1b[..m)
pub(crate) fn resolve_color_override(color: &str) -> String {
    if color.is_empty() {
        return String::new();
    }

    // Already an ANSI escape sequence
    if color.starts_with("\x1b[") || color.starts_with("\\x1b[") {
        return color.replace("\\x1b", "\x1b");
    }

    // 256 color code (e.g., "38;5;123")
    if color.contains(';') {
        return format!("\x1b[{}m", color);
    }

    // Hex color
    if color.starts_with('#') {
        return hex_to_ansi(color);
    }

    // Named color
    match color.to_lowercase().as_str() {
        "red" => "\x1b[31m".to_string(),
        "green" => "\x1b[32m".to_string(),
        "yellow" => "\x1b[33m".to_string(),
        "blue" => "\x1b[34m".to_string(),
        "magenta" => "\x1b[35m".to_string(),
        "cyan" => "\x1b[36m".to_string(),
        "white" => "\x1b[37m".to_string(),
        "gray" | "grey" => "\x1b[90m".to_string(),
        "orange" => "\x1b[38;5;208m".to_string(),
        "light_gray" | "light_grey" => "\x1b[38;5;245m".to_string(),
        _ => String::new(), // Unknown color, return empty
    }
}

/// Convert hex color (#RGB or #RRGGBB) to 24-bit ANSI escape code
fn hex_to_ansi(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');

    let (r, g, b) = if hex.len() == 3 {
        // Short form: #RGB -> #RRGGBB
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).unwrap_or(0);
        (r, g, b)
    } else if hex.len() == 6 {
        // Long form: #RRGGBB
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        (r, g, b)
    } else {
        return String::new(); // Invalid hex format
    };

    format!("\x1b[38;2;{};{};{}m", r, g, b)
}

/// Format a token rate with the specified unit
pub(crate) fn format_rate_with_unit(rate: f64, unit: &str, color: &str, reset: &str) -> String {
    if rate >= 1000.0 {
        format!("{}{:.1}K {}{}", color, rate / 1000.0, unit, reset)
    } else {
        format!("{}{:.1} {}{}", color, rate, unit, reset)
    }
}

/// Format a token count with K/M suffix
pub(crate) fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        format!("{}", count)
    }
}

/// Clean up multiple consecutive separators and trailing separators
pub(super) fn clean_separators(s: &str, separator: &str) -> String {
    if separator.is_empty() {
        return s.to_string();
    }

    let mut result = s.to_string();

    // Replace multiple consecutive separators with single separator
    let double_sep = format!("{}{}", separator, separator);
    while result.contains(&double_sep) {
        result = result.replace(&double_sep, separator);
    }

    // Remove leading separator on each line
    let lines: Vec<&str> = result.lines().collect();
    let cleaned_lines: Vec<String> = lines
        .iter()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with(separator.trim()) {
                trimmed
                    .strip_prefix(separator.trim())
                    .unwrap_or(trimmed)
                    .trim_start()
                    .to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect();

    // Remove trailing separator on each line
    let final_lines: Vec<String> = cleaned_lines
        .iter()
        .map(|line| {
            if line.trim_end().ends_with(separator.trim()) {
                line.trim_end()
                    .strip_suffix(separator.trim())
                    .unwrap_or(line)
                    .trim_end()
                    .to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    // Filter out empty lines (except intentional newlines)
    let non_empty: Vec<&str> = final_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.as_str())
        .collect();

    non_empty.join("\n")
}
