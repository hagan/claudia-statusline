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

/// `config path`: report the active config file and the full search order.
///
/// Always exits 0 — this is a reporting command, not a verdict (D-08/D-10).
fn path(json_output: bool) -> Result<()> {
    use crate::utils::sanitize_for_terminal;
    use serde_json::json;

    let resolved = config::resolve_config_source();

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

        let report = json!({
            "active": active,
            "active_source": active_source,
            "candidates": candidates,
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
    }

    Ok(())
}
