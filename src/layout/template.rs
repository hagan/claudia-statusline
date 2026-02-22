//! Template parsing and rendering for the layout engine.

use std::collections::HashMap;

use super::format::clean_separators;
use super::presets::get_preset_format;
use crate::config::LayoutConfig;
use crate::utils::sanitize_for_terminal;

/// Layout renderer that handles template substitution
pub struct LayoutRenderer {
    /// The format template string
    pub(super) template: String,
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
