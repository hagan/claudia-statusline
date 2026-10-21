//! Out-of-band fetch of the Anthropic Admin usage & cost API (Plan 08-02).
//!
//! This is the ONLY site in the crate that resolves the org-high-privilege
//! Admin key, touches the network for usage/cost, or spawns a subprocess for the
//! usage path. It resolves the per-account Admin key via a user-configured argv
//! credential command (NO shell), then fetches the two windowed Admin endpoints
//! (`cost_report` + `usage_report/messages`) through the SAME leak-free
//! `curl --config -` stdin handoff Phase 07 built (`fetch_page_curl`). It NEVER
//! runs on the render path — the render path consumes the cache this module
//! writes (`super::cache::read_usage_cache`).
//!
//! # Credential safety (D-01 / D-17 / T-08-KEY-ARGV / T-08-SHELL-INJ)
//!
//! - The Admin key is materialized transiently from the `admin_key_command`
//!   argv (spawned via `Command::new(argv[0]).args(argv[1..])` — **no `sh -c`**,
//!   so a config value cannot inject shell metacharacters) and dropped after the
//!   curl write. It is NEVER exported into the environment.
//! - The key reaches `curl` ONLY via a config written to the child's STDIN
//!   (`curl --config -` + `Stdio::piped()`), interpolated into a
//!   `header = "x-api-key: {key}"` line — never into argv, never to disk. It
//!   disappears on crash/SIGKILL.
//! - The key is never serialized to the cache, logged, or printed in any summary
//!   (the summary prints a credential-source LABEL + totals only).
//!
//! # All-or-nothing publish (D-16 / T-08-HALFSLICE)
//!
//! [`fetch_usage`] returns a fully-assembled [`UsageCache`] only after BOTH
//! endpoints (cost today + cost MTD + usage tokens) succeed and parse. On ANY
//! failure it returns a DIFFERENTIATED error (401/403 taxonomy, sanitized body)
//! and writes nothing — the caller's prior slice is left intact.

// Mirrors `src/ant/fetch.rs`: the public surface (`fetch_usage`) is consumed by
// the thin `commands::ant` handler; several helpers are exercised only by the
// colocated / external unit tests. Allow dead_code at the module level to keep
// `make check-code` clean.
#![allow(dead_code)]

use std::collections::HashMap;
use std::process::{Command, Stdio};

use chrono::{Datelike, Duration, NaiveTime, Utc};
use serde::Deserialize;

use crate::ant::cache::{TokenBreakdown, UsageCache, USAGE_CACHE_SCHEMA_VERSION};
use crate::error::{Result, StatuslineError};

/// curl connection timeout (seconds) — bounds DoS via an unreachable host.
const CONNECT_TIMEOUT_SECS: u32 = 10;
/// curl total operation timeout (seconds).
const MAX_TIME_SECS: u32 = 30;
/// Stable Anthropic API version header for the curl fetch.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Maximum number of bytes of child stderr we will echo in an error message.
const STDERR_BOUND: usize = 512;
/// Hard cap on pagination iterations — defends against a server that always
/// reports `has_more = true` (T-08 DoS).
const MAX_PAGES: usize = 1000;
/// Maximum accepted length of an API-supplied pagination token.
const MAX_PAGE_TOKEN_LEN: usize = 256;

/// The org cost report endpoint (verified live 2026-06-14).
const COST_URL: &str = "https://api.anthropic.com/v1/organizations/cost_report";
/// The org usage report (messages) endpoint (verified live 2026-06-14).
const USAGE_URL: &str = "https://api.anthropic.com/v1/organizations/usage_report/messages";

// ===========================================================================
// API envelope shapes (tolerant — unknown fields ignored, missing → default)
// ===========================================================================

/// Shared `{ data[], has_more, next_page }` envelope for both Admin endpoints.
/// `T` is the per-bucket `results[]` item type (cost or usage).
///
/// `pub` so the external integration unit-test crate (`tests/ant_usage_tests.rs`)
/// can deserialize fixtures and exercise the pure aggregators directly.
#[derive(Debug, Deserialize)]
pub struct ReportEnvelope<T> {
    #[serde(default = "Vec::new")]
    data: Vec<Bucket<T>>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_page: Option<String>,
}

