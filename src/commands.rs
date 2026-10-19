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

// Terminal-safety helpers shared by the `sessions` and `stats` handlers and
// by `error_for_terminal`. This is the single D-12/D-13 boundary: harden it
// here and every command picks the change up (WR-07).

/// One piece of untrusted text made terminal-safe: control characters and
/// escape sequences removed by `sanitize_for_terminal`, tabs turned into spaces
/// so they cannot break a column layout. Applied per piece, before the layout
/// is assembled, so intended `\n` between pieces stay.
pub(crate) fn safe(s: &str) -> String {
    crate::utils::sanitize_for_terminal(s).replace('\t', " ")
}

/// Sanitize first (D-12), then keep at most `n` chars, never bytes (D-13).
/// Truncating first could cut an escape sequence in half and leave its tail for
/// the terminal; a byte slice through a multi-byte char panics, and under
/// `panic = "abort"` that kills the process.
pub(crate) fn trunc(s: &str, n: usize) -> String {
    safe(s).chars().take(n).collect()
}

/// Shown under an empty or zero-attribution `stats` / `sessions` view when the
/// effective config does not request subagent attribution (W-NEW-2, D-04).
/// Attribution is opt-in: renders only scan transcripts when an agents value is
/// displayed, so a default user's views stay empty.
pub(crate) const ATTRIBUTION_OFF_HINT: &str = "subagent attribution is off (it is opt-in): enable it with [display].show_agents = true, or use an {agents} variable in your layout template";

/// Shown under an empty attribution view when attribution is on (D-04).
pub(crate) const ATTRIBUTION_ON_EMPTY: &str = "attribution is on: a session appears here once a render with stats updates has read its transcript";

/// Neutral cause of a stored row with zero attribution (D-06). Describes the
/// stored row, not the current config, so it stays true after a config change.
pub(crate) const ZERO_CAUSE: &str =
    "attribution was off when the session rendered, or the session predates attribution";

/// The empty-view hint for the current attribution state.
pub(crate) fn attribution_hint(on: bool) -> &'static str {
    if on {
        ATTRIBUTION_ON_EMPTY
    } else {
        ATTRIBUTION_OFF_HINT
    }
}

/// Whether the effective config requests subagent attribution, with the same
/// predicate the render path uses. `get_config` honors the CLI → env → file →
/// default precedence and never fails: a config error falls back to the
/// default config, i.e. attribution off (D-04, threat T-18.1-07).
pub(crate) fn attribution_on() -> bool {
    crate::display::agents_requested(crate::config::get_config())
}

/// `a / b` as `f64`, `0.0` when `b == 0`.
pub(crate) fn div(a: i64, b: i64) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

/// Agent share in percent of `agent + main`, `0.0` when both are 0.
///
/// The callers pass counters read through [`count_lossy`], which never returns
/// a negative value, so the `0.0` guard only fires for `0 / 0` and the result
/// is always within `0..=100`.
///
/// Computed in `f64`: [`count_lossy`] saturates a huge cell at `i64::MAX`, and
/// `agent * 100` / `agent + main` in `i64` would then panic in a debug build and
/// wrap to a wrong percentage in release.
pub(crate) fn share(agent: i64, main: i64) -> f64 {
    let total = agent as f64 + main as f64;
    if total == 0.0 {
        0.0
    } else {
        agent as f64 * 100.0 / total
    }
}

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

/// Read an INTEGER column lossily: NULL, a BLOB, text that is not a number and
/// a non-finite value (`inf`, `'1e999'`, NaN) read as `0`, a finite REAL
/// truncated toward zero (saturating at the `i64` bounds).
///
/// SQLite's INTEGER affinity keeps a non-integral REAL (`1.5`) or non-numeric
/// TEXT (`'abc'`) as stored, and `SUM()` returns REAL as soon as one input is
/// not an integer. A strict `Row::get::<i64>` fails on such a cell, and like an
/// invalid UTF-8 cell for [`text_lossy`], that one cell would abort the whole
/// `sessions` / `stats` command.
pub(crate) fn int_lossy(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<i64> {
    use rusqlite::types::ValueRef;
    Ok(match r.get_ref(i)? {
        ValueRef::Integer(n) => n,
        ValueRef::Real(f) => finite_to_i64(f),
        ValueRef::Text(b) => {
            let s = String::from_utf8_lossy(b);
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(finite_to_i64))
                .unwrap_or(0)
        }
        ValueRef::Null | ValueRef::Blob(_) => 0,
    })
}

