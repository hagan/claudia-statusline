// Hook handler for Claude Code PreCompact and Stop events
//
// This module provides handlers for Claude Code's hook system to track
// compaction state in real-time via file-based state management.

use chrono::Utc;

use crate::error::Result;
use crate::state::{clear_state, write_state, HookState};

/// Handle PreCompact hook event
///
/// Called when Claude is about to compact the conversation.
/// Writes compaction state to file for statusline to detect.
///
/// # Arguments
///
/// * `session_id` - Current Claude session ID
/// * `trigger` - Compaction trigger type ("auto" or "manual")
///
/// # Returns
///
/// Ok(()) on success, error on file write failure
pub fn handle_precompact(session_id: &str, trigger: &str) -> Result<()> {
    let state = HookState {
        state: "compacting".to_string(),
        trigger: trigger.to_string(),
        session_id: session_id.to_string(),
        started_at: Utc::now(),
        pid: Some(std::process::id()),
    };

    write_state(&state)?;

    log::info!(
        "PreCompact hook: session={}, trigger={}",
        session_id,
        trigger
    );

    Ok(())
}

/// Handle Stop hook event
///
/// Called after each Claude agent response (fires frequently).
/// Only clears compaction state file - does NOT reset database.
///
/// NOTE: Stop hook fires after EVERY agent response, not just after compaction.
/// For post-compaction cleanup, use handle_postcompact() instead.
///
/// # Arguments
///
/// * `session_id` - Current Claude session ID
///
/// # Returns
///
/// Ok(()) on success, error on file deletion failure
pub fn handle_stop(session_id: &str) -> Result<()> {
    clear_state(session_id)?;

    log::info!("Stop hook: session={}", session_id);

    Ok(())
}

/// Handle PostCompact hook event (via SessionStart[compact])
///
/// Called after compaction completes. This is triggered by configuring
/// a SessionStart hook with matcher "compact" in Claude Code settings.
///
/// This handler:
/// 1. Clears the "compacting" state file created by PreCompact
/// 2. Resets max_tokens_observed so Phase 2 detection starts fresh
///
/// # Arguments
///
/// * `session_id` - Current Claude session ID
///
/// # Returns
///
/// Ok(()) on success, error on cleanup failure
///
/// # Example Configuration
///
/// ```json
/// {
///   "hooks": {
///     "SessionStart": [{
///       "matcher": "compact",
///       "hooks": [{"type": "command", "command": "statusline hook postcompact"}]
///     }]
///   }
/// }
/// ```
pub fn handle_postcompact(session_id: &str) -> Result<()> {
    use crate::common::get_data_dir;
    use crate::database::SqliteDatabase;

    // Clear the "compacting" state file created by PreCompact hook
    clear_state(session_id)?;

    // Reset max_tokens_observed for this session
    // This ensures Phase 2 heuristic detection starts fresh
    // and won't give false positives from pre-compaction token counts
    let db_path = get_data_dir().join("stats.db");
    if db_path.exists() {
        if let Ok(db) = SqliteDatabase::new(&db_path) {
            if let Err(e) = db.reset_session_max_tokens(session_id) {
                log::warn!("Failed to reset max_tokens after compaction: {}", e);
            } else {
                log::debug!("Reset max_tokens_observed for session {}", session_id);
            }
        }
    }

    log::info!(
        "PostCompact hook (via SessionStart[compact]): session={}",
        session_id
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::read_state;

    fn test_session_id() -> String {
        format!("test-hook-{}", std::process::id())
    }

    #[test]
    fn test_handle_precompact() {
        let session_id = format!("{}-precompact", test_session_id());

        // Handle precompact
        handle_precompact(&session_id, "auto").unwrap();

        // Verify state was written
        let state = read_state(&session_id).expect("State should exist");
        assert_eq!(state.state, "compacting");
        assert_eq!(state.trigger, "auto");
        assert_eq!(state.session_id, session_id);
        assert!(state.pid.is_some());

        // Cleanup
        clear_state(&session_id).unwrap();
    }

    #[test]
    fn test_handle_stop() {
        let session_id = format!("{}-stop", test_session_id());

        // Create state first
        handle_precompact(&session_id, "manual").unwrap();
        assert!(read_state(&session_id).is_some());

        // Handle stop
        handle_stop(&session_id).unwrap();

        // Verify state was cleared
        assert!(read_state(&session_id).is_none());
    }

    #[test]
    fn test_handle_precompact_manual_trigger() {
        let session_id = format!("{}-manual", test_session_id());

        handle_precompact(&session_id, "manual").unwrap();

        let state = read_state(&session_id).expect("State should exist");
        assert_eq!(state.trigger, "manual");

        // Cleanup
        clear_state(&session_id).unwrap();
    }

    #[test]
    fn test_multiple_precompact_calls() {
        let session_id = format!("{}-multi", test_session_id());

        // First call
        handle_precompact(&session_id, "auto").unwrap();
        let state1 = read_state(&session_id).expect("State should exist");

        // Second call (should overwrite)
        handle_precompact(&session_id, "manual").unwrap();
        let state2 = read_state(&session_id).expect("State should exist");

        // Should have updated trigger
        assert_eq!(state2.trigger, "manual");
        assert!(state2.started_at >= state1.started_at);

        // Cleanup
        clear_state(&session_id).unwrap();
    }

    #[test]
    fn test_stop_without_precompact() {
        let session_id = format!("{}-no-precompact", test_session_id());

        // Stop without precompact should not error (idempotent)
        let result = handle_stop(&session_id);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_postcompact() {
        let session_id = format!("{}-postcompact", test_session_id());

        // Create compacting state first (simulates PreCompact)
        handle_precompact(&session_id, "auto").unwrap();
        assert!(read_state(&session_id).is_some());

        // Handle postcompact (simulates SessionStart[compact])
        handle_postcompact(&session_id).unwrap();

        // Verify state was cleared
        assert!(read_state(&session_id).is_none());
    }

    #[test]
    fn test_postcompact_without_precompact() {
        let session_id = format!("{}-no-precompact-post", test_session_id());

        // PostCompact without PreCompact should not error (idempotent)
        let result = handle_postcompact(&session_id);
        assert!(result.is_ok());
    }

    #[test]
    fn test_full_compaction_flow() {
        let session_id = format!("{}-full-flow", test_session_id());

        // 1. PreCompact: Compaction starts
        handle_precompact(&session_id, "auto").unwrap();
        let state = read_state(&session_id).expect("State should exist after PreCompact");
        assert_eq!(state.state, "compacting");
        assert_eq!(state.trigger, "auto");

        // 2. PostCompact: Compaction ends (via SessionStart[compact])
        handle_postcompact(&session_id).unwrap();

        // 3. Verify clean state
        assert!(
            read_state(&session_id).is_none(),
            "State should be cleared after PostCompact"
        );
    }

    #[test]
    fn test_postcompact_manual_trigger() {
        let session_id = format!("{}-postcompact-manual", test_session_id());

        // Create compacting state with manual trigger
        handle_precompact(&session_id, "manual").unwrap();
        let state = read_state(&session_id).expect("State should exist");
        assert_eq!(state.trigger, "manual");

        // PostCompact clears regardless of trigger type
        handle_postcompact(&session_id).unwrap();
        assert!(read_state(&session_id).is_none());
    }
}
