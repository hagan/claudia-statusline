//! Shared statusline rendering logic.
//!
//! The binary entry point (`main`) and the [`crate::render_statusline`] embedding API
//! historically duplicated the stats-update flow (session cost update → max-token
//! tracking for compaction detection → adaptive context learning). The two copies had
//! drifted, so any fix applied to one path could silently miss the other (see issue #32).
//!
//! This module holds the single implementation of that flow. Both crate roots
//! (`src/main.rs` and `src/lib.rs`) compile the same module files, so the binary and the
//! library share this function directly.

use crate::models::StatuslineInput;

/// Apply persistent stats updates for `input` and return today's daily cost total.
///
/// When `update_stats` is `true` and the input carries a session id, this:
/// 1. records the session's cost / lines / token breakdown (when `cost.total_cost_usd`
///    is present),
/// 2. tracks `max_tokens_observed` for compaction detection whenever a transcript is
///    available (independent of cost), and
/// 3. runs adaptive context learning when it is enabled in config.
///
/// When `update_stats` is `false`, or no session/cost is present, it performs no writes
/// for that step and simply reads back today's accumulated daily total.
///
/// The returned value is the daily total observed immediately after the cost update
/// (the later max-token write does not change the returned figure), matching the prior
/// behavior of both call sites.
pub fn update_stats_and_daily_total(input: &StatuslineInput, update_stats: bool) -> f64 {
    use crate::{common, config, stats, utils};

    let model_name = input.model.as_ref().and_then(|m| m.display_name.as_deref());
    let transcript_path = input.transcript.as_deref();
    let session_id = input.session_id.as_deref();

    // 1. Session cost update (or read-back of the existing daily total).
    let daily_total = if update_stats {
        if let (Some(session_id), Some(cost)) = (session_id, input.cost.as_ref()) {
            if let Some(total_cost) = cost.total_cost_usd {
                let workspace_dir = input
                    .workspace
                    .as_ref()
                    .and_then(|w| w.current_dir.as_deref());

                // Extract token breakdown from transcript if available.
                let token_breakdown =
                    transcript_path.and_then(utils::get_token_breakdown_from_transcript);

                // Device ID for the audit trail.
                let device_id = common::get_device_id();

                use crate::database::SessionUpdate;
                let (daily_total, _monthly_total) = stats::update_stats_data(|data| {
                    data.update_session(
                        session_id,
                        SessionUpdate {
                            cost: total_cost,
                            lines_added: cost.total_lines_added.unwrap_or(0),
                            lines_removed: cost.total_lines_removed.unwrap_or(0),
                            model_name: model_name.map(|s| s.to_string()),
                            workspace_dir: workspace_dir.map(|s| s.to_string()),
                            device_id: Some(device_id),
                            token_breakdown,
                            max_tokens_observed: None, // updated separately below
                            // active_time_seconds / last_activity are owned and computed by
                            // SqliteDatabase::update_session (src/database/session.rs); not available
                            // at this SessionUpdate construction site. v3.0.0 cleanup per CONTEXT.md D-13.
                            active_time_seconds: None,
                            last_activity: None,
                        },
                    )
                });
                daily_total
            } else {
                // Session present but no cost figure — read back existing daily total.
                stats::get_daily_total(&stats::get_or_load_stats_data())
            }
        } else {
            // No session/cost to update — read back existing daily total.
            stats::get_daily_total(&stats::get_or_load_stats_data())
        }
    } else {
        // Not updating stats — just read back today's accumulated daily total.
        stats::get_daily_total(&stats::get_or_load_stats_data())
    };

    // 2. Track max_tokens_observed for compaction detection.
    //    Runs whenever a transcript + session are present, regardless of cost.
    if update_stats {
        if let (Some(transcript), Some(session)) = (transcript_path, session_id) {
            if let Some(current_tokens) = utils::get_token_count_from_transcript(transcript) {
                // Updates both in-memory stats and the SQLite database.
                stats::update_stats_data(|data| {
                    data.update_max_tokens(session, current_tokens);
                    // Return unchanged totals (this step only updates token tracking).
                    use crate::common::{current_date, current_month};
                    let today = current_date();
                    let month = current_month();
                    let daily = data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0);
                    let monthly = data
                        .monthly
                        .get(&month)
                        .map(|m| m.total_cost)
                        .unwrap_or(0.0);
                    (daily, monthly)
                });

                // 3. Adaptive context learning: observe token usage if enabled.
                if let Some(model) = model_name {
                    let config = config::get_config();
                    if config.context.adaptive_learning {
                        // Previous token count from session stats, for delta-based learning.
                        let previous_tokens = stats::get_or_load_stats_data()
                            .sessions
                            .get(session)
                            .and_then(|s| s.max_tokens_observed)
                            .map(|t| t as usize);

                        use crate::common::get_data_dir;
                        use crate::context_learning::ContextLearner;
                        use crate::database::SqliteDatabase;

                        let db_path = get_data_dir().join("stats.db");
                        if let Ok(db) = SqliteDatabase::new(&db_path) {
                            let learner = ContextLearner::new(db);
                            let workspace_dir = input
                                .workspace
                                .as_ref()
                                .and_then(|w| w.current_dir.as_deref());
                            let device_id = common::get_device_id();
                            // Adaptive learning is experimental; ignore errors so it never blocks
                            // the statusline.
                            let _ = learner.observe_usage(
                                model,
                                current_tokens as usize,
                                previous_tokens,
                                Some(transcript),
                                workspace_dir,
                                Some(&device_id),
                            );
                        }
                    }
                }
            }
        }
    }

    daily_total
}
