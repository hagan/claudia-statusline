//! Binary-only subcommand handlers for the statusline CLI.
//!
//! Each themed submodule owns the handler functions for one dispatch group in
//! `main`. This module is intentionally **not** part of the library crate
//! (`lib.rs`): its handlers reference clap types (`Cli`, `HookAction`) defined
//! in `main.rs`, so it is declared with `mod commands;` from the binary only.
//!
//! Handlers are `pub(crate)` and called fully-qualified from the `main` dispatch
//! match (e.g. `commands::migrate::run_schema_migrations()`).

pub(crate) mod context_learning;
pub(crate) mod health;
pub(crate) mod hooks;
pub(crate) mod list_vars;
pub(crate) mod maintenance;
pub(crate) mod migrate;

#[cfg(feature = "turso-sync")]
pub(crate) mod sync;
