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

/// Load a user-defined preset from the config directory
fn load_user_preset(name: &str) -> Option<String> {
    let preset_dir = dirs::config_dir()?
        .join("claudia-statusline")
        .join("presets");
    let preset_path = preset_dir.join(format!("{}.toml", name.to_lowercase()));

    if !preset_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&preset_path).ok()?;

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
