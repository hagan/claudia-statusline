//! Layout rendering module for customizable statusline format.
//!
//! This module provides template-based rendering of the statusline,
//! allowing users to customize the format and order of components.

use std::collections::HashMap;

use crate::config::{
    CostComponentConfig, DirectoryComponentConfig, GitComponentConfig, LayoutConfig,
    ModelComponentConfig,
};
use crate::utils::sanitize_for_terminal;

/// Built-in layout presets
pub const PRESET_DEFAULT: &str = "{directory}{sep}{git}{sep}{context}{sep}{model}{sep}{cost}";
pub const PRESET_COMPACT: &str = "{dir_short} {git_branch} {model} {cost_short}";
pub const PRESET_DETAILED: &str =
    "{directory}{sep}{git}\n{context}{sep}{model}{sep}{duration}{sep}{cost}";
pub const PRESET_MINIMAL: &str = "{directory} {model}";
pub const PRESET_POWER: &str =
    "{directory}{sep}{git}{sep}{context}\n{model}{sep}{duration}{sep}{lines}{sep}{cost} ({burn_rate})";

/// Get the format string for a preset name
///
/// Looks up presets in this order:
/// 1. User presets in ~/.config/claudia-statusline/presets/<name>.toml
/// 2. Built-in presets (default, compact, detailed, minimal, power)
pub fn get_preset_format(preset: &str) -> String {
    // Try user preset first
    if let Some(user_format) = load_user_preset(preset) {
        return user_format;
    }

    // Fall back to built-in presets
    match preset.to_lowercase().as_str() {
        "compact" => PRESET_COMPACT.to_string(),
        "detailed" => PRESET_DETAILED.to_string(),
        "minimal" => PRESET_MINIMAL.to_string(),
        "power" => PRESET_POWER.to_string(),
        _ => PRESET_DEFAULT.to_string(), // "default" or unknown
    }
}

/// Load a user-defined preset from the config directory
fn load_user_preset(name: &str) -> Option<String> {
    let preset_dir = dirs::config_dir()?.join("claudia-statusline").join("presets");
    let preset_path = preset_dir.join(format!("{}.toml", name.to_lowercase()));

    if !preset_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&preset_path).ok()?;

    // Parse TOML to extract format string
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct PresetFile {
        format: Option<String>,
        #[serde(default)]
        separator: String, // Reserved for future use
    }

    let parsed: PresetFile = toml::from_str(&content).ok()?;
    parsed.format
}

/// List all available presets (built-in + user)
#[allow(dead_code)]
pub fn list_available_presets() -> Vec<String> {
    let mut presets = vec![
        "default".to_string(),
        "compact".to_string(),
        "detailed".to_string(),
        "minimal".to_string(),
        "power".to_string(),
    ];

    // Add user presets
    if let Some(preset_dir) = dirs::config_dir().map(|d| d.join("claudia-statusline").join("presets"))
    {
        if preset_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&preset_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().file_stem() {
                        if let Some(name_str) = name.to_str() {
                            let preset_name = name_str.to_lowercase();
                            if !presets.contains(&preset_name) {
                                presets.push(preset_name);
                            }
                        }
                    }
                }
            }
        }
    }

    presets
}

/// Resolve a color override string to an ANSI code
///
/// Supports:
/// - Named colors: "red", "green", "blue", "cyan", "yellow", "magenta", "white", "gray"
/// - Hex colors: "#FF5733" or "#F53"
/// - ANSI codes: "\x1b[32m" (passthrough)
/// - 256 colors: "38;5;123" (wrapped in \x1b[..m)
fn resolve_color_override(color: &str) -> String {
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

/// Layout renderer that handles template substitution
pub struct LayoutRenderer {
    /// The format template string
    template: String,
    /// Separator to use for {sep}
    separator: String,
}

impl LayoutRenderer {
    /// Create a new layout renderer from configuration
    pub fn from_config(config: &LayoutConfig) -> Self {
        let template = if config.format.is_empty() {
            get_preset_format(&config.preset).to_string()
        } else {
            config.format.clone()
        };

        Self {
            template,
            separator: config.separator.clone(),
        }
    }

    /// Create a renderer with a specific format string
    #[allow(dead_code)]
    pub fn with_format(format: &str, separator: &str) -> Self {
        Self {
            template: format.to_string(),
            separator: separator.to_string(),
        }
    }

    /// Render the template with the provided variables
    ///
    /// Variables are provided as a HashMap where:
    /// - Key: variable name without braces (e.g., "directory")
    /// - Value: the rendered component string (with colors)
    ///
    /// Unknown variables are replaced with empty string.
    /// The {sep} variable is replaced with the configured separator.
    pub fn render(&self, variables: &HashMap<String, String>) -> String {
        let mut result = self.template.clone();

        // Sanitize separator (user-provided, could contain control characters)
        // but preserve valid ANSI colors in template output
        let safe_separator = sanitize_for_terminal(&self.separator);

        // Replace {sep} with sanitized separator
        result = result.replace("{sep}", &safe_separator);

        // Replace all variables
        for (key, value) in variables {
            let placeholder = format!("{{{}}}", key);
            result = result.replace(&placeholder, value);
        }

        // Remove any unreplaced variables (unknown or empty)
        result = remove_unreplaced_variables(&result);

        // Clean up multiple separators (when components are empty)
        // Use same sanitized separator for consistent matching
        result = clean_separators(&result, &safe_separator);

        result
    }

    /// Check if the template uses a specific variable
    #[allow(dead_code)]
    pub fn uses_variable(&self, name: &str) -> bool {
        let placeholder = format!("{{{}}}", name);
        self.template.contains(&placeholder)
    }

    /// Get list of variables used in the template
    #[allow(dead_code)]
    pub fn get_used_variables(&self) -> Vec<String> {
        let mut variables = Vec::new();
        let mut chars = self.template.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                let mut var_name = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        if !var_name.is_empty() && var_name != "sep" {
                            variables.push(var_name);
                        }
                        break;
                    }
                    var_name.push(c);
                }
            }
        }

        variables
    }
}

