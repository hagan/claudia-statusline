//! Theme system for customizable statusline colors.
//!
//! This module provides a comprehensive theme system that allows users to customize
//! all statusline colors through TOML configuration files. Themes can be:
//!
//! - Built-in (embedded in the binary)
//! - User-defined (in ~/.config/claudia-statusline/themes/)
//! - Shared as simple TOML files
//!
//! # Architecture
//!
//! ```text
//! Theme Discovery Order:
//!   1. CLI flag: --theme gruvbox
//!   2. Config file: theme = "gruvbox"
//!   3. Environment: CLAUDE_THEME=gruvbox
//!   4. User themes: ~/.config/claudia-statusline/themes/*.toml
//!   5. Embedded themes: dark.toml / light.toml
//!   6. Fallback: Theme::default()
//! ```

use serde::{de::Error as _, Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

// Embedded theme files compiled into binary
const EMBEDDED_DARK_THEME: &str = include_str!("../themes/dark.toml");
const EMBEDDED_LIGHT_THEME: &str = include_str!("../themes/light.toml");

/// Main theme structure containing all color definitions.
///
/// Themes are loaded from TOML files and cached for performance.
/// All color fields are required, with sensible defaults provided.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    /// Theme name (e.g., "dark", "gruvbox")
    pub name: String,

    /// Optional description
    #[serde(default)]
    pub description: Option<String>,

    /// All color definitions
    pub colors: ThemeColors,

    /// Optional custom color palette
    #[serde(default)]
    pub palette: Option<Palette>,
}

/// Complete set of theme colors for all statusline components.
///
/// Each field corresponds to a specific UI element or state.
/// All fields have defaults to ensure themes are always usable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    // ===== Component Colors =====
    /// Directory path color
    #[serde(default = "default_cyan")]
    pub directory: String,

    /// Git branch name color
    #[serde(default = "default_green")]
    pub git_branch: String,

    /// Model name color (e.g., "S4.5")
    #[serde(default = "default_cyan")]
    pub model: String,

    /// Session duration color
    #[serde(default = "default_light_gray")]
    pub duration: String,

    /// Separator bullet (•) color
    #[serde(default = "default_light_gray")]
    pub separator: String,

    // ===== State-Based Colors =====
    /// Lines added (+123) color
    #[serde(default = "default_green")]
    pub lines_added: String,

    /// Lines removed (-45) color
    #[serde(default = "default_red")]
    pub lines_removed: String,

    // ===== Cost Threshold Colors =====
    /// Cost < $5 color
    #[serde(default = "default_green")]
    pub cost_low: String,

    /// Cost $5-$20 color
    #[serde(default = "default_yellow")]
    pub cost_medium: String,

    /// Cost >= $20 color
    #[serde(default = "default_red")]
    pub cost_high: String,

    // ===== Context Usage Threshold Colors =====
    /// Context < 50% color
    #[serde(default = "default_white")]
    pub context_normal: String,

    /// Context 50-70% color
    #[serde(default = "default_yellow")]
    pub context_caution: String,

    /// Context 70-90% color
    #[serde(default = "default_orange")]
    pub context_warning: String,

    /// Context >= 90% color
    #[serde(default = "default_red")]
    pub context_critical: String,
}

/// Optional custom color palette for advanced theme customization.
///
/// Allows users to define custom ANSI escape codes or use extended color spaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palette {
    /// Custom color definitions (name -> ANSI code)
    #[serde(default)]
    pub custom: HashMap<String, String>,
}

// ===== Default Color Values =====

fn default_cyan() -> String {
    "cyan".to_string()
}

fn default_green() -> String {
    "green".to_string()
}

fn default_red() -> String {
    "red".to_string()
}

fn default_yellow() -> String {
    "yellow".to_string()
}

fn default_orange() -> String {
    "orange".to_string()
}

fn default_white() -> String {
    "white".to_string()
}

fn default_light_gray() -> String {
    "light_gray".to_string()
}

// ===== Theme Manager =====

/// Theme manager for discovering and loading themes.
///
/// Handles theme discovery with caching for performance.
/// Searches for themes in this order:
/// 1. User themes: ~/.config/claudia-statusline/themes/
/// 2. Embedded themes: dark.toml, light.toml
pub struct ThemeManager {
    /// Path to user themes directory
    themes_dir: PathBuf,
    /// Cached theme (loaded once per process)
    cache: Arc<Mutex<Option<Theme>>>,
}

