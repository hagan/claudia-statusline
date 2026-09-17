//! `config` subcommand group handlers: `config generate` and `config path`.
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