/// Builder for creating the variables HashMap from statusline components.
///
/// Each method sets a variable that can be referenced in the layout template.
/// Variables are rendered with colors before being stored.
///
/// # Example
///
/// ```ignore
/// let variables = VariableBuilder::new()
///     .directory("~/projects/app", Some("cyan"))
///     .model("S4.5", Some("cyan"))
///     .cost(12.50, None)
///     .build();
/// ```
#[derive(Default)]
pub struct VariableBuilder {
    variables: HashMap<String, String>,
}

impl VariableBuilder {
    /// Create a new empty variable builder
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    /// Set a variable directly
    #[allow(dead_code)]
    pub fn set(mut self, key: &str, value: String) -> Self {
        if !value.is_empty() {
            self.variables.insert(key.to_string(), value);
        }
        self
    }

    /// Set directory variables ({directory}, {dir_short}) with optional config
    #[allow(dead_code)]
    pub fn directory(mut self, path: &str, short_path: &str, color: &str, reset: &str) -> Self {
        // Full shortened path
        if !path.is_empty() {
            self.variables.insert(
                "directory".to_string(),
                format!("{}{}{}", color, path, reset),
            );
        }
        // Basename only
        if !short_path.is_empty() {
            self.variables.insert(
                "dir_short".to_string(),
                format!("{}{}{}", color, short_path, reset),
            );
        }
        self
    }

    /// Set directory variables with component configuration
    ///
    /// Applies format, max_length, and color overrides from config.
    pub fn directory_with_config(
        mut self,
        full_path: &str,
        short_path: &str,
        basename: &str,
        default_color: &str,
        reset: &str,
        config: &DirectoryComponentConfig,
    ) -> Self {
        // Determine which color to use
        let color = if config.color.is_empty() {
            default_color.to_string()
        } else {
            resolve_color_override(&config.color)
        };

        // Apply truncation if configured (character-based, not byte-based for UTF-8 safety)
        let truncate = |s: &str| -> String {
            let char_count = s.chars().count();
            if config.max_length > 0 && char_count > config.max_length {
                let skip = char_count - config.max_length + 1;
                format!("…{}", s.chars().skip(skip).collect::<String>())
            } else {
                s.to_string()
            }
        };

        // Format based on config
        let display_value = match config.format.as_str() {
            "full" => truncate(full_path),
            "basename" => truncate(basename),
            _ => truncate(short_path), // "short" is default
        };

        if !display_value.is_empty() {
            self.variables.insert(
                "directory".to_string(),
                format!("{}{}{}", color, display_value, reset),
            );
        }

        // Also set dir_short for templates that want it
        if !basename.is_empty() {
            self.variables.insert(
                "dir_short".to_string(),
                format!("{}{}{}", color, truncate(basename), reset),
            );
        }

        self
    }

    /// Set git variables ({git}, {git_branch})
    #[allow(dead_code)]
    pub fn git(mut self, full_info: &str, branch: Option<&str>) -> Self {
        if !full_info.is_empty() {
            self.variables
                .insert("git".to_string(), full_info.to_string());
        }
        if let Some(b) = branch {
            if !b.is_empty() {
                self.variables
                    .insert("git_branch".to_string(), b.to_string());
            }
        }
        self
    }