impl ThemeManager {
    /// Creates a new ThemeManager.
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::ThemeManager;
    ///
    /// let manager = ThemeManager::new();
    /// let theme = manager.load_theme("dark").unwrap();
    /// ```
    pub fn new() -> Self {
        let themes_dir = crate::common::get_config_dir().join("themes");

        Self {
            themes_dir,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Loads a theme by name with precedence chain.
    ///
    /// Discovery order:
    /// 1. User themes: ~/.config/claudia-statusline/themes/{name}.toml
    /// 2. Embedded themes: dark, light
    /// 3. Fallback: Theme::default()
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::ThemeManager;
    ///
    /// let manager = ThemeManager::new();
    /// let theme = manager.load_theme("dark").unwrap();
    /// assert_eq!(theme.name, "dark");
    /// ```
    pub fn load_theme(&self, name: &str) -> Result<Theme, String> {
        // Try user themes first
        if let Ok(theme) = self.load_from_file(name) {
            return Ok(theme);
        }

        // Try embedded themes
        if let Ok(theme) = Theme::load_embedded(name) {
            return Ok(theme);
        }

        // Fallback to default
        Err(format!(
            "Theme '{}' not found. Available embedded themes: {}",
            name,
            Theme::embedded_themes().join(", ")
        ))
    }

    /// Loads a theme from user themes directory.
    ///
    /// Searches for `~/.config/claudia-statusline/themes/{name}.toml`
    fn load_from_file(&self, name: &str) -> Result<Theme, String> {
        let theme_path = self.themes_dir.join(format!("{}.toml", name));

        if !theme_path.exists() {
            return Err(format!("Theme file not found: {}", theme_path.display()));
        }

        let content = fs::read_to_string(&theme_path)
            .map_err(|e| format!("Failed to read theme file: {}", e))?;

        Theme::from_toml(&content).map_err(|e| format!("Failed to parse theme '{}': {}", name, e))
    }

    /// Lists all available themes (user + embedded).
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::ThemeManager;
    ///
    /// let manager = ThemeManager::new();
    /// let themes = manager.list_themes();
    /// assert!(themes.contains(&"dark".to_string()));
    /// ```
    pub fn list_themes(&self) -> Vec<String> {
        let mut themes = Vec::new();

        // Add embedded themes
        for name in Theme::embedded_themes() {
            themes.push(name.to_string());
        }

        // Add user themes if directory exists
        if self.themes_dir.exists() {
            if let Ok(entries) = fs::read_dir(&self.themes_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().file_stem() {
                        if let Some(name_str) = name.to_str() {
                            if entry.path().extension().and_then(|s| s.to_str()) == Some("toml") {
                                themes.push(name_str.to_string());
                            }
                        }
                    }
                }
            }
        }

        themes.sort();
        themes.dedup();
        themes
    }

    /// Gets or loads a cached theme.
    ///
    /// Caches the theme for the lifetime of the process to avoid repeated file I/O.
    pub fn get_or_load(&self, name: &str) -> Result<Theme, String> {
        let mut cache = self.cache.lock().unwrap();

        if let Some(ref theme) = *cache {
            if theme.name.eq_ignore_ascii_case(name) {
                return Ok(theme.clone());
            }
        }

        // Not cached or different theme requested
        let theme = self.load_theme(name)?;
        *cache = Some(theme.clone());
        Ok(theme)
    }
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global theme manager instance.
static THEME_MANAGER: OnceLock<ThemeManager> = OnceLock::new();

/// Gets the global theme manager instance.
pub fn get_theme_manager() -> &'static ThemeManager {
    THEME_MANAGER.get_or_init(ThemeManager::new)
}

// ===== Theme Implementation =====

impl Theme {
    /// Creates a theme from TOML content.
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::Theme;
    ///
    /// let toml = r#"
    ///     name = "custom"
    ///     [colors]
    ///     directory = "bright_blue"
    /// "#;
    ///
    /// let theme = Theme::from_toml(toml).unwrap();
    /// assert_eq!(theme.name, "custom");
    /// ```
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Loads an embedded theme by name.
    ///
    /// Built-in themes are compiled into the binary for zero-overhead loading.
    ///
    /// # Available Themes
    ///
    /// - `"dark"` - Default dark theme
    /// - `"light"` - Light theme for bright terminals
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::Theme;
    ///
    /// let theme = Theme::load_embedded("dark").unwrap();
    /// assert_eq!(theme.name, "dark");
    ///
    /// let theme = Theme::load_embedded("light").unwrap();
    /// assert_eq!(theme.name, "light");
    /// ```
    pub fn load_embedded(name: &str) -> Result<Self, toml::de::Error> {
        let content = match name.to_lowercase().as_str() {
            "dark" => EMBEDDED_DARK_THEME,
            "light" => EMBEDDED_LIGHT_THEME,
            _ => {
                return Err(toml::de::Error::custom(format!(
                    "Unknown embedded theme '{}'. Available: dark, light",
                    name
                )));
            }
        };

        Self::from_toml(content)
    }