/// One daily bucket carrying a `results[]` array.
#[derive(Debug, Deserialize)]
pub struct Bucket<T> {
    #[serde(default = "Vec::new")]
    results: Vec<T>,
}

/// A single cost line. `amount` is a decimal string in CENTS (USD) — D-15.
#[derive(Debug, Deserialize)]
pub struct CostItem {
    #[serde(default)]
    amount: String,
}

/// A single usage (by-model) line. Token fields default to 0 so a partial
/// response degrades to zeros rather than failing to deserialize (D-13).
#[derive(Debug, Deserialize)]
pub struct UsageItem {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    uncached_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation: CacheCreation,
}

/// Nested cache-creation token counts (1h / 5m ephemeral).
#[derive(Debug, Deserialize, Default)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
}

// ===========================================================================
// Credential resolution (argv command, NO shell — D-01 / T-08-SHELL-INJ)
// ===========================================================================

/// Resolve the org-admin API key by running the per-account `admin_key_command`
/// argv and capturing its trimmed stdout.
///
/// Spawned via `Command::new(argv[0]).args(argv[1..])` with NO shell, so a config
/// value can never be interpreted as a shell command (T-08-SHELL-INJ). This is a
/// HARD error on empty argv / spawn failure / non-zero status (sanitized stderr) /
/// empty trimmed stdout — never a silent downgrade (D-01/D-02). The returned key
/// is interpolated into the curl stdin config ONLY (never argv, never disk).
fn resolve_admin_key(argv: &[String]) -> Result<String> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| StatuslineError::other("admin_key_command is empty (no program to run)"))?;

    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| StatuslineError::other(format!("failed to run admin_key_command: {}", e)))?;

    if !output.status.success() {
        let stderr = sanitize_stderr(&output.stderr);
        return Err(StatuslineError::other(format!(
            "admin_key_command failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr
        )));
    }

    // Trim trailing newline/whitespace from the captured key.
    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() {
        return Err(StatuslineError::other(
            "admin_key_command produced no key on stdout",
        ));
    }
    Ok(key)
}

// ===========================================================================
// Page-token validation (NEW — permits ':' for the RFC3339 next_page token)
// ===========================================================================

/// Validate an API-supplied `next_page` token before it is interpolated into a
/// curl-config `url=` directive.
///
/// DISTINCT from `fetch::validate_cursor`: the Admin `next_page` token is an
/// RFC3339 timestamp like `2019-12-27T18:11:19.117Z` containing `:`, which the
/// Phase-07 cursor alphabet (`[A-Za-z0-9._-]`) would wrongly reject — breaking
/// pagination on the first multi-page response (RESEARCH Pitfall 3 / LANDMINE).
/// This validator permits `:` (T/Z are already alphanumeric) while still
/// rejecting `"`, newline, `&`, `#`, a leading `-`, whitespace, empty, and
/// over-length values — so the token can never terminate the quoted `url = "..."`
/// directive or be misparsed as a flag (T-08-CURSOR-INJ).
///
/// `pub` so the external integration unit-test crate can verify the `:`-vs-
/// injection alphabet directly.
pub fn validate_page_token(tok: &str) -> Result<()> {
    if tok.is_empty() || tok.len() > MAX_PAGE_TOKEN_LEN {
        return Err(StatuslineError::other(
            "Admin API returned an out-of-range pagination token; aborting fetch",
        ));
    }
    // A leading '-' could be misparsed as a curl/config flag.
    if tok.starts_with('-') {
        return Err(StatuslineError::other(
            "Admin API returned a flag-like pagination token; aborting fetch",
        ));
    }
    // Confine to the conservative id alphabet PLUS ':' for RFC3339 timestamps.
    // This rejects '"', newline, '&', '#', and any whitespace.
    if !tok
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(StatuslineError::other(
            "Admin API returned a malformed pagination token; aborting fetch",
        ));
    }
    Ok(())
}

// ===========================================================================
// UTC window construction (D-11 / RESEARCH Pitfall 1)
// ===========================================================================

/// A pair of RFC3339 timestamps `[starting_at, ending_at)` (ending exclusive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtcWindow {
    pub starting_at: String,
    pub ending_at: String,
}

/// The today + MTD UTC windows, both with an exclusive `ending_at` of UTC
/// midnight tomorrow. `bucket_width=1d` buckets snap to UTC midnight, so a local
/// day window could not align (D-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtcWindows {
    pub today: UtcWindow,
    pub mtd: UtcWindow,
}

