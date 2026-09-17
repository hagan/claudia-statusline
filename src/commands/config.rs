//! `config` subcommand group handlers: `config validate`, `config generate` and
//! `config path`.
//!
//! This handler is deliberately **THIN** — argv in, one library call, print out.
//! Every fact it reports is computed in `crate::config` (the diagnostic
//! `resolve_config_source()` search-order walk, `default_config_path()`,
//! `example_toml()`) and, from plan 12-09, in `crate::config_validation`;
//! no config semantics are decided here. This mirrors the handler/logic split
//! documented at `src/commands/ant.rs:44-46`.
//!
//! Two invariants this module keeps:
//!
//! 1. It calls the DIAGNOSTIC resolver `crate::config::resolve_config_source()`
//!    and nothing else: not the eager whole-config loader, not the
//!    process-global config accessor, and above all not the render path's
//!    private search-order walk (`src/config.rs:2004`). That walk returns on its
//!    FIRST hit by design, and a diagnostic command must not cost the render
//!    path that short-circuit (T-12-59) — nor would reporting only the winner be
//!    useful here. The grep in this plan's acceptance criteria is a real
//!    tripwire, so those three names are described rather than spelled.
//! 2. Every path echoed to stdout goes through
//!    `crate::utils::sanitize_for_terminal` (T-12-31). Config paths arrive from
//!    the environment and are untrusted.

use std::sync::OnceLock;

use crate::config;
use crate::error::Result;

/// Dispatch entry for `statusline config <action>`.
pub(crate) fn handle_config_command(action: crate::ConfigAction) -> Result<()> {
    match action {
        crate::ConfigAction::Generate => generate(),
        crate::ConfigAction::Path { json } => path(json),
        crate::ConfigAction::Validate {
            path: target,
            json,
            strict,
        } => validate(target, json, strict),
    }
}

/// Dispatch entry for the DEPRECATED top-level `statusline generate-config`.
///
/// D-05 keeps that spelling working verbatim so no script breaks; it is hidden
/// from `--help` and announces itself once on stderr. Identical behaviour
/// otherwise: the same [`generate`] body, the same file, the same permissions.
pub(crate) fn handle_deprecated_generate_config() -> Result<()> {
    warn_generate_config_deprecated();
    generate()
}

static GENERATE_CONFIG_DEPRECATION_WARNED: OnceLock<()> = OnceLock::new();

/// Emit the one-shot deprecation note for `statusline generate-config`.
///
/// `OnceLock` + `eprintln!`, the idiom of
/// `config::warn_json_backup_legacy_if_set` (`src/config.rs:2318`): it fires at
/// most once per process and never pollutes stdout, so a script that pipes
/// `generate-config`'s output keeps parsing exactly what it parsed before.
fn warn_generate_config_deprecated() {
    GENERATE_CONFIG_DEPRECATION_WARNED.get_or_init(|| {
        eprintln!(
            "note: `statusline generate-config` is deprecated; use `statusline config generate` instead"
        );
    });
}

/// `config generate`: write the example config to `default_config_path()`.
///
/// This body was MOVED verbatim from `src/main.rs`'s `Commands::GenerateConfig`
/// arm; it is not a rewrite. In particular the 0700 directory mode, the 0600
/// file mode, `create(true).truncate(true)` and the deliberate absence of an
/// overwrite prompt are pre-existing shipped behaviour (T-12-32 / T-12-35) and
/// are pinned numerically by `config_generate_writes_restrictive_permissions`.
fn generate() -> Result<()> {
    let config_path = config::Config::default_config_path()?;
    println!("Generating example config file at: {:?}", config_path);

    // Create parent directories with secure permissions (0o700 on Unix)
    if let Some(parent) = config_path.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(parent)?;
        }

        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(parent)?;
        }
    }

    // Write example config with secure permissions (0o600 on Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&config_path)?;
        std::io::Write::write_all(&mut file, config::Config::example_toml().as_bytes())?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&config_path, config::Config::example_toml())?;
    }
    println!("Config file generated successfully!");
    println!("Edit {} to customize settings", config_path.display());
    Ok(())
}