    /// Lists all available embedded theme names.
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::Theme;
    ///
    /// let themes = Theme::embedded_themes();
    /// assert!(themes.contains(&"dark"));
    /// assert!(themes.contains(&"light"));
    /// ```
    pub fn embedded_themes() -> Vec<&'static str> {
        vec!["dark", "light"]
    }

    /// Resolves a color name to its ANSI escape code.
    ///
    /// Supports:
    /// - Named colors: "cyan", "green", "red", etc.
    /// - Direct ANSI codes: "\x1b[36m"
    /// - Custom palette colors
    ///
    /// # Examples
    ///
    /// ```
    /// use statusline::theme::Theme;
    ///
    /// let theme = Theme::default();
    /// let cyan = theme.resolve_color("cyan");
    /// assert_eq!(cyan, "\x1b[36m");
    /// ```
    pub fn resolve_color(&self, name: &str) -> String {
        // Check if it's already an ANSI code (single backslash from Rust code)
        if name.starts_with("\x1b[") {
            return name.to_string();
        }

        // Check if it's an escaped ANSI code from TOML (double backslash becomes \\x1b)
        if name.starts_with("\\x1b[") {
            // Convert TOML escape sequence to actual ANSI code
            return name.replace("\\x1b", "\x1b");
        }

        // Check custom palette first
        if let Some(palette) = &self.palette {
            if let Some(custom_color) = palette.custom.get(name) {
                // Handle escaped ANSI codes in palette
                if custom_color.starts_with("\\x1b[") {
                    return custom_color.replace("\\x1b", "\x1b");
                }
                return custom_color.clone();
            }
        }

        // Resolve named colors to ANSI codes
        match name {
            // Basic 16 ANSI colors
            "black" => "\x1b[30m".to_string(),
            "red" => "\x1b[31m".to_string(),
            "green" => "\x1b[32m".to_string(),
            "yellow" => "\x1b[33m".to_string(),
            "blue" => "\x1b[34m".to_string(),
            "magenta" => "\x1b[35m".to_string(),
            "cyan" => "\x1b[36m".to_string(),
            "white" => "\x1b[37m".to_string(),
            "gray" => "\x1b[90m".to_string(),

            // Bright variants
            "bright_red" => "\x1b[91m".to_string(),
            "bright_green" => "\x1b[92m".to_string(),
            "bright_yellow" => "\x1b[93m".to_string(),
            "bright_blue" => "\x1b[94m".to_string(),
            "bright_magenta" => "\x1b[95m".to_string(),
            "bright_cyan" => "\x1b[96m".to_string(),
            "bright_white" => "\x1b[97m".to_string(),

            // Aliases
            "light_gray" => "\x1b[38;5;245m".to_string(),
            "orange" => "\x1b[38;5;208m".to_string(),

            // Unknown color - default to white
            _ => {
                log::warn!("Unknown color name '{}', using white", name);
                "\x1b[37m".to_string()
            }
        }
    }
}