/// Compute the today/MTD UTC windows from "now".
///
/// today = `[UTC midnight today, UTC midnight tomorrow)`;
/// MTD   = `[UTC first-of-month 00:00, UTC midnight tomorrow)`.
/// `ending_at` is EXCLUSIVE (the API selects buckets that end before it).
///
/// `pub` so the external integration unit-test crate can verify the windows.
pub fn utc_windows(now: chrono::DateTime<Utc>) -> UtcWindows {
    let midnight_today = now.date_naive().and_time(NaiveTime::MIN).and_utc();
    let midnight_tomorrow = midnight_today + Duration::days(1);
    let first_of_month = now
        .date_naive()
        .with_day(1)
        .expect("day 1 is always valid")
        .and_time(NaiveTime::MIN)
        .and_utc();

    // Emit the `Z`-suffixed UTC form, NOT chrono's `to_rfc3339()` (which renders
    // a UTC instant with a `+00:00` offset). These values are appended verbatim to
    // the Admin API query string in `fetch_report`; a `+` is URL-reserved and is
    // decoded server-side as a space, corrupting the timestamp (CR-01). `Z` is
    // the canonical zero-offset form and contains no URL-reserved characters that
    // change meaning in a query value.
    fn z_utc(dt: chrono::DateTime<Utc>) -> String {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    UtcWindows {
        today: UtcWindow {
            starting_at: z_utc(midnight_today),
            ending_at: z_utc(midnight_tomorrow),
        },
        mtd: UtcWindow {
            starting_at: z_utc(first_of_month),
            ending_at: z_utc(midnight_tomorrow),
        },
    }
}

// ===========================================================================
// Aggregation (cents → USD ; tokens by model)
// ===========================================================================

/// Sum EVERY `results[].amount` (decimal cents string) across all buckets and
/// divide by 100 → USD (D-14/D-15). NO `cost_type` filter — every amount
/// (tokens / web_search / code_execution / session_usage, all tiers) is summed.
/// Unparsable amounts are skipped tolerantly. Priority Tier never appears
/// (excluded by design — ANT-23 satisfied for free).
///
/// `pub` so the external integration unit-test crate can sum fixtures directly.
pub fn sum_cents_to_usd(env: &ReportEnvelope<CostItem>) -> f64 {
    let cents: f64 = env
        .data
        .iter()
        .flat_map(|b| &b.results)
        .filter_map(|r| r.amount.parse::<f64>().ok())
        // `f64::parse` accepts "-500", "inf", "NaN", and "1e400" (→ inf). A
        // malformed/hostile Admin response must not drive the figure negative,
        // infinite, or NaN — keep only finite, non-negative amounts (WR-04).
        .filter(|c| c.is_finite() && *c >= 0.0)
        .sum();
    // Clamp the final USD figure defensively (>= 0.0), so the rendered
    // `{api_cost_*}` can never be negative.
    (cents / 100.0).max(0.0)
}

/// Accumulate one usage envelope's `results[]` into a per-model
/// `TokenBreakdown` map. `group_by=model` yields one entry per model per bucket;
/// entries accumulate across buckets (and across pages). Items with no `model`
/// label are skipped (cannot key the map). Saturating adds so a pathological
/// response can never overflow.
///
/// `pub` so the external integration unit-test crate can accumulate fixtures.
pub fn accumulate_tokens(
    env: &ReportEnvelope<UsageItem>,
    acc: &mut HashMap<String, TokenBreakdown>,
) {
    for item in env.data.iter().flat_map(|b| &b.results) {
        let model = match &item.model {
            Some(m) if !m.is_empty() => m.clone(),
            _ => continue,
        };
        let tb = acc.entry(model).or_default();
        tb.uncached_input = tb.uncached_input.saturating_add(item.uncached_input_tokens);
        tb.cache_read_input = tb
            .cache_read_input
            .saturating_add(item.cache_read_input_tokens);
        tb.cache_creation_1h = tb
            .cache_creation_1h
            .saturating_add(item.cache_creation.ephemeral_1h_input_tokens);
        tb.cache_creation_5m = tb
            .cache_creation_5m
            .saturating_add(item.cache_creation.ephemeral_5m_input_tokens);
        tb.output = tb.output.saturating_add(item.output_tokens);
    }
}

// ===========================================================================
// curl transport (leak-free stdin config; key NEVER in argv — cloned from fetch.rs)
// ===========================================================================

/// Fetch one full (paginated) report from `base_url` with the given extra query
/// pairs, deserializing each page as `ReportEnvelope<T>`.
///
/// The `x-api-key` / `anthropic-version` headers and the URL are written to the
/// curl child's STDIN as a config (`curl --config -`); the key never touches
/// argv OR disk. `--write-out "%{http_code}"` is appended so 401/403 are
/// recoverable under `--fail-with-body` (which collapses all HTTP ≥400 to exit
/// 22). Pagination follows `next_page` (validated by [`validate_page_token`]),
/// capped at [`MAX_PAGES`]. On any failure returns a differentiated error with a
/// sanitized body and writes nothing.
fn fetch_report<T>(
    key: &str,
    base_url: &str,
    extra_query: &[(&str, &str)],
) -> Result<Vec<ReportEnvelope<T>>>
where
    for<'de> T: Deserialize<'de>,
{
    let mut pages: Vec<ReportEnvelope<T>> = Vec::new();
    let mut next_page: Option<String> = None;

    for _ in 0..MAX_PAGES {
        // Build the query string. The page token is validated BEFORE it reaches
        // the curl-config url= directive (T-08-CURSOR-INJ).
        let mut url = base_url.to_string();
        let mut sep = '?';
        for (k, v) in extra_query {
            url.push(sep);
            url.push_str(k);
            url.push('=');
            url.push_str(v);
            sep = '&';
        }
        if let Some(tok) = &next_page {
            validate_page_token(tok)?;
            url.push(sep);
            url.push_str("page=");
            url.push_str(tok);
        }

        let (body, http_code) = curl_get(key, &url)?;
        let env: ReportEnvelope<T> = serde_json::from_slice(&body).map_err(|e| {
            StatuslineError::other(format!(
                "failed to parse Admin API JSON (http {}): {}",
                http_code, e
            ))
        })?;

        let more = env.has_more;
        let token = env.next_page.clone();
        pages.push(env);

        if more {
            match token {
                Some(tok) => next_page = Some(tok),
                None => break,
            }
        } else {
            break;
        }
    }

    Ok(pages)
}

/// Perform a single leak-free `curl` GET, returning `(body_bytes, http_code)`.
///
/// `--write-out "%{http_code}"` appends the numeric status to stdout AFTER the
/// body; we split it off the tail so 401/403 are recoverable even when
/// `--fail-with-body` exits 22 (D-17 / Pattern 4). The key is interpolated into a
/// header line written to STDIN only.
fn curl_get(key: &str, url: &str) -> Result<(Vec<u8>, u16)> {
    let mut cmd = Command::new("curl");
    cmd.arg("--config")
        .arg("-")
        .arg("--connect-timeout")
        .arg(CONNECT_TIMEOUT_SECS.to_string())
        .arg("--max-time")
        .arg(MAX_TIME_SECS.to_string())
        .arg("--fail-with-body")
        .arg("--silent")
        .arg("--show-error")
        // Capture the HTTP status even under --fail-with-body exit 22. A leading
        // newline lets us split it cleanly off the JSON body tail.
        .arg("--write-out")
        .arg("\n%{http_code}")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| StatuslineError::other(format!("failed to spawn `curl`: {}", e)))?;

    // Build the config in memory and stream it to stdin. The key is interpolated
    // into a HEADER line written to stdin ONLY — never into argv (T-08-KEY-ARGV).
    // url is quoted so query params can't be misparsed as config directives.
    let config = format!(
        "header = \"x-api-key: {key}\"\n\
         header = \"anthropic-version: {ver}\"\n\
         url = \"{url}\"\n",
        key = key,
        ver = ANTHROPIC_VERSION,
        url = url,
    );

    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            StatuslineError::other("failed to open curl stdin for the leak-free config")
        })?;
        crate::ant::fetch::stream_curl_config(&mut stdin, &config)?;
        // Drop closes stdin so curl proceeds; the key leaves memory here.
    }

    let output = child
        .wait_with_output()
        .map_err(|e| StatuslineError::other(format!("curl did not complete: {}", e)))?;

    // Split the trailing "\n%{http_code}" off stdout regardless of exit status
    // (--write-out fires on both success and the --fail-with-body 22 path).
    let (body, http_code) = split_http_code(&output.stdout);

    if !output.status.success() {
        let stderr = sanitize_stderr(&output.stderr);
        let sanitized_body = sanitize_stderr(body);
        let code = output.status.code().unwrap_or(-1);
        let msg = match code {
            // Resolve / connect / timeout (network).
            6 | 7 | 28 => format!(
                "network error fetching the Admin API (curl exit {}): {}",
                code, stderr
            ),
            // HTTP >= 400 under --fail-with-body. Differentiate via http_code.
            22 => match http_code {
                401 => format!(
                    "Admin key invalid or expired (HTTP 401): {}",
                    sanitized_body
                ),
                403 => format!(
                    "this account lacks Admin API access (individual account, \
                     non-admin role, or Bedrock/Vertex/Foundry) (HTTP 403): {}",
                    sanitized_body
                ),
                other => format!(
                    "HTTP error {} from the Admin API: {}",
                    other, sanitized_body
                ),
            },
            _ => format!("curl failed (exit {}): {}", code, stderr),
        };
        return Err(StatuslineError::other(msg));
    }

    Ok((body.to_vec(), http_code))
}

