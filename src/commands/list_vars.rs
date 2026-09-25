//! `--list-vars` handler: print the static render-variable catalog
//! (`crate::layout::RENDER_VARIABLES`), the effective layout, and the
//! provider-only gsd variables.

use std::env;
use std::fmt::Write as _;
use std::io::{self, Read, Write};

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

    // Read JSON from stdin (only the cwd is used, for the gsd section). Stdin is
    // optional context: a read error or non-UTF-8 bytes fall back to defaults
    // rather than failing the command.
    let mut raw = Vec::new();
    let _ = io::stdin().read_to_end(&mut raw);
    let buffer = String::from_utf8_lossy(&raw);

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

    // Output is buffered and written once at the end so a closed stdout
    // (`--list-vars | head`) is a clean exit rather than a `println!` panic,
    // which aborts under the release profile's `panic = "abort"`.
    let mut out = String::new();
    macro_rules! outln {
        ($($arg:tt)*) => {
            let _ = writeln!(out, $($arg)*);
        };
    }

    outln!("Template Variables");
    outln!("==================");
    if cli.no_color {
        outln!("(Colors disabled)");
    }
    outln!();

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

    outln!("=== effective layout ===");
    // Debug formatting: user-authored strings must not inject terminal escapes.
    outln!("  preset = {:?}", layout_config.preset);
    outln!("  template = {:?}", effective_template);
    if use_layout_system {
        outln!("  The variables below are substituted into this template.");
    } else {
        outln!(
            "  Template variables are not used: the built-in statusline renders because \
             [layout] format is empty and preset is \"default\". Set [layout] format or \
             another preset to use them."
        );
    }

    // --- Render variable catalog ---
    let mut current_group: Option<&str> = None;
    for row in crate::layout::RENDER_VARIABLES {
        if current_group != Some(row.group) {
            outln!();
            outln!("=== {} ===", row.group);
            current_group = Some(row.group);
        }
        let var = format!("{{{}}}", row.name);
        outln!(
            "  {:<30} e.g. {:<24} — {}",
            var,
            row.example,
            row.description
        );
    }

    // --- Provider-only gsd variables (last) ---
    outln!();
    outln!(
        "=== gsd (provider-only: not available in the statusline render; see `statusline gsd state --json`) ==="
    );
    let gsd = crate::gsd::GsdProvider::new(&full_config.gsd, std::path::Path::new(&current_dir));
    match gsd.collect() {
        Ok(vars) => {
            let vars: BTreeMap<String, String> = vars.into_iter().collect();
            if !gsd.is_available() {
                outln!("  (no GSD project detected for this directory, or [gsd] enabled = false)");
                for key in vars.keys() {
                    outln!("  {} = (empty)", key);
                }
            } else {
                for (key, value) in &vars {
                    if value.is_empty() {
                        outln!("  {} = (empty)", key);
                    } else {
                        // Debug: escapes control bytes from repo-controlled files (T-12-36).
                        outln!("  {} = {:?}", key, value);
                    }
                }
            }
        }
        Err(e) => {
            outln!("  (gsd provider error: {:?})", e.to_string());
        }
    }
    outln!();

    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(out.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => Ok(()),
        // The reader went away (e.g. `| head`): nothing left to deliver.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e.into()),
    }
}
