//! Preset layout definitions and user preset loading.

/// Built-in layout presets
pub const PRESET_DEFAULT: &str = "{directory}{sep}{git}{sep}{context}{sep}{model}{sep}{cost}";
pub const PRESET_COMPACT: &str = "{dir_short} {git_branch} {model} {cost_short}";
pub const PRESET_DETAILED: &str =
    "{directory}{sep}{git}\n{context}{sep}{model}{sep}{duration}{sep}{cost}";
pub const PRESET_MINIMAL: &str = "{directory} {model}";
pub const PRESET_POWER: &str =
    "{directory}{sep}{git}{sep}{context}\n{model}{sep}{duration}{sep}{lines}{sep}{cost} ({burn_rate})";

/// Get the format string for a preset name
///
/// Looks up presets in this order:
/// 1. User presets in ~/.config/claudia-statusline/presets/<name>.toml
/// 2. Built-in presets (default, compact, detailed, minimal, power)
pub fn get_preset_format(preset: &str) -> String {
    // Try user preset first
    if let Some(user_format) = load_user_preset(preset) {
        return user_format;
    }

    // Fall back to built-in presets
    match preset.to_lowercase().as_str() {
        "compact" => PRESET_COMPACT.to_string(),
        "detailed" => PRESET_DETAILED.to_string(),
        "minimal" => PRESET_MINIMAL.to_string(),
        "power" => PRESET_POWER.to_string(),
        _ => PRESET_DEFAULT.to_string(), // "default" or unknown
    }
}

/// Byte cap for a user preset file (T-12-17). A preset holds one format
/// string and a separator — a few hundred bytes — so 64 KiB is orders of
/// magnitude of headroom while still bounding a hostile or runaway file.
pub const MAX_USER_PRESET_BYTES: u64 = 64 * 1024;

/// Load a user-defined preset from the config directory
///
/// Two layers close T-12-17 (a config NAME reaching a FIFO or `/dev/zero`):
/// the lowercased name must be a bare file stem (no separator, `..`, `:` or
/// NUL), and the file is read through
/// [`crate::user_file::read_regular_file_capped`], which refuses a non-regular
/// node from metadata without opening it and bounds the read to
/// [`MAX_USER_PRESET_BYTES`]. There is deliberately no `.exists()` probe: it
/// follows symlinks and is true for a FIFO.
fn load_user_preset(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    if !crate::user_file::is_safe_file_stem(&lower) {
        return None;
    }
    let preset_dir = dirs::config_dir()?
        .join("claudia-statusline")
        .join("presets");
    let preset_path = preset_dir.join(format!("{lower}.toml"));

    let content =
        crate::user_file::read_regular_file_capped(&preset_path, MAX_USER_PRESET_BYTES).ok()?;

    // Parse TOML to extract format string
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct PresetFile {
        format: Option<String>,
        #[serde(default)]
        separator: String, // Reserved for future use
    }

    let parsed: PresetFile = toml::from_str(&content).ok()?;
    parsed.format
}

/// Is `name` a user preset that will actually LOAD?
///
/// This exists so `config validate` decides preset legality by the SAME
/// resolution [`get_preset_format`] performs. Membership in
/// [`list_available_presets`] is NOT that oracle and gives a wrong answer in
/// two directions:
///
/// * it enumerates every directory-entry FILE STEM, including non-`.toml`
///   files — a stray `notes.txt` becomes the "available preset" `notes`, which
///   `load_user_preset` can never load; and
/// * a `.toml` that parses but omits the `format` key also yields `None` from
///   `load_user_preset`, so the renderer falls back to [`PRESET_DEFAULT`]
///   silently even though the file exists and is listed.
///
/// Delegating to `load_user_preset` adds no new path-interpolation site of its
/// own: the path probed here is the one the render path already builds. That
/// site is NOT harmless by itself — the name comes from the config file — so
/// `load_user_preset` validates it as a bare stem and reads the file through
/// the bounded, metadata-first ladder (T-12-17).
pub fn user_preset_is_usable(name: &str) -> bool {
    load_user_preset(name).is_some()
}

/// List all available presets (built-in + user)
#[allow(dead_code)]
pub fn list_available_presets() -> Vec<String> {
    let mut presets = vec![
        "default".to_string(),
        "compact".to_string(),
        "detailed".to_string(),
        "minimal".to_string(),
        "power".to_string(),
    ];

    // Add user presets
    if let Some(preset_dir) =
        dirs::config_dir().map(|d| d.join("claudia-statusline").join("presets"))
    {
        if preset_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&preset_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().file_stem() {
                        if let Some(name_str) = name.to_str() {
                            let preset_name = name_str.to_lowercase();
                            if !presets.contains(&preset_name) {
                                presets.push(preset_name);
                            }
                        }
                    }
                }
            }
        }
    }

    presets
}