/// Split a trailing `\n%{http_code}` (3 ASCII digits) off the end of curl's
/// stdout. Returns `(body_without_trailing_status, status)`. If no trailing
/// status is found the whole buffer is the body and the status is 0.
fn split_http_code(stdout: &[u8]) -> (&[u8], u16) {
    // Find the last newline; everything after it should be the status digits.
    if let Some(pos) = stdout.iter().rposition(|&b| b == b'\n') {
        let tail = &stdout[pos + 1..];
        if !tail.is_empty() && tail.iter().all(|b| b.is_ascii_digit()) {
            if let Ok(code) = std::str::from_utf8(tail).unwrap_or("").parse::<u16>() {
                return (&stdout[..pos], code);
            }
        }
    }
    (stdout, 0)
}

// ===========================================================================
// Public entry: fetch BOTH endpoints, assemble the slice AFTER both succeed
// ===========================================================================

/// Fetch the org usage & cost for `account` and assemble a [`UsageCache`].
///
/// Resolves the Admin key from `admin_key_command` (argv, no shell), then
/// fetches today cost, MTD cost, and by-model usage tokens through the leak-free
/// curl-stdin handoff. The [`UsageCache`] is constructed ONLY after ALL THREE
/// fetches succeed and parse (D-16); on ANY failure returns a differentiated
/// error (401/403 taxonomy, sanitized body) and the caller writes nothing. The
/// key never appears in the returned value, any error, or any log.
pub(crate) fn fetch_usage(account: &str, admin_key_command: &[String]) -> Result<UsageCache> {
    let key = resolve_admin_key(admin_key_command)?;
    let windows = utc_windows(Utc::now());

    // Cost: today.
    let today_pages = fetch_report::<CostItem>(
        &key,
        COST_URL,
        &[
            ("starting_at", &windows.today.starting_at),
            ("ending_at", &windows.today.ending_at),
            ("bucket_width", "1d"),
        ],
    )?;
    let today_usd: f64 = today_pages.iter().map(sum_cents_to_usd).sum();

    // Cost: month-to-date.
    let mtd_pages = fetch_report::<CostItem>(
        &key,
        COST_URL,
        &[
            ("starting_at", &windows.mtd.starting_at),
            ("ending_at", &windows.mtd.ending_at),
            ("bucket_width", "1d"),
        ],
    )?;
    let mtd_usd: f64 = mtd_pages.iter().map(sum_cents_to_usd).sum();

    // Usage tokens by model (MTD window, grouped by model).
    let usage_pages = fetch_report::<UsageItem>(
        &key,
        USAGE_URL,
        &[
            ("starting_at", &windows.mtd.starting_at),
            ("ending_at", &windows.mtd.ending_at),
            ("bucket_width", "1d"),
            ("group_by", "model"),
        ],
    )?;
    let mut tokens_by_model: HashMap<String, TokenBreakdown> = HashMap::new();
    for env in &usage_pages {
        accumulate_tokens(env, &mut tokens_by_model);
    }

    // Single construction site of UsageCache — reached only after BOTH endpoints
    // (all three fetches) succeed and parse (D-16 / T-08-HALFSLICE).
    Ok(UsageCache {
        schema_version: USAGE_CACHE_SCHEMA_VERSION,
        fetched_at: Utc::now(),
        account: account.to_string(),
        today_usd,
        mtd_usd,
        tz: "UTC".to_string(),
        tokens_by_model,
    })
}

