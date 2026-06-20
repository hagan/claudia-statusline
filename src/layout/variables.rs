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

    /// Set rate-limit variables ({rate_limits}, {rate_limit_5h}, {rate_limit_7d},
    /// {rate_limit_5h_reset}, {rate_limit_7d_reset}).
    ///
    /// Sourced from Claude Code's `rate_limits` payload (Pro/Max only). Callers
    /// pass each window's already-rendered piece (e.g. `5h:24%`, or `5h:24%
    /// (2h13m)` when the reset countdown is enabled) plus the bare countdown
    /// string (e.g. `2h13m`), which is always exposed as its own variable.
    /// `{rate_limits}` is the space-joined combination of the pieces. Variables
    /// are absent when not provided, so referencing them in a template is the opt-in.
    pub fn rate_limits(
        mut self,
        five_hour: Option<&str>,
        five_hour_reset: Option<&str>,
        seven_day: Option<&str>,
        seven_day_reset: Option<&str>,
        color: &str,
        reset: &str,
    ) -> Self {
        let mut combined = Vec::new();
        if let Some(piece) = five_hour {
            let s = format!("{}{}{}", color, piece, reset);
            self.variables
                .insert("rate_limit_5h".to_string(), s.clone());
            combined.push(s);
        }
        if let Some(cd) = five_hour_reset {
            self.variables.insert(
                "rate_limit_5h_reset".to_string(),
                format!("{}{}{}", color, cd, reset),
            );
        }
        if let Some(piece) = seven_day {
            let s = format!("{}{}{}", color, piece, reset);
            self.variables
                .insert("rate_limit_7d".to_string(), s.clone());
            combined.push(s);
        }
        if let Some(cd) = seven_day_reset {
            self.variables.insert(
                "rate_limit_7d_reset".to_string(),
                format!("{}{}{}", color, cd, reset),
            );
        }
        if !combined.is_empty() {
            self.variables
                .insert("rate_limits".to_string(), combined.join(" "));
        }
        self
    }

    /// Set session-metadata variables from the modern payload:
    /// `{effort}` (e.g. `xhigh`), `{cc_version}` (e.g. `v2.1.90`), `{over_200k}`
    /// (`200k+` when the response crossed the fixed 200k threshold), and
    /// `{repo}` (`owner/name`). Variables are absent when not provided, so
    /// referencing them in a template is the opt-in.
    #[allow(clippy::too_many_arguments)]
    pub fn session_meta(
        mut self,
        effort: Option<&str>,
        version: Option<&str>,
        over_200k: bool,
        repo_owner: Option<&str>,
        repo_name: Option<&str>,
        color: &str,
        reset: &str,
    ) -> Self {
        if let Some(e) = effort.filter(|s| !s.is_empty()) {
            self.variables
                .insert("effort".to_string(), format!("{}{}{}", color, e, reset));
        }
        if let Some(v) = version.filter(|s| !s.is_empty()) {
            self.variables.insert(
                "cc_version".to_string(),
                format!("{}v{}{}", color, v, reset),
            );
        }
        if over_200k {
            self.variables
                .insert("over_200k".to_string(), format!("{}200k+{}", color, reset));
        }
        let repo = match (
            repo_owner.filter(|s| !s.is_empty()),
            repo_name.filter(|s| !s.is_empty()),
        ) {
            (Some(o), Some(n)) => Some(format!("{}/{}", o, n)),
            (None, Some(n)) => Some(n.to_string()),
            _ => None,
        };
        if let Some(r) = repo {
            self.variables
                .insert("repo".to_string(), format!("{}{}{}", color, r, reset));
        }
        self
    }

    /// Set opt-in org usage/cost variables from a cached per-account slice:
    /// `{api_cost_today}`, `{api_cost_mtd}` (clean `$X.XX`), `{api_tokens_by_model}`
    /// (space-joined `model:total` ordered by total tokens descending, mirroring
    /// `{rate_limits}`), `{api_account}`, and `{api_tz}` (the cache's tz label,
    /// e.g. `UTC`).
    ///
    /// Mirrors `session_meta`'s present-only insertion: when `slice` is `None`,
    /// NONE of the `api_*` keys are inserted, so the default render is
    /// byte-identical to v3.1.0. Referencing these variables in a template is the
    /// opt-in; the slice is only present when `[ant]` is enabled and the active
    /// account's usage cache loaded (D-16 total read).
    pub fn api_usage(
        mut self,
        slice: Option<&crate::ant::cache::UsageCache>,
        color: &str,
        reset: &str,
    ) -> Self {
        if let Some(u) = slice {
            // Clean `$X.XX` cost figures — no baked-in "org" marker (D-07).
            self.variables.insert(
                "api_cost_today".to_string(),
                format!("{}${:.2}{}", color, u.today_usd, reset),
            );
            self.variables.insert(
                "api_cost_mtd".to_string(),
                format!("{}${:.2}{}", color, u.mtd_usd, reset),
            );

            // Per-model totals, ordered by total tokens descending (RESEARCH OQ1),
            // humanized via the shared `format_token_count` helper, space-joined
            // like `{rate_limits}`.
            let mut pairs: Vec<(&String, u64)> = u
                .tokens_by_model
                .iter()
                .map(|(model, tb)| (model, tb.total()))
                .collect();
            // Sort by total desc, then model name asc for a stable, deterministic
            // ordering on ties.
            pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            if !pairs.is_empty() {
                let combined = pairs
                    .iter()
                    .map(|(model, total)| format!("{}:{}", model, format_token_count(*total)))
                    .collect::<Vec<_>>()
                    .join(" ");
                self.variables.insert(
                    "api_tokens_by_model".to_string(),
                    format!("{}{}{}", color, combined, reset),
                );
            }

            self.variables.insert(
                "api_account".to_string(),
                format!("{}{}{}", color, u.account, reset),
            );
            self.variables
                .insert("api_tz".to_string(), format!("{}{}{}", color, u.tz, reset));
        }
        self
    }

    /// Set the opt-in per-cache staleness variables `{api_usage_age}` and
    /// `{api_models_age}` from PRE-COMPUTED humanized age strings (e.g. `10m`,
    /// `2h`, `3d`).
    ///
    /// Mirrors `api_usage`/`rate_limits`'s present-only insertion: a key is
    /// inserted ONLY when its age is `Some`, so a never-synced cache renders NO
    /// var at all (D-12 — it can never be misread as `$0`) and the default render
    /// stays byte-identical (D-08). Both vars use the dim `color` (light_gray,
    /// same family as `{api_*}`): per D-11 the dim wrapper IS the staleness
    /// treatment, so both the fresh and stale branches use `color` today. The
    /// `usage_stale`/`models_stale` flags are threaded so a future, louder stale
    /// treatment can branch here without changing this method's signature.
    #[allow(clippy::too_many_arguments)]
    pub fn api_age(
        mut self,
        usage_age: Option<&str>,
        usage_stale: bool,
        models_age: Option<&str>,
        models_stale: bool,
        color: &str,
        reset: &str,
    ) -> Self {
        if let Some(a) = usage_age {
            // Dim is the stale treatment (D-11); both branches use `color`. Bind
            // the flag so the signature stays stable for a future louder treatment.
            let _ = usage_stale;
            self.variables
                .insert("api_usage_age".to_string(), format!("{color}{a}{reset}"));
        }
        if let Some(a) = models_age {
            let _ = models_stale;
            self.variables
                .insert("api_models_age".to_string(), format!("{color}{a}{reset}"));
        }
        self
    }

    /// Build the final HashMap
    pub fn build(self) -> HashMap<String, String> {
        self.variables
    }
}

