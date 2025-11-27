//! Layout rendering module for customizable statusline format.
//!
//! This module provides template-based rendering of the statusline,
//! allowing users to customize the format and order of components.

use std::collections::HashMap;

use crate::config::LayoutConfig;

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

        // Replace {sep} with separator
        result = result.replace("{sep}", &self.separator);

        // Replace all variables
        for (key, value) in variables {
            let placeholder = format!("{{{}}}", key);
            result = result.replace(&placeholder, value);
        }

        // Remove any unreplaced variables (unknown or empty)
        result = remove_unreplaced_variables(&result);

        // Clean up multiple separators (when components are empty)
        result = clean_separators(&result, &self.separator);

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
}
