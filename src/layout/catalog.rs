//! Static catalog of every layout render variable.
//!
//! `RENDER_VARIABLES` lists every variable [`super::VariableBuilder`] can insert
//! into the layout template, plus `{sep}` (resolved by the renderer from
//! `[layout] separator`). It is the source of `statusline --list-vars` output.
//!
//! Drift contract: this list is drift-guarded in BOTH directions by
//! `tests/list_vars_tests.rs`, which scans the production slice of
//! `src/layout/variables.rs`. Do not add a row the builder cannot emit (it would
//! advertise a variable that always renders empty), and do not add a builder
//! key without a row here (`--list-vars` would hide it). The provider-only
//! `gsd_*` variables are deliberately NOT listed: they are not available in the
//! statusline render (D-08).
//!
//! This catalog lives in its own file on purpose — the drift scan reads
//! `variables.rs`, and name literals placed there would make the guard vacuous.

/// One layout render variable, as shown by `statusline --list-vars`.
#[derive(Debug, Clone, Copy)]
pub struct RenderVar {
    /// Variable name as referenced in a template, without braces (`cost` -> `{cost}`).
    pub name: &'static str,
    /// Section heading this variable is listed under (printed verbatim).
    pub group: &'static str,
    /// A representative rendered value (colors omitted).
    pub example: &'static str,
    /// What the variable contains and when it is present.
    pub description: &'static str,
}

const LOCATION: &str = "location";
const GIT: &str = "git";
const CONTEXT: &str = "context";
const MODEL: &str = "model";
const SESSION: &str = "session";
const COST: &str = "cost";
const TOKEN_RATE: &str = "token_rate (requires [token_rate] enabled = true)";
const RATE_LIMITS: &str = "rate_limits (Claude.ai Pro/Max payloads only)";
const API_USAGE: &str = "api_usage (requires [ant] enabled = true + `statusline ant sync-usage`)";
const API_EQUIV: &str = "api_equiv (notional API-equivalent cost from [pricing]; not billed spend)";
const LAYOUT: &str = "layout";

