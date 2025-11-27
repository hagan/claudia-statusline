//! Layout rendering module for customizable statusline format.
//!
//! This module provides template-based rendering of the statusline,
//! allowing users to customize the format and order of components.

use std::collections::HashMap;

use crate::config::LayoutConfig;
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
pub fn get_preset_format(preset: &str) -> &'static str {
    match preset.to_lowercase().as_str() {
        "compact" => PRESET_COMPACT,
        "detailed" => PRESET_DETAILED,
        "minimal" => PRESET_MINIMAL,
        "power" => PRESET_POWER,
        _ => PRESET_DEFAULT, // "default" or unknown
    }
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

    /// Set directory variables ({directory}, {dir_short})
    pub fn directory(mut self, path: &str, short_path: &str, color: &str, reset: &str) -> Self {
        // Full shortened path
        if !path.is_empty() {
            self.variables
                .insert("directory".to_string(), format!("{}{}{}", color, path, reset));
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

    /// Set git variables ({git}, {git_branch})
    pub fn git(mut self, full_info: &str, branch: Option<&str>) -> Self {
        if !full_info.is_empty() {
            self.variables
                .insert("git".to_string(), full_info.to_string());
        }
        if let Some(b) = branch {
            if !b.is_empty() {
                self.variables.insert("git_branch".to_string(), b.to_string());
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
}