/// Human-readable label for a candidate's stable machine token.
///
/// The tokens themselves are the `--json` contract; these are what the human
/// report prints so the output reads as the SEARCH ORDER rather than as an
/// unordered list of paths.
fn candidate_label(source: &str) -> &'static str {
    match source {
        "env_statusline_config_path" => "STATUSLINE_CONFIG_PATH",
        "env_statusline_config" => "STATUSLINE_CONFIG",
        "xdg_config_dir" => "XDG config dir",
        "home_dotfile" => "~/.claudia-statusline.toml",
        _ => "unknown source",
    }
}

/// One config file found at a location the resolver NEVER consults (D-10).
///
/// `path` and `reason` are both already sanitized for terminal output; the
/// reason names where the file has to move to be read, because that specificity
/// is the entire point of the probe.
struct MisplacedConfig {
    path: String,
    reason: String,
}

/// What a single `symlink_metadata` probe could establish about a path.
///
/// `Unverifiable` exists because "the stat failed" and "the file is not there"
/// are different facts, and silently merging them is the failure mode this whole
/// report was written to end (T-12-58).
enum ProbeVerdict {
    /// The path is there (including as a dangling symlink).
    Present,
    /// The stat was refused — a config may or may not be sitting here.
    Unverifiable,
}

/// The D-10 misplacement probe.
///
/// These are NOT "losing candidates" — the resolver never looks at any of them,
/// so surfacing them requires deliberately probing a fixed list of known-wrong
/// locations. The list is bounded at five (T-12-33) and grounded in
/// `12-RESEARCH.md` §7.4, which reproduced two different `config.toml` files
/// living in two different directories on one developer machine, with one env
/// var deciding which was dead.
///
/// The general rule, which is what makes this robust rather than a hardcoded
/// list: warn for any probed path that EXISTS, is not the active file, and is
/// not one of the four search candidates.
///
/// Existence is tested with `std::fs::symlink_metadata`, never `Path::exists()`.
/// The two differ in a way that was measured on this platform rather than
/// assumed, because the plan's stated rationale turned out to be wrong:
///
/// | probed path | `exists()` | `symlink_metadata().is_ok()` |
/// |---|---|---|
/// | file under a `chmod 000` parent | false | **false** (`PermissionDenied`) |
/// | DANGLING SYMLINK | false | **true** |
/// | file with mode `000` | true | true |
///
/// So `symlink_metadata` does NOT by itself rescue a config inside a restrictive
/// directory — both predicates lose that one, because stat-ing a child requires
/// `+x` on the parent. What it does buy is the dangling-symlink row: a misplaced
/// `config.toml` symlinked at a file that has since moved is a broken config the
/// report must still name, and `exists()` follows the link and calls it absent.
///
/// T-12-58 (a misplaced config inside a restrictive directory going unreported)
/// is therefore mitigated explicitly instead: a `PermissionDenied` verdict is
/// REPORTED as unverifiable rather than collapsed to "absent", which is the only
/// honest answer a stat can give there.
///
/// Nothing here reads, opens or canonicalizes a file, and `symlink_metadata`
/// does not follow symlinks (T-12-33).
fn probe_misplaced_configs(resolved: &config::ResolvedConfig) -> Vec<MisplacedConfig> {
    use crate::utils::sanitize_for_terminal;
    use std::path::PathBuf;

    let home = dirs::home_dir();
    let app_config_dir = crate::common::get_config_dir();

    // Where a misplaced file has to move. Taken from the reported search order
    // itself, so the advice cannot drift from the candidates printed above it.
    let destination = resolved
        .candidates
        .iter()
        .find(|c| c.source == "xdg_config_dir")
        .and_then(|c| c.path.clone())
        .unwrap_or_else(|| app_config_dir.join("config.toml"));
    let destination = sanitize_for_terminal(&destination.display().to_string());

    let probes: Vec<(Option<PathBuf>, &str)> = vec![
        (
            home.as_ref()
                .map(|h| h.join(".config/claudia-statusline/config.toml")),
            "~/.config is only searched when XDG_CONFIG_HOME points at it — on macOS \
             the config dir is ~/Library/Application Support",
        ),
        (
            dirs::config_dir().map(|d| d.join("claudia-statusline/config.toml")),
            "the platform config directory is only searched when XDG_CONFIG_HOME is \
             unset or points here",
        ),
        (
            home.as_ref()
                .map(|h| h.join(".config/statusline/config.toml")),
            "wrong application directory name — it is `claudia-statusline`",
        ),
        (
            Some(app_config_dir.join("statusline.toml")),
            "wrong file name — the file in the config directory must be `config.toml`",
        ),
        (
            home.as_ref()
                .map(|h| h.join(".claudia-statusline/config.toml")),
            "the home dotfile candidate is the FILE `~/.claudia-statusline.toml`, \
             not a directory",
        ),
    ];

    let mut found: Vec<(PathBuf, String)> = Vec::new();
    for (candidate, why) in probes {
        let Some(probed) = candidate else {
            continue;
        };
        let verdict = match std::fs::symlink_metadata(&probed) {
            Ok(_) => ProbeVerdict::Present,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                ProbeVerdict::Unverifiable
            }
            Err(_) => continue,
        };
        if resolved.active.as_ref() == Some(&probed) {
            continue;
        }
        if resolved
            .candidates
            .iter()
            .any(|c| c.path.as_ref() == Some(&probed))
        {
            continue;
        }
        let reason = match verdict {
            ProbeVerdict::Present => {
                format!("{why}; statusline never reads it — move it to {destination}")
            }
            ProbeVerdict::Unverifiable => format!(
                "permission denied while checking this path, so statusline cannot tell \
                 whether a config is sitting here; {why}; anything found here must move \
                 to {destination}"
            ),
        };
        found.push((probed, reason));
    }

    // Sorted and de-duplicated so repeated runs are byte-identical, and so the
    // two overlapping directory probes (identical wherever XDG_CONFIG_HOME is
    // `~/.config`) report one file once.
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.0 == b.0);

    found
        .into_iter()
        .map(|(probed, reason)| MisplacedConfig {
            path: sanitize_for_terminal(&probed.display().to_string()),
            reason: sanitize_for_terminal(&reason),
        })
        .collect()
}