/// Every variable the layout render can substitute, in `--list-vars` display order.
pub const RENDER_VARIABLES: &[RenderVar] = &[
    // --- location ---
    RenderVar {
        name: "directory",
        group: LOCATION,
        example: "~/projects/app",
        description: "Working directory, home-shortened (format/max_length from [layout.components.directory])",
    },
    RenderVar {
        name: "dir_short",
        group: LOCATION,
        example: "app",
        description: "Directory basename only",
    },
    // --- git ---
    RenderVar {
        name: "git",
        group: GIT,
        example: "main +2 ~1",
        description: "Branch plus status counts (format/show_when from [layout.components.git]); absent outside a repo",
    },
    RenderVar {
        name: "git_branch",
        group: GIT,
        example: "main",
        description: "Branch name only",
    },
    // --- context ---
    RenderVar {
        name: "context",
        group: CONTEXT,
        example: "75% [======>---]",
        description: "Context usage: percentage and bar by default (format = full|bar|percent|tokens)",
    },
    RenderVar {
        name: "context_pct",
        group: CONTEXT,
        example: "75%",
        description: "Context usage percentage",
    },
    RenderVar {
        name: "context_tokens",
        group: CONTEXT,
        example: "150k/200k",
        description: "Context tokens used / context window, in thousands",
    },
    RenderVar {
        name: "over_200k",
        group: CONTEXT,
        example: "200k+",
        description: "Present only when the payload reports the response exceeded 200k tokens",
    },
    // --- model ---
    RenderVar {
        name: "model",
        group: MODEL,
        example: "S4.5",
        description: "Model, abbreviated by default (format = abbreviation|full|name|version)",
    },
    RenderVar {
        name: "model_full",
        group: MODEL,
        example: "Claude Sonnet 4.5",
        description: "Full model name from the payload",
    },
    RenderVar {
        name: "model_name",
        group: MODEL,
        example: "Sonnet",
        description: "Model family name only",
    },
    // --- session ---
    RenderVar {
        name: "duration",
        group: SESSION,
        example: "5m",
        description: "Session duration",
    },
    RenderVar {
        name: "lines",
        group: SESSION,
        example: "+50 -10",
        description: "Lines added/removed this session; absent when both are zero",
    },
    RenderVar {
        name: "effort",
        group: SESSION,
        example: "xhigh",
        description: "Reasoning effort level from the payload, when provided",
    },
    RenderVar {
        name: "cc_version",
        group: SESSION,
        example: "v2.1.90",
        description: "Claude Code version from the payload, when provided",
    },
    RenderVar {
        name: "repo",
        group: SESSION,
        example: "owner/name",
        description: "Repository owner/name from the payload, when provided",
    },
    // --- cost ---
    RenderVar {
        name: "cost",
        group: COST,
        example: "$12.50 ($3.50/hr)",
        description: "Session cost; under the default [layout.components.cost] format = \"full\" the burn rate is appended once the session is older than burn_rate.min_duration_seconds",
    },
    RenderVar {
        name: "cost_short",
        group: COST,
        example: "$12",
        description: "Session cost rounded to whole dollars",
    },
    RenderVar {
        name: "burn_rate",
        group: COST,
        example: "$3.50/hr",
        description: "Session cost per hour; absent until a rate is available",
    },
    RenderVar {
        name: "daily_total",
        group: COST,
        example: "$45.00",
        description: "Today's total cost across sessions; present only once it exceeds the current session's cost (another session ran today)",
    },
    // --- token_rate ---
    RenderVar {
        name: "token_rate",
        group: TOKEN_RATE,
        example: "12.5 tok/s • 150K",
        description: "Combined token rate display (respects display_mode and rate_display)",
    },
    RenderVar {
        name: "token_rate_only",
        group: TOKEN_RATE,
        example: "12.5 tok/s",
        description: "Total token rate only",
    },
    RenderVar {
        name: "token_session_total",
        group: TOKEN_RATE,
        example: "150K",
        description: "Session token total",
    },
    RenderVar {
        name: "token_daily_total",
        group: TOKEN_RATE,
        example: "day: 2.5M",
        description: "Daily token total",
    },
    RenderVar {
        name: "token_input_rate",
        group: TOKEN_RATE,
        example: "5.2K tok/s",
        description: "Input + cache-read token rate",
    },
    RenderVar {
        name: "token_output_rate",
        group: TOKEN_RATE,
        example: "8.7K tok/s",
        description: "Output token rate",
    },
    RenderVar {
        name: "token_cache_rate",
        group: TOKEN_RATE,
        example: "41.7K tok/s",
        description: "Cache-read token rate",
    },
    RenderVar {
        name: "token_cache_hit",
        group: TOKEN_RATE,
        example: "85%",
        description: "Cache hit ratio",
    },
    RenderVar {
        name: "token_cache_roi",
        group: TOKEN_RATE,
        example: "12.3x",
        description: "Cache ROI multiplier",
    },
    // --- rate_limits ---
    RenderVar {
        name: "rate_limit_5h",
        group: RATE_LIMITS,
        example: "5h:24%",
        description: "5-hour window usage (with reset countdown appended when [display] rate_limit_reset_countdown = true)",
    },
    RenderVar {
        name: "rate_limit_5h_reset",
        group: RATE_LIMITS,
        example: "2h13m",
        description: "Time until the 5-hour window resets",
    },
    RenderVar {
        name: "rate_limit_7d",
        group: RATE_LIMITS,
        example: "7d:41%",
        description: "7-day window usage",
    },
    RenderVar {
        name: "rate_limit_7d_reset",
        group: RATE_LIMITS,
        example: "3d5h",
        description: "Time until the 7-day window resets",
    },
    RenderVar {
        name: "rate_limits",
        group: RATE_LIMITS,
        example: "5h:24% 7d:41%",
        description: "Both rate-limit windows, space-joined",
    },
    // --- api_usage ---
    RenderVar {
        name: "api_cost_today",
        group: API_USAGE,
        example: "$4.20",
        description: "Org-wide spend today (UTC; the Cost API lags about a day)",
    },
    RenderVar {
        name: "api_cost_mtd",
        group: API_USAGE,
        example: "$120.00",
        description: "Org-wide month-to-date spend (UTC)",
    },
    RenderVar {
        name: "api_tokens_by_model",
        group: API_USAGE,
        example: "claude-opus-4-8:1.2M claude-sonnet-4-5:300K",
        description: "Org-wide token usage by model, largest first",
    },
    RenderVar {
        name: "api_account",
        group: API_USAGE,
        example: "work",
        description: "Active [ant.accounts.<name>] account (STATUSLINE_ANT_ACCOUNT)",
    },
    RenderVar {
        name: "api_tz",
        group: API_USAGE,
        example: "UTC",
        description: "Timezone label of the usage figures",
    },
    RenderVar {
        name: "api_usage_age",
        group: API_USAGE,
        example: "10m",
        description: "Age of the usage cache; present only once it is older than usage_stale_after",
    },
    RenderVar {
        name: "api_models_age",
        group: API_USAGE,
        example: "2d",
        description: "Age of the models cache; present only once it is older than models_stale_after",
    },
    // --- api_equiv ---
    RenderVar {
        name: "api_equiv_cost",
        group: API_EQUIV,
        example: "$18.53",
        description: "Session tokens priced at API rates; a trailing + marks a partial-basis lower bound, and unknown is shown for an unpriced model",
    },
    RenderVar {
        name: "api_equiv_cost_labeled",
        group: API_EQUIV,
        example: "~$18.53 API-equiv",
        description: "The same figure, pre-labeled so it is never read as billed spend",
    },
    RenderVar {
        name: "api_equiv_cost_input",
        group: API_EQUIV,
        example: "$2.10",
        description: "API-equivalent cost of uncached input tokens",
    },
    RenderVar {
        name: "api_equiv_cost_output",
        group: API_EQUIV,
        example: "$9.75",
        description: "API-equivalent cost of output tokens",
    },
    RenderVar {
        name: "api_equiv_cost_cache_write",
        group: API_EQUIV,
        example: "$4.18",
        description: "API-equivalent cost of cache-write tokens",
    },
    RenderVar {
        name: "api_equiv_cost_cache_read",
        group: API_EQUIV,
        example: "$2.50",
        description: "API-equivalent cost of cache-read tokens (priced at the cache-read rate)",
    },
    RenderVar {
        name: "api_equiv_cost_by_model",
        group: API_EQUIV,
        example: "claude-opus-4-8:$15.20 claude-sonnet-4-5:$3.33",
        description: "Org usage cache priced per model, highest first (requires the usage cache)",
    },
    // --- layout ---
    RenderVar {
        name: "sep",
        group: LAYOUT,
        example: " • ",
        description: "Separator, resolved from [layout] separator",
    },
];
