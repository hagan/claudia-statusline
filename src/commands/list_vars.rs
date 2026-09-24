//! `--list-vars` handler: print the static render-variable catalog
//! (`crate::layout::RENDER_VARIABLES`), the effective layout, and the
//! provider-only gsd variables.

use std::env;
use std::io::{self, Read};

use crate::error::Result;
use crate::models::StatuslineInput;
use crate::Cli;

/// Handle the `--list-vars` CLI flag.
///
/// Prints, in order: the effective layout (and whether template variables are
/// used by the current config), every render variable from the static catalog
/// `crate::layout::RENDER_VARIABLES` grouped with an example and description,
/// and finally the provider-only `gsd_*` variables in a section labeled as not
/// available in the statusline render (D-05 / D-07 / D-08).
///
/// Never fails on bad input: malformed stdin falls back to defaults, and gsd
/// provider problems are reported inline.
pub(crate) fn handle_list_vars(cli: &Cli) -> Result<()> {
    use crate::provider::DataProvider;
    use std::collections::BTreeMap;

    // Read JSON from stdin (only the cwd is used, for the gsd section)
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let input: StatuslineInput = serde_json::from_str(&buffer).unwrap_or_default();

    let current_dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_ref())
        .cloned()
        .unwrap_or_else(|| {
            env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "~".to_string())
        });

    let full_config = crate::config::get_config();

    println!("Template Variables");
    println!("==================");
    if cli.no_color {
        println!("(Colors disabled)");
    }
    println!();

    // --- Effective layout ---
    // Deliberately mirrors src/display.rs (effective template at ~:700-704,
    // `use_layout_system` at ~:1197-1198). This is a known duplication
    // (R3-WR-06): if display.rs changes how it picks the template or decides to
    // use the layout system, this must change with it.
    let layout_config = &full_config.layout;
    let effective_template = if layout_config.format.is_empty() {
        crate::layout::get_preset_format(&layout_config.preset).to_string()
    } else {
        layout_config.format.clone()
    };
    let use_layout_system = !full_config.layout.format.is_empty()
        || full_config.layout.preset.to_lowercase() != "default";

    println!("=== effective layout ===");
    // Debug formatting: user-authored strings must not inject terminal escapes.
    println!("  preset = {:?}", layout_config.preset);
    println!("  template = {:?}", effective_template);
    if use_layout_system {
        println!("  The variables below are substituted into this template.");
    } else {
        println!(
            "  Template variables are not used: the built-in statusline renders because \
             [layout] format is empty and preset is \"default\". Set [layout] format or \
             another preset to use them."
        );
    }

    // --- Render variable catalog ---
    let mut current_group: Option<&str> = None;
    for row in crate::layout::RENDER_VARIABLES {
        if current_group != Some(row.group) {
            println!();
            println!("=== {} ===", row.group);
            current_group = Some(row.group);
        }
        let var = format!("{{{}}}", row.name);
        println!(
            "  {:<30} e.g. {:<24} — {}",
            var, row.example, row.description
        );
    }

    // --- Provider-only gsd variables (last) ---
    println!();
    println!(
        "=== gsd (provider-only: not available in the statusline render; see `statusline gsd state --json`) ==="
    );
    let gsd = crate::gsd::GsdProvider::new(&full_config.gsd, std::path::Path::new(&current_dir));
    match gsd.collect() {
        Ok(vars) => {
            let vars: BTreeMap<String, String> = vars.into_iter().collect();
            if !gsd.is_available() {
                println!(
                    "  (no GSD project detected for this directory, or [gsd] enabled = false)"
                );
                for key in vars.keys() {
                    println!("  {} = (empty)", key);
                }
            } else {
                for (key, value) in &vars {
                    if value.is_empty() {
                        println!("  {} = (empty)", key);
                    } else {
                        // Debug: escapes control bytes from repo-controlled files (T-12-36).
                        println!("  {} = {:?}", key, value);
                    }
                }
            }
        }
        Err(e) => {
            println!("  (gsd provider error: {:?})", e.to_string());
        }
    }
    println!();

    Ok(())
}