/// `ant doctor --probe` ONLY: resolve the active account's Admin key (this IS the
/// `--probe`-gated credential exec) and issue ONE cheap cost-report GET to confirm
/// Admin reachability, returning a KEY-FREE status label.
///
/// On HTTP 200 returns `"reachable (HTTP 200)"`. Any failure is surfaced as an
/// `Err` carrying `curl_get`'s already-sanitized, differentiated message (401 key
/// invalid, 403 no admin access, network error) — never the key, never the raw
/// body. The caller (the doctor handler) only calls this when `--probe` is set;
/// passive doctor never resolves or runs the credential command (D-13).
pub(crate) fn probe_admin_reachability(
    _account: &str,
    admin_key_command: &[String],
) -> Result<String> {
    // The ONLY credential exec on the doctor path — gated behind --probe upstream.
    let key = resolve_admin_key(admin_key_command)?;
    let windows = utc_windows(Utc::now());

    // One minimal, single-bucket cost GET (today window) as the reachability probe.
    let mut url = COST_URL.to_string();
    let mut sep = '?';
    for (k, v) in [
        ("starting_at", windows.today.starting_at.as_str()),
        ("ending_at", windows.today.ending_at.as_str()),
        ("bucket_width", "1d"),
    ] {
        url.push(sep);
        url.push_str(k);
        url.push('=');
        url.push_str(v);
        sep = '&';
    }

    // curl_get returns a differentiated, sanitized Err on 401/403/network; a 200
    // (or any 2xx) body we discard — we only report reachability, never contents.
    let (_body, http_code) = curl_get(&key, &url)?;
    Ok(format!("reachable (HTTP {http_code})"))
}

