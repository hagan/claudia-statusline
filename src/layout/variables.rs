//! Variable builder for creating the template substitution HashMap.

use std::collections::HashMap;

use super::format::{format_rate_with_unit, format_token_count, resolve_color_override};
use crate::config::{
    ContextComponentConfig, CostComponentConfig, DirectoryComponentConfig, GitComponentConfig,
    ModelComponentConfig,
};

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
    #[allow(dead_code)]
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

    /// Set context variables with component configuration
    ///
    /// Format options: "full" (default), "bar", "percent", "tokens"
    pub fn context_with_config(
        mut self,
        bar_only: &str,
        percentage: Option<u32>,
        tokens: Option<(u64, u64)>,
        config: &ContextComponentConfig,
    ) -> Self {
        // Always set individual variables for templates that want them
        if let Some(pct) = percentage {
            self.variables
                .insert("context_pct".to_string(), format!("{}%", pct));
        }
        if let Some((current, max)) = tokens {
            self.variables.insert(
                "context_tokens".to_string(),
                format!("{}k/{}k", current / 1000, max / 1000),
            );
        }

        // Build {context} variable based on format config
        let context_value = match config.format.as_str() {
            "bar" => {
                // Just the progress bar
                if !bar_only.is_empty() {
                    Some(bar_only.to_string())
                } else {
                    None
                }
            }
            "percent" => {
                // Just the percentage
                percentage.map(|pct| format!("{}%", pct))
            }
            "tokens" => {
                // Just the token counts
                tokens.map(|(current, max)| format!("{}k/{}k", current / 1000, max / 1000))
            }
            _ => {
                // "full" is default - percentage + bar + optional tokens
                let mut parts = Vec::new();
                if let Some(pct) = percentage {
                    parts.push(format!("{}%", pct));
                }
                if !bar_only.is_empty() {
                    parts.push(bar_only.to_string());
                }
                if config.show_tokens {
                    if let Some((current, max)) = tokens {
                        parts.push(format!("{}k/{}k", current / 1000, max / 1000));
                    }
                }
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(" "))
                }
            }
        };

        if let Some(value) = context_value {
            self.variables.insert("context".to_string(), value);
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
    /// Format options: "abbreviation" (default), "full", "name", "version"
    #[allow(clippy::too_many_arguments)]
    pub fn model_with_config(
        mut self,
        abbreviation: &str,
        full_name: &str,
        family_name: &str,
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
            "name" => family_name,
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

        // Always set model_name for templates that want just the family name
        if !family_name.is_empty() {
            self.variables.insert(
                "model_name".to_string(),
                format!("{}{}{}", color, family_name, reset),
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

    /// Set token rate with component configuration ({token_rate})
    ///
    /// Supports different formats, time units, and session/daily totals.
    #[allow(dead_code)]
    pub fn token_rate_with_config(
        mut self,
        rate: f64,
        session_total: Option<u64>,
        daily_total: Option<u64>,
        default_color: &str,
        reset: &str,
        config: &crate::config::TokenRateComponentConfig,
    ) -> Self {
        if rate <= 0.0 && session_total.is_none() && daily_total.is_none() {
            return self;
        }

        let color = if config.color.is_empty() {
            default_color.to_string()
        } else {
            resolve_color_override(&config.color)
        };

        // Format rate based on time_unit
        let rate_str = if rate > 0.0 {
            let (adjusted_rate, unit) = match config.time_unit.as_str() {
                "minute" => (rate * 60.0, "tok/min"),
                "hour" => (rate * 3600.0, "tok/hr"),
                _ => (rate, "tok/s"), // default to second
            };
            format_rate_with_unit(adjusted_rate, unit, &color, reset)
        } else {
            String::new()
        };

        // Always set individual variables for templates
        if !rate_str.is_empty() {
            self.variables
                .insert("token_rate_only".to_string(), rate_str.clone());
        }

        if let Some(session) = session_total {
            self.variables.insert(
                "token_session_total".to_string(),
                format!("{}{}{}", color, format_token_count(session), reset),
            );
        }

        if let Some(daily) = daily_total {
            self.variables.insert(
                "token_daily_total".to_string(),
                format!("{}day: {}{}", color, format_token_count(daily), reset),
            );
        }

        // Build {token_rate} variable based on format config
        let token_rate_str = match config.format.as_str() {
            "with_session" => {
                let mut parts = Vec::new();
                if !rate_str.is_empty() {
                    parts.push(rate_str);
                }
                if let Some(session) = session_total {
                    parts.push(format!("{}{}{}", color, format_token_count(session), reset));
                }
                parts.join(" • ")
            }
            "with_daily" => {
                let mut parts = Vec::new();
                if !rate_str.is_empty() {
                    parts.push(rate_str);
                }
                if let Some(daily) = daily_total {
                    parts.push(format!(
                        "{}(day: {}){}",
                        color,
                        format_token_count(daily),
                        reset
                    ));
                }
                parts.join(" ")
            }
            "full" => {
                let mut parts = Vec::new();
                if !rate_str.is_empty() {
                    parts.push(rate_str);
                }
                if let Some(session) = session_total {
                    parts.push(format!("{}{}{}", color, format_token_count(session), reset));
                }
                let main_part = parts.join(" • ");
                if let Some(daily) = daily_total {
                    format!(
                        "{} {}(day: {}){}",
                        main_part,
                        color,
                        format_token_count(daily),
                        reset
                    )
                } else {
                    main_part
                }
            }
            _ => rate_str, // "rate_only" or default
        };

        if !token_rate_str.is_empty() {
            self.variables
                .insert("token_rate".to_string(), token_rate_str);
        }

        self
    }

    /// Set token rate with full metrics and respect rate_display config
    ///
    /// Exposes individual rate variables and respects the rate_display setting:
    /// - "both": Shows both input and output rates
    /// - "output_only": Shows only output rate
    /// - "input_only": Shows only input rate
    #[allow(dead_code)]
    pub fn token_rate_with_metrics(
        mut self,
        metrics: &crate::stats::TokenRateMetrics,
        default_color: &str,
        reset: &str,
        component_config: &crate::config::TokenRateComponentConfig,
        token_rate_config: &crate::config::TokenRateConfig,
    ) -> Self {
        let color = if component_config.color.is_empty() {
            default_color.to_string()
        } else {
            resolve_color_override(&component_config.color)
        };

        // Get time unit multiplier and suffix
        let (time_mult, unit_suffix) = match component_config.time_unit.as_str() {
            "minute" => (60.0, "tok/min"),
            "hour" => (3600.0, "tok/hr"),
            _ => (1.0, "tok/s"),
        };

        // Format individual rates
        let effective_input_rate = metrics.input_rate + metrics.cache_read_rate;
        let input_rate_str =
            format_rate_with_unit(effective_input_rate * time_mult, unit_suffix, &color, reset);
        let output_rate_str =
            format_rate_with_unit(metrics.output_rate * time_mult, unit_suffix, &color, reset);
        let cache_rate_str = format_rate_with_unit(
            metrics.cache_read_rate * time_mult,
            unit_suffix,
            &color,
            reset,
        );
        let total_rate_str =
            format_rate_with_unit(metrics.total_rate * time_mult, unit_suffix, &color, reset);

        // Set individual rate variables for templates
        self.variables
            .insert("token_input_rate".to_string(), input_rate_str.clone());
        self.variables
            .insert("token_output_rate".to_string(), output_rate_str.clone());
        self.variables
            .insert("token_rate_only".to_string(), total_rate_str.clone());

        // Set cache-related variables only if cache_metrics is enabled
        if token_rate_config.cache_metrics {
            self.variables
                .insert("token_cache_rate".to_string(), cache_rate_str);
            if let Some(hit_ratio) = metrics.cache_hit_ratio {
                let cache_pct = (hit_ratio * 100.0) as u8;
                self.variables
                    .insert("token_cache_hit".to_string(), format!("{}%", cache_pct));

                if let Some(roi) = metrics.cache_roi {
                    let roi_str = if roi.is_infinite() {
                        "∞".to_string()
                    } else {
                        format!("{:.1}x", roi)
                    };
                    self.variables
                        .insert("token_cache_roi".to_string(), roi_str);
                }
            }
        }

        // Set session and daily totals
        self.variables.insert(
            "token_session_total".to_string(),
            format!(
                "{}{}{}",
                color,
                format_token_count(metrics.session_total_tokens),
                reset
            ),
        );
        self.variables.insert(
            "token_daily_total".to_string(),
            format!(
                "{}day: {}{}",
                color,
                format_token_count(metrics.daily_total_tokens),
                reset
            ),
        );

        // Build {token_rate} based on display_mode and rate_display
        let rate_display_str = match token_rate_config.display_mode.as_str() {
            "detailed" => {
                // Respect rate_display config
                match token_rate_config.rate_display.as_str() {
                    "output_only" => format!("{}Out:{}{}", color, output_rate_str, reset),
                    "input_only" => format!("{}In:{}{}", color, input_rate_str, reset),
                    _ => format!(
                        "{}In:{} Out:{}{}",
                        color, input_rate_str, output_rate_str, reset
                    ),
                }
            }
            "cache_only" => {
                // Only show cache metrics if enabled in config
                if token_rate_config.cache_metrics {
                    if let Some(hit_ratio) = metrics.cache_hit_ratio {
                        let cache_pct = (hit_ratio * 100.0) as u8;
                        if let Some(roi) = metrics.cache_roi {
                            if roi.is_infinite() {
                                format!("{}Cache:{}% (∞ ROI){}", color, cache_pct, reset)
                            } else {
                                format!("{}Cache:{}% ({:.1}x ROI){}", color, cache_pct, roi, reset)
                            }
                        } else {
                            format!("{}Cache:{}%{}", color, cache_pct, reset)
                        }
                    } else {
                        total_rate_str.clone()
                    }
                } else {
                    // cache_metrics disabled, fall back to total rate
                    total_rate_str.clone()
                }
            }
            _ => total_rate_str.clone(), // "summary" or default
        };

        // Build final token_rate variable based on format
        let token_rate_str = match component_config.format.as_str() {
            "with_session" => {
                format!(
                    "{} • {}{}{}",
                    rate_display_str,
                    color,
                    format_token_count(metrics.session_total_tokens),
                    reset
                )
            }
            "with_daily" => {
                format!(
                    "{} {}(day: {}){}",
                    rate_display_str,
                    color,
                    format_token_count(metrics.daily_total_tokens),
                    reset
                )
            }
            "full" => {
                format!(
                    "{} • {}{}{} {}(day: {}){}",
                    rate_display_str,
                    color,
                    format_token_count(metrics.session_total_tokens),
                    reset,
                    color,
                    format_token_count(metrics.daily_total_tokens),
                    reset
                )
            }
            _ => rate_display_str, // "rate_only" or default
        };

        self.variables
            .insert("token_rate".to_string(), token_rate_str);

        self
    }

    /// Build the final HashMap
    pub fn build(self) -> HashMap<String, String> {
        self.variables
    }
}
