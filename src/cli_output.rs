//! Bin-only choke point for subcommand stdout.
//!
//! When stdout's reader goes away (`statusline stats | head`), a write returns
//! `ErrorKind::BrokenPipe`: nothing is left to deliver, so the process exits 0
//! silently. Any other stdout error is written to stderr and exits 1.
//!
//! Only stdout goes through here. Errors on OTHER pipes (e.g. the curl stdin
//! pipe in `src/ant`) never pass through this module and must stay reported.
//!
//! This module is declared only in `src/main.rs`, never in `src/lib.rs`: a
//! library must never call `process::exit`.

use std::io::Write;

fn handle_stdout_error(e: std::io::Error) -> ! {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        // Reader went away (`| head`): nothing left to deliver.
        std::process::exit(0);
    }
    let _ = writeln!(std::io::stderr(), "error: writing to stdout: {e}");
    std::process::exit(1);
}

/// Write formatted arguments to stdout, exiting quietly on a closed pipe.
#[doc(hidden)]
pub(crate) fn emit_stdout(args: std::fmt::Arguments<'_>) {
    let mut out = std::io::stdout().lock();
    if let Err(e) = out.write_fmt(args) {
        handle_stdout_error(e);
    }
}

/// Flush stdout with the same closed-pipe rule as [`emit_stdout`].
pub(crate) fn flush_stdout() {
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