/// `config path`: report the active config file, the full search order, and any
/// config file sitting at a location the resolver never consults.
///
/// Always exits 0 — this is a reporting command, not a verdict (D-08/D-10).
fn path(json_output: bool) -> Result<()> {
    use crate::utils::sanitize_for_terminal;
    use serde_json::json;

    let resolved = config::resolve_config_source();
    let misplaced = probe_misplaced_configs(&resolved);

    let active = resolved
        .active
        .as_ref()
        .map(|p| sanitize_for_terminal(&p.display().to_string()));
    let active_source = resolved.active_source;

    if json_output {
        let candidates: Vec<serde_json::Value> = resolved
            .candidates
            .iter()
            .map(|c| {
                json!({
                    "source": c.source,
                    "path": c.path.as_ref()
                        .map(|p| sanitize_for_terminal(&p.display().to_string())),
                    "exists": c.exists,
                })
            })
            .collect();

        let misplaced: Vec<serde_json::Value> = misplaced
            .iter()
            .map(|m| json!({ "path": m.path, "reason": m.reason }))
            .collect();

        let report = json!({
            "active": active,
            "active_source": active_source,
            "candidates": candidates,
            "misplaced": misplaced,
        });
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!("Claudia Statusline Config Path");
        println!("==============================");
        println!();
        println!("Active config:");
        match (&active, active_source) {
            (Some(p), Some(src)) => println!("  {} (source: {})", p, src),
            // `active` and `active_source` are set together by the resolver.
            _ => println!("  none found — using defaults"),
        }
        println!();
        println!("Search order:");
        for candidate in &resolved.candidates {
            let rendered = match candidate.path.as_ref() {
                Some(p) => sanitize_for_terminal(&p.display().to_string()),
                None => String::from("(not set)"),
            };
            println!(
                "  {} {}: {}",
                if candidate.exists { "✅" } else { "❌" },
                candidate_label(candidate.source),
                rendered
            );
        }
        println!();
        println!("Misplaced config files:");
        if misplaced.is_empty() {
            println!("  ✅ none found");
        } else {
            for m in &misplaced {
                println!("  ⚠️  {}", m.path);
                println!("      {}", m.reason);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `config validate` (QUAL-01 / QUAL-02 / SC1)
// ---------------------------------------------------------------------------

/// Hard cap on the bytes `config validate` will read from its target file.
///
/// 1 MiB, the same number `src/pricing/cache.rs` caps a price cache at and the
/// same number `src/config_validation.rs` caps a cache probe at. Over-cap is a
/// REJECTION, never a truncate-and-parse: reading exactly the cap from an
/// oversized file can yield a complete, valid TOML document followed by
/// padding, which would let an arbitrarily large file through the supposed
/// bound. That is the discipline `src/pricing/cache.rs:348-353` established.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// What the target-file ladder produced: the text, or a value-free reason.
type TargetRead = std::result::Result<String, String>;

/// Read the TARGET config file, metadata first.
///
/// Every failure is a `String` the caller turns into ONE
/// `FindingKind::ConfigIo` finding — never a panic, never a `?`-propagated
/// error, because the report still has to render and the exit code still has to
/// follow the SC1 contract.
///
/// The ladder mirrors `crate::config_validation::classify_cache`, including the
/// two steps whose obvious shape is wrong:
///
/// * **`symlink_metadata` first, then follow.** A SYMLINK is resolved with
///   `std::fs::metadata` and judged by its TARGET, because the consumer —
///   the whole-file loader at `src/config.rs:1920` — uses
///   `std::fs::read_to_string`, which follows links. A symlinked config is the
///   normal dotfiles arrangement and works today; rejecting it as "not a regular
///   file" would be a false failure on a working setup. A dangling link is
///   reported as absent. Both calls are stats, and a stat never blocks.
/// * **A non-regular target RETURNS WITHOUT OPENING.** This is mandatory, not
///   stylistic: opening a writer-less FIFO blocks indefinitely before a single
///   byte is read (verified live, `timeout 3` -> exit 124), so a byte cap does
///   not bound it. `/dev/zero` is caught here too, as a character device.
fn read_target_config(p: &std::path::Path) -> TargetRead {
    use crate::config_validation::{file_type_label, io_kind_label};
    use std::io::ErrorKind;

    let link_md = match std::fs::symlink_metadata(p) {
        Ok(md) => md,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err("no file exists at this path".to_string())
        }
        Err(e) => {
            return Err(format!(
                "the file could not be examined ({})",
                io_kind_label(e.kind())
            ))
        }
    };

    let md = if link_md.file_type().is_symlink() {
        match std::fs::metadata(p) {
            Ok(md) => md,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err("dangling symlink — it points at nothing".to_string())
            }
            Err(e) => {
                return Err(format!(
                    "the symlink target could not be examined ({})",
                    io_kind_label(e.kind())
                ))
            }
        }
    } else {
        link_md
    };

    let ft = md.file_type();
    if !ft.is_file() {
        return Err(format!(
            "not a regular file ({}) — a config file must be a regular file",
            file_type_label(&ft)
        ));
    }
    if md.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "larger than the {MAX_CONFIG_BYTES} byte config size cap"
        ));
    }

    let mut buf = String::new();
    let read = std::fs::File::open(p).and_then(|f| {
        use std::io::Read;
        f.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut buf)
    });
    if let Err(e) = read {
        return Err(format!(
            "the file could not be read ({})",
            io_kind_label(e.kind())
        ));
    }
    if buf.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!(
            "larger than the {MAX_CONFIG_BYTES} byte config size cap"
        ));
    }
    Ok(buf)
}