impl Default for Theme {
    /// Creates the default dark theme.
    ///
    /// This theme matches the current hardcoded color scheme for backward compatibility.
    fn default() -> Self {
        Theme {
            name: "dark".to_string(),
            description: Some("Default dark theme for dark terminals".to_string()),
            colors: ThemeColors {
                directory: "cyan".to_string(),
                git_branch: "green".to_string(),
                model: "cyan".to_string(),
                duration: "light_gray".to_string(),
                separator: "light_gray".to_string(),
                lines_added: "green".to_string(),
                lines_removed: "red".to_string(),
                cost_low: "green".to_string(),
                cost_medium: "yellow".to_string(),
                cost_high: "red".to_string(),
                context_normal: "white".to_string(),
                context_caution: "yellow".to_string(),
                context_warning: "orange".to_string(),
                context_critical: "red".to_string(),
            },
            palette: None,
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(desc) = &self.description {
            write!(f, " - {}", desc)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== ThemeManager Tests =====

    #[test]
    fn test_theme_manager_new() {
        let manager = ThemeManager::new();

        // Should be in config directory (platform-agnostic check)
        let config_dir = crate::common::get_config_dir();
        let expected_themes_dir = config_dir.join("themes");
        assert_eq!(
            manager.themes_dir, expected_themes_dir,
            "Themes directory should be in config directory"
        );

        // Should end with claudia-statusline/themes (platform-agnostic)
        assert!(manager.themes_dir.ends_with("claudia-statusline/themes"));
    }

    #[test]
    fn test_theme_manager_load_embedded() {
        let manager = ThemeManager::new();
        let theme = manager.load_theme("dark").unwrap();
        assert_eq!(theme.name, "dark");

        let theme = manager.load_theme("light").unwrap();
        assert_eq!(theme.name, "light");
    }

    #[test]
    fn test_theme_manager_load_unknown() {
        let manager = ThemeManager::new();
        let result = manager.load_theme("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_theme_manager_list_themes() {
        let manager = ThemeManager::new();
        let themes = manager.list_themes();
        assert!(themes.contains(&"dark".to_string()));
        assert!(themes.contains(&"light".to_string()));
        assert_eq!(themes.len(), 2); // Only embedded themes in test env
    }

    #[test]
    fn test_theme_manager_cache() {
        let manager = ThemeManager::new();

        // Load theme first time
        let theme1 = manager.get_or_load("dark").unwrap();
        assert_eq!(theme1.name, "dark");

        // Load same theme - should use cache
        let theme2 = manager.get_or_load("dark").unwrap();
        assert_eq!(theme2.name, "dark");

        // Load different theme - should reload
        let theme3 = manager.get_or_load("light").unwrap();
        assert_eq!(theme3.name, "light");
    }

    #[test]
    fn test_get_theme_manager() {
        let manager1 = get_theme_manager();
        let manager2 = get_theme_manager();
        // Should be the same instance
        assert!(std::ptr::eq(manager1, manager2));
    }

    // ===== Theme Tests =====

    #[test]
    fn test_default_theme() {
        let theme = Theme::default();
        assert_eq!(theme.name, "dark");
        assert_eq!(theme.colors.directory, "cyan");
        assert_eq!(theme.colors.cost_high, "red");
    }

    #[test]
    fn test_load_embedded_dark() {
        let theme = Theme::load_embedded("dark").unwrap();
        assert_eq!(theme.name, "dark");
        assert_eq!(theme.colors.directory, "cyan");
        assert_eq!(theme.colors.context_normal, "white");
    }

    #[test]
    fn test_load_embedded_light() {
        let theme = Theme::load_embedded("light").unwrap();
        assert_eq!(theme.name, "light");
        assert_eq!(theme.colors.directory, "blue"); // Darker for light bg
        assert_eq!(theme.colors.context_normal, "gray"); // Gray instead of white
    }

    #[test]
    fn test_load_embedded_case_insensitive() {
        let dark = Theme::load_embedded("DARK").unwrap();
        assert_eq!(dark.name, "dark");

        let light = Theme::load_embedded("Light").unwrap();
        assert_eq!(light.name, "light");
    }

    #[test]
    fn test_load_embedded_unknown() {
        let result = Theme::load_embedded("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown embedded theme"));
    }

    #[test]
    fn test_embedded_themes_list() {
        let themes = Theme::embedded_themes();
        assert_eq!(themes.len(), 2);
        assert!(themes.contains(&"dark"));
        assert!(themes.contains(&"light"));
    }

    #[test]
    fn test_theme_from_toml() {
        let toml = r#"
            name = "test"
            description = "Test theme"

            [colors]
            directory = "bright_blue"
            git_branch = "bright_green"
            model = "cyan"
            duration = "gray"
            separator = "gray"
            lines_added = "green"
            lines_removed = "red"
            cost_low = "green"
            cost_medium = "yellow"
            cost_high = "red"
            context_normal = "white"
            context_caution = "yellow"
            context_warning = "orange"
            context_critical = "red"
        "#;

        let theme = Theme::from_toml(toml).unwrap();
        assert_eq!(theme.name, "test");
        assert_eq!(theme.description, Some("Test theme".to_string()));
        assert_eq!(theme.colors.directory, "bright_blue");
    }

    #[test]
    fn test_theme_with_defaults() {
        // Theme with only required fields should get defaults
        let toml = r#"
            name = "minimal"
            [colors]
        "#;

        let theme = Theme::from_toml(toml).unwrap();
        assert_eq!(theme.name, "minimal");
        assert_eq!(theme.colors.directory, "cyan"); // Default
        assert_eq!(theme.colors.cost_high, "red"); // Default
    }

    #[test]
    fn test_resolve_basic_colors() {
        let theme = Theme::default();
        assert_eq!(theme.resolve_color("cyan"), "\x1b[36m");
        assert_eq!(theme.resolve_color("green"), "\x1b[32m");
        assert_eq!(theme.resolve_color("red"), "\x1b[31m");
        assert_eq!(theme.resolve_color("yellow"), "\x1b[33m");
    }

    #[test]
    fn test_resolve_bright_colors() {
        let theme = Theme::default();
        assert_eq!(theme.resolve_color("bright_blue"), "\x1b[94m");
        assert_eq!(theme.resolve_color("bright_green"), "\x1b[92m");
    }

    #[test]
    fn test_resolve_aliases() {
        let theme = Theme::default();
        assert_eq!(theme.resolve_color("orange"), "\x1b[38;5;208m");
        assert_eq!(theme.resolve_color("light_gray"), "\x1b[38;5;245m");
    }

    #[test]
    fn test_resolve_ansi_passthrough() {
        let theme = Theme::default();
        let ansi = "\x1b[38;5;214m";
        assert_eq!(theme.resolve_color(ansi), ansi);
    }

    #[test]
    fn test_resolve_unknown_color() {
        let theme = Theme::default();
        // Unknown colors default to white
        assert_eq!(theme.resolve_color("invalid_color"), "\x1b[37m");
    }

    #[test]
    fn test_custom_palette() {
        // In TOML, use double backslash: \\x1b becomes literal \x1b string
        // which resolve_color then converts to actual ANSI code
        let toml = r#"
            name = "custom"
            [colors]
            directory = "my_blue"

            [palette.custom]
            my_blue = "\\x1b[38;5;39m"
        "#;

        let theme = Theme::from_toml(toml).unwrap();
        // resolve_color converts \\x1b to actual \x1b ANSI code
        assert_eq!(theme.resolve_color("my_blue"), "\x1b[38;5;39m");
    }

    #[test]
    fn test_resolve_escaped_ansi() {
        let theme = Theme::default();
        // Simulates TOML file with escaped backslash
        let escaped = "\\x1b[38;5;214m";
        assert_eq!(theme.resolve_color(escaped), "\x1b[38;5;214m");
    }

    #[test]
    fn test_theme_display() {
        let theme = Theme::default();
        assert_eq!(
            format!("{}", theme),
            "dark - Default dark theme for dark terminals"
        );

        let minimal = Theme {
            name: "test".to_string(),
            description: None,
            colors: ThemeColors::default(),
            palette: None,
        };
        assert_eq!(format!("{}", minimal), "test");
    }
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            directory: default_cyan(),
            git_branch: default_green(),
            model: default_cyan(),
            duration: default_light_gray(),
            separator: default_light_gray(),
            lines_added: default_green(),
            lines_removed: default_red(),
            cost_low: default_green(),
            cost_medium: default_yellow(),
            cost_high: default_red(),
            context_normal: default_white(),
            context_caution: default_yellow(),
            context_warning: default_orange(),
            context_critical: default_red(),
        }
    }
}
