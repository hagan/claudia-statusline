//! `gsd` subcommand group handlers: `gsd state`.
//!
//! This handler is deliberately **THIN** — argv in, one library call, print
//! out. Every fact it reports is computed in [`crate::gsd::summary`], which in
//! turn reuses the shipped `.planning/` parsers; no GSD semantics are decided
//! here. This mirrors the handler/logic split of `src/commands/config.rs`.
//!
//! # The contract (D-19)
//!
//! `gsd state --json` emits **exactly one** JSON document on stdout carrying
//! the three facts SC4 names — milestone, phase and progress — so an external
//! tool reads them with no prose parsing of its own. Nothing else is ever
//! printed to stdout in that mode: that is why warnings travel INSIDE the
//! document as an array rather than as lines.
//!
//! `schema_version` is the stability gate. Version `1` guarantees the key
//! paths documented in `docs/USAGE.md`; ADDING a key is a minor, non-breaking
//! change that does not bump it, while removing or retyping one does. A
//! consumer checks `schema_version` before trusting any other field.
//!
//! `source.state_frontmatter_version` echoes the raw `gsd_state_version` text
//! of the STATE.md that produced the answer. It is provenance, not a second
//! gate: the D-15 major-version gate lives in `crate::gsd::state` and this
//! field only makes its effect visible.
//!
//! # Exit contract (D-19 clause 4)
//!
//! - **0** whenever a well-formed document was produced — *including* when
//!   every field is `null` and `warnings` explains why. "There is no GSD state
//!   here" is a valid answer, and a CI consumer must be able to read it
//!   without treating the exit code as the signal.
//! - **1** only when `--dir` names a path that is not a readable directory.
//!   That is a usage error, not an absence of state. The message goes to
//!   STDERR and **nothing** is written to stdout, so the single-document
//!   contract is never broken by a half-written document.
//!
//! # Boundaries
//!
//! - Statusline never writes `STATE.md` (D-13). This command only reads.
//! - Every string echoed to stdout goes through
//!   [`crate::utils::sanitize_for_terminal`], per PIECE (T-12-64). `STATE.md`
//!   and `ROADMAP.md` are agent- or hand-authored and untrusted, and the
//!   `VariableBuilder` boundary that protects the render path does not cover
//!   this code path at all.
//! - Nothing here touches the render path, the config loader, the stats
//!   database or the network.

use std::path::PathBuf;

use serde_json::json;

use crate::error::Result;
use crate::gsd::summary::{build_summary, GsdSummary};
use crate::utils::sanitize_for_terminal;

/// Dispatch entry for `statusline gsd <action>`.
pub(crate) fn handle_gsd_command(action: crate::GsdAction) -> Result<()> {
    match action {
        crate::GsdAction::State { json, dir } => state(json, dir),
    }
}

/// `statusline gsd state [--json] [--dir PROJECT_DIR]`.
fn state(json_output: bool, dir: Option<PathBuf>) -> Result<()> {
    // `--dir` names a PROJECT directory; its `.planning` subdirectory is what
    // gets read, matching `[gsd] project_dir`'s semantics. A `--dir` that is
    // not a directory at all is the ONE usage error that exits non-zero.
    let planning_dir = match dir {
        Some(project_dir) => {
            if !project_dir.is_dir() {
                eprintln!(
                    "error: --dir {} is not a directory",
                    sanitize_for_terminal(&project_dir.display().to_string())
                );
                std::process::exit(1);
            }
            Some(project_dir.join(".planning"))
        }
        None => None,
    };

    let summary = build_summary(planning_dir.as_deref());

    if json_output {
        print_json(&summary)?;
    } else {
        print_human(&summary);
    }
    Ok(())
}

