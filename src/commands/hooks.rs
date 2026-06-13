//! `hook` subcommand handlers: dispatch Claude Code hook events.

use std::io::{self, Read};

use crate::error::Result;
use crate::HookAction;

/// Handle hook command invocations from Claude Code
pub(crate) fn handle_hook_command(action: HookAction) -> Result<()> {
    match action {
        HookAction::Precompact {
            session_id,
            trigger,
        } => {
            // If CLI args provided, use them; otherwise read from stdin
            let (sid, trig) = if let (Some(s), Some(t)) = (session_id, trigger) {
                (s, t)
            } else {
                read_hook_json_from_stdin()?
            };

            crate::hook_handler::handle_precompact(&sid, &trig)?;
            println!("PreCompact hook processed for session: {}", sid);
        }
        HookAction::Stop { session_id } => {
            // If CLI arg provided, use it; otherwise read from stdin
            let sid = if let Some(s) = session_id {
                s
            } else {
                let (s, _) = read_hook_json_from_stdin()?;
                s
            };

            crate::hook_handler::handle_stop(&sid)?;
            println!("Stop hook processed for session: {}", sid);
        }
        HookAction::Postcompact { session_id } => {
            // If CLI arg provided, use it; otherwise read from stdin
            let sid = if let Some(s) = session_id {
                s
            } else {
                let (s, _) = read_hook_json_from_stdin()?;
                s
            };

            crate::hook_handler::handle_postcompact(&sid)?;
            println!("PostCompact hook processed for session: {}", sid);
        }
    }
    Ok(())
}

/// Read hook event JSON from stdin
///
/// Claude Code sends hook data as JSON via stdin with fields:
/// - session_id: string
/// - trigger: string (for PreCompact)
/// - hook_event_name: string
/// - transcript_path: string
///
/// Returns (session_id, trigger) tuple
fn read_hook_json_from_stdin() -> Result<(String, String)> {
    use serde_json::Value;

    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let json: Value = serde_json::from_str(&buffer)?;

    let session_id = json
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::error::StatuslineError::other("Missing 'session_id' in hook JSON"))?
        .to_string();

    let trigger = json
        .get("trigger")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string();

    Ok((session_id, trigger))
}
