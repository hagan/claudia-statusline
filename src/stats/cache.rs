//! Token rate metrics and cache efficiency calculations.
//!
//! Contains `TokenRateMetrics`, token rate calculation functions, and
//! cache hit ratio / ROI computation.

use super::session::{get_session_duration, get_session_duration_by_mode};
use super::StatsData;

/// Token rate metrics for display
///
/// All fields are public API for library consumers, even if not all are
/// used internally by the statusline binary.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Public API - fields used by library consumers
pub struct TokenRateMetrics {
    pub input_rate: f64,              // Input tokens per second
    pub output_rate: f64,             // Output tokens per second
    pub cache_read_rate: f64,         // Cache read tokens per second
    pub cache_creation_rate: f64,     // Cache creation tokens per second
    pub total_rate: f64,              // Total tokens per second
    pub duration_seconds: u64,        // Duration used for calculation
    pub cache_hit_ratio: Option<f64>, // Cache hit ratio (0.0-1.0)
    pub cache_roi: Option<f64>,       // Cache ROI (return on investment)
    pub session_total_tokens: u64,    // Total tokens for current session
    pub daily_total_tokens: u64,      // Total tokens for today (across all sessions)
}

/// Calculate token rates for a session
///
/// Uses the same duration mode as burn_rate (wall_clock, active_time, or auto_reset).
/// Returns None if token breakdown or duration is not available.
///
/// When `rate_window_seconds > 0` in config and `transcript_path` is provided,
/// uses rolling window calculation for more responsive rate updates.
/// Otherwise falls back to session average (total_tokens / session_duration).
///
/// Requires a database handle - hot path callers should create the handle once and reuse it.
/// For convenience (non-hot paths), use `calculate_token_rates()` which creates its own handle.
pub fn calculate_token_rates_with_db(
    session_id: &str,
    db: &crate::database::SqliteDatabase,
) -> Option<TokenRateMetrics> {
    calculate_token_rates_with_db_and_transcript(session_id, db, None)
}

/// Calculate token rates with optional rolling window support
///
/// When `rate_window_seconds > 0` in config and `transcript_path` is provided,
/// uses rolling window calculation from transcript for more responsive rate updates.
/// The displayed rate reflects recent activity while totals remain accurate from the database.
pub fn calculate_token_rates_with_db_and_transcript(
    session_id: &str,
    db: &crate::database::SqliteDatabase,
    transcript_path: Option<&str>,
) -> Option<TokenRateMetrics> {
    let config = crate::config::get_config();

    // Check if token rate feature is enabled
    if !config.token_rate.enabled {
        return None;
    }

    // Token rates require SQLite-only mode (json_backup = false)
    // JSON backup doesn't store token breakdowns needed for rate calculation
    if config.database.json_backup {
        log::debug!("Token rates disabled: requires SQLite-only mode (json_backup = false)");
        return None;
    }

    // Get token breakdown from database (for totals - always accurate)
    let (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) =
        db.get_session_token_breakdown(session_id)?;

    // Get daily token total
    let daily_total_tokens = db.get_today_token_total().unwrap_or(0);

    // Calculate total tokens (cast to u64 first to prevent overflow with long sessions)
    let total_tokens = input_tokens as u64
        + output_tokens as u64
        + cache_read_tokens as u64
        + cache_creation_tokens as u64;

    // No tokens yet, skip calculation
    if total_tokens == 0 {
        return None;
    }

    // Check if rolling window is configured and transcript is available
    let window_seconds = config.token_rate.rate_window_seconds;
    if window_seconds > 0 {
        if let Some(path) = transcript_path {
            // Try rolling window calculation for OUTPUT rate (responsive)
            // Input rate uses session average (stable, since input_tokens = context size, not delta)
            if let Some((_, rolling_output_rate, _, rolling_cache_creation_rate, window_duration)) =
                crate::utils::get_rolling_window_rates(path, window_seconds)
            {
                // Get session duration for input rate calculation
                let session_duration = if config.token_rate.inherit_duration_mode {
                    get_session_duration_by_mode(session_id).unwrap_or(60)
                } else {
                    get_session_duration(session_id).unwrap_or(60)
                };
                let session_duration_f64 = session_duration.max(1) as f64;

                // Hybrid approach:
                // - Input rate: session average (input_tokens = context size, not cumulative)
                // - Output rate: rolling window (output_tokens ARE cumulative per message)
                // - Cache read rate: session average (like input, represents context)
                // - Cache creation rate: rolling window (like output, cumulative work)
                let input_rate = input_tokens as f64 / session_duration_f64;
                let cache_read_rate = cache_read_tokens as f64 / session_duration_f64;
                let output_rate = rolling_output_rate;
                let cache_creation_rate = rolling_cache_creation_rate;

                let total_rate = input_rate + output_rate + cache_read_rate + cache_creation_rate;

                // Calculate cache metrics from session totals (more stable)
                let (cache_hit_ratio, cache_roi) = calculate_cache_metrics(
                    config,
                    cache_read_tokens,
                    input_tokens,
                    cache_creation_tokens,
                );

                return Some(TokenRateMetrics {
                    input_rate,
                    output_rate,
                    cache_read_rate,
                    cache_creation_rate,
                    total_rate,
                    duration_seconds: window_duration,
                    cache_hit_ratio,
                    cache_roi,
                    session_total_tokens: total_tokens,
                    daily_total_tokens,
                });
            }
            // Fall through to session average if rolling window fails
        }
    }

    // Fall back to session average calculation
    let duration = if config.token_rate.inherit_duration_mode {
        // Use burn_rate.mode
        get_session_duration_by_mode(session_id)?
    } else {
        // Always use wall_clock
        get_session_duration(session_id)?
    };

    // Require minimum session duration for meaningful rates
    // Uses the same configurable threshold as burn rate
    if duration < config.burn_rate.min_duration_seconds {
        return None;
    }

    let duration_f64 = duration as f64;

    // Calculate rates (tokens per second)
    let input_rate = input_tokens as f64 / duration_f64;
    let output_rate = output_tokens as f64 / duration_f64;
    let cache_read_rate = cache_read_tokens as f64 / duration_f64;
    let cache_creation_rate = cache_creation_tokens as f64 / duration_f64;
    let total_rate = total_tokens as f64 / duration_f64;

    // Calculate cache metrics
    let (cache_hit_ratio, cache_roi) = calculate_cache_metrics(
        config,
        cache_read_tokens,
        input_tokens,
        cache_creation_tokens,
    );

    Some(TokenRateMetrics {
        input_rate,
        output_rate,
        cache_read_rate,
        cache_creation_rate,
        total_rate,
        duration_seconds: duration,
        cache_hit_ratio,
        cache_roi,
        session_total_tokens: total_tokens,
        daily_total_tokens,
    })
}

