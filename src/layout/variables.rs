//! Variable builder for creating the template substitution HashMap.

use std::collections::HashMap;

use super::format::{format_rate_with_unit, format_token_count, resolve_color_override};
use crate::config::{
    ContextComponentConfig, CostComponentConfig, DirectoryComponentConfig, GitComponentConfig,
    ModelComponentConfig,
};
use crate::utils::sanitize_for_terminal;

/// Payload session token counts (`context_window.current_usage`) threaded into
/// the cost-from-tokens builder. Each field is `Option<u64>`: `None`/absent is
/// treated as 0 for the additive sum, but presence is tracked so that a payload
/// carrying NO token field at all yields var-absence (D-10), never a `$0.00`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiEquivTokens {
    /// Uncached input tokens (`input_tokens`), priced on the input rate.
    pub input: Option<u64>,
    /// Output tokens (`output_tokens`), priced on the output rate.
    pub output: Option<u64>,
    /// Cache-creation/write tokens (`cache_creation_input_tokens`).
    pub cache_creation: Option<u64>,
    /// Cache-read tokens (`cache_read_input_tokens`), priced on its OWN
    /// (~0.1x) rate — NEVER collapsed into the input rate (D-05/D-09, SC4).
    pub cache_read: Option<u64>,
}

impl ApiEquivTokens {
    /// True when at least one token field is present (`Some`). When false the
    /// cost-from-tokens builder inserts NOTHING (D-10), never a `$0.00`.
    fn any_present(&self) -> bool {
        self.input.is_some()
            || self.output.is_some()
            || self.cache_creation.is_some()
            || self.cache_read.is_some()
    }

    /// True when ALL four cost dimensions are present, i.e. the headline total
    /// covers the complete cost basis.
    ///
    /// When false the headline is a LOWER BOUND: an absent field cannot be
    /// distinguished from a genuine zero, so the real API-equivalent cost is
    /// greater than or equal to what was summed. The headline discloses that
    /// with a `+` suffix rather than presenting a partial sum as a total
    /// (10-VERIFICATION-INDEPENDENT.md — the honesty rule applied to the
    /// per-type vars had not been applied to the headline they feed).
    fn all_present(&self) -> bool {
        self.input.is_some()
            && self.output.is_some()
            && self.cache_creation.is_some()
            && self.cache_read.is_some()
    }
}

/// The SINGLE shared literal marker rendered for an unpriceable-but-token-present
/// model. Reused byte-identically across the headline, the labeled headline, all
/// four per-type vars, and every per-model entry (review MEDIUM-4 / D-13) so an
/// unpriceable model can NEVER be mistaken for a `$0.00` (PRICE-05).
const API_EQUIV_UNKNOWN: &str = "unknown";