/// Build the `Config` value the cache classifier needs, from the sections that
/// PARSED.
///
/// Deliberately NOT the eager whole-config loader and NOT the process-global
/// config accessor (the two names this module's acceptance grep forbids, and
/// which the module doc above describes rather than spells). The first is
/// all-or-nothing and would collapse a multi-defect report into one opaque
/// error; the second additionally applies the `CLAUDE_*` environment overrides
/// at `src/config.rs:1403-1445`, so a `CLAUDE_THEME`-set shell would mask an
/// invalid `display.theme` in the file under validation (T-12-41).
///
/// Only `[pricing]` and `[ant]` are needed — they carry the three staleness
/// thresholds D-12 puts in this command's domain. Each is deserialized
/// leniently and INDEPENDENTLY from the same `toml::Value`, so a broken
/// `[display]` cannot stop the cache checks from running.
fn cache_config_from_text(text: Option<&str>) -> config::Config {
    use serde::Deserialize;

    let mut cfg = config::Config::default();
    let Some(text) = text else {
        return cfg;
    };
    let Ok(document) = toml::from_str::<toml::Value>(text) else {
        return cfg;
    };
    if let Some(section) = document.get("pricing") {
        if let Ok(parsed) = crate::pricing::PricingConfig::deserialize(section.clone()) {
            cfg.pricing = parsed;
        }
    }
    if let Some(section) = document.get("ant") {
        if let Ok(parsed) = crate::ant::config::AntConfig::deserialize(section.clone()) {
            cfg.ant = parsed;
        }
    }
    cfg
}