#[cfg(test)]
mod api_usage_tests {
    use super::*;
    use crate::ant::cache::{TokenBreakdown, UsageCache, USAGE_CACHE_SCHEMA_VERSION};
    use std::collections::HashMap;

    /// Strip the no-op color/reset wrappers (empty under NO_COLOR/tests) — the
    /// builder formats `{color}{value}{reset}`; with empty color/reset args the
    /// stored value is exactly `value`.
    fn slice_with(today: f64, mtd: f64) -> UsageCache {
        let mut tokens = HashMap::new();
        // opus total = 1_200_000 -> "1.2M"; sonnet total = 800_000 -> "800.0K".
        tokens.insert(
            "opus".to_string(),
            TokenBreakdown {
                uncached_input: 1_200_000,
                ..Default::default()
            },
        );
        tokens.insert(
            "sonnet".to_string(),
            TokenBreakdown {
                output: 800_000,
                ..Default::default()
            },
        );
        UsageCache {
            schema_version: USAGE_CACHE_SCHEMA_VERSION,
            fetched_at: chrono::Utc::now(),
            account: "work".to_string(),
            today_usd: today,
            mtd_usd: mtd,
            tz: "UTC".to_string(),
            tokens_by_model: tokens,
        }
    }

    #[test]
    fn some_slice_inserts_all_five_vars_formatted() {
        let slice = slice_with(12.5, 340.0);
        let vars = VariableBuilder::new()
            .api_usage(Some(&slice), "", "")
            .build();

        assert_eq!(
            vars.get("api_cost_today").map(String::as_str),
            Some("$12.50")
        );
        assert_eq!(
            vars.get("api_cost_mtd").map(String::as_str),
            Some("$340.00")
        );
        // Ordered by total tokens desc: opus (1.2M) before sonnet (800.0K).
        assert_eq!(
            vars.get("api_tokens_by_model").map(String::as_str),
            Some("opus:1.2M sonnet:800.0K")
        );
        assert_eq!(vars.get("api_account").map(String::as_str), Some("work"));
        assert_eq!(vars.get("api_tz").map(String::as_str), Some("UTC"));
    }

