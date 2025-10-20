//! Integration tests for theme system.
//!
//! Tests theme loading, color resolution, user themes,
//! and ANSI escape sequence handling.

use statusline::theme::{get_theme_manager, Theme};

// ============================================================================
// Embedded Theme Tests
// ============================================================================

#[test]
fn test_embedded_dark_theme() {
    let theme = Theme::load_embedded("dark").expect("Dark theme should load");

    assert_eq!(theme.name, "dark");
    assert_eq!(
        theme.resolve_color(&theme.colors.directory),
        "\x1b[36m",
        "Dark theme directory should be cyan"
    );
    assert_eq!(
        theme.resolve_color(&theme.colors.context_normal),
        "\x1b[37m",
        "Dark theme context_normal should be white"
    );
}

#[test]
fn test_embedded_light_theme() {
    let theme = Theme::load_embedded("light").expect("Light theme should load");

    assert_eq!(theme.name, "light");
    assert_eq!(
        theme.resolve_color(&theme.colors.directory),
        "\x1b[34m",
        "Light theme directory should be blue"
    );
    assert_eq!(
        theme.resolve_color(&theme.colors.context_normal),
        "\x1b[90m",
        "Light theme context_normal should be gray"
    );
}

#[test]
fn test_embedded_theme_all_colors_defined() {
    let theme = Theme::load_embedded("dark").expect("Dark theme should load");

    // Verify all 14 color fields have values
    assert!(
        !theme.colors.directory.is_empty(),
        "directory color defined"
    );
    assert!(!theme.colors.model.is_empty(), "model color defined");
    assert!(
        !theme.colors.git_branch.is_empty(),
        "git_branch color defined"
    );
    assert!(
        !theme.colors.context_normal.is_empty(),
        "context_normal color defined"
    );
    assert!(
        !theme.colors.context_caution.is_empty(),
        "context_caution color defined"
    );
    assert!(
        !theme.colors.context_warning.is_empty(),
        "context_warning color defined"
    );
    assert!(
        !theme.colors.context_critical.is_empty(),
        "context_critical color defined"
    );
    assert!(!theme.colors.cost_low.is_empty(), "cost_low color defined");
    assert!(
        !theme.colors.cost_medium.is_empty(),
        "cost_medium color defined"
    );
    assert!(
        !theme.colors.cost_high.is_empty(),
        "cost_high color defined"
    );
    assert!(
        !theme.colors.separator.is_empty(),
        "separator color defined"
    );
    assert!(!theme.colors.duration.is_empty(), "duration color defined");
    assert!(
        !theme.colors.lines_added.is_empty(),
        "lines_added color defined"
    );
    assert!(
        !theme.colors.lines_removed.is_empty(),
        "lines_removed color defined"
    );
}

// ============================================================================
// User Theme Tests
// ============================================================================

#[test]
fn test_user_theme_loading() {
    // Create a custom user theme TOML
    let toml_content = r#"
name = "custom"

[colors]
directory = "magenta"
model = "cyan"
git_branch = "green"
context_normal = "white"
context_caution = "yellow"
context_warning = "yellow"
context_critical = "red"
cost_low = "green"
cost_medium = "yellow"
cost_high = "red"
separator = "light_gray"
duration = "light_gray"
lines_added = "green"
lines_removed = "red"
"#;

    // Load the user theme from TOML string
    let theme = Theme::from_toml(toml_content).expect("Should load user theme");

    assert_eq!(theme.name, "custom");
    assert_eq!(
        theme.resolve_color(&theme.colors.directory),
        "\x1b[35m",
        "Custom theme directory should be magenta"
    );
}

#[test]
fn test_user_theme_with_ansi_escape() {
    // Create theme with ANSI escape sequences
    let toml_content = r#"
name = "ansi"

[colors]
directory = "\\x1b[38;5;123m"
model = "\\x1b[38;5;45m"
git_branch = "green"
context_normal = "white"
context_caution = "yellow"
context_warning = "yellow"
context_critical = "red"
cost_low = "green"
cost_medium = "yellow"
cost_high = "red"
separator = "light_gray"
duration = "light_gray"
lines_added = "green"
lines_removed = "red"
"#;

    let theme = Theme::from_toml(toml_content).unwrap();

    // ANSI escapes should be properly converted
    assert_eq!(
        theme.resolve_color(&theme.colors.directory),
        "\x1b[38;5;123m",
        "ANSI escape should be properly converted"
    );
    assert_eq!(
        theme.resolve_color(&theme.colors.model),
        "\x1b[38;5;45m",
        "ANSI escape should be properly converted"
    );
}

// ============================================================================
// Theme Manager Tests
// ============================================================================

#[test]
fn test_theme_manager_caching() {
    // Load theme twice, should use cached version
    let manager1 = get_theme_manager();
    let theme1 = manager1.get_or_load("dark").unwrap();

    let manager2 = get_theme_manager();
    let theme2 = manager2.get_or_load("dark").unwrap();

    // Both should have same name
    assert_eq!(theme1.name, theme2.name);
}

#[test]
fn test_theme_manager_error_on_nonexistent() {
    let manager = get_theme_manager();

    // Non-existent theme should return error
    let result = manager.get_or_load("nonexistent");
    assert!(result.is_err(), "Should return error for nonexistent theme");

    // Error message should be helpful
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("not found"),
        "Error should mention 'not found'"
    );
    assert!(
        err_msg.contains("dark"),
        "Error should list available themes"
    );
}

// ============================================================================
// Color Resolution Tests
// ============================================================================

#[test]
fn test_color_resolution_named_colors() {
    let theme = Theme::load_embedded("dark").unwrap();

    // Test common named color resolutions
    let green = theme.resolve_color("green");
    assert_eq!(green, "\x1b[32m", "green should resolve to \\x1b[32m");

    let cyan = theme.resolve_color("cyan");
    assert_eq!(cyan, "\x1b[36m", "cyan should resolve to \\x1b[36m");

    let red = theme.resolve_color("red");
    assert_eq!(red, "\x1b[31m", "red should resolve to \\x1b[31m");
}

#[test]
fn test_color_resolution_ansi_escape() {
    let theme = Theme::load_embedded("dark").unwrap();

    // Test ANSI escape sequence resolution
    let ansi_color = "\\x1b[38;5;208m".to_string();
    let resolved = theme.resolve_color(&ansi_color);
    assert_eq!(
        resolved, "\x1b[38;5;208m",
        "ANSI escape should be properly converted"
    );
}

// ============================================================================
// Environment Variable Tests
// ============================================================================

#[test]
fn test_theme_env_variable_precedence() {
    // Save original env var
    let original = std::env::var("STATUSLINE_THEME").ok();

    // Set STATUSLINE_THEME
    std::env::set_var("STATUSLINE_THEME", "light");
    let manager = get_theme_manager();
    let theme = manager.get_or_load("light").unwrap();
    assert_eq!(theme.name, "light");

    // Clean up
    if let Some(value) = original {
        std::env::set_var("STATUSLINE_THEME", value);
    } else {
        std::env::remove_var("STATUSLINE_THEME");
    }
}