/// Everything `validate` computes before it renders anything.
///
/// Computing first and branching second is the shipped diagnostic idiom
/// (`src/commands/health.rs:79`), and here it is also what makes the two output
/// modes provably agree: one report, two renderers.
struct ValidationOutcome {
    report: crate::config_validation::Report,
    caches: Vec<crate::config_validation::CacheStatus>,
    target_path: Option<std::path::PathBuf>,
    target_source: Option<String>,
    active_path: Option<std::path::PathBuf>,
    active_source: Option<&'static str>,
}

/// `config validate`: resolve the target, read it, drive the engine, report.
///
/// Target resolution (D-06): with no positional `PATH` this validates the config
/// that would ACTUALLY be loaded, via the DIAGNOSTIC resolver. A positional
/// `PATH` WINS — it is the more specific request, and it validates that file
/// without pretending the file is active. When the two differ the report carries
/// a STRUCTURED notice naming both, never a bare line: in `--json` mode a stray
/// line before the document would break the single-JSON-document contract, and a
/// notice printed only in human mode would make the two modes disagree.
///
/// `--config <PATH>` reaches the same place by a different route (it sets
/// `STATUSLINE_CONFIG_PATH`, candidate 1 of the search order), so both spellings
/// work; only the notice distinguishes them.
fn validate(target: Option<std::path::PathBuf>, json_output: bool, strict: bool) -> Result<()> {
    let outcome = run_validation(target);
    render_validation(&outcome, json_output, strict)
}

/// The whole computation, with no IO to stdout. Split out so both renderers —
/// and any future one — consume one identical report.
fn run_validation(target: Option<std::path::PathBuf>) -> ValidationOutcome {
    use crate::config_validation::{
        classify_all_caches, report_cache_findings, validate_config_text, FindingKind, Report,
    };
    use crate::utils::sanitize_for_terminal;

    let mut report = Report::new();
    let resolved = config::resolve_config_source();

    let positional = target.is_some();
    let target_path = target.clone().or_else(|| resolved.active.clone());
    let target_source = if positional {
        Some("positional".to_string())
    } else {
        resolved.active_source.map(|s| s.to_string())
    };

    // D-06: name BOTH files when the positional request is not the active one.
    if let Some(ref chosen) = target {
        if resolved.active.as_ref() != Some(chosen) {
            let chosen_text = sanitize_for_terminal(&chosen.display().to_string());
            let message = match resolved.active.as_ref() {
                Some(active) => format!(
                    "validating {} because it was given as an argument; the config statusline \
                     would actually load is {}",
                    chosen_text,
                    sanitize_for_terminal(&active.display().to_string())
                ),
                None => format!(
                    "validating {} because it was given as an argument; no config file is \
                     active, so statusline is running on built-in defaults",
                    chosen_text
                ),
            };
            report.notice("positional_target_differs_from_active", message);
        }
    }

    // D-10: a config sitting where the resolver never looks. Run only when this
    // command is reporting on the ACTIVE config — for an explicit positional
    // target the user is vetting one file, and the state of the environment
    // around it is noise. The probe already excludes the active file and all
    // four search candidates, so a hit here is always a file statusline cannot
    // read. WARNINGS, so they cannot fail a validation on their own (D-11).
    if !positional {
        for misplaced in probe_misplaced_configs(&resolved) {
            report.warn(
                FindingKind::MisplacedConfig,
                MISPLACED_KEY,
                format!("{} — {}", misplaced.path, misplaced.reason),
            );
        }
    }

    let mut text: Option<String> = None;
    match target_path.as_ref() {
        // D-10: running on defaults is legitimate and common — a quiet PASS.
        None => report.notice(
            "no_config_found",
            "no config file was found at any searched location; statusline is running on its \
             built-in defaults",
        ),
        Some(path) => match read_target_config(path) {
            Ok(body) => text = Some(body),
            // Every one of the four IO failure modes is a FINDING of the
            // dedicated `config_io` kind. `syntax_error` is NOT borrowed for
            // them: it means "the bytes were read but are not TOML".
            Err(detail) => report.error(FindingKind::ConfigIo, CONFIG_IO_KEY, detail),
        },
    }

    if let Some(ref body) = text {
        validate_config_text(body, &mut report);
    }

    // SC3: the caches this config governs. Driven off the sections that parsed,
    // so a broken `[display]` cannot suppress them.
    let cfg = cache_config_from_text(text.as_deref());
    let caches = classify_all_caches(&cfg);
    report_cache_findings(&caches, &mut report);

    ValidationOutcome {
        report,
        caches,
        target_path,
        target_source,
        active_path: resolved.active.clone(),
        active_source: resolved.active_source,
    }
}