    #[test]
    fn none_slice_inserts_no_api_vars() {
        let vars = VariableBuilder::new().api_usage(None, "", "").build();
        for key in [
            "api_cost_today",
            "api_cost_mtd",
            "api_tokens_by_model",
            "api_account",
            "api_tz",
        ] {
            assert!(
                !vars.contains_key(key),
                "absent slice must NOT insert `{key}` (byte-identical guarantee)"
            );
        }
    }

    #[test]
    fn api_age_inserts_both_present_keys_formatted() {
        // Empty color/reset => stored value is exactly the humanized age string.
        let vars = VariableBuilder::new()
            .api_age(Some("10m"), false, Some("2h"), true, "", "")
            .build();
        assert_eq!(vars.get("api_usage_age").map(String::as_str), Some("10m"));
        assert_eq!(vars.get("api_models_age").map(String::as_str), Some("2h"));
    }

    #[test]
    fn api_age_usage_absent_inserts_only_models() {
        let vars = VariableBuilder::new()
            .api_age(None, false, Some("2h"), false, "", "")
            .build();
        assert!(
            !vars.contains_key("api_usage_age"),
            "a never-synced usage cache must NOT insert api_usage_age (D-12)"
        );
        assert_eq!(vars.get("api_models_age").map(String::as_str), Some("2h"));
    }

    #[test]
    fn api_age_both_absent_inserts_neither() {
        let vars = VariableBuilder::new()
            .api_age(None, false, None, false, "", "")
            .build();
        for key in ["api_usage_age", "api_models_age"] {
            assert!(
                !vars.contains_key(key),
                "both-absent must NOT insert `{key}` (byte-identical default)"
            );
        }
    }

    #[test]
    fn api_age_wraps_in_color_and_reset() {
        let vars = VariableBuilder::new()
            .api_age(Some("3d"), false, None, false, "<c>", "<r>")
            .build();
        assert_eq!(
            vars.get("api_usage_age").map(String::as_str),
            Some("<c>3d<r>")
        );
    }
}

#[cfg(test)]
mod api_equiv_cost_tests {
    use super::*;
    use crate::ant::cache::{TokenBreakdown, UsageCache, USAGE_CACHE_SCHEMA_VERSION};
    use crate::pricing::{PriceEntry, PriceLookup};
    use std::collections::HashMap;