    /// Set git variables with component configuration
    ///
    /// Applies format and show_when options from config.
    /// show_when: "always" (default), "dirty" (only when dirty), "never"
    #[allow(clippy::too_many_arguments)]
    pub fn git_with_config(
        mut self,
        full_info: &str,
        branch: Option<&str>,
        status_only: Option<&str>,
        is_dirty: bool,
        default_color: &str,
        reset: &str,
        config: &GitComponentConfig,
    ) -> Self {
        // Check show_when condition
        let should_show = match config.show_when.as_str() {
            "never" => false,
            "dirty" => is_dirty,
            _ => true, // "always" is default
        };

        if !should_show {
            return self;
        }

        // Determine color
        let color = if config.color.is_empty() {
            default_color.to_string()
        } else {
            resolve_color_override(&config.color)
        };

        // Format based on config
        match config.format.as_str() {
            "branch" => {
                if let Some(b) = branch {
                    if !b.is_empty() {
                        self.variables
                            .insert("git".to_string(), format!("{}{}{}", color, b, reset));
                    }
                }
            }
            "status" => {
                if let Some(s) = status_only {
                    if !s.is_empty() {
                        self.variables.insert("git".to_string(), s.to_string());
                    }
                }
            }
            _ => {
                // "full" is default
                if !full_info.is_empty() {
                    self.variables
                        .insert("git".to_string(), full_info.to_string());
                }
            }
        }

        // Always set git_branch for templates that want it
        if let Some(b) = branch {
            if !b.is_empty() {
                self.variables
                    .insert("git_branch".to_string(), format!("{}{}{}", color, b, reset));
            }
        }

        self
    }

    /// Set context variables ({context}, {context_pct}, {context_tokens})
    pub fn context(
        mut self,
        bar_display: &str,
        percentage: Option<u32>,
        tokens: Option<(u64, u64)>,
    ) -> Self {
        if !bar_display.is_empty() {
            self.variables
                .insert("context".to_string(), bar_display.to_string());
        }
        if let Some(pct) = percentage {
            self.variables
                .insert("context_pct".to_string(), pct.to_string());
        }
        if let Some((current, max)) = tokens {
            self.variables.insert(
                "context_tokens".to_string(),
                format!("{}k/{}k", current / 1000, max / 1000),
            );
        }
        self
    }

    /// Set model variables ({model}, {model_full})
    #[allow(dead_code)]
    pub fn model(mut self, abbreviation: &str, full_name: &str, color: &str, reset: &str) -> Self {
        if !abbreviation.is_empty() {
            self.variables.insert(
                "model".to_string(),
                format!("{}{}{}", color, abbreviation, reset),
            );
        }
        if !full_name.is_empty() {
            self.variables.insert(
                "model_full".to_string(),
                format!("{}{}{}", color, full_name, reset),
            );
        }
        self
    }

    /// Set model variables with component configuration
    ///
    /// Format options: "abbreviation" (default), "full", "version"
    pub fn model_with_config(
        mut self,
        abbreviation: &str,
        full_name: &str,
        version: &str,
        default_color: &str,
        reset: &str,
        config: &ModelComponentConfig,
    ) -> Self {
        let color = if config.color.is_empty() {
            default_color.to_string()
        } else {
            resolve_color_override(&config.color)
        };

        // Format based on config
        let display_value = match config.format.as_str() {
            "full" => full_name,
            "version" => version,
            _ => abbreviation, // "abbreviation" is default
        };

        if !display_value.is_empty() {
            self.variables.insert(
                "model".to_string(),
                format!("{}{}{}", color, display_value, reset),
            );
        }

        // Always set model_full for templates that want it
        if !full_name.is_empty() {
            self.variables.insert(
                "model_full".to_string(),
                format!("{}{}{}", color, full_name, reset),
            );
        }

        self
    }

    /// Set duration variable ({duration})
    pub fn duration(mut self, formatted: &str, color: &str, reset: &str) -> Self {
        if !formatted.is_empty() {
            self.variables.insert(
                "duration".to_string(),
                format!("{}{}{}", color, formatted, reset),
            );
        }
        self
    }

    /// Set cost variables ({cost}, {burn_rate}, {daily_total}, {cost_short})
    #[allow(dead_code)]
    pub fn cost(
        mut self,
        session_cost: Option<f64>,
        burn_rate: Option<f64>,
        daily_total: Option<f64>,
        cost_color: &str,
        rate_color: &str,
        reset: &str,
    ) -> Self {
        if let Some(cost) = session_cost {
            self.variables.insert(
                "cost".to_string(),
                format!("{}${:.2}{}", cost_color, cost, reset),
            );
            self.variables.insert(
                "cost_short".to_string(),
                format!("{}${:.0}{}", cost_color, cost, reset),
            );
        }
        if let Some(rate) = burn_rate {
            if rate > 0.0 {
                self.variables.insert(
                    "burn_rate".to_string(),
                    format!("{}${:.2}/hr{}", rate_color, rate, reset),
                );
            }
        }
        if let Some(daily) = daily_total {
            if daily > 0.0 {
                self.variables.insert(
                    "daily_total".to_string(),
                    format!("{}${:.2}{}", cost_color, daily, reset),
                );
            }
        }
        self
    }

