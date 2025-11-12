// Hook-based state management for real-time compaction tracking
//
// This module provides file-based state persistence for Claude Code hooks.
// State files are session-scoped, ephemeral, and automatically cleaned up.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::error::Result;

/// Hook state tracked via file for real-time detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookState {
    /// Current state (e.g., "compacting")
    pub state: String,

    /// Trigger type: "auto" or "manual"
    pub trigger: String,

    /// Session ID for isolation
    pub session_id: String,

    /// When the state was created
    pub started_at: DateTime<Utc>,

    /// Optional process ID for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Staleness timeout: state files older than this are deleted
const STALE_TIMEOUT_SECONDS: i64 = 120; // 2 minutes

/// Get the cache directory for state files
fn get_cache_dir() -> Result<PathBuf> {
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| {
            crate::error::StatuslineError::Config("Cannot determine cache directory".to_string())
        })?
        .join("claudia-statusline");

    // Ensure directory exists
    fs::create_dir_all(&cache_dir)?;

    Ok(cache_dir)
}

/// Get the state file path for a session
fn get_state_file_path(session_id: &str) -> Result<PathBuf> {
    let cache_dir = get_cache_dir()?;
    Ok(cache_dir.join(format!("state-{}.json", session_id)))
}

/// Write state to file atomically
pub fn write_state(state: &HookState) -> Result<()> {
    let state_file = get_state_file_path(&state.session_id)?;

    // Serialize to JSON
    let json = serde_json::to_string_pretty(state)?;

    // Write atomically (write to temp file, then rename)
    let temp_file = state_file.with_extension("json.tmp");
    fs::write(&temp_file, json)?;
    fs::rename(temp_file, state_file)?;

    log::debug!(
        "Wrote state for session {}: {} ({})",
        state.session_id,
        state.state,
        state.trigger
    );

    Ok(())
}

/// Read state from file, checking for staleness
pub fn read_state(session_id: &str) -> Option<HookState> {
    let state_file = match get_state_file_path(session_id) {
        Ok(path) => path,
        Err(e) => {
            log::warn!("Cannot determine state file path: {}", e);
            return None;
        }
    };

    // Check if file exists
    if !state_file.exists() {
        return None;
    }

    // Read and parse JSON
    let content = match fs::read_to_string(&state_file) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Cannot read state file: {}", e);
            return None;
        }
    };

    let state: HookState = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Corrupted state file, deleting: {}", e);
            let _ = fs::remove_file(&state_file);
            return None;
        }
    };

    // Validate session ID matches
    if state.session_id != session_id {
        log::debug!(
            "Session ID mismatch: expected {}, got {}",
            session_id,
            state.session_id
        );
        return None;
    }

    // Check staleness
    let age = Utc::now().signed_duration_since(state.started_at);
    if age > Duration::seconds(STALE_TIMEOUT_SECONDS) {
        log::info!(
            "Stale state detected (age: {}s), deleting",
            age.num_seconds()
        );
        let _ = fs::remove_file(&state_file);
        return None;
    }

    log::debug!(
        "Read state for session {}: {} (age: {}s)",
        session_id,
        state.state,
        age.num_seconds()
    );

    Some(state)
}

/// Clear state for a session
pub fn clear_state(session_id: &str) -> Result<()> {
    let state_file = get_state_file_path(session_id)?;

    if state_file.exists() {
        fs::remove_file(&state_file)?;
        log::debug!("Cleared state for session {}", session_id);
    }

    Ok(())
}