/// Helper to calculate cache metrics (hit ratio and ROI)
pub fn calculate_cache_metrics(
    config: &crate::config::Config,
    cache_read_tokens: u32,
    input_tokens: u32,
    cache_creation_tokens: u32,
) -> (Option<f64>, Option<f64>) {
    if !config.token_rate.cache_metrics {
        return (None, None);
    }

    // Cache hit ratio: cache_read / (cache_read + input)
    let total_potential_cache = cache_read_tokens + input_tokens;
    let hit_ratio = if total_potential_cache > 0 {
        Some(cache_read_tokens as f64 / total_potential_cache as f64)
    } else {
        None
    };

    // Cache ROI: tokens saved / cost of creating cache
    // ROI = cache_read / cache_creation (how many times we benefited from cache)
    let roi = if cache_creation_tokens > 0 {
        Some(cache_read_tokens as f64 / cache_creation_tokens as f64)
    } else if cache_read_tokens > 0 {
        Some(f64::INFINITY) // Free cache reads (cache created elsewhere)
    } else {
        None
    };

    (hit_ratio, roi)
}

/// Convenience wrapper that creates its own database connection.
///
/// For better performance on hot paths, use `calculate_token_rates_with_db()` with a
/// pre-created database handle.
///
/// Returns None if:
/// - Token rate feature is disabled
/// - Database doesn't exist
/// - Session has no token data
#[allow(dead_code)] // Public API - used by library consumers and tests
pub fn calculate_token_rates(session_id: &str) -> Option<TokenRateMetrics> {
    // Check if token rate feature is enabled before creating db
    let config = crate::config::get_config();
    if !config.token_rate.enabled {
        return None;
    }

    // Create database connection (acceptable overhead for convenience callers)
    let db_path = StatsData::get_sqlite_path().ok()?;
    if !db_path.exists() {
        return None; // Don't create new DB
    }
    let db = crate::database::SqliteDatabase::new(&db_path).ok()?;

    calculate_token_rates_with_db(session_id, &db)
}

/// Test-only: Calculate token rate metrics from raw values without config lookup.
/// This bypasses the OnceLock config to allow deterministic testing.
#[cfg(test)]
pub fn calculate_token_rates_from_raw(
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    duration_seconds: u64,
    daily_total_tokens: u64,
) -> Option<TokenRateMetrics> {
    if duration_seconds < 60 {
        return None; // Minimum 60 seconds for stable rates
    }

    let total_tokens = input_tokens as u64
        + output_tokens as u64
        + cache_read_tokens as u64
        + cache_creation_tokens as u64;

    if total_tokens == 0 {
        return None;
    }

    let duration_f64 = duration_seconds as f64;

    let input_rate = input_tokens as f64 / duration_f64;
    let output_rate = output_tokens as f64 / duration_f64;
    let cache_read_rate = cache_read_tokens as f64 / duration_f64;
    let cache_creation_rate = cache_creation_tokens as f64 / duration_f64;
    let total_rate = total_tokens as f64 / duration_f64;

    // Calculate cache metrics (consistent with calculate_cache_metrics)
    // Cache hit ratio = cache_read / (cache_read + input) - percentage of input from cache
    let total_potential_cache = cache_read_tokens as u64 + input_tokens as u64;
    let cache_hit_ratio = if total_potential_cache > 0 {
        Some(cache_read_tokens as f64 / total_potential_cache as f64)
    } else {
        None
    };

    let cache_roi = if cache_creation_tokens > 0 {
        // ROI = reads / (creation * cost_multiplier)
        // Cache creation costs 1.25x input, reads cost 0.1x
        // So ROI = (reads * 0.1) / (creation * 1.25) * effective_factor
        // Simplified: reads / (creation * 1.25) shows how many tokens saved per investment
        Some(cache_read_tokens as f64 / (cache_creation_tokens as f64 * 1.25))
    } else if cache_read_tokens > 0 {
        Some(f64::INFINITY) // All reads, no creation cost
    } else {
        None
    };

    Some(TokenRateMetrics {
        input_rate,
        output_rate,
        cache_read_rate,
        cache_creation_rate,
        total_rate,
        duration_seconds,
        cache_hit_ratio,
        cache_roi,
        session_total_tokens: total_tokens,
        daily_total_tokens,
    })
}