/// The ONE document. One `json!` literal, one compact serialization, one
/// `println!` — nothing else reaches stdout in this mode.
fn print_json(summary: &GsdSummary) -> Result<()> {
    let document = json!({
        "schema_version": summary.schema_version,
        "source": {
            "planning_dir": path_field(&summary.planning_dir),
            "state_md": path_field(&summary.state_md),
            "roadmap_md": path_field(&summary.roadmap_md),
            "state_frontmatter_version": text_field(&summary.state_frontmatter_version),
        },
        "milestone": {
            "id": text_field(&summary.milestone_id),
            "name": text_field(&summary.milestone_name),
        },
        "phase": {
            "id": text_field(&summary.phase_id),
            "number": text_field(&summary.phase_number),
            "name": text_field(&summary.phase_name),
            "display": text_field(&summary.phase_display),
        },
        "progress": {
            "phases_completed": summary.phases_completed,
            "phases_total": summary.phases_total,
            "phases_percent": summary.phases_percent,
            "plans_completed": summary.plans_completed,
            "plans_total": summary.plans_total,
        },
        "warnings": summary
            .warnings
            .iter()
            .map(|w| sanitize_for_terminal(w))
            .collect::<Vec<String>>(),
    });
    println!("{}", serde_json::to_string(&document)?);
    Ok(())
}

/// The human report, in the `health` report's two-space-indented style.
fn print_human(summary: &GsdSummary) {
    println!("GSD Project State");
    println!("=================");
    println!();
    println!("Source:");
    println!(
        "  Planning dir: {}",
        or_unknown(path_text(&summary.planning_dir))
    );
    println!("  STATE.md: {}", or_unknown(path_text(&summary.state_md)));
    println!(
        "  ROADMAP.md: {}",
        or_unknown(path_text(&summary.roadmap_md))
    );
    println!(
        "  Frontmatter version: {}",
        or_unknown(text(&summary.state_frontmatter_version))
    );
    println!();
    println!("Milestone:");
    println!("  ID: {}", or_unknown(text(&summary.milestone_id)));
    println!("  Name: {}", or_unknown(text(&summary.milestone_name)));
    println!();
    println!("Phase:");
    println!("  Number: {}", or_unknown(text(&summary.phase_number)));
    println!("  Name: {}", or_unknown(text(&summary.phase_name)));
    println!("  Display: {}", or_unknown(text(&summary.phase_display)));
    println!();
    println!("Progress:");
    println!(
        "  Phases: {} (computed from ROADMAP.md)",
        fraction(summary.phases_completed, summary.phases_total)
    );
    println!(
        "  Percent: {}",
        summary
            .phases_percent
            .map(|p| format!("{}%", p))
            .unwrap_or_else(|| "(unknown)".to_string())
    );
    println!(
        "  Plans in this phase: {}",
        fraction(summary.plans_completed, summary.plans_total)
    );

    if !summary.warnings.is_empty() {
        println!();
        println!("Warnings:");
        for warning in &summary.warnings {
            // Per PIECE: the sanitizer strips newlines, so each warning is
            // sanitized on its own and the layout is assembled afterwards.
            println!("  - {}", sanitize_for_terminal(warning));
        }
    }
}

/// A sanitized path, or `None`.
fn path_text(path: &Option<PathBuf>) -> Option<String> {
    path.as_ref()
        .map(|p| sanitize_for_terminal(&p.display().to_string()))
}

/// A sanitized string, or `None`.
fn text(value: &Option<String>) -> Option<String> {
    value.as_ref().map(|v| sanitize_for_terminal(v))
}

/// A sanitized path as a JSON string-or-null.
fn path_field(path: &Option<PathBuf>) -> serde_json::Value {
    match path_text(path) {
        Some(s) => json!(s),
        None => serde_json::Value::Null,
    }
}

/// A sanitized string as a JSON string-or-null.
fn text_field(value: &Option<String>) -> serde_json::Value {
    match text(value) {
        Some(s) => json!(s),
        None => serde_json::Value::Null,
    }
}

fn or_unknown(value: Option<String>) -> String {
    value.unwrap_or_else(|| "(unknown)".to_string())
}

fn fraction(completed: Option<u32>, total: Option<u32>) -> String {
    match (completed, total) {
        (Some(c), Some(t)) => format!("{}/{}", c, t),
        _ => "(unknown)".to_string(),
    }
}