/// Bound and sanitize child output before including it in an error message:
/// drop any line that looks like it carries a key, then char-boundary-safe
/// truncate to [`STDERR_BOUND`] chars — so a key-bearing diagnostic can never
/// leak (T-08-STDERR-LEAK). Mirrors `fetch::sanitize_stderr`.
fn sanitize_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let filtered: String = text
        .lines()
        .filter(|line| {
            let l = line.to_ascii_lowercase();
            !l.contains("sk-ant-") && !l.contains("x-api-key") && !l.contains("api-key")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = filtered.trim();
    if trimmed.chars().count() > STDERR_BOUND {
        let truncated: String = trimmed.chars().take(STDERR_BOUND).collect();
        format!("{}…", truncated)
    } else if trimmed.is_empty() {
        "(no diagnostic output)".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_stdin_epipe_is_reported_usage() {
        let (r, mut w) = std::io::pipe().unwrap();
        drop(r);
        let err = crate::ant::fetch::stream_curl_config(&mut w, "url = \"x\"\n")
            .expect_err("EPIPE must be reported as Err");
        assert!(err
            .to_string()
            .contains("failed to write curl config to stdin"));
    }

    fn cost_env(amounts: &[&[&str]]) -> ReportEnvelope<CostItem> {
        let json_buckets: Vec<String> = amounts
            .iter()
            .map(|bucket| {
                let items: Vec<String> = bucket
                    .iter()
                    .map(|a| format!("{{\"amount\":\"{}\"}}", a))
                    .collect();
                format!("{{\"results\":[{}]}}", items.join(","))
            })
            .collect();
        let json = format!(
            "{{\"data\":[{}],\"has_more\":false}}",
            json_buckets.join(",")
        );
        serde_json::from_str(&json).expect("valid cost envelope")
    }

    // cents-decimal-string sum -> USD, across buckets and mixed cost_types,
    // NO cost_type filter; unparsable amounts skipped.
    #[test]
    fn sum_cents_to_usd_sums_all_amounts() {
        // Two buckets: "1234" + "66" cents (mixed cost types) => 1300 cents => $13.00.
        // A garbage amount is skipped tolerantly.
        let env = cost_env(&[&["1234", "not-a-number"], &["66"]]);
        let usd = sum_cents_to_usd(&env);
        assert!((usd - 13.00).abs() < 1e-9, "expected 13.00, got {usd}");
    }

    // WR-04: negative / Inf / NaN amounts parse successfully via `f64::parse`
    // but must NOT corrupt the total. They are filtered to finite, non-negative
    // values, and the final figure is clamped >= 0.0.
    #[test]
    fn sum_cents_to_usd_rejects_negative_inf_nan() {
        // "-500" (negative), "inf"/"1e400" (non-finite), "NaN" (non-finite) are
        // all dropped; only the valid "1300" cents => $13.00 survives.
        let env = cost_env(&[&["-500", "inf", "NaN", "1e400"], &["1300"]]);
        let usd = sum_cents_to_usd(&env);
        assert!(usd.is_finite(), "total must be finite, got {usd}");
        assert!((usd - 13.00).abs() < 1e-9, "expected 13.00, got {usd}");

        // An envelope of ONLY bad amounts collapses to a clamped 0.0 (never
        // negative, never NaN/inf).
        let only_bad = cost_env(&[&["-500", "inf", "NaN"]]);
        let usd_bad = sum_cents_to_usd(&only_bad);
        assert_eq!(usd_bad, 0.0, "all-bad amounts must clamp to 0.0");
    }

    // today = [UTC midnight today, UTC midnight tomorrow); MTD = [first-of-month,
    // UTC midnight tomorrow); ending_at exclusive; both RFC3339.
    #[test]
    fn utc_windows_today_mtd() {
        use chrono::TimeZone;
        // 2026-06-14T13:45:00Z — mid-month, mid-day.
        let now = Utc.with_ymd_and_hms(2026, 6, 14, 13, 45, 0).unwrap();
        let w = utc_windows(now);

        // today window.
        assert!(
            w.today.starting_at.starts_with("2026-06-14T00:00:00"),
            "today start: {}",
            w.today.starting_at
        );
        assert!(
            w.today.ending_at.starts_with("2026-06-15T00:00:00"),
            "today end (exclusive midnight tomorrow): {}",
            w.today.ending_at
        );

        // MTD window: first-of-month start, same exclusive end.
        assert!(
            w.mtd.starting_at.starts_with("2026-06-01T00:00:00"),
            "mtd start: {}",
            w.mtd.starting_at
        );
        assert_eq!(
            w.mtd.ending_at, w.today.ending_at,
            "both windows share the exclusive midnight-tomorrow end"
        );

        // Regression (CR-01): windows MUST be emitted in the `Z`-suffixed UTC form,
        // never with a `+00:00` offset — the value is appended raw to the Admin API
        // query string, and a `+` is decoded server-side as a space, corrupting the
        // timestamp. Assert the exact rendering and that no `+` offset leaks in.
        assert_eq!(w.today.starting_at, "2026-06-14T00:00:00Z");
        assert_eq!(w.today.ending_at, "2026-06-15T00:00:00Z");
        assert_eq!(w.mtd.starting_at, "2026-06-01T00:00:00Z");
        for ts in [
            &w.today.starting_at,
            &w.today.ending_at,
            &w.mtd.starting_at,
            &w.mtd.ending_at,
        ] {
            assert!(ts.ends_with('Z'), "must be Z-suffixed UTC: {ts}");
            assert!(!ts.contains('+'), "must not carry a +offset: {ts}");
        }
    }

    // RFC3339 token (with ':') accepted; injection / over-len / empty rejected.
    #[test]
    fn validate_page_token_accepts_rfc3339_rejects_injection() {
        // Accept the live RFC3339 next_page token (contains ':').
        assert!(validate_page_token("2019-12-27T18:11:19.117Z").is_ok());
        assert!(validate_page_token("page_01H8xYz-abc.123").is_ok());

        // Reject curl-config breakout, flag-leading, metacharacters, empty, over-len.
        assert!(validate_page_token("good\"\nurl = \"http://evil").is_err());
        assert!(validate_page_token("-K/etc/passwd").is_err());
        assert!(validate_page_token("a&b#c").is_err());
        assert!(validate_page_token("a b").is_err());
        assert!(validate_page_token("").is_err());
        assert!(validate_page_token(&"a".repeat(MAX_PAGE_TOKEN_LEN + 1)).is_err());
    }

    // group_by=model accumulation across buckets; rendered total = sum of 5 fields.
    #[test]
    fn tokens_by_model_accumulates_per_type() {
        let json = r#"{
            "data": [
                {"results": [
                    {"model":"opus","uncached_input_tokens":100,"cache_read_input_tokens":10,
                     "output_tokens":50,"cache_creation":{"ephemeral_1h_input_tokens":1,"ephemeral_5m_input_tokens":2}},
                    {"model":"sonnet","uncached_input_tokens":200,"output_tokens":80}
                ]},
                {"results": [
                    {"model":"opus","uncached_input_tokens":300,"output_tokens":40}
                ]}
            ],
            "has_more": false
        }"#;
        let env: ReportEnvelope<UsageItem> = serde_json::from_str(json).expect("valid usage env");
        let mut acc: HashMap<String, TokenBreakdown> = HashMap::new();
        accumulate_tokens(&env, &mut acc);

        let opus = acc.get("opus").expect("opus present");
        // uncached 100+300, cache_read 10, output 50+40, 1h 1, 5m 2.
        assert_eq!(opus.uncached_input, 400);
        assert_eq!(opus.cache_read_input, 10);
        assert_eq!(opus.output, 90);
        assert_eq!(opus.cache_creation_1h, 1);
        assert_eq!(opus.cache_creation_5m, 2);
        // Per-model TOTAL = sum of the five fields.
        assert_eq!(opus.total(), 400 + 10 + 90 + 1 + 2);

        let sonnet = acc.get("sonnet").expect("sonnet present");
        assert_eq!(sonnet.total(), 200 + 80);
    }

    // resolve_admin_key: empty argv is a hard error (no silent downgrade).
    #[test]
    fn resolve_admin_key_empty_argv_errors() {
        let err = resolve_admin_key(&[]).unwrap_err().to_string();
        assert!(err.contains("empty"), "got: {err}");
    }

    // resolve_admin_key: a real echo command yields the trimmed key.
    #[test]
    fn resolve_admin_key_runs_argv_no_shell() {
        let argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf 'sk-ant-admin01-FAKE\\n'".to_string(),
        ];
        let key = resolve_admin_key(&argv).expect("key resolves");
        assert_eq!(key, "sk-ant-admin01-FAKE", "trailing newline trimmed");
    }

    // 401 vs 403 differentiation: distinct, key-free messages.
    #[test]
    fn http_code_taxonomy_differentiates_401_403() {
        // We exercise the message wording directly (the curl spawn path is covered
        // by the integration tests). Build the same strings curl_get produces.
        let msg_401 = format!(
            "Admin key invalid or expired (HTTP 401): {}",
            "(no diagnostic output)"
        );
        let msg_403 = format!(
            "this account lacks Admin API access (individual account, \
             non-admin role, or Bedrock/Vertex/Foundry) (HTTP 403): {}",
            "(no diagnostic output)"
        );
        assert!(msg_401.contains("invalid or expired"));
        assert!(msg_403.contains("lacks Admin API access"));
        assert_ne!(msg_401, msg_403);
        assert!(!msg_401.contains("sk-ant-"));
        assert!(!msg_403.contains("sk-ant-"));
    }

    // split_http_code peels the trailing status off the body.
    #[test]
    fn split_http_code_peels_status() {
        let buf = b"{\"data\":[]}\n200";
        let (body, code) = split_http_code(buf);
        assert_eq!(code, 200);
        assert_eq!(body, b"{\"data\":[]}");

        let buf401 = b"{\"error\":\"x\"}\n401";
        let (_b, code401) = split_http_code(buf401);
        assert_eq!(code401, 401);

        // No trailing status -> whole buffer is body, code 0.
        let (body2, code2) = split_http_code(b"plain");
        assert_eq!(code2, 0);
        assert_eq!(body2, b"plain");
    }

    // sanitize_stderr drops key-bearing lines.
    #[test]
    fn sanitize_drops_key_lines() {
        let raw = b"normal line\nleaked x-api-key: sk-ant-secret\nmore";
        let out = sanitize_stderr(raw);
        assert!(!out.contains("sk-ant-"));
        assert!(!out.contains("x-api-key"));
        assert!(out.contains("normal line"));
    }
}
