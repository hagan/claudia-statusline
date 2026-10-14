//! Binary-only subcommand handlers for the statusline CLI.
//!
//! Each themed submodule owns the handler functions for one dispatch group in
//! `main`. This module is intentionally **not** part of the library crate
//! (`lib.rs`): its handlers reference clap types (`Cli`, `HookAction`) defined
//! in `main.rs`, so it is declared with `mod commands;` from the binary only.
//!
//! Handlers are `pub(crate)` and called fully-qualified from the `main` dispatch
//! match (e.g. `commands::migrate::run_schema_migrations()`).

pub(crate) mod ant;
pub(crate) mod config;
pub(crate) mod context_learning;
pub(crate) mod gsd;
pub(crate) mod health;
pub(crate) mod hooks;
pub(crate) mod list_vars;
pub(crate) mod maintenance;
pub(crate) mod migrate;
pub(crate) mod sessions;
pub(crate) mod stats;

#[cfg(feature = "turso-sync")]
pub(crate) mod sync;

/// Read a TEXT column lossily: `None` for NULL, invalid UTF-8 replaced by
/// U+FFFD, numbers rendered as text.
///
/// A strict `Row::get::<String>` fails on a single malformed cell (Turso pull,
/// hand edit, damaged store), and because the `sessions` / `stats` handlers load
/// every row up front, that one cell would abort the whole command. Reading
/// lossily keeps the row; its text is still sanitized before it is printed.
pub(crate) fn text_lossy(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<Option<String>> {
    use rusqlite::types::ValueRef;
    Ok(match r.get_ref(i)? {
        ValueRef::Null => None,
        ValueRef::Text(b) | ValueRef::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
        ValueRef::Integer(n) => Some(n.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
    })
}

/// Render an error from a `sessions` / `stats` handler for the terminal (D-12).
///
/// - `StatuslineError::Other(msg)`: messages assembled by the command handlers
///   (`sessions::resolve` selection errors, the `pick` non-TTY refusal, the
///   `workspace_filter` current-dir error). Their untrusted pieces are already
///   sanitized per piece and their `'\n'` are intentional layout, so each
///   `'\n'`-separated line is sanitized on its own and the lines are rejoined.
/// - Every other variant (`Database`, `Io`, `Json`, ...) is opaque text that may
///   embed env-derived paths (rusqlite `CannotOpen` carries the db filename) or
///   stored strings, so the whole `Display` is sanitized, which also drops
///   `'\n'`/`'\r'` and flattens it to one line.
///
/// Tabs become spaces in both cases. This covers what the commands print, not
/// env_logger diagnostics (`RUST_LOG` / `--log-level`), which may still carry a
/// raw path (threat T-18-20, accepted).
pub(crate) fn error_for_terminal(e: &crate::error::StatuslineError) -> String {
    fn clean(s: &str) -> String {
        crate::utils::sanitize_for_terminal(s).replace('\t', " ")
    }
    match e {
        crate::error::StatuslineError::Other(msg) => {
            msg.split('\n').map(clean).collect::<Vec<_>>().join("\n")
        }
        other => clean(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::error_for_terminal;
    use crate::error::StatuslineError;

    /// Any C0 control (including '\n'), DEL or C1 control.
    fn has_control(s: &str) -> bool {
        s.chars()
            .any(|c| (c as u32) < 0x20 || (0x7F..=0x9F).contains(&(c as u32)))
    }

    #[test]
    fn error_for_terminal_flattens_opaque_errors() {
        let e = StatuslineError::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(14),
            Some("unable to open database file: /h\x1b[31mX/\nfake\r/stats.db".into()),
        ));
        let out = error_for_terminal(&e);
        assert!(!has_control(&out), "{out:?}");
        assert!(
            out.contains("unable to open database file: /hX/fake/stats.db"),
            "{out:?}"
        );

        let e = StatuslineError::Io(std::io::Error::other("bad\x1b]0;t\x07path"));
        let out = error_for_terminal(&e);
        assert!(!has_control(&out), "{out:?}");
    }

    #[test]
    fn error_for_terminal_keeps_selection_layout() {
        let msg = "'ab' matches 2 sessions, lengthen the prefix:\n  ab-1\n  ab-2";
        let e = StatuslineError::Other(msg.into());
        assert_eq!(error_for_terminal(&e), msg);
    }

    #[test]
    fn error_for_terminal_sanitizes_each_line_of_other() {
        let e = StatuslineError::Other("a\x1b[2J\rb\nc\x07\td".into());
        assert_eq!(error_for_terminal(&e), "a[2Jb\nc d");
    }
}
