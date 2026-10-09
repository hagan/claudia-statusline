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
/// When `update_stats` is `false`, it performs no database writes at all: today's daily
/// total is read through a read-only open (`StatsData::load_read_only`) that never
/// creates, migrates, repairs or checkpoints stats.db (AGENT-04). When `update_stats` is
/// `true` but no session/cost is present, that step performs no write and reads back the
/// existing daily total.
///
/// The returned daily total is the one observed immediately after the cost update
/// (the later max-token write does not change the returned figure), matching the prior
/// behavior of both call sites. The second element is the main/agent token attribution
/// for the session (see `agents::scan`), computed whenever a session and transcript are
/// present. Attribution is persisted inside `agents::scan` only when `update_stats` is
/// `true`; read-only renders open the database read-only and never write.
pub fn update_stats_and_daily_total(
    input: &StatuslineInput,
    update_stats: bool,
) -> (f64, Option<crate::agents::AgentsSummary>) {
    use crate::{common, config, stats, utils};

    // Sample "now" before any transcript read or DB open: lock waits (busy_timeout 10 s plus
    // retries) would otherwise inflate the inferred process start (RESEARCH Pitfall 3).
    let now_ms = crate::stats::accounting::unix_now_ms();

    let model_name = input.model.as_ref().and_then(|m| m.detection_name());
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
                            // Inferred process start for restart/correction classification
                            // (stats::accounting, D-01/D-17); None when duration is unknown.
                            inferred_start_ms: crate::stats::accounting::inferred_start_ms(
                                cost.total_duration_ms,
                                now_ms,
                            ),
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
        // Not updating stats — read back today's accumulated daily total through a
        // read-only open (AGENT-04): never creates, migrates or repairs stats.db.
        stats::get_daily_total(&crate::stats::StatsData::load_read_only())
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

    // 4. Main/agent token attribution. `agents::scan` persists its state only when
    //    `update_stats` is true (one IMMEDIATE transaction, derived session totals);
    //    read-only renders open stats.db read-only and never write (AGENT-04).
    let agents = match (session_id, transcript_path) {
        (Some(sid), Some(t)) => crate::agents::scan(sid, t, input.version.as_deref(), update_stats),
        _ => None,
    };

    (daily_total, agents)
}