/// Finding key for a target-file IO failure. There is no config KEY to blame —
/// the file itself is the subject — so the pseudo-key matches the one
/// `validate_config_text` already uses for a whole-document syntax error.
const CONFIG_IO_KEY: &str = "<file>";

/// Finding key for a D-10 misplacement. Deliberately NOT the misplaced path: a
/// finding `key` is a dotted CONFIG path, and a filesystem path arriving from
/// the environment does not belong in that field. The path lives in the message,
/// where it is sanitized with everything else.
const MISPLACED_KEY: &str = "<misplaced-config>";

/// Render the outcome and decide the exit code.
///
/// Plan 12-09 Task 2 replaces this with the deterministic dual-mode renderer
/// (sorted findings, the full `--json` schema, the cache section). This first
/// form exists so the engine drive of Task 1 is observable and committable.
fn render_validation(outcome: &ValidationOutcome, json_output: bool, strict: bool) -> Result<()> {
    use crate::utils::sanitize_for_terminal;

    let report = &outcome.report;
    let errors = report.error_count();
    let warnings = report.warning_count();

    if json_output {
        let doc = serde_json::json!({
            "valid": errors == 0,
            "counts": { "errors": errors, "warnings": warnings },
        });
        println!("{}", serde_json::to_string(&doc)?);
    } else {
        match (outcome.target_path.as_ref(), outcome.target_source.as_deref()) {
            (Some(p), Some(src)) => println!(
                "Target: {} (source: {})",
                sanitize_for_terminal(&p.display().to_string()),
                sanitize_for_terminal(src)
            ),
            _ => println!("Target: none — validating built-in defaults"),
        }
        if outcome.active_path.as_ref() != outcome.target_path.as_ref() {
            match (outcome.active_path.as_ref(), outcome.active_source) {
                (Some(p), Some(src)) => println!(
                    "Active: {} (source: {})",
                    sanitize_for_terminal(&p.display().to_string()),
                    sanitize_for_terminal(src)
                ),
                _ => println!("Active: none found — statusline is using built-in defaults"),
            }
        }
        for status in &outcome.caches {
            println!(
                "cache {}: {}",
                sanitize_for_terminal(status.name),
                sanitize_for_terminal(status.state.as_str())
            );
        }
        for notice in &report.notices {
            println!(
                "note [{}]: {}",
                notice.code,
                sanitize_for_terminal(&notice.message)
            );
        }
        for finding in &report.findings {
            println!(
                "{} {}: {}",
                finding.severity.as_str(),
                sanitize_for_terminal(&finding.key),
                sanitize_for_terminal(&finding.message)
            );
        }
        println!(
            "{} — {} error(s), {} warning(s)",
            if errors == 0 { "PASS" } else { "FAIL" },
            errors,
            warnings
        );
    }

    if errors > 0 || (strict && warnings > 0) {
        std::process::exit(1);
    }
    Ok(())
}
