//! Bin-only choke point for subcommand stdout.
//!
//! When stdout's reader goes away (`statusline stats | head`), a write returns
//! `ErrorKind::BrokenPipe`. Nothing is left to deliver, so the first such error
//! sets a process-wide "stdout closed" flag and every later stdout write is
//! silently dropped. The command is NOT stopped: it runs to completion, runs its
//! cleanup (temp files, `Drop` impls) and returns its real exit status. A
//! read-only command therefore exits 0, while a mutating command
//! (`context-learning --reset-all --rebuild`, `db-maintain`, `sync push`) still
//! finishes its work, and a failure exit (`db-maintain` integrity failure,
//! `config` validation) stays non-zero.
//!
//! Any other stdout error (e.g. `EIO`, `ENOSPC`) is written to stderr and the
//! process exits with status 1. That does stop the command midway, but it
//! reports failure rather than masking it, and such errors mean output the user
//! asked for was lost.
//!
//! Only stdout goes through here. Errors on OTHER pipes (e.g. the curl stdin
//! pipe in `src/ant`) never pass through this module and must stay reported.
//!
//! This module is declared only in `src/main.rs`, never in `src/lib.rs`: a
//! library must never call `process::exit`.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once stdout's reader has gone away; later stdout writes are dropped.
static STDOUT_CLOSED: AtomicBool = AtomicBool::new(false);

fn handle_stdout_error(e: std::io::Error) {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        // Reader went away (`| head`): drop further output, keep running so the
        // command finishes and keeps its real exit status.
        STDOUT_CLOSED.store(true, Ordering::Relaxed);
        return;
    }
    let _ = writeln!(std::io::stderr(), "error: writing to stdout: {e}");
    std::process::exit(1);
}

/// Write formatted arguments to stdout, dropping output once the pipe closed.
#[doc(hidden)]
pub(crate) fn emit_stdout(args: std::fmt::Arguments<'_>) {
    if STDOUT_CLOSED.load(Ordering::Relaxed) {
        return;
    }
    let mut out = std::io::stdout().lock();
    if let Err(e) = out.write_fmt(args) {
        handle_stdout_error(e);
    }
}

/// Flush stdout with the same closed-pipe rule as [`emit_stdout`].
pub(crate) fn flush_stdout() {
    if STDOUT_CLOSED.load(Ordering::Relaxed) {
        return;
    }
    if let Err(e) = std::io::stdout().flush() {
        handle_stdout_error(e);
    }
}

/// `print!` replacement for subcommand output (see module docs).
macro_rules! cli_print {
    ($($a:tt)*) => {
        $crate::cli_output::emit_stdout(format_args!($($a)*))
    };
}

/// `println!` replacement for subcommand output (see module docs).
macro_rules! cli_println {
    () => {
        $crate::cli_output::emit_stdout(format_args!("\n"))
    };
    ($($a:tt)*) => {
        $crate::cli_output::emit_stdout(format_args!("{}\n", format_args!($($a)*)))
    };
}
