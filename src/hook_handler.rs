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
/// Called when Claude session ends or compaction completes.
/// Clears compaction state file so statusline shows normal state.
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
}