    /// Set cost variables with component configuration
    ///
    /// Format options: "full" (default), "cost_only", "rate_only", "with_daily"
    #[allow(clippy::too_many_arguments)]
    pub fn cost_with_config(
        mut self,
        session_cost: Option<f64>,
        burn_rate: Option<f64>,
        daily_total: Option<f64>,
        default_cost_color: &str,
        rate_color: &str,
        reset: &str,
        config: &CostComponentConfig,
    ) -> Self {
        let cost_color = if config.color.is_empty() {
            default_cost_color.to_string()
        } else {
            resolve_color_override(&config.color)
        };

        // Always set individual variables for templates that want them
        if let Some(cost) = session_cost {
            self.variables.insert(
                "cost_short".to_string(),
                format!("{}${:.0}{}", cost_color, cost, reset),
            );
        }

        if let Some(rate) = burn_rate {
            if rate > 0.0 {
                self.variables.insert(
                    "burn_rate".to_string(),
                    format!("{}${:.2}/hr{}", rate_color, rate, reset),
                );
            }
        }

        if let Some(daily) = daily_total {
            if daily > 0.0 {
                self.variables.insert(
                    "daily_total".to_string(),
                    format!("{}${:.2}{}", cost_color, daily, reset),
                );
            }
        }

        // Build {cost} variable based on format config
        match config.format.as_str() {
            "cost_only" => {
                if let Some(cost) = session_cost {
                    self.variables.insert(
                        "cost".to_string(),
                        format!("{}${:.2}{}", cost_color, cost, reset),
                    );
                }
            }
            "rate_only" => {
                if let Some(rate) = burn_rate {
                    if rate > 0.0 {
                        self.variables.insert(
                            "cost".to_string(),
                            format!("{}${:.2}/hr{}", rate_color, rate, reset),
                        );
                    }
                }
            }
            "with_daily" => {
                let mut parts = Vec::new();
                if let Some(cost) = session_cost {
                    parts.push(format!("{}${:.2}{}", cost_color, cost, reset));
                }
                if let Some(daily) = daily_total {
                    if daily > 0.0 {
                        parts.push(format!("day:{}${:.2}{}", cost_color, daily, reset));
                    }
                }
                if !parts.is_empty() {
                    self.variables.insert("cost".to_string(), parts.join(" "));
                }
            }
            _ => {
                // "full" is default - cost with burn rate
                let mut parts = Vec::new();
                if let Some(cost) = session_cost {
                    parts.push(format!("{}${:.2}{}", cost_color, cost, reset));
                }
                if let Some(rate) = burn_rate {
                    if rate > 0.0 {
                        parts.push(format!("({}${:.2}/hr{})", rate_color, rate, reset));
                    }
                }
                if !parts.is_empty() {
                    self.variables.insert("cost".to_string(), parts.join(" "));
                }
            }
        }

        self
    }

    /// Set lines changed variable ({lines})
    pub fn lines_changed(
        mut self,
        added: u64,
        removed: u64,
        add_color: &str,
        remove_color: &str,
        reset: &str,
    ) -> Self {
        if added > 0 || removed > 0 {
            let mut parts = Vec::new();
            if added > 0 {
                parts.push(format!("{}+{}{}", add_color, added, reset));
            }
            if removed > 0 {
                parts.push(format!("{}-{}{}", remove_color, removed, reset));
            }
            self.variables.insert("lines".to_string(), parts.join(" "));
        }
        self
    }

    /// Set token rate variable ({token_rate})
    #[allow(dead_code)]
    pub fn token_rate(mut self, rate: f64, color: &str, reset: &str) -> Self {
        if rate > 0.0 {
            self.variables.insert(
                "token_rate".to_string(),
                format!("{}{:.1} tok/s{}", color, rate, reset),
            );
        }
        self
    }

    /// Build the final HashMap
    pub fn build(self) -> HashMap<String, String> {
        self.variables
    }
}

/// Remove unreplaced {variable} placeholders from the string
fn remove_unreplaced_variables(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            // Check if this is a variable placeholder
            let mut var_content = String::new();
            let mut found_close = false;

            for c in chars.by_ref() {
                if c == '}' {
                    found_close = true;
                    break;
                }
                var_content.push(c);
            }

            if !found_close {
                // Not a valid placeholder, keep the opening brace
                result.push('{');
                result.push_str(&var_content);
            }
            // If found_close is true, we skip the whole {var} placeholder
        } else {
            result.push(c);
        }
    }

    result
}