/// Builder for creating the variables HashMap from statusline components.
///
/// Each method sets a variable that can be referenced in the layout template.
/// Variables are rendered with colors before being stored.
///
/// # Terminal-safety boundary (R5-CR-01)
///
/// **This builder IS the terminal-safety boundary.** [`super::LayoutRenderer::render`]
/// — the method that produces the shipped statusline — copies variable VALUES
/// into its output buffer verbatim and sanitizes only the separator. That is
/// deliberate (plan 11-11's single-pass contract: a substituted value must never
/// be re-examined), and its sibling `render_template`'s blanket sanitize pass is
/// not on the shipped path. So the ONLY place a boundary can live without
/// reintroducing a re-scan is here, at the builder — which also means it does
/// not depend on which renderer runs, or on a call site remembering.
///
/// Three rules, applied per ARGUMENT rather than per method:
///
/// 1. **Sanitize here** — every PLAIN untrusted value: Claude Code payload text
///    (`current_dir`, `basename`, `effort`, `version`, `repo`), `git` subprocess
///    output (branch names), and text read out of an on-disk cache (`account`,
///    `tz`, model ids). Sanitize the value at method ENTRY, BEFORE it is wrapped
///    in color, and before any truncation (truncating first can cut an escape
///    mid-sequence and leave a bare ESC the sanitizer's regex no longer matches).
///    Several of these are ALSO sanitized at their `src/display.rs` call sites;
///    that is belt-and-braces, and double-sanitizing is idempotent by design.
///    CLAUDE.md: "external input is untrusted".
///
/// 2. **Never sanitize here** — a PRE-COMPOSED COLORED string. `git_with_config`'s
///    `full_info` arrives from `crate::git::format_git_info`, which sanitizes the
///    branch itself and then ADDS its own SGR codes; running
///    [`sanitize_for_terminal`] over it would strip exactly those colors, because
///    the sanitizer's first act is to delete every `ESC [ .. m` sequence. That is
///    the `R5-WR-02` failure mode: a colorless statusline that still passes a
///    naive "no ESC in the output" assertion. Pinned by
///    `directory_with_config_preserves_the_color_wrapper` and
///    `git_with_config_sanitizes_the_branch` in `src/layout/tests.rs`.
///
/// 3. **Nothing to sanitize** — values this crate formats itself: costs,
///    percentages, token counts, durations, humanized ages, progress bars,
///    rate-limit pieces, and fixed literals such as `200k+`.
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
    ///
    /// Both paths are untrusted payload text and are sanitized at entry, before
    /// the color wrap (rule 1 of the terminal-safety boundary; R5-CR-01).
    #[allow(dead_code)]
    pub fn directory(mut self, path: &str, short_path: &str, color: &str, reset: &str) -> Self {
        let path = sanitize_for_terminal(path);
        let short_path = sanitize_for_terminal(short_path);
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
    ///
    /// # Terminal safety (R5-CR-01)
    ///
    /// All three path inputs are untrusted: `src/display.rs` derives them from
    /// the payload's `workspace.current_dir`, and passes `full_path` and
    /// `basename` RAW (only `short_path` is sanitized at that call site). Since
    /// `config.format` selects between them — `"full"`, `"basename"`, or the
    /// default `"short"` — and `{dir_short}` is set from `basename`
    /// UNCONDITIONALLY (which the BUILT-IN `compact` preset renders), all three
    /// are sanitized HERE, at entry, before the color wrap and before
    /// truncation. Truncating first could cut an escape mid-sequence and leave a
    /// bare ESC that the sanitizer's `ESC [ .. m` regex no longer matches; the
    /// ordering is pinned by `directory_with_config_sanitizes_before_truncating`.
    /// Re-sanitizing the already-sanitized `short_path` is idempotent and
    /// intended — the boundary must not depend on the call site.
    pub fn directory_with_config(
        mut self,
        full_path: &str,
        short_path: &str,
        basename: &str,
        default_color: &str,
        reset: &str,
        config: &DirectoryComponentConfig,
    ) -> Self {
        // Untrusted payload text -> sanitize BEFORE truncation and BEFORE the
        // color wrap. Never sanitize the composed value: that would strip the
        // wrapper's own SGR codes (R5-WR-02).
        let full_path = sanitize_for_terminal(full_path);
        let short_path = sanitize_for_terminal(short_path);
        let basename = sanitize_for_terminal(basename);

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
            "full" => truncate(&full_path),
            "basename" => truncate(&basename),
            _ => truncate(&short_path), // "short" is default
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
                format!("{}{}{}", color, truncate(&basename), reset),
            );
        }

        self
    }

    /// Set git variables ({git}, {git_branch})
    ///
    /// `branch` is plain untrusted `git` output and is sanitized here (rule 1).
    /// `full_info` is NOT: it arrives PRE-COMPOSED WITH COLOR from
    /// [`crate::git::format_git_info`], which sanitizes the branch itself and
    /// then adds SGR codes — sanitizing it again would strip them (rule 2,
    /// R5-WR-02).
    #[allow(dead_code)]
    pub fn git(mut self, full_info: &str, branch: Option<&str>) -> Self {
        if !full_info.is_empty() {
            self.variables
                .insert("git".to_string(), full_info.to_string());
        }
        if let Some(b) = branch {
            if !b.is_empty() {
                self.variables
                    .insert("git_branch".to_string(), sanitize_for_terminal(b));
            }
        }
        self
    }

    /// Set git variables with component configuration
    ///
    /// Applies format and show_when options from config.
    /// show_when: "always" (default), "dirty" (only when dirty), "never"
    ///
    /// # Terminal safety (R5-CR-01)
    ///
    /// `branch` is a branch NAME — plain untrusted `git` output, attacker
    /// influenceable in a cloned repo — and is sanitized here (rule 1). Its only
    /// in-tree call site also sanitizes (`src/display.rs:741`), which is
    /// deliberate belt-and-braces: this method is `pub`, and the boundary must
    /// not depend on a caller remembering.
    ///
    /// `full_info` is deliberately NOT sanitized (rule 2): it arrives
    /// PRE-COMPOSED WITH COLOR from [`crate::git::format_git_info`], which
    /// sanitizes the branch at its true source and then adds its own SGR codes.
    /// Running the sanitizer over it would delete exactly those codes and ship a
    /// colorless git segment (R5-WR-02). `status_only` is built from integer
    /// counts (rule 3).
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
        let branch = branch.map(sanitize_for_terminal);
        let branch = branch.as_deref();

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
    ///
    /// `full_name` is the raw payload model name (rule 1). `abbreviation` is
    /// derived from [`crate::models::ModelType`] and cannot carry a control byte
    /// — see [`Self::model_with_config`].
    #[allow(dead_code)]
    pub fn model(mut self, abbreviation: &str, full_name: &str, color: &str, reset: &str) -> Self {
        let full_name = sanitize_for_terminal(full_name);
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
    ///
    /// # Terminal safety (R5-CR-01)
    ///
    /// `full_name` is the raw payload model name (`model.display_name` /
    /// `model.id`) and is sanitized here (rule 1), even though both in-tree call
    /// sites already sanitize it (`src/display.rs:511`, `:806`) — the boundary
    /// must not depend on a caller remembering.
    ///
    /// `abbreviation`, `family_name` and `version` are NOT sanitized, and cannot
    /// need it: [`crate::models::ModelType::from_name`] reduces any input to one
    /// of five fixed family literals plus a version matched by
    /// `\d+(?:[.\-]\d+)?` and normalized to digits and dots, so none of the
    /// three can carry a control byte whatever the payload says. If that
    /// construction ever changes — e.g. a family echoed from the input — they
    /// become rule-1 values and must be sanitized here too.
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
        let full_name = sanitize_for_terminal(full_name);

        let color = if config.color.is_empty() {
            default_color.to_string()
        } else {
            resolve_color_override(&config.color)
        };

        // Format based on config
        let display_value = match config.format.as_str() {
            "full" => full_name.as_str(),
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
        // Every field below is untrusted payload text and is inserted with NO
        // call-site sanitization, so the ONLY boundary is here. Sanitize the
        // inner value before the color wrap — never the composed string, which
        // would strip the wrapper's own SGR codes (R5-CR-01 / R5-WR-02).
        if let Some(e) = effort.filter(|s| !s.is_empty()) {
            self.variables.insert(
                "effort".to_string(),
                format!("{}{}{}", color, sanitize_for_terminal(e), reset),
            );
        }
        if let Some(v) = version.filter(|s| !s.is_empty()) {
            self.variables.insert(
                "cc_version".to_string(),
                format!("{}v{}{}", color, sanitize_for_terminal(v), reset),
            );
        }
        if over_200k {
            self.variables
                .insert("over_200k".to_string(), format!("{}200k+{}", color, reset));
        }
        // Joined from the SANITIZED parts. Sanitizing the join would be
        // equivalent — `/` is not a control character — but doing it per part
        // keeps the boundary on the values that actually crossed it.
        let repo = match (
            repo_owner.filter(|s| !s.is_empty()),
            repo_name.filter(|s| !s.is_empty()),
        ) {
            (Some(o), Some(n)) => Some(format!(
                "{}/{}",
                sanitize_for_terminal(o),
                sanitize_for_terminal(n)
            )),
            (None, Some(n)) => Some(sanitize_for_terminal(n)),
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
                    .map(|(model, total)| {
                        // Model ids come from the Admin usage cache (untrusted
                        // external input) — sanitize before they reach the
                        // terminal (10-REVIEW-CODEX.md MEDIUM 1).
                        format!(
                            "{}:{}",
                            sanitize_for_terminal(model),
                            format_token_count(*total)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.variables.insert(
                    "api_tokens_by_model".to_string(),
                    format!("{}{}{}", color, combined, reset),
                );
            }

            // `account` and `tz` are read VERBATIM out of a JSON file on disk.
            // `read_usage_cache` sanitizes the account name used to build the
            // PATH, but nothing validates these FIELDS inside the file — a
            // hand-edited or foreign-producer cache carries whatever it likes.
            // Same class as the model ids above (R5-CR-01, rule 1).
            self.variables.insert(
                "api_account".to_string(),
                format!("{}{}{}", color, sanitize_for_terminal(&u.account), reset),
            );
            self.variables.insert(
                "api_tz".to_string(),
                format!("{}{}{}", color, sanitize_for_terminal(&u.tz), reset),
            );
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

    /// Set the opt-in, explicitly-labeled cost-from-tokens variables (PRICE-02 /
    /// PRICE-03 / PRICE-05).
    ///
    /// Emits, when the payload carries usable token data:
    /// - `{api_equiv_cost}` — the clean bare `$X.XX` notional API-equivalent
    ///   session cost (the intentional CLEAN value; its companion carries the
    ///   label — review MEDIUM-5).
    /// - `{api_equiv_cost_labeled}` — the ONLY pre-labeled variant, carrying an
    ///   unmistakable `API-equiv` marker so a Max user never reads it as real
    ///   spend (D-06).
    /// - `{api_equiv_cost_input}` / `{api_equiv_cost_output}` /
    ///   `{api_equiv_cost_cache_write}` / `{api_equiv_cost_cache_read}` — the four
    ///   per-token-type clean figures. Cache-read is priced on its OWN (~0.1x)
    ///   rate, never collapsed into input (D-05/D-09, SC4).
    /// - `{api_equiv_cost_by_model}` — only when the optional Phase-08
    ///   `UsageCache` slice is present: a `model:$X.XX` string sorted by cost
    ///   desc then name asc, joined " " (mirrors `{api_tokens_by_model}`).
    ///
    /// Presence-gating (D-10): with NO token field present, NOTHING is inserted —
    /// the vars are simply absent, never `$0.00`. Honesty (D-13, review MEDIUM-4):
    /// valid tokens but an unpriceable model (or no model resolved) renders the
    /// SINGLE shared [`API_EQUIV_UNKNOWN`] marker CONSISTENTLY across the
    /// headline, the labeled headline, and all four per-type vars — and each
    /// unpriceable per-model entry renders `model:unknown` — never a fabricated
    /// `$0.00`.
    ///
    /// **This method NEVER resolves a price source.** It receives `synced` — the
    /// already-resolved snapshot `src/display.rs` selected for this render — plus
    /// the `aliases` map, and prices every row against that one value. That is
    /// what makes headline/breakdown agreement STRUCTURAL rather than a matter of
    /// discipline: there is no second source to disagree with (Pitfall 6 /
    /// verification gap 2b / CR-01).
    ///
    /// PROHIBITED: reintroducing a `select_synced` or `lookup_with_source` call
    /// here re-opens CR-01 — one render would read `prices.json` twice and could
    /// price the same model from two different snapshots if a concurrent
    /// `ant sync-pricing` landed in between. It is blocked by
    /// `structural_guard_single_price_resolution_site` in
    /// `tests/ant_invariant_tests.rs`.
    ///
    /// `pricing` is the resolved lookup outcome for the current payload model:
    /// `Some(Priced(_))` to price, `Some(Unpriceable)` for a known-but-unpriceable
    /// model, or `None` when no model id was available (also unpriceable). The
    /// method performs PURE LOCAL arithmetic only — no network, no subprocess
    /// (enforced by the static-scan guard in `tests`).
    #[allow(clippy::too_many_arguments)]
    pub fn api_equiv_cost(
        mut self,
        pricing: Option<crate::pricing::PriceLookup>,
        tokens: ApiEquivTokens,
        by_model: Option<&crate::ant::cache::UsageCache>,
        aliases: &std::collections::HashMap<String, String>,
        synced: Option<&crate::pricing::cache::PriceCache>,
        color: &str,
        reset: &str,
    ) -> Self {
        use crate::pricing::PriceLookup;

        // Per-model breakdown is INDEPENDENT of the session-token presence gate:
        // it is driven solely by the optional UsageCache slice (D-08). EACH model
        // is priced by its OWN exact-match lookup (with the same `[pricing.aliases]`
        // map), so a single unpriceable row renders `model:unknown` while its
        // priceable siblings still price (review MEDIUM-4).
        if let Some(slice) = by_model {
            // `synced` is the CALLER's snapshot — the single one `display.rs`
            // resolved for this render — reused for every row. Resolving a source
            // here (or calling the one-shot `pricing::lookup_with_source` per
            // model) would re-read and re-parse the cache and could price two
            // models in the SAME line from two different snapshots, because a
            // concurrent `ant sync-pricing` swaps the file atomically mid-render.
            // One read, one snapshot, one number (Pitfall 6 / CR-01).
            let mut pairs: Vec<(String, Option<f64>)> = slice
                .tokens_by_model
                .iter()
                .map(|(model, tb)| {
                    let cost = match crate::pricing::lookup_in(model, aliases, synced) {
                        // The two cache-creation TTLs are priced on their OWN
                        // rates: a 1-hour write costs ~1.6x a 5-minute one, so
                        // summing the token counts onto the 5-minute rate
                        // understates it by ~37% (10-REVIEW-CODEX.md HIGH 2).
                        // Each term is multiplied independently in f64, so there
                        // is no integer sum to overflow.
                        PriceLookup::Priced(e) => {
                            // A row without a 1-hour rate cannot price 1-hour
                            // tokens. Refuse the whole figure rather than
                            // substitute the 5-minute rate (R2-1) — but only
                            // when 1-hour tokens are actually present, so a row
                            // still prices the dimensions it CAN.
                            let h_term = if tb.cache_creation_1h == 0 {
                                Some(0.0)
                            } else {
                                e.cache_creation_1h_rate()
                                    .map(|r| (tb.cache_creation_1h as f64) * r)
                            };
                            h_term.map(|h| {
                                (tb.uncached_input as f64) * e.input
                                    + (tb.cache_read_input as f64) * e.cache_read
                                    + (tb.cache_creation_5m as f64) * e.cache_creation
                                    + h
                                    + (tb.output as f64) * e.output
                            })
                        }
                        PriceLookup::Unpriceable => None,
                    };
                    // Model ids come from the Admin usage cache (untrusted
                    // external input) — sanitize before they reach the terminal
                    // (10-REVIEW-CODEX.md MEDIUM 1).
                    (sanitize_for_terminal(model), cost)
                })
                .collect();
            // Sort: priced entries by cost desc; unpriceable (None) sort last,
            // then by model name asc for a stable, deterministic ordering.
            pairs.sort_by(|a, b| match (a.1, b.1) {
                (Some(ca), Some(cb)) => cb
                    .partial_cmp(&ca)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.0.cmp(&b.0),
            });
            let combined = pairs
                .iter()
                .map(|(model, cost)| match cost {
                    Some(c) => format!("{model}:${c:.2}"),
                    None => format!("{model}:{API_EQUIV_UNKNOWN}"),
                })
                .collect::<Vec<_>>()
                .join(" ");
            if !combined.is_empty() {
                self.variables.insert(
                    "api_equiv_cost_by_model".to_string(),
                    format!("{color}{combined}{reset}"),
                );
            }
        }

        // Session cost-from-tokens vars: presence-gated (D-10).
        if !tokens.any_present() {
            return self;
        }

        // Helper: format a per-component figure routed through the SHARED unknown
        // marker so an unpriceable model never renders `$0.00` anywhere.
        let fmt = |value: Option<f64>| -> String {
            match value {
                Some(v) => format!("{color}${v:.2}{reset}"),
                None => format!("{color}{API_EQUIV_UNKNOWN}{reset}"),
            }
        };

        match pricing {
            Some(PriceLookup::Priced(e)) => {
                // Each component is priced only when its OWN token count is
                // present. An absent count must NOT render as `$0.00` — that is
                // indistinguishable from genuine zero usage (10-REVIEW-CODEX.md
                // HIGH 4). The headline totals whatever IS present.
                let input = tokens.input.map(|t| (t as f64) * e.input);
                let output = tokens.output.map(|t| (t as f64) * e.output);
                // The session payload reports one undifferentiated
                // `cache_creation_input_tokens` with no 1h/5m split, so it is
                // priced on the 5-minute rate. Only the per-model Admin-cache
                // path (above) holds the split and can price the 1-hour rate.
                let cache_write = tokens.cache_creation.map(|t| (t as f64) * e.cache_creation);
                let cache_read = tokens.cache_read.map(|t| (t as f64) * e.cache_read);
                let total = input.unwrap_or(0.0)
                    + output.unwrap_or(0.0)
                    + cache_write.unwrap_or(0.0)
                    + cache_read.unwrap_or(0.0);
                // A partial basis makes the headline a LOWER BOUND, not a total.
                // `+` reads as "at least this much" and is honest whether the
                // absent field means "no such usage" or "not reported".
                let partial = if tokens.all_present() { "" } else { "+" };

                self.variables.insert(
                    "api_equiv_cost".to_string(),
                    format!("{color}${total:.2}{partial}{reset}"),
                );
                self.variables.insert(
                    "api_equiv_cost_labeled".to_string(),
                    format!("{color}~${total:.2}{partial} API-equiv{reset}"),
                );
                for (key, value) in [
                    ("api_equiv_cost_input", input),
                    ("api_equiv_cost_output", output),
                    ("api_equiv_cost_cache_write", cache_write),
                    ("api_equiv_cost_cache_read", cache_read),
                ] {
                    if let Some(v) = value {
                        self.variables.insert(key.to_string(), fmt(Some(v)));
                    }
                }
            }
            // Unpriceable OR no model resolved: render the SHARED `unknown` marker
            // consistently across every var (D-13, review MEDIUM-4) — never $0.00.
            Some(PriceLookup::Unpriceable) | None => {
                self.variables
                    .insert("api_equiv_cost".to_string(), fmt(None));
                self.variables.insert(
                    "api_equiv_cost_labeled".to_string(),
                    format!("{color}{API_EQUIV_UNKNOWN} API-equiv{reset}"),
                );
                for key in [
                    "api_equiv_cost_input",
                    "api_equiv_cost_output",
                    "api_equiv_cost_cache_write",
                    "api_equiv_cost_cache_read",
                ] {
                    self.variables.insert(key.to_string(), fmt(None));
                }
            }
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
    // These are Opus 4.8's real published rates ($5/$25 per MTok). They are pinned
    // against the embedded table by `pricing::bundled_opus_4_8_matches_published_rates`
    // so this oracle can never drift back into agreeing with a mispriced table.
    fn opus_entry() -> &'static PriceEntry {
        static E: PriceEntry = PriceEntry {
            input: 5e-06,
            output: 2.5e-05,
            cache_creation: 6.25e-06,
            cache_read: 5e-07,
            cache_creation_1h: Some(1e-05),
        };
        &E
    }

    fn priced() -> PriceLookup {
        PriceLookup::Priced(*opus_entry())
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
        // input 100k -> $0.50, output 10k -> $0.25, cache_creation 20k -> $0.125
        // (-> "$0.12" under f64 round-to-even), cache_read 200k -> $0.10.
        // Total = 0.975 -> "$0.97" (f64 representation rounds down).
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(100_000), Some(10_000), Some(20_000), Some(200_000)),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost").map(String::as_str),
            Some("$0.97")
        );
    }

    #[test]
    fn cache_read_priced_on_own_rate_not_input() {
        // 200k cache_read on cache_read rate (5e-7) = $0.10, NOT input rate
        // (5e-6 -> $1.00). SC4: cache-read never collapsed into input.
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(None, None, None, Some(200_000)),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost_cache_read").map(String::as_str),
            Some("$0.10")
        );
        assert_ne!(
            vars.get("api_equiv_cost_cache_read").map(String::as_str),
            Some("$1.00"),
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
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost_input").map(String::as_str),
            Some("$0.50")
        );
        assert_eq!(
            vars.get("api_equiv_cost_output").map(String::as_str),
            Some("$0.25")
        );
        // 20k * 6.25e-6 = 0.125 -> "$0.12" (f64 {:.2} round-to-even/representation).
        assert_eq!(
            vars.get("api_equiv_cost_cache_write").map(String::as_str),
            Some("$0.12")
        );
        assert_eq!(
            vars.get("api_equiv_cost_cache_read").map(String::as_str),
            Some("$0.10")
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
                &HashMap::new(),
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
            labeled.contains("$0.50"),
            "labeled var must carry the dollar figure, got {labeled:?}"
        );
    }

    #[test]
    fn no_token_data_inserts_nothing_never_zero() {
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(None, None, None, None),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
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
    fn partial_token_data_prices_present_fields_and_omits_absent_ones() {
        // Only output present -> headline = output cost only, and the three
        // ABSENT components render nothing at all. Previously they rendered
        // `$0.00`, which is indistinguishable from genuine zero usage
        // (10-REVIEW-CODEX.md HIGH 4).
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(None, Some(10_000), None, None),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost").map(String::as_str),
            Some("$0.25+"),
            "a partial basis must be disclosed as a lower bound, not shown as a total"
        );
        // Per-type var for output is present and priced.
        assert_eq!(
            vars.get("api_equiv_cost_output").map(String::as_str),
            Some("$0.25")
        );
        for key in [
            "api_equiv_cost_input",
            "api_equiv_cost_cache_write",
            "api_equiv_cost_cache_read",
        ] {
            assert!(
                !vars.contains_key(key),
                "absent token type must NOT insert `{key}` (never $0.00)"
            );
        }
    }

    #[test]
    fn complete_basis_headline_carries_no_partial_marker() {
        // All four dimensions present -> the headline IS the total, no `+`.
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(100_000), Some(10_000), Some(20_000), Some(200_000)),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost").map(String::as_str),
            Some("$0.97")
        );
        assert_eq!(
            vars.get("api_equiv_cost_labeled").map(String::as_str),
            Some("~$0.97 API-equiv")
        );
    }

    #[test]
    fn partial_basis_headline_is_marked_as_a_lower_bound() {
        // cache_read absent -> the true cost is >= the sum. Both the bare and
        // labeled headline must say so; neither may present it as a total.
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(100_000), Some(10_000), Some(20_000), None),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost").map(String::as_str),
            Some("$0.88+")
        );
        assert_eq!(
            vars.get("api_equiv_cost_labeled").map(String::as_str),
            Some("~$0.88+ API-equiv")
        );
    }

    #[test]
    fn zero_token_count_is_distinguishable_from_an_absent_one() {
        // An explicit 0 IS real data and must still price (as $0.00); only an
        // ABSENT count is omitted. This is the distinction HIGH 4 destroyed.
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(0), Some(10_000), None, None),
                None,
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost_input").map(String::as_str),
            Some("$0.00"),
            "an explicitly-reported 0 must render, not vanish"
        );
        assert!(
            !vars.contains_key("api_equiv_cost_cache_read"),
            "an absent count must still be omitted"
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
                    &HashMap::new(),
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
            "claude-haiku-4-5".to_string(),
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
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        let by_model = vars
            .get("api_equiv_cost_by_model")
            .map(String::as_str)
            .expect("by_model present when slice present");
        // opus ($0.50) sorts before the haiku entry (cheaper) by cost desc.
        assert!(
            by_model.starts_with("claude-opus-4-8:$0.50"),
            "by_model must lead with the costliest model, got {by_model:?}"
        );
        assert!(
            by_model.contains(' '),
            "must be space-joined, got {by_model:?}"
        );
    }

    #[test]
    fn by_model_prices_one_hour_cache_writes_on_their_own_rate() {
        // 100k 1-hour cache-creation tokens on claude-opus-4-8. The bundled 1h
        // rate is $10/MTok -> $1.00. Pricing them on the 5-minute rate
        // ($6.25/MTok -> $0.62) understates by ~37% (10-REVIEW-CODEX.md HIGH 2).
        let mut tokens_by_model = HashMap::new();
        tokens_by_model.insert(
            "claude-opus-4-8".to_string(),
            TokenBreakdown {
                cache_creation_1h: 100_000,
                ..Default::default()
            },
        );
        let slice = UsageCache {
            tokens_by_model,
            ..cache_slice()
        };
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                Some(&slice),
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        let by_model = vars
            .get("api_equiv_cost_by_model")
            .map(String::as_str)
            .expect("by_model present");
        assert_eq!(
            by_model, "claude-opus-4-8:$1.00",
            "1-hour cache writes must use the 1-hour rate, got {by_model:?}"
        );
        assert!(
            !by_model.contains("$0.62"),
            "1-hour writes must NOT be priced at the 5-minute rate, got {by_model:?}"
        );
    }

    #[test]
    fn model_lacking_a_1h_rate_renders_unknown_not_a_substituted_rate() {
        // `claude-4-opus-20250514` is a legacy alias whose upstream row carries
        // no `above_1hr` rate. With 1-hour tokens present it must render
        // `unknown`, NOT the 5-minute rate — which understated it by 37% and
        // made the same model price differently by id (R2-1).
        let mut tokens_by_model = HashMap::new();
        tokens_by_model.insert(
            "claude-4-opus-20250514".to_string(),
            TokenBreakdown {
                cache_creation_1h: 100_000,
                ..Default::default()
            },
        );
        let slice = UsageCache {
            tokens_by_model,
            ..cache_slice()
        };
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                Some(&slice),
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        let by_model = vars
            .get("api_equiv_cost_by_model")
            .map(String::as_str)
            .expect("by_model present");
        assert_eq!(
            by_model, "claude-4-opus-20250514:unknown",
            "a row lacking the 1h rate must refuse, got {by_model:?}"
        );
        // The 5-minute substitution would have produced $1.87.
        assert!(
            !by_model.contains("$1.87"),
            "must not fall back to the 5-minute rate, got {by_model:?}"
        );
    }

    #[test]
    fn model_lacking_a_1h_rate_still_prices_when_no_1h_tokens() {
        // The refusal is scoped to the dimension: with zero 1-hour tokens the
        // same row prices normally on the dimensions it CAN price.
        let mut tokens_by_model = HashMap::new();
        tokens_by_model.insert(
            "claude-4-opus-20250514".to_string(),
            TokenBreakdown {
                uncached_input: 100_000,
                ..Default::default()
            },
        );
        let slice = UsageCache {
            tokens_by_model,
            ..cache_slice()
        };
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                Some(&slice),
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        assert_eq!(
            vars.get("api_equiv_cost_by_model").map(String::as_str),
            Some("claude-4-opus-20250514:$1.50"),
            "100k input at $15/MTok prices normally without 1h tokens"
        );
    }

    #[test]
    fn by_model_sanitizes_untrusted_model_ids() {
        // Model ids come from the Admin usage cache. A control sequence in one
        // must not reach the terminal (10-REVIEW-CODEX.md MEDIUM 1).
        let mut tokens_by_model = HashMap::new();
        tokens_by_model.insert(
            "evil\u{1b}[31m\u{7}model".to_string(),
            TokenBreakdown {
                uncached_input: 1_000,
                ..Default::default()
            },
        );
        let slice = UsageCache {
            tokens_by_model,
            ..cache_slice()
        };
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                Some(&slice),
                &HashMap::new(),
                None,
                "",
                "",
            )
            .build();
        let by_model = vars
            .get("api_equiv_cost_by_model")
            .map(String::as_str)
            .expect("by_model present");
        assert!(
            !by_model.contains('\u{1b}') && !by_model.contains('\u{7}'),
            "untrusted model id must be sanitized, got {by_model:?}"
        );
    }

    #[test]
    fn by_model_absent_when_no_slice() {
        let vars = VariableBuilder::new()
            .api_equiv_cost(
                Some(priced()),
                tokens(Some(1), None, None, None),
                None,
                &HashMap::new(),
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
                &HashMap::new(),
                None,
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
            by_model.contains("claude-opus-4-8:$0.50"),
            "priceable model still prices, got {by_model:?}"
        );
    }
}