    // A stable opus-like entry mirroring the bundled `claude-opus-4-8` rates so the
    // arithmetic in these tests is deterministic and independent of the live table.
    fn opus_entry() -> &'static PriceEntry {
        static E: PriceEntry = PriceEntry {
            input: 1.5e-05,
            output: 7.5e-05,
            cache_creation: 1.875e-05,
            cache_read: 1.5e-06,
        };
        &E
    }

    fn priced() -> PriceLookup {
        PriceLookup::Priced(opus_entry())
    }

    fn tokens(
        input: Option<u64>,
        output: Option<u64>,
        cache_creation: Option<u64>,
        cache_read: Option<u64>,
    ) -> ApiEquivTokens {
        ApiEquivTokens {
            input,
            output,
            cache_creation,
            cache_read,
        }
    }

    #[test]
    fn additive_sum_formats_headline_dollar() {
        // input 100k -> $1.50, output 10k -> $0.75, cache_creation 20k -> $0.375,
        // cache_read 200k -> $0.30. Total = $2.925 -> "$2.93".
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(100_000), Some(10_000), Some(20_000), Some(200_000)),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(vars.get("api_equiv_cost").map(String::as_str), Some("$2.93"));
    }

    #[test]
    fn cache_read_priced_on_own_rate_not_input() {
        // 200k cache_read on cache_read rate (1.5e-6) = $0.30, NOT input rate
        // (1.5e-5 -> $3.00). SC4: cache-read never collapsed into input.
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(None, None, None, Some(200_000)),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost_cache_read").map(String::as_str),
            Some("$0.30")
        );
        assert_ne!(
            vars.get("api_equiv_cost_cache_read").map(String::as_str),
            Some("$3.00"),
            "cache-read must NOT be priced at the input rate"
        );
    }

    #[test]
    fn four_per_type_vars_present_only_headline_has_labeled() {
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(100_000), Some(10_000), Some(20_000), Some(200_000)),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost_input").map(String::as_str),
            Some("$1.50")
        );
        assert_eq!(
            vars.get("api_equiv_cost_output").map(String::as_str),
            Some("$0.75")
        );
        // 20k * 1.875e-5 = 0.375 -> "$0.38" (banker-free round-half-up via {:.2}).
        assert_eq!(
            vars.get("api_equiv_cost_cache_write").map(String::as_str),
            Some("$0.38")
        );
        assert_eq!(
            vars.get("api_equiv_cost_cache_read").map(String::as_str),
            Some("$0.30")
        );
        // Only the headline gets a labeled companion; per-type vars do not.
        assert!(vars.contains_key("api_equiv_cost_labeled"));
        assert!(!vars.contains_key("api_equiv_cost_input_labeled"));
    }

    #[test]
    fn labeled_carries_unmistakable_marker() {
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(100_000), None, None, None),
                None,
                "",
                "",
            )
            .build();
        let labeled = vars
            .get("api_equiv_cost_labeled")
            .map(String::as_str)
            .expect("labeled var present");
        assert!(
            labeled.contains("API-equiv"),
            "labeled var must carry an unmistakable API-equivalent marker, got {labeled:?}"
        );
        assert!(
            labeled.contains("$1.50"),
            "labeled var must carry the dollar figure, got {labeled:?}"
        );
    }

    #[test]
    fn no_token_data_inserts_nothing_never_zero() {
        let vars = VariableBuilder::new()
            .api_equiv_cost(Some(priced()), tokens(None, None, None, None), None, "", "")
            .build();
        for key in [
            "api_equiv_cost",
            "api_equiv_cost_labeled",
            "api_equiv_cost_input",
            "api_equiv_cost_output",
            "api_equiv_cost_cache_write",
            "api_equiv_cost_cache_read",
        ] {
            assert!(
                !vars.contains_key(key),
                "no token data must NOT insert `{key}` (D-10, never $0.00)"
            );
        }
    }

    #[test]
    fn partial_token_data_prices_present_fields() {
        // Only output present -> headline = output cost only; absent fields = 0.
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(None, Some(10_000), None, None),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(vars.get("api_equiv_cost").map(String::as_str), Some("$0.75"));
        // Per-type var for output is present and priced.
        assert_eq!(
            vars.get("api_equiv_cost_output").map(String::as_str),
            Some("$0.75")
        );
    }

    #[test]
    fn unpriceable_with_tokens_renders_consistent_unknown_across_all_vars() {
        for lookup in [Some(PriceLookup::Unpriceable), None] {
            let vars = VariableBuilder::new()
                .api_equiv_cost(
                    lookup,
                    tokens(Some(100_000), Some(10_000), Some(20_000), Some(200_000)),
                    None,
                    "",
                    "",
                )
                .build();
            for key in [
                "api_equiv_cost",
                "api_equiv_cost_input",
                "api_equiv_cost_output",
                "api_equiv_cost_cache_write",
                "api_equiv_cost_cache_read",
            ] {
                assert_eq!(
                    vars.get(key).map(String::as_str),
                    Some("unknown"),
                    "`{key}` must render the shared `unknown` marker (lookup={lookup:?})"
                );
            }
            // Labeled var carries the marker (with its tag) but no dollar amount.
            let labeled = vars
                .get("api_equiv_cost_labeled")
                .map(String::as_str)
                .expect("labeled var present for unpriceable");
            assert!(
                labeled.contains("unknown") && !labeled.contains('$'),
                "labeled unpriceable must show the marker, not $, got {labeled:?}"
            );
        }
    }

    fn cache_slice() -> UsageCache {
        let mut tokens = HashMap::new();
        // opus priceable: uncached_input 100k -> $1.50.
        tokens.insert(
            "claude-opus-4-8".to_string(),
            TokenBreakdown {
                uncached_input: 100_000,
                ..Default::default()
            },
        );
        // a cheaper priceable model so ordering by cost desc is observable.
        tokens.insert(
            "claude-3-5-haiku-20241022".to_string(),
            TokenBreakdown {
                uncached_input: 1_000,
                ..Default::default()
            },
        );
        UsageCache {
            schema_version: USAGE_CACHE_SCHEMA_VERSION,
            fetched_at: chrono::Utc::now(),
            account: "work".to_string(),
            today_usd: 0.0,
            mtd_usd: 0.0,
            tz: "UTC".to_string(),
            tokens_by_model: tokens,
        }
    }

    #[test]
    fn by_model_renders_sorted_combined_string_when_slice_present() {
        let slice = cache_slice();
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                Some(&slice),
                "",
                "",
            )
            .build();
        let by_model = vars
            .get("api_equiv_cost_by_model")
            .map(String::as_str)
            .expect("by_model present when slice present");
        // opus ($1.50) sorts before the haiku entry (cheaper) by cost desc.
        assert!(
            by_model.starts_with("claude-opus-4-8:$1.50"),
            "by_model must lead with the costliest model, got {by_model:?}"
        );
        assert!(by_model.contains(' '), "must be space-joined, got {by_model:?}");
    }

    #[test]
    fn by_model_absent_when_no_slice() {
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                None,
                "",
                "",
            )
            .build();
        assert!(
            !vars.contains_key("api_equiv_cost_by_model"),
            "no slice must NOT insert api_equiv_cost_by_model"
        );
    }

    #[test]
    fn by_model_unpriceable_entry_renders_marker_not_zero() {
        let mut tokens_map = HashMap::new();
        tokens_map.insert(
            "claude-opus-4-8".to_string(),
            TokenBreakdown {
                uncached_input: 100_000,
                ..Default::default()
            },
        );
        tokens_map.insert(
            "weird-unpriceable-model".to_string(),
            TokenBreakdown {
                uncached_input: 100_000,
                ..Default::default()
            },
        );
        let slice = UsageCache {
            schema_version: USAGE_CACHE_SCHEMA_VERSION,
            fetched_at: chrono::Utc::now(),
            account: "work".to_string(),
            today_usd: 0.0,
            mtd_usd: 0.0,
            tz: "UTC".to_string(),
            tokens_by_model: tokens_map,
        };
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                Some(&slice),
                "",
                "",
            )
            .build();
        let by_model = vars
            .get("api_equiv_cost_by_model")
            .map(String::as_str)
            .expect("by_model present");
        assert!(
            by_model.contains("weird-unpriceable-model:unknown"),
            "unpriceable model must render `model:unknown`, got {by_model:?}"
        );
        assert!(
            !by_model.contains("weird-unpriceable-model:$0.00"),
            "unpriceable model must NEVER render $0.00, got {by_model:?}"
        );
        // The priceable opus entry still renders its dollar figure.
        assert!(
            by_model.contains("claude-opus-4-8:$1.50"),
            "priceable model still prices, got {by_model:?}"
        );
    }
}