/// A finite `f64` truncated toward zero (`as` saturates at the `i64` bounds),
/// `0` for a non-finite one: an infinite cell is damage, not a huge count.
fn finite_to_i64(f: f64) -> i64 {
    if f.is_finite() {
        f as i64
    } else {
        0
    }
}

/// Read a counter column (tokens, requests, lines, seconds, sizes) lossily:
/// [`int_lossy`], clamped at `0`.
///
/// No counter is ever negative; a negative cell can only come from a damaged
/// or hand-edited store, and rendering it would print nonsense such as
/// `lines +0 / --5`, `0h-1m` or an agent share above 100% (IN-01). This is the
/// single coercion every `sessions` / `stats` view applies to a counter cell,
/// so the views agree on the same store (IN-02).
pub(crate) fn count_lossy(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<i64> {
    Ok(int_lossy(r, i)?.max(0))
}

/// Read a REAL column lossily: NULL, a BLOB, text that is not a number and a
/// non-finite value read as `0.0`. Same reason as [`int_lossy`].
pub(crate) fn real_lossy(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<f64> {
    use rusqlite::types::ValueRef;
    let v = match r.get_ref(i)? {
        ValueRef::Integer(n) => n as f64,
        ValueRef::Real(f) => f,
        ValueRef::Text(b) => String::from_utf8_lossy(b)
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0),
        ValueRef::Null | ValueRef::Blob(_) => 0.0,
    };
    Ok(if v.is_finite() { v } else { 0.0 })
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
    match e {
        crate::error::StatuslineError::Other(msg) => {
            msg.split('\n').map(safe).collect::<Vec<_>>().join("\n")
        }
        other => safe(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        attribution_hint, count_lossy, error_for_terminal, int_lossy, real_lossy, share,
        ATTRIBUTION_OFF_HINT, ATTRIBUTION_ON_EMPTY, ZERO_CAUSE,
    };
    use crate::error::StatuslineError;

    #[test]
    fn hint_constants_follow_state() {
        assert_eq!(attribution_hint(false), ATTRIBUTION_OFF_HINT);
        assert_eq!(attribution_hint(true), ATTRIBUTION_ON_EMPTY);
        assert!(ATTRIBUTION_OFF_HINT.contains("opt-in"));
        assert!(ATTRIBUTION_OFF_HINT.contains("[display].show_agents = true"));
        assert!(!ATTRIBUTION_ON_EMPTY.contains("off"));
        // Constants are already terminal-safe.
        for s in [ATTRIBUTION_OFF_HINT, ATTRIBUTION_ON_EMPTY, ZERO_CAUSE] {
            assert_eq!(super::safe(s), s);
        }
    }

    #[test]
    fn numeric_readers_tolerate_any_cell() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let (ints, counts, reals): (Vec<i64>, Vec<i64>, Vec<f64>) = conn
            .query_row(
                "SELECT 7, 1.5, -2.9, 'abc', ' 12 ', '3.7', NULL, X'00ff', 1e300, '1e999',
                        9e999, -9e999, '-1e999', -5, ' -7 '",
                [],
                |r| {
                    let mut ints = Vec::new();
                    let mut counts = Vec::new();
                    let mut reals = Vec::new();
                    for i in 0..15 {
                        ints.push(int_lossy(r, i)?);
                        counts.push(count_lossy(r, i)?);
                        reals.push(real_lossy(r, i)?);
                    }
                    Ok((ints, counts, reals))
                },
            )
            .unwrap();
        // Finite but huge saturates; non-finite (inf, -inf, '1e999') reads 0.
        assert_eq!(
            ints,
            [7, 1, -2, 0, 12, 3, 0, 0, i64::MAX, 0, 0, 0, 0, -5, -7]
        );
        // Counters additionally never go negative (IN-01).
        assert_eq!(
            counts,
            [7, 1, 0, 0, 12, 3, 0, 0, i64::MAX, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            reals,
            [7.0, 1.5, -2.9, 0.0, 12.0, 3.7, 0.0, 0.0, 1e300, 0.0, 0.0, 0.0, 0.0, -5.0, -7.0]
        );
    }

    #[test]
    fn share_survives_saturated_counters() {
        assert_eq!(share(25, 75), 25.0);
        assert_eq!(share(0, 0), 0.0);
        let s = share(i64::MAX, 100);
        assert!((99.99..=100.0).contains(&s), "{s}");
        assert_eq!(share(i64::MAX, i64::MAX), 50.0);
        assert!(share(100, i64::MAX) < 1e-10);
        // Counters reach share() through count_lossy, so they are >= 0: the
        // guard fires only for 0/0 and the share never exceeds 100%.
        assert_eq!(share(0, 5), 0.0);
        assert_eq!(share(5, 0), 100.0);
    }

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