/// Clean up multiple consecutive separators and trailing separators
fn clean_separators(s: &str, separator: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_substitution() {
        let renderer = LayoutRenderer::with_format("{directory} {model}", "");
        let mut vars = HashMap::new();
        vars.insert("directory".to_string(), "~/test".to_string());
        vars.insert("model".to_string(), "S4.5".to_string());

        let result = renderer.render(&vars);
        assert_eq!(result, "~/test S4.5");
    }

    #[test]
    fn test_separator_substitution() {
        let renderer = LayoutRenderer::with_format("{directory}{sep}{model}", " • ");
        let mut vars = HashMap::new();
        vars.insert("directory".to_string(), "~/test".to_string());
        vars.insert("model".to_string(), "S4.5".to_string());

        let result = renderer.render(&vars);
        assert_eq!(result, "~/test • S4.5");
    }

    #[test]
    fn test_missing_variable_removed() {
        let renderer = LayoutRenderer::with_format("{directory}{sep}{unknown}{sep}{model}", " • ");
        let mut vars = HashMap::new();
        vars.insert("directory".to_string(), "~/test".to_string());
        vars.insert("model".to_string(), "S4.5".to_string());

        let result = renderer.render(&vars);
        assert_eq!(result, "~/test • S4.5");
    }

    #[test]
    fn test_multiline() {
        let renderer = LayoutRenderer::with_format("{directory}\n{model}", "");
        let mut vars = HashMap::new();
        vars.insert("directory".to_string(), "~/test".to_string());
        vars.insert("model".to_string(), "S4.5".to_string());

        let result = renderer.render(&vars);
        assert_eq!(result, "~/test\nS4.5");
    }

    #[test]
    fn test_preset_default() {
        let format = get_preset_format("default");
        assert!(format.contains("{directory}"));
        assert!(format.contains("{model}"));
    }

    #[test]
    fn test_preset_compact() {
        let format = get_preset_format("compact");
        assert!(format.contains("{dir_short}"));
        assert!(!format.contains("{sep}"));
    }

    #[test]
    fn test_get_used_variables() {
        let renderer = LayoutRenderer::with_format("{directory}{sep}{model} {cost}", " • ");
        let vars = renderer.get_used_variables();
        assert!(vars.contains(&"directory".to_string()));
        assert!(vars.contains(&"model".to_string()));
        assert!(vars.contains(&"cost".to_string()));
        assert!(!vars.contains(&"sep".to_string())); // sep is filtered out
    }

    #[test]
    fn test_from_config_preset() {
        let config = LayoutConfig {
            preset: "compact".to_string(),
            format: String::new(),
            separator: " | ".to_string(),
            ..Default::default()
        };

        let renderer = LayoutRenderer::from_config(&config);
        assert!(renderer.template.contains("{dir_short}"));
    }

    #[test]
    fn test_from_config_custom_format() {
        let config = LayoutConfig {
            preset: "default".to_string(),
            format: "{custom} {format}".to_string(),
            separator: " | ".to_string(),
            ..Default::default()
        };

        let renderer = LayoutRenderer::from_config(&config);
        assert_eq!(renderer.template, "{custom} {format}");
    }

    #[test]
    fn test_clean_double_separators() {
        let renderer = LayoutRenderer::with_format("{a}{sep}{b}{sep}{c}", " • ");
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "A".to_string());
        // b is missing
        vars.insert("c".to_string(), "C".to_string());

        let result = renderer.render(&vars);
        assert_eq!(result, "A • C");
    }

    #[test]
    fn test_empty_template() {
        let renderer = LayoutRenderer::with_format("", " • ");
        let vars = HashMap::new();
        let result = renderer.render(&vars);
        assert_eq!(result, "");
    }

    #[test]
    fn test_unbalanced_brace_opening() {
        // Unclosed brace should be preserved (not a valid variable)
        let renderer = LayoutRenderer::with_format("{unclosed text", "");
        let vars = HashMap::new();
        let result = renderer.render(&vars);
        assert_eq!(result, "{unclosed text");
    }

    #[test]
    fn test_unbalanced_brace_closing() {
        // Extra closing brace should be preserved
        let renderer = LayoutRenderer::with_format("text} more", "");
        let vars = HashMap::new();
        let result = renderer.render(&vars);
        assert_eq!(result, "text} more");
    }

    #[test]
    fn test_nested_braces() {
        // Nested braces - outer { starts variable capture, inner content becomes var name
        let renderer = LayoutRenderer::with_format("{{nested}}", "");
        let vars = HashMap::new();
        let result = renderer.render(&vars);
        // {{nested}} -> variable name is "{nested", removed, leaving trailing "}"
        assert_eq!(result, "}");
    }

    #[test]
    fn test_separator_with_control_chars() {
        // Control characters in separator should be sanitized
        let renderer = LayoutRenderer::with_format("{a}{sep}{b}", "\x07bell\x00null");
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "A".to_string());
        vars.insert("b".to_string(), "B".to_string());

        let result = renderer.render(&vars);
        // Bell and null should be stripped, leaving just "bellnull"
        assert_eq!(result, "AbellnullB");
    }

    #[test]
    fn test_only_separators() {
        // Template with only separators and missing variables
        let renderer = LayoutRenderer::with_format("{sep}{missing}{sep}", " | ");
        let vars = HashMap::new();
        let result = renderer.render(&vars);
        // All separators cleaned up when no content
        assert_eq!(result, "");
    }

    #[test]
    fn test_whitespace_only_variables() {
        // Empty string variables should be treated as missing
        let renderer = LayoutRenderer::with_format("{a}{sep}{b}{sep}{c}", " • ");
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "A".to_string());
        vars.insert("b".to_string(), "".to_string()); // Empty value
        vars.insert("c".to_string(), "C".to_string());

        let result = renderer.render(&vars);
        // Empty b is replaced with "", separators cleaned up
        assert_eq!(result, "A • C");
    }

    // VariableBuilder tests
    #[test]
    fn test_variable_builder_basic() {
        let vars = VariableBuilder::new()
            .set("custom", "value".to_string())
            .build();
        assert_eq!(vars.get("custom"), Some(&"value".to_string()));
    }

    #[test]
    fn test_variable_builder_directory() {
        let vars = VariableBuilder::new()
            .directory("~/projects/app", "app", "\x1b[36m", "\x1b[0m")
            .build();

        assert!(vars.get("directory").unwrap().contains("~/projects/app"));
        assert!(vars.get("dir_short").unwrap().contains("app"));
    }

    #[test]
    fn test_variable_builder_cost() {
        let vars = VariableBuilder::new()
            .cost(
                Some(12.50),
                Some(3.25),
                Some(45.00),
                "\x1b[32m", // green
                "\x1b[90m", // gray
                "\x1b[0m",
            )
            .build();

        assert!(vars.get("cost").unwrap().contains("$12.50"));
        assert!(vars.get("burn_rate").unwrap().contains("$3.25/hr"));
        assert!(vars.get("daily_total").unwrap().contains("$45.00"));
        assert!(vars.get("cost_short").unwrap().contains("$12"));
    }

    #[test]
    fn test_variable_builder_lines_changed() {
        let vars = VariableBuilder::new()
            .lines_changed(100, 50, "\x1b[32m", "\x1b[31m", "\x1b[0m")
            .build();

        let lines = vars.get("lines").unwrap();
        assert!(lines.contains("+100"));
        assert!(lines.contains("-50"));
    }

    #[test]
    fn test_variable_builder_empty_values_ignored() {
        let vars = VariableBuilder::new()
            .set("empty", "".to_string())
            .directory("", "", "", "")
            .build();

        // Empty values should not be inserted
        assert!(!vars.contains_key("empty"));
        assert!(!vars.contains_key("directory"));
    }

    #[test]
    fn test_variable_builder_with_renderer() {
        // Integration test: builder + renderer
        let vars = VariableBuilder::new()
            .set("directory", "~/test".to_string())
            .set("model", "S4.5".to_string())
            .build();

        let renderer = LayoutRenderer::with_format("{directory} {model}", "");
        let result = renderer.render(&vars);
        assert_eq!(result, "~/test S4.5");
    }

    // Component configuration tests
    #[test]
    fn test_directory_with_config_format_short() {
        let config = DirectoryComponentConfig {
            format: "short".to_string(),
            max_length: 0,
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .directory_with_config(
                "/home/user/projects/app",
                "~/projects/app",
                "app",
                "",
                "",
                &config,
            )
            .build();

        assert_eq!(vars.get("directory"), Some(&"~/projects/app".to_string()));
    }

    #[test]
    fn test_directory_with_config_format_basename() {
        let config = DirectoryComponentConfig {
            format: "basename".to_string(),
            max_length: 0,
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .directory_with_config(
                "/home/user/projects/app",
                "~/projects/app",
                "app",
                "",
                "",
                &config,
            )
            .build();

        assert_eq!(vars.get("directory"), Some(&"app".to_string()));
    }

    #[test]
    fn test_directory_with_config_max_length() {
        let config = DirectoryComponentConfig {
            format: "short".to_string(),
            max_length: 10,
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .directory_with_config(
                "/home/user/projects/app",
                "~/projects/app",
                "app",
                "",
                "",
                &config,
            )
            .build();

        let dir = vars.get("directory").unwrap();
        assert!(dir.starts_with('…'));
        // Check character count, not byte count (… is 3 bytes)
        assert!(dir.chars().count() <= 11); // 10 chars + ellipsis
    }

    #[test]
    fn test_directory_with_config_color_override() {
        let config = DirectoryComponentConfig {
            format: "short".to_string(),
            max_length: 0,
            color: "red".to_string(),
        };
        let vars = VariableBuilder::new()
            .directory_with_config(
                "/home/user/projects/app",
                "~/projects/app",
                "app",
                "\x1b[36m", // cyan (default)
                "\x1b[0m",
                &config,
            )
            .build();

        let dir = vars.get("directory").unwrap();
        assert!(dir.contains("\x1b[31m")); // red override
        assert!(!dir.contains("\x1b[36m")); // not default cyan
    }

    #[test]
    fn test_directory_with_config_utf8_truncation() {
        // Test that truncation works correctly with multi-byte UTF-8 characters
        // This would panic with byte-based slicing if truncation cuts mid-character
        let config = DirectoryComponentConfig {
            format: "short".to_string(),
            max_length: 8,
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .directory_with_config(
                "/home/用户/项目/приложение", // Mixed UTF-8: Chinese, Russian
                "~/用户/项目/приложение",
                "приложение", // Russian: 10 characters but 20 bytes
                "",
                "",
                &config,
            )
            .build();

        let dir = vars.get("directory").unwrap();
        // Should truncate to 8 chars (including ellipsis replacement)
        // … + 7 chars from the end = 8 visible characters
        assert!(dir.starts_with('…'));
        assert_eq!(dir.chars().count(), 8); // Exactly 8 characters
        // Should not panic - this is the main test
    }

    #[test]
    fn test_git_with_config_show_when_dirty_when_clean() {
        let config = GitComponentConfig {
            format: "full".to_string(),
            show_when: "dirty".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .git_with_config(
                "main",
                Some("main"),
                None,
                false, // is_dirty = false
                "",
                "",
                &config,
            )
            .build();

        // Should not show git when clean and show_when = "dirty"
        assert!(!vars.contains_key("git"));
    }

    #[test]
    fn test_git_with_config_show_when_dirty_when_dirty() {
        let config = GitComponentConfig {
            format: "full".to_string(),
            show_when: "dirty".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .git_with_config(
                "main +2",
                Some("main"),
                Some("+2"),
                true, // is_dirty = true
                "",
                "",
                &config,
            )
            .build();

        // Should show git when dirty
        assert!(vars.contains_key("git"));
    }

    #[test]
    fn test_git_with_config_format_branch_only() {
        let config = GitComponentConfig {
            format: "branch".to_string(),
            show_when: "always".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .git_with_config(
                "main +2 ~1",
                Some("main"),
                Some("+2 ~1"),
                true,
                "",
                "",
                &config,
            )
            .build();

        assert_eq!(vars.get("git"), Some(&"main".to_string()));
    }

    #[test]
    fn test_model_with_config_format_full() {
        let config = ModelComponentConfig {
            format: "full".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .model_with_config("S4.5", "Claude Sonnet 4.5", "4.5", "", "", &config)
            .build();

        assert_eq!(vars.get("model"), Some(&"Claude Sonnet 4.5".to_string()));
    }

    #[test]
    fn test_model_with_config_format_version() {
        let config = ModelComponentConfig {
            format: "version".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .model_with_config("S4.5", "Claude Sonnet 4.5", "4.5", "", "", &config)
            .build();

        assert_eq!(vars.get("model"), Some(&"4.5".to_string()));
    }

    #[test]
    fn test_cost_with_config_cost_only() {
        let config = CostComponentConfig {
            format: "cost_only".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .cost_with_config(Some(12.50), Some(3.25), Some(45.00), "", "", "", &config)
            .build();

        let cost = vars.get("cost").unwrap();
        assert!(cost.contains("12.50"));
        assert!(!cost.contains("/hr")); // No burn rate
    }

    #[test]
    fn test_cost_with_config_rate_only() {
        let config = CostComponentConfig {
            format: "rate_only".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .cost_with_config(Some(12.50), Some(3.25), Some(45.00), "", "", "", &config)
            .build();

        let cost = vars.get("cost").unwrap();
        assert!(cost.contains("3.25/hr"));
        assert!(!cost.contains("12.50")); // No session cost
    }

    #[test]
    fn test_cost_with_config_with_daily() {
        let config = CostComponentConfig {
            format: "with_daily".to_string(),
            color: String::new(),
        };
        let vars = VariableBuilder::new()
            .cost_with_config(Some(12.50), Some(3.25), Some(45.00), "", "", "", &config)
            .build();

        let cost = vars.get("cost").unwrap();
        assert!(cost.contains("12.50"));
        assert!(cost.contains("day:"));
        assert!(cost.contains("45.00"));
    }

    #[test]
    fn test_resolve_color_override_named() {
        assert_eq!(resolve_color_override("red"), "\x1b[31m");
        assert_eq!(resolve_color_override("green"), "\x1b[32m");
        assert_eq!(resolve_color_override("cyan"), "\x1b[36m");
    }

    #[test]
    fn test_resolve_color_override_hex() {
        assert_eq!(resolve_color_override("#FF0000"), "\x1b[38;2;255;0;0m");
        assert_eq!(resolve_color_override("#F00"), "\x1b[38;2;255;0;0m");
    }

    #[test]
    fn test_resolve_color_override_256() {
        assert_eq!(resolve_color_override("38;5;208"), "\x1b[38;5;208m");
    }

    #[test]
    fn test_resolve_color_override_passthrough() {
        assert_eq!(resolve_color_override("\x1b[32m"), "\x1b[32m");
    }

    // Preset integration tests
    #[test]
    fn test_preset_default_format() {
        let format = get_preset_format("default");
        assert!(format.contains("{directory}"));
        assert!(format.contains("{git}"));
        assert!(format.contains("{context}"));
        assert!(format.contains("{model}"));
        assert!(format.contains("{cost}"));
    }

    #[test]
    fn test_preset_compact_format() {
        let format = get_preset_format("compact");
        assert!(format.contains("{dir_short}"));
        assert!(format.contains("{git_branch}"));
        assert!(format.contains("{model}"));
        assert!(format.contains("{cost_short}"));
        assert!(!format.contains("{sep}")); // Compact uses spaces
    }

    #[test]
    fn test_preset_detailed_format() {
        let format = get_preset_format("detailed");
        assert!(format.contains("{directory}"));
        assert!(format.contains("{context}"));
        assert!(format.contains('\n')); // Multi-line
    }

    #[test]
    fn test_preset_minimal_format() {
        let format = get_preset_format("minimal");
        assert!(format.contains("{directory}"));
        assert!(format.contains("{model}"));
        assert!(!format.contains("{git}")); // No git in minimal
        assert!(!format.contains("{cost}")); // No cost in minimal
    }

    #[test]
    fn test_preset_power_format() {
        let format = get_preset_format("power");
        assert!(format.contains("{directory}"));
        assert!(format.contains("{git}"));
        assert!(format.contains("{context}"));
        assert!(format.contains("{model}"));
        assert!(format.contains("{duration}"));
        assert!(format.contains("{lines}"));
        assert!(format.contains("{cost}"));
        assert!(format.contains("{burn_rate}"));
        assert!(format.contains('\n')); // Multi-line
    }

    #[test]
    fn test_preset_case_insensitive() {
        assert_eq!(get_preset_format("COMPACT"), get_preset_format("compact"));
        assert_eq!(get_preset_format("Detailed"), get_preset_format("detailed"));
        assert_eq!(get_preset_format("MINIMAL"), get_preset_format("minimal"));
    }

    #[test]
    fn test_unknown_preset_falls_back_to_default() {
        assert_eq!(get_preset_format("unknown"), get_preset_format("default"));
        assert_eq!(get_preset_format(""), get_preset_format("default"));
        assert_eq!(get_preset_format("random"), get_preset_format("default"));
    }

    #[test]
    fn test_list_available_presets() {
        let presets = list_available_presets();
        assert!(presets.contains(&"default".to_string()));
        assert!(presets.contains(&"compact".to_string()));
        assert!(presets.contains(&"detailed".to_string()));
        assert!(presets.contains(&"minimal".to_string()));
        assert!(presets.contains(&"power".to_string()));
        assert!(presets.len() >= 5);
    }

    #[test]
    fn test_preset_rendering_default() {
        let format = get_preset_format("default");
        let renderer = LayoutRenderer::with_format(&format, " • ");

        let vars = VariableBuilder::new()
            .set("directory", "~/test".to_string())
            .set("git", "main".to_string())
            .set("context", "75%".to_string())
            .set("model", "S4.5".to_string())
            .set("cost", "$12".to_string())
            .build();

        let result = renderer.render(&vars);
        assert!(result.contains("~/test"));
        assert!(result.contains("main"));
        assert!(result.contains("75%"));
        assert!(result.contains("S4.5"));
        assert!(result.contains("$12"));
    }

    #[test]
    fn test_preset_rendering_compact() {
        let format = get_preset_format("compact");
        let renderer = LayoutRenderer::with_format(&format, "");

        let vars = VariableBuilder::new()
            .set("dir_short", "test".to_string())
            .set("git_branch", "main".to_string())
            .set("model", "S4.5".to_string())
            .set("cost_short", "$12".to_string())
            .build();

        let result = renderer.render(&vars);
        assert!(result.contains("test"));
        assert!(result.contains("main"));
        assert!(result.contains("S4.5"));
        assert!(result.contains("$12"));
    }

    #[test]
    fn test_preset_rendering_minimal() {
        let format = get_preset_format("minimal");
        let renderer = LayoutRenderer::with_format(&format, "");

        let vars = VariableBuilder::new()
            .set("directory", "~/test".to_string())
            .set("model", "S4.5".to_string())
            .build();

        let result = renderer.render(&vars);
        assert!(result.contains("~/test"));
        assert!(result.contains("S4.5"));
    }
}