/// Clean up all stale state files in the cache directory
#[allow(dead_code)]
pub fn cleanup_stale_states() -> Result<usize> {
    let cache_dir = get_cache_dir()?;
    let mut cleaned = 0;

    // Iterate over state files
    for entry in fs::read_dir(&cache_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Only process state-*.json files
        if let Some(filename) = path.file_name() {
            let filename = filename.to_string_lossy();
            if filename.starts_with("state-") && filename.ends_with(".json") {
                // Extract session ID
                if let Some(session_id) = filename
                    .strip_prefix("state-")
                    .and_then(|s| s.strip_suffix(".json"))
                {
                    // Try to read state (this will auto-delete if stale)
                    if read_state(session_id).is_none() && !path.exists() {
                        cleaned += 1;
                    }
                }
            }
        }
    }

    if cleaned > 0 {
        log::info!("Cleaned up {} stale state files", cleaned);
    }

    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session_id() -> String {
        format!("test-{}", std::process::id())
    }

    #[test]
    fn test_write_and_read_state() {
        let session_id = format!("{}-write-read", test_session_id());
        let state = HookState {
            state: "compacting".to_string(),
            trigger: "auto".to_string(),
            session_id: session_id.clone(),
            started_at: Utc::now(),
            pid: Some(std::process::id()),
        };

        // Write state
        write_state(&state).unwrap();

        // Read back
        let read = read_state(&session_id).unwrap();
        assert_eq!(read.state, "compacting");
        assert_eq!(read.trigger, "auto");
        assert_eq!(read.session_id, session_id);

        // Cleanup
        clear_state(&session_id).unwrap();
    }

    #[test]
    fn test_stale_state_detected() {
        let session_id = format!("{}-stale", test_session_id());
        let state = HookState {
            state: "compacting".to_string(),
            trigger: "auto".to_string(),
            session_id: session_id.clone(),
            started_at: Utc::now() - Duration::minutes(5), // 5 minutes ago (stale)
            pid: None,
        };

        // Write stale state
        write_state(&state).unwrap();

        // Try to read - should return None and delete file
        let read = read_state(&session_id);
        assert!(read.is_none());

        // Verify file was deleted
        let state_file = get_state_file_path(&session_id).unwrap();
        assert!(!state_file.exists());
    }

    #[test]
    fn test_corrupted_json_handled() {
        let session_id = format!("{}-corrupted", test_session_id());
        let state_file = get_state_file_path(&session_id).unwrap();

        // Write invalid JSON
        fs::write(&state_file, "{ invalid json }").unwrap();

        // Try to read - should return None and delete file
        let read = read_state(&session_id);
        assert!(read.is_none());

        // Verify file was deleted
        assert!(!state_file.exists());
    }

    #[test]
    fn test_session_id_validation() {
        let session_id_a = format!("{}-a", test_session_id());
        let session_id_b = format!("{}-b", test_session_id());

        let state = HookState {
            state: "compacting".to_string(),
            trigger: "auto".to_string(),
            session_id: session_id_a.clone(),
            started_at: Utc::now(),
            pid: None,
        };

        // Write state for session A
        write_state(&state).unwrap();

        // Try to read with session B - should return None
        let read = read_state(&session_id_b);
        assert!(read.is_none());

        // Cleanup
        clear_state(&session_id_a).unwrap();
    }

    #[test]
    fn test_cleanup_stale_states() {
        // Create multiple stale state files
        for i in 0..3 {
            let session_id = format!("{}-cleanup-{}", test_session_id(), i);
            let state = HookState {
                state: "compacting".to_string(),
                trigger: "auto".to_string(),
                session_id: session_id.clone(),
                started_at: Utc::now() - Duration::minutes(5), // Stale
                pid: None,
            };
            write_state(&state).unwrap();
        }

        // Run cleanup
        let cleaned = cleanup_stale_states().unwrap();
        assert!(cleaned >= 3, "Should clean at least 3 stale states");
    }

    #[test]
    fn test_clear_state() {
        let session_id = format!("{}-clear", test_session_id());
        let state = HookState {
            state: "compacting".to_string(),
            trigger: "auto".to_string(),
            session_id: session_id.clone(),
            started_at: Utc::now(),
            pid: None,
        };

        // Write state
        write_state(&state).unwrap();

        // Verify it exists
        assert!(read_state(&session_id).is_some());

        // Clear state
        clear_state(&session_id).unwrap();

        // Verify it's gone
        assert!(read_state(&session_id).is_none());
    }
}
