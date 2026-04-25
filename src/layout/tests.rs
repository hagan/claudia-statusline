//! Tests for the layout module.

use std::collections::HashMap;

use super::*;
use crate::config::{
    ContextComponentConfig, CostComponentConfig, DirectoryComponentConfig, GitComponentConfig,
    LayoutConfig, ModelComponentConfig,
};

#[test]
fn test_basic_substitution() {
    let renderer = LayoutRenderer::with_format("{directory} {model}", "");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let result = renderer.render(&vars);
    assert_eq!(result, "~/test S4.5");
}

#[test]
fn test_separator_substitution() {
    let renderer = LayoutRenderer::with_format("{directory}{sep}{model}", " • ");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let result = renderer.render(&vars);
    assert_eq!(result, "~/test • S4.5");
}

#[test]
fn test_missing_variable_removed() {
    let renderer = LayoutRenderer::with_format("{directory}{sep}{unknown}{sep}{model}", " • ");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let result = renderer.render(&vars);
    assert_eq!(result, "~/test • S4.5");
}

#[test]
fn test_multiline() {
    let renderer = LayoutRenderer::with_format("{directory}\n{model}", "");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let result = renderer.render(&vars);
    assert_eq!(result, "~/test\nS4.5");
}

#[test]
fn test_preset_default() {
    let format = get_preset_format("default");
    assert!(format.contains("{directory}"));
    assert!(format.contains("{model}"));
}

#[test]
fn test_preset_compact() {
    let format = get_preset_format("compact");
    assert!(format.contains("{dir_short}"));
    assert!(!format.contains("{sep}"));
}

#[test]
fn test_get_used_variables() {
    let renderer = LayoutRenderer::with_format("{directory}{sep}{model} {cost}", " • ");
    let vars = renderer.get_used_variables();
    assert!(vars.contains(&"directory".to_string()));
    assert!(vars.contains(&"model".to_string()));
    assert!(vars.contains(&"cost".to_string()));
    assert!(!vars.contains(&"sep".to_string())); // sep is filtered out
}

#[test]
fn test_from_config_preset() {
    let config = LayoutConfig {
        preset: "compact".to_string(),
        format: String::new(),
        separator: " | ".to_string(),
        ..Default::default()
    };

    let renderer = LayoutRenderer::from_config(&config);
    assert!(renderer.template.contains("{dir_short}"));
}

#[test]
fn test_from_config_custom_format() {
    let config = LayoutConfig {
        preset: "default".to_string(),
        format: "{custom} {format}".to_string(),
        separator: " | ".to_string(),
        ..Default::default()
    };

    let renderer = LayoutRenderer::from_config(&config);
    assert_eq!(renderer.template, "{custom} {format}");
}

#[test]
fn test_clean_double_separators() {
    let renderer = LayoutRenderer::with_format("{a}{sep}{b}{sep}{c}", " • ");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "A".to_string());
    // b is missing
    vars.insert("c".to_string(), "C".to_string());

    let result = renderer.render(&vars);
    assert_eq!(result, "A • C");
}

#[test]
fn test_empty_template() {
    let renderer = LayoutRenderer::with_format("", " • ");
    let vars = HashMap::new();
    let result = renderer.render(&vars);
    assert_eq!(result, "");
}

#[test]
fn test_unbalanced_brace_opening() {
    // Unclosed brace should be preserved (not a valid variable)
    let renderer = LayoutRenderer::with_format("{unclosed text", "");
    let vars = HashMap::new();
    let result = renderer.render(&vars);
    assert_eq!(result, "{unclosed text");
}

#[test]
fn test_unbalanced_brace_closing() {
    // Extra closing brace should be preserved
    let renderer = LayoutRenderer::with_format("text} more", "");
    let vars = HashMap::new();
    let result = renderer.render(&vars);
    assert_eq!(result, "text} more");
}

#[test]
fn test_nested_braces() {
    // Nested braces - outer { starts variable capture, inner content becomes var name
    let renderer = LayoutRenderer::with_format("{{nested}}", "");
    let vars = HashMap::new();
    let result = renderer.render(&vars);
    // {{nested}} -> variable name is "{nested", removed, leaving trailing "}"
    assert_eq!(result, "}");
}

#[test]
fn test_separator_with_control_chars() {
    // Control characters in separator should be sanitized
    let renderer = LayoutRenderer::with_format("{a}{sep}{b}", "\x07bell\x00null");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "A".to_string());
    vars.insert("b".to_string(), "B".to_string());

    let result = renderer.render(&vars);
    // Bell and null should be stripped, leaving just "bellnull"
    assert_eq!(result, "AbellnullB");
}

#[test]
fn test_only_separators() {
    // Template with only separators and missing variables
    let renderer = LayoutRenderer::with_format("{sep}{missing}{sep}", " | ");
    let vars = HashMap::new();
    let result = renderer.render(&vars);
    // All separators cleaned up when no content
    assert_eq!(result, "");
}

#[test]
fn test_whitespace_only_variables() {
    // Empty string variables should be treated as missing
    let renderer = LayoutRenderer::with_format("{a}{sep}{b}{sep}{c}", " • ");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "A".to_string());
    vars.insert("b".to_string(), "".to_string()); // Empty value
    vars.insert("c".to_string(), "C".to_string());

    let result = renderer.render(&vars);
    // Empty b is replaced with "", separators cleaned up
    assert_eq!(result, "A • C");
}

// VariableBuilder tests
#[test]
fn test_variable_builder_basic() {
    let vars = VariableBuilder::new()
        .set("custom", "value".to_string())
        .build();
    assert_eq!(vars.get("custom"), Some(&"value".to_string()));
}

#[test]
fn test_variable_builder_directory() {
    let vars = VariableBuilder::new()
        .directory("~/projects/app", "app", "\x1b[36m", "\x1b[0m")
        .build();

    assert!(vars.get("directory").unwrap().contains("~/projects/app"));
    assert!(vars.get("dir_short").unwrap().contains("app"));
}

#[test]
fn test_variable_builder_cost() {
    let vars = VariableBuilder::new()
        .cost(
            Some(12.50),
            Some(3.25),
            Some(45.00),
            "\x1b[32m", // green
            "\x1b[90m", // gray
            "\x1b[0m",
        )
        .build();

    assert!(vars.get("cost").unwrap().contains("$12.50"));
    assert!(vars.get("burn_rate").unwrap().contains("$3.25/hr"));
    assert!(vars.get("daily_total").unwrap().contains("$45.00"));
    assert!(vars.get("cost_short").unwrap().contains("$12"));
}

#[test]
fn test_variable_builder_lines_changed() {
    let vars = VariableBuilder::new()
        .lines_changed(100, 50, "\x1b[32m", "\x1b[31m", "\x1b[0m")
        .build();

    let lines = vars.get("lines").unwrap();
    assert!(lines.contains("+100"));
    assert!(lines.contains("-50"));
}

#[test]
fn test_variable_builder_empty_values_ignored() {
    let vars = VariableBuilder::new()
        .set("empty", "".to_string())
        .directory("", "", "", "")
        .build();

    // Empty values should not be inserted
    assert!(!vars.contains_key("empty"));
    assert!(!vars.contains_key("directory"));
}

#[test]
fn test_variable_builder_with_renderer() {
    // Integration test: builder + renderer
    let vars = VariableBuilder::new()
        .set("directory", "~/test".to_string())
        .set("model", "S4.5".to_string())
        .build();

    let renderer = LayoutRenderer::with_format("{directory} {model}", "");
    let result = renderer.render(&vars);
    assert_eq!(result, "~/test S4.5");
}

// Component configuration tests
#[test]
fn test_directory_with_config_format_short() {
    let config = DirectoryComponentConfig {
        format: "short".to_string(),
        max_length: 0,
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .directory_with_config(
            "/home/user/projects/app",
            "~/projects/app",
            "app",
            "",
            "",
            &config,
        )
        .build();

    assert_eq!(vars.get("directory"), Some(&"~/projects/app".to_string()));
}

#[test]
fn test_directory_with_config_format_basename() {
    let config = DirectoryComponentConfig {
        format: "basename".to_string(),
        max_length: 0,
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .directory_with_config(
            "/home/user/projects/app",
            "~/projects/app",
            "app",
            "",
            "",
            &config,
        )
        .build();

    assert_eq!(vars.get("directory"), Some(&"app".to_string()));
}

#[test]
fn test_directory_with_config_max_length() {
    let config = DirectoryComponentConfig {
        format: "short".to_string(),
        max_length: 10,
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .directory_with_config(
            "/home/user/projects/app",
            "~/projects/app",
            "app",
            "",
            "",
            &config,
        )
        .build();

    let dir = vars.get("directory").unwrap();
    assert!(dir.starts_with('…'));
    // Check character count, not byte count (… is 3 bytes)
    assert!(dir.chars().count() <= 11); // 10 chars + ellipsis
}

#[test]
fn test_directory_with_config_color_override() {
    let config = DirectoryComponentConfig {
        format: "short".to_string(),
        max_length: 0,
        color: "red".to_string(),
    };
    let vars = VariableBuilder::new()
        .directory_with_config(
            "/home/user/projects/app",
            "~/projects/app",
            "app",
            "\x1b[36m", // cyan (default)
            "\x1b[0m",
            &config,
        )
        .build();

    let dir = vars.get("directory").unwrap();
    assert!(dir.contains("\x1b[31m")); // red override
    assert!(!dir.contains("\x1b[36m")); // not default cyan
}

#[test]
fn test_directory_with_config_utf8_truncation() {
    // Test that truncation works correctly with multi-byte UTF-8 characters
    // This would panic with byte-based slicing if truncation cuts mid-character
    let config = DirectoryComponentConfig {
        format: "short".to_string(),
        max_length: 8,
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .directory_with_config(
            "/home/用户/项目/приложение", // Mixed UTF-8: Chinese, Russian
            "~/用户/项目/приложение",
            "приложение", // Russian: 10 characters but 20 bytes
            "",
            "",
            &config,
        )
        .build();

    let dir = vars.get("directory").unwrap();
    // Should truncate to 8 chars (including ellipsis replacement)
    // … + 7 chars from the end = 8 visible characters
    assert!(dir.starts_with('…'));
    assert_eq!(dir.chars().count(), 8); // Exactly 8 characters
                                        // Should not panic - this is the main test
}

#[test]
fn test_git_with_config_show_when_dirty_when_clean() {
    let config = GitComponentConfig {
        format: "full".to_string(),
        show_when: "dirty".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .git_with_config(
            "main",
            Some("main"),
            None,
            false, // is_dirty = false
            "",
            "",
            &config,
        )
        .build();

    // Should not show git when clean and show_when = "dirty"
    assert!(!vars.contains_key("git"));
}

#[test]
fn test_git_with_config_show_when_dirty_when_dirty() {
    let config = GitComponentConfig {
        format: "full".to_string(),
        show_when: "dirty".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .git_with_config(
            "main +2",
            Some("main"),
            Some("+2"),
            true, // is_dirty = true
            "",
            "",
            &config,
        )
        .build();

    // Should show git when dirty
    assert!(vars.contains_key("git"));
}

#[test]
fn test_git_with_config_format_branch_only() {
    let config = GitComponentConfig {
        format: "branch".to_string(),
        show_when: "always".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .git_with_config(
            "main +2 ~1",
            Some("main"),
            Some("+2 ~1"),
            true,
            "",
            "",
            &config,
        )
        .build();

    assert_eq!(vars.get("git"), Some(&"main".to_string()));
}

#[test]
fn test_model_with_config_format_full() {
    let config = ModelComponentConfig {
        format: "full".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .model_with_config(
            "S4.5",
            "Claude Sonnet 4.5",
            "Sonnet",
            "4.5",
            "",
            "",
            &config,
        )
        .build();

    assert_eq!(vars.get("model"), Some(&"Claude Sonnet 4.5".to_string()));
    assert_eq!(vars.get("model_name"), Some(&"Sonnet".to_string()));
}

#[test]
fn test_model_with_config_format_version() {
    let config = ModelComponentConfig {
        format: "version".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .model_with_config(
            "S4.5",
            "Claude Sonnet 4.5",
            "Sonnet",
            "4.5",
            "",
            "",
            &config,
        )
        .build();

    assert_eq!(vars.get("model"), Some(&"4.5".to_string()));
    // model_full and model_name should always be set regardless of format
    assert_eq!(
        vars.get("model_full"),
        Some(&"Claude Sonnet 4.5".to_string())
    );
    assert_eq!(vars.get("model_name"), Some(&"Sonnet".to_string()));
}

#[test]
fn test_model_with_config_format_name() {
    let config = ModelComponentConfig {
        format: "name".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .model_with_config("O4.5", "Claude Opus 4.5", "Opus", "4.5", "", "", &config)
        .build();

    assert_eq!(vars.get("model"), Some(&"Opus".to_string()));
    assert_eq!(vars.get("model_full"), Some(&"Claude Opus 4.5".to_string()));
    assert_eq!(vars.get("model_name"), Some(&"Opus".to_string()));
}

#[test]
fn test_cost_with_config_cost_only() {
    let config = CostComponentConfig {
        format: "cost_only".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .cost_with_config(Some(12.50), Some(3.25), Some(45.00), "", "", "", &config)
        .build();

    let cost = vars.get("cost").unwrap();
    assert!(cost.contains("12.50"));
    assert!(!cost.contains("/hr")); // No burn rate
}

#[test]
fn test_cost_with_config_rate_only() {
    let config = CostComponentConfig {
        format: "rate_only".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .cost_with_config(Some(12.50), Some(3.25), Some(45.00), "", "", "", &config)
        .build();

    let cost = vars.get("cost").unwrap();
    assert!(cost.contains("3.25/hr"));
    assert!(!cost.contains("12.50")); // No session cost
}

#[test]
fn test_cost_with_config_with_daily() {
    let config = CostComponentConfig {
        format: "with_daily".to_string(),
        color: String::new(),
    };
    let vars = VariableBuilder::new()
        .cost_with_config(Some(12.50), Some(3.25), Some(45.00), "", "", "", &config)
        .build();

    let cost = vars.get("cost").unwrap();
    assert!(cost.contains("12.50"));
    assert!(cost.contains("day:"));
    assert!(cost.contains("45.00"));
}

// Context component config tests
#[test]
fn test_context_with_config_format_full() {
    let config = ContextComponentConfig {
        format: "full".to_string(),
        bar_width: None,
        show_tokens: false,
    };
    let vars = VariableBuilder::new()
        .context_with_config("[=====>----]", Some(50), Some((100_000, 200_000)), &config)
        .build();

    let context = vars.get("context").unwrap();
    assert!(context.contains("50%"));
    assert!(context.contains("[=====>----]"));
    // No tokens because show_tokens=false
    assert!(!context.contains("100k"));
}

#[test]
fn test_context_with_config_format_bar() {
    let config = ContextComponentConfig {
        format: "bar".to_string(),
        bar_width: None,
        show_tokens: true, // Should be ignored for bar format
    };
    let vars = VariableBuilder::new()
        .context_with_config("[=====>----]", Some(50), Some((100_000, 200_000)), &config)
        .build();

    let context = vars.get("context").unwrap();
    assert_eq!(context, "[=====>----]");
    assert!(!context.contains("50%"));
}

#[test]
fn test_context_with_config_format_percent() {
    let config = ContextComponentConfig {
        format: "percent".to_string(),
        bar_width: None,
        show_tokens: true, // Should be ignored for percent format
    };
    let vars = VariableBuilder::new()
        .context_with_config("[=====>----]", Some(75), Some((150_000, 200_000)), &config)
        .build();

    let context = vars.get("context").unwrap();
    assert_eq!(context, "75%");
}

#[test]
fn test_context_with_config_format_tokens() {
    let config = ContextComponentConfig {
        format: "tokens".to_string(),
        bar_width: None,
        show_tokens: true, // Should be ignored for tokens format
    };
    let vars = VariableBuilder::new()
        .context_with_config("[=====>----]", Some(50), Some((100_000, 200_000)), &config)
        .build();

    let context = vars.get("context").unwrap();
    assert_eq!(context, "100k/200k");
}

#[test]
fn test_context_with_config_full_with_tokens() {
    let config = ContextComponentConfig {
        format: "full".to_string(),
        bar_width: None,
        show_tokens: true, // Enable tokens in full format
    };
    let vars = VariableBuilder::new()
        .context_with_config("[=====>----]", Some(50), Some((100_000, 200_000)), &config)
        .build();

    let context = vars.get("context").unwrap();
    assert!(context.contains("50%"));
    assert!(context.contains("[=====>----]"));
    assert!(context.contains("100k/200k"));
}

#[test]
fn test_resolve_color_override_named() {
    use super::format::resolve_color_override;
    assert_eq!(resolve_color_override("red"), "\x1b[31m");
    assert_eq!(resolve_color_override("green"), "\x1b[32m");
    assert_eq!(resolve_color_override("cyan"), "\x1b[36m");
}

#[test]
fn test_resolve_color_override_hex() {
    use super::format::resolve_color_override;
    assert_eq!(resolve_color_override("#FF0000"), "\x1b[38;2;255;0;0m");
    assert_eq!(resolve_color_override("#F00"), "\x1b[38;2;255;0;0m");
}

#[test]
fn test_resolve_color_override_256() {
    use super::format::resolve_color_override;
    assert_eq!(resolve_color_override("38;5;208"), "\x1b[38;5;208m");
}

#[test]
fn test_resolve_color_override_passthrough() {
    use super::format::resolve_color_override;
    assert_eq!(resolve_color_override("\x1b[32m"), "\x1b[32m");
}

// Preset integration tests
#[test]
fn test_preset_default_format() {
    let format = get_preset_format("default");
    assert!(format.contains("{directory}"));
    assert!(format.contains("{git}"));
    assert!(format.contains("{context}"));
    assert!(format.contains("{model}"));
    assert!(format.contains("{cost}"));
}

#[test]
fn test_preset_compact_format() {
    let format = get_preset_format("compact");
    assert!(format.contains("{dir_short}"));
    assert!(format.contains("{git_branch}"));
    assert!(format.contains("{model}"));
    assert!(format.contains("{cost_short}"));
    assert!(!format.contains("{sep}")); // Compact uses spaces
}

#[test]
fn test_preset_detailed_format() {
    let format = get_preset_format("detailed");
    assert!(format.contains("{directory}"));
    assert!(format.contains("{context}"));
    assert!(format.contains('\n')); // Multi-line
}

#[test]
fn test_preset_minimal_format() {
    let format = get_preset_format("minimal");
    assert!(format.contains("{directory}"));
    assert!(format.contains("{model}"));
    assert!(!format.contains("{git}")); // No git in minimal
    assert!(!format.contains("{cost}")); // No cost in minimal
}

#[test]
fn test_preset_power_format() {
    let format = get_preset_format("power");
    assert!(format.contains("{directory}"));
    assert!(format.contains("{git}"));
    assert!(format.contains("{context}"));
    assert!(format.contains("{model}"));
    assert!(format.contains("{duration}"));
    assert!(format.contains("{lines}"));
    assert!(format.contains("{cost}"));
    assert!(format.contains("{burn_rate}"));
    assert!(format.contains('\n')); // Multi-line
}

#[test]
fn test_preset_case_insensitive() {
    assert_eq!(get_preset_format("COMPACT"), get_preset_format("compact"));
    assert_eq!(get_preset_format("Detailed"), get_preset_format("detailed"));
    assert_eq!(get_preset_format("MINIMAL"), get_preset_format("minimal"));
}

#[test]
fn test_unknown_preset_falls_back_to_default() {
    assert_eq!(get_preset_format("unknown"), get_preset_format("default"));
    assert_eq!(get_preset_format(""), get_preset_format("default"));
    assert_eq!(get_preset_format("random"), get_preset_format("default"));
}

#[test]
fn test_list_available_presets() {
    let presets = list_available_presets();
    assert!(presets.contains(&"default".to_string()));
    assert!(presets.contains(&"compact".to_string()));
    assert!(presets.contains(&"detailed".to_string()));
    assert!(presets.contains(&"minimal".to_string()));
    assert!(presets.contains(&"power".to_string()));
    assert!(presets.len() >= 5);
}

#[test]
fn test_preset_rendering_default() {
    let format = get_preset_format("default");
    let renderer = LayoutRenderer::with_format(&format, " • ");

    let vars = VariableBuilder::new()
        .set("directory", "~/test".to_string())
        .set("git", "main".to_string())
        .set("context", "75%".to_string())
        .set("model", "S4.5".to_string())
        .set("cost", "$12".to_string())
        .build();

    let result = renderer.render(&vars);
    assert!(result.contains("~/test"));
    assert!(result.contains("main"));
    assert!(result.contains("75%"));
    assert!(result.contains("S4.5"));
    assert!(result.contains("$12"));
}

#[test]
fn test_preset_rendering_compact() {
    let format = get_preset_format("compact");
    let renderer = LayoutRenderer::with_format(&format, "");

    let vars = VariableBuilder::new()
        .set("dir_short", "test".to_string())
        .set("git_branch", "main".to_string())
        .set("model", "S4.5".to_string())
        .set("cost_short", "$12".to_string())
        .build();

    let result = renderer.render(&vars);
    assert!(result.contains("test"));
    assert!(result.contains("main"));
    assert!(result.contains("S4.5"));
    assert!(result.contains("$12"));
}

#[test]
fn test_preset_rendering_minimal() {
    let format = get_preset_format("minimal");
    let renderer = LayoutRenderer::with_format(&format, "");

    let vars = VariableBuilder::new()
        .set("directory", "~/test".to_string())
        .set("model", "S4.5".to_string())
        .build();

    let result = renderer.render(&vars);
    assert!(result.contains("~/test"));
    assert!(result.contains("S4.5"));
}

#[test]
fn test_token_rate_with_config_sets_all_variables() {
    // Test that token_rate_with_config sets all expected layout variables
    let config = crate::config::TokenRateComponentConfig {
        format: "full".to_string(),
        time_unit: "second".to_string(),
        show_session_total: true,
        show_daily_total: true,
        color: String::new(),
    };

    let vars = VariableBuilder::new()
        .token_rate_with_config(
            10.5,          // rate (tok/s)
            Some(15_000),  // session total
            Some(250_000), // daily total
            "\x1b[90m",    // default color (gray)
            "\x1b[0m",     // reset
            &config,
        )
        .build();

    // Verify all expected variables are set
    assert!(vars.contains_key("token_rate"), "token_rate should be set");
    assert!(
        vars.contains_key("token_rate_only"),
        "token_rate_only should be set"
    );
    assert!(
        vars.contains_key("token_session_total"),
        "token_session_total should be set"
    );
    assert!(
        vars.contains_key("token_daily_total"),
        "token_daily_total should be set"
    );

    // Verify token_rate_only contains rate
    let rate_only = vars.get("token_rate_only").unwrap();
    assert!(rate_only.contains("tok/s"), "Should contain tok/s unit");

    // Verify session total formatting (15K)
    let session = vars.get("token_session_total").unwrap();
    assert!(session.contains("15"), "Should contain 15 (from 15K)");

    // Verify daily total formatting (250K)
    let daily = vars.get("token_daily_total").unwrap();
    assert!(daily.contains("day:"), "Should contain 'day:' prefix");
    assert!(daily.contains("250"), "Should contain 250 (from 250K)");

    // Verify full format combines everything
    let full = vars.get("token_rate").unwrap();
    assert!(full.contains("tok/s"), "Full format should contain rate");
    assert!(
        full.contains("15"),
        "Full format should contain session total"
    );
    assert!(
        full.contains("day:"),
        "Full format should contain daily prefix"
    );
}

#[test]
fn test_token_rate_with_config_layout_integration() {
    // Test that token rate variables work in layout format strings
    let config = crate::config::TokenRateComponentConfig {
        format: "rate_only".to_string(),
        time_unit: "minute".to_string(),
        show_session_total: true,
        show_daily_total: true,
        color: String::new(),
    };

    let vars = VariableBuilder::new()
        .set("directory", "~/test".to_string())
        .token_rate_with_config(5.0, Some(10_000), Some(100_000), "", "", &config)
        .build();

    // Test layout rendering with individual token variables
    let format = "{directory} | rate: {token_rate_only} | session: {token_session_total}";
    let renderer = LayoutRenderer::with_format(format, " • ");
    let result = renderer.render(&vars);

    assert!(
        result.contains("~/test"),
        "Should render directory: {}",
        result
    );
    assert!(
        result.contains("tok/min"),
        "Should render rate with minute unit: {}",
        result
    );
    assert!(
        result.contains("10"),
        "Should render session total: {}",
        result
    );
}

#[test]
fn test_token_rate_with_metrics_cache_metrics_disabled() {
    // Test that cache_metrics = false hides cache-related variables
    let metrics = crate::stats::TokenRateMetrics {
        input_rate: 5.0,
        output_rate: 8.5,
        cache_read_rate: 40.0,
        cache_creation_rate: 2.5,
        total_rate: 56.0,
        duration_seconds: 3600,
        cache_hit_ratio: Some(0.90),
        cache_roi: Some(15.0),
        session_total_tokens: 50000,
        daily_total_tokens: 200000,
    };

    let component_config = crate::config::TokenRateComponentConfig {
        format: "rate_only".to_string(),
        time_unit: "second".to_string(),
        show_session_total: false,
        show_daily_total: false,
        color: String::new(),
    };

    // Cache metrics DISABLED
    let token_rate_config = crate::config::TokenRateConfig {
        enabled: true,
        display_mode: "summary".to_string(),
        cache_metrics: false, // KEY: disabled
        rate_display: "both".to_string(),
        rate_window_seconds: 300,
        inherit_duration_mode: true,
    };

    let vars = VariableBuilder::new()
        .token_rate_with_metrics(&metrics, "", "", &component_config, &token_rate_config)
        .build();

    // Individual rates should be set
    assert!(
        vars.contains_key("token_input_rate"),
        "token_input_rate should be set"
    );
    assert!(
        vars.contains_key("token_output_rate"),
        "token_output_rate should be set"
    );

    // Cache variables should NOT be set when cache_metrics = false
    assert!(
        !vars.contains_key("token_cache_rate"),
        "token_cache_rate should NOT be set when cache_metrics = false"
    );
    assert!(
        !vars.contains_key("token_cache_hit"),
        "token_cache_hit should NOT be set when cache_metrics = false"
    );
    assert!(
        !vars.contains_key("token_cache_roi"),
        "token_cache_roi should NOT be set when cache_metrics = false"
    );
}

#[test]
fn test_token_rate_with_metrics_cache_metrics_enabled() {
    // Test that cache_metrics = true includes cache-related variables
    let metrics = crate::stats::TokenRateMetrics {
        input_rate: 5.0,
        output_rate: 8.5,
        cache_read_rate: 40.0,
        cache_creation_rate: 2.5,
        total_rate: 56.0,
        duration_seconds: 3600,
        cache_hit_ratio: Some(0.90),
        cache_roi: Some(15.0),
        session_total_tokens: 50000,
        daily_total_tokens: 200000,
    };

    let component_config = crate::config::TokenRateComponentConfig {
        format: "rate_only".to_string(),
        time_unit: "second".to_string(),
        show_session_total: false,
        show_daily_total: false,
        color: String::new(),
    };

    // Cache metrics ENABLED
    let token_rate_config = crate::config::TokenRateConfig {
        enabled: true,
        display_mode: "summary".to_string(),
        cache_metrics: true, // KEY: enabled
        rate_display: "both".to_string(),
        rate_window_seconds: 300,
        inherit_duration_mode: true,
    };

    let vars = VariableBuilder::new()
        .token_rate_with_metrics(&metrics, "", "", &component_config, &token_rate_config)
        .build();

    // All variables should be set
    assert!(
        vars.contains_key("token_input_rate"),
        "token_input_rate should be set"
    );
    assert!(
        vars.contains_key("token_output_rate"),
        "token_output_rate should be set"
    );
    assert!(
        vars.contains_key("token_cache_rate"),
        "token_cache_rate should be set when cache_metrics = true"
    );
    assert!(
        vars.contains_key("token_cache_hit"),
        "token_cache_hit should be set when cache_metrics = true"
    );
    assert!(
        vars.contains_key("token_cache_roi"),
        "token_cache_roi should be set when cache_metrics = true"
    );

    // Verify cache values are formatted correctly
    let cache_hit = vars.get("token_cache_hit").unwrap();
    assert!(
        cache_hit.contains("90"),
        "Cache hit should show 90%: {}",
        cache_hit
    );

    let cache_roi = vars.get("token_cache_roi").unwrap();
    assert!(
        cache_roi.contains("15"),
        "Cache ROI should show 15x: {}",
        cache_roi
    );
}

#[test]
fn test_token_rate_with_metrics_rate_display_options() {
    // Test rate_display options: "both", "input_only", "output_only"
    // NOTE: rate_display only applies to "detailed" display_mode
    let metrics = crate::stats::TokenRateMetrics {
        input_rate: 5.0,
        output_rate: 8.5,
        cache_read_rate: 0.0,
        cache_creation_rate: 0.0,
        total_rate: 13.5,
        duration_seconds: 3600,
        cache_hit_ratio: None,
        cache_roi: None,
        session_total_tokens: 0,
        daily_total_tokens: 0,
    };

    let component_config = crate::config::TokenRateComponentConfig {
        format: "rate_only".to_string(),
        time_unit: "second".to_string(),
        show_session_total: false,
        show_daily_total: false,
        color: String::new(),
    };

    // Test "output_only" (requires detailed mode)
    let config_output_only = crate::config::TokenRateConfig {
        enabled: true,
        display_mode: "detailed".to_string(), // Must be detailed for rate_display
        cache_metrics: false,
        rate_display: "output_only".to_string(),
        rate_window_seconds: 300,
        inherit_duration_mode: true,
    };

    let vars = VariableBuilder::new()
        .token_rate_with_metrics(&metrics, "", "", &component_config, &config_output_only)
        .build();

    let token_rate = vars.get("token_rate").unwrap();
    assert!(
        token_rate.contains("Out:"),
        "output_only should show Out: {}",
        token_rate
    );
    assert!(
        !token_rate.contains("In:"),
        "output_only should NOT show In: {}",
        token_rate
    );

    // Test "input_only" (requires detailed mode)
    let config_input_only = crate::config::TokenRateConfig {
        enabled: true,
        display_mode: "detailed".to_string(), // Must be detailed for rate_display
        cache_metrics: false,
        rate_display: "input_only".to_string(),
        rate_window_seconds: 300,
        inherit_duration_mode: true,
    };

    let vars = VariableBuilder::new()
        .token_rate_with_metrics(&metrics, "", "", &component_config, &config_input_only)
        .build();

    let token_rate = vars.get("token_rate").unwrap();
    assert!(
        token_rate.contains("In:"),
        "input_only should show In: {}",
        token_rate
    );
    assert!(
        !token_rate.contains("Out:"),
        "input_only should NOT show Out: {}",
        token_rate
    );

    // Test "both" (requires detailed mode)
    let config_both = crate::config::TokenRateConfig {
        enabled: true,
        display_mode: "detailed".to_string(), // Must be detailed for rate_display
        cache_metrics: false,
        rate_display: "both".to_string(),
        rate_window_seconds: 300,
        inherit_duration_mode: true,
    };

    let vars = VariableBuilder::new()
        .token_rate_with_metrics(&metrics, "", "", &component_config, &config_both)
        .build();

    let token_rate = vars.get("token_rate").unwrap();
    assert!(
        token_rate.contains("In:"),
        "both should show In: {}",
        token_rate
    );
    assert!(
        token_rate.contains("Out:"),
        "both should show Out: {}",
        token_rate
    );

    // Test "summary" mode ignores rate_display (always shows total)
    let config_summary = crate::config::TokenRateConfig {
        enabled: true,
        display_mode: "summary".to_string(),
        cache_metrics: false,
        rate_display: "output_only".to_string(), // Ignored in summary mode
        rate_window_seconds: 300,
        inherit_duration_mode: true,
    };

    let vars = VariableBuilder::new()
        .token_rate_with_metrics(&metrics, "", "", &component_config, &config_summary)
        .build();

    let token_rate = vars.get("token_rate").unwrap();
    assert!(
        token_rate.contains("13.5"),
        "summary should show total rate: {}",
        token_rate
    );
    assert!(
        !token_rate.contains("Out:"),
        "summary should NOT show Out: {}",
        token_rate
    );
}

// =========================================================================
// Conditional template engine tests (render_template)
// =========================================================================

#[test]
fn test_template_simple_variable_substitution() {
    // Backward compat: simple variable substitution works through AST path
    let renderer = LayoutRenderer::with_format("{cost} {model}", "");
    let mut vars = HashMap::new();
    vars.insert("cost".to_string(), "12.50".to_string());
    vars.insert("model".to_string(), "opus".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "12.50 opus");
}

#[test]
fn test_template_conditional_true() {
    let renderer = LayoutRenderer::with_format("{if git}branch: {git}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("git".to_string(), "main".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "branch: main");
}

#[test]
fn test_template_conditional_false() {
    let renderer = LayoutRenderer::with_format("{if git}branch: {git}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("git".to_string(), "".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_conditional_false_missing() {
    // Variable completely absent from map
    let renderer = LayoutRenderer::with_format("{if git}branch: {git}{endif}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_conditional_with_else_true() {
    let renderer = LayoutRenderer::with_format("{if git}{git}{else}no git{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("git".to_string(), "main".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "main");
}

#[test]
fn test_template_conditional_with_else_false() {
    let renderer = LayoutRenderer::with_format("{if git}{git}{else}no git{endif}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "no git");
}

#[test]
fn test_template_negation() {
    let renderer = LayoutRenderer::with_format("{if !gsd_phase}no gsd{endif}", "");
    let vars = HashMap::new(); // gsd_phase absent -> negation is true

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "no gsd");
}

#[test]
fn test_template_negation_with_empty_value() {
    let renderer = LayoutRenderer::with_format("{if !gsd_phase}no gsd{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("gsd_phase".to_string(), "".to_string()); // empty -> negation is true

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "no gsd");
}

#[test]
fn test_template_negation_false() {
    let renderer = LayoutRenderer::with_format("{if !gsd_phase}no gsd{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("gsd_phase".to_string(), "Phase 5".to_string()); // non-empty -> negation is false

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_equality() {
    let renderer = LayoutRenderer::with_format("{if model == opus}fast{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("model".to_string(), "opus".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "fast");
}

#[test]
fn test_template_equality_false() {
    let renderer = LayoutRenderer::with_format("{if model == opus}fast{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("model".to_string(), "sonnet".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_inequality() {
    let renderer = LayoutRenderer::with_format("{if model != opus}not opus{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("model".to_string(), "sonnet".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "not opus");
}

#[test]
fn test_template_inequality_false() {
    let renderer = LayoutRenderer::with_format("{if model != opus}not opus{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("model".to_string(), "opus".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_inequality_missing_var() {
    // Missing variable with != should be true (absent != value)
    let renderer = LayoutRenderer::with_format("{if model != opus}not opus{endif}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "not opus");
}

#[test]
fn test_template_nested_conditionals() {
    let renderer =
        LayoutRenderer::with_format("{if git}{if gsd_phase}both{else}git only{endif}{endif}", "");

    // Both present
    let mut vars = HashMap::new();
    vars.insert("git".to_string(), "main".to_string());
    vars.insert("gsd_phase".to_string(), "Phase 5".to_string());
    assert_eq!(renderer.render_template(&vars, false), "both");

    // Only git
    let mut vars = HashMap::new();
    vars.insert("git".to_string(), "main".to_string());
    assert_eq!(renderer.render_template(&vars, false), "git only");

    // Neither
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_brace_escaping() {
    let renderer = LayoutRenderer::with_format("cost: {{12.50}}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "cost: {12.50}");
}

#[test]
fn test_template_brace_escaping_mixed() {
    let renderer = LayoutRenderer::with_format("{{literal}} and {var}", "");
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "value".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "{literal} and value");
}

#[test]
fn test_template_unknown_variable_show() {
    let renderer = LayoutRenderer::with_format("{unknown}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, true);
    assert_eq!(result, "{unknown}");
}

#[test]
fn test_template_unknown_variable_hide() {
    let renderer = LayoutRenderer::with_format("{unknown}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_parse_error() {
    let renderer = LayoutRenderer::with_format("{if git}no endif", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "[tmpl err]");
}

#[test]
fn test_template_parse_error_stray_else() {
    let renderer = LayoutRenderer::with_format("text{else}more", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "[tmpl err]");
}

#[test]
fn test_template_parse_error_stray_endif() {
    let renderer = LayoutRenderer::with_format("text{endif}more", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "[tmpl err]");
}

#[test]
fn test_template_empty() {
    let renderer = LayoutRenderer::with_format("", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "");
}

#[test]
fn test_template_mixed_segments() {
    // Realistic mixed template with conditionals
    let renderer = LayoutRenderer::with_format(
        "dir: {directory}{if git} | {git}{endif}{if gsd_phase} | {gsd_summary}{endif}",
        "",
    );

    // All segments present
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("git".to_string(), "main".to_string());
    vars.insert("gsd_phase".to_string(), "Phase 5".to_string());
    vars.insert("gsd_summary".to_string(), "P5: Layout 2/6".to_string());
    assert_eq!(
        renderer.render_template(&vars, false),
        "dir: ~/proj | main | P5: Layout 2/6"
    );

    // Only directory and git
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("git".to_string(), "main".to_string());
    assert_eq!(renderer.render_template(&vars, false), "dir: ~/proj | main");

    // Only directory
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    assert_eq!(renderer.render_template(&vars, false), "dir: ~/proj");
}

#[test]
fn test_template_literal_only() {
    let renderer = LayoutRenderer::with_format("hello world", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "hello world");
}

#[test]
fn test_template_separator_handling() {
    // Conditionals with separators
    let renderer = LayoutRenderer::with_format(
        "{directory}{if git} | {git}{endif}{if model} | {model}{endif}",
        "",
    );

    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("model".to_string(), "opus".to_string());
    // git is missing -- separator before git should not appear
    assert_eq!(renderer.render_template(&vars, false), "~/proj | opus");
}

#[test]
fn test_template_conditional_no_space_after_if() {
    // {if!var} syntax (no space after if)
    let renderer = LayoutRenderer::with_format("{if !var}hidden{endif}", "");
    let vars = HashMap::new();

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "hidden");
}

#[test]
fn test_template_deeply_nested() {
    // 3 levels of nesting
    let renderer = LayoutRenderer::with_format(
        "{if a}{if b}{if c}deep{else}ab{endif}{else}a only{endif}{else}none{endif}",
        "",
    );

    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "1".to_string());
    vars.insert("c".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "deep");

    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "ab");

    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "a only");

    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "none");
}

#[test]
fn test_template_multiple_conditionals_in_sequence() {
    let renderer = LayoutRenderer::with_format("{if a}A{endif}{if b}B{endif}{if c}C{endif}", "");

    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("c".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "AC");
}

#[test]
fn test_template_equality_with_spaces() {
    // Spaces around == should be trimmed
    let renderer = LayoutRenderer::with_format("{if model == opus}yes{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("model".to_string(), "opus".to_string());
    assert_eq!(renderer.render_template(&vars, false), "yes");
}

#[test]
fn test_template_unclosed_brace_as_literal() {
    // A { without a matching } should be treated as literal
    let renderer = LayoutRenderer::with_format("price: {5", "");
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "price: {5");
}

#[test]
fn test_template_render_backward_compat() {
    // Verify render_template produces same result as render for simple templates
    let renderer = LayoutRenderer::with_format("{directory} {model}", "");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let legacy = renderer.render(&vars);
    let template = renderer.render_template(&vars, false);
    assert_eq!(legacy, template);
}

#[test]
fn test_template_with_sep_replacement() {
    // {sep} should be replaced before AST parsing
    let renderer = LayoutRenderer::with_format("{directory}{sep}{model}", " | ");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "~/test | S4.5");
}

#[test]
fn test_template_conditional_with_sep() {
    // Conditional segments with separator
    let renderer = LayoutRenderer::with_format(
        "{directory}{if git}{sep}{git}{endif}{if model}{sep}{model}{endif}",
        " | ",
    );

    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());
    // git missing -- its separator should not appear
    assert_eq!(renderer.render_template(&vars, false), "~/test | S4.5");
}

// =========================================================================
// Phase 5 Plan 3: Template engine edge case tests
// =========================================================================

#[test]
fn test_template_deeply_nested_3_levels_all_present() {
    let renderer = LayoutRenderer::with_format("{if a}{if b}{if c}deep{endif}{endif}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "1".to_string());
    vars.insert("c".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "deep");
}

#[test]
fn test_template_deeply_nested_3_levels_ab_only() {
    let renderer = LayoutRenderer::with_format("{if a}{if b}{if c}deep{endif}{endif}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "1".to_string());
    // c absent -- inner {if c} is false, outputs nothing
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_deeply_nested_3_levels_a_only() {
    let renderer = LayoutRenderer::with_format("{if a}{if b}{if c}deep{endif}{endif}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_deeply_nested_3_levels_none() {
    let renderer = LayoutRenderer::with_format("{if a}{if b}{if c}deep{endif}{endif}{endif}", "");
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_adjacent_conditionals_all() {
    let renderer = LayoutRenderer::with_format("{if a}A{endif}{if b}B{endif}{if c}C{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "1".to_string());
    vars.insert("c".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "ABC");
}

#[test]
fn test_template_adjacent_conditionals_a_and_c() {
    let renderer = LayoutRenderer::with_format("{if a}A{endif}{if b}B{endif}{if c}C{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("c".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "AC");
}

#[test]
fn test_template_adjacent_conditionals_b_only() {
    let renderer = LayoutRenderer::with_format("{if a}A{endif}{if b}B{endif}{if c}C{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("b".to_string(), "1".to_string());
    assert_eq!(renderer.render_template(&vars, false), "B");
}

#[test]
fn test_template_adjacent_conditionals_none() {
    let renderer = LayoutRenderer::with_format("{if a}A{endif}{if b}B{endif}{if c}C{endif}", "");
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_empty_if_with_else_fallback() {
    // Empty if-branch, non-empty else
    let renderer = LayoutRenderer::with_format("{if var}{else}fallback{endif}", "");

    // var is empty -- should show fallback
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "".to_string());
    assert_eq!(renderer.render_template(&vars, false), "fallback");

    // var is absent -- should show fallback
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "fallback");

    // var is present -- empty if-branch, so outputs nothing
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "hello".to_string());
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_content_with_empty_else() {
    // Non-empty if-branch, empty else
    let renderer = LayoutRenderer::with_format("{if var}content{else}{endif}", "");

    // var present -- shows content
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "hello".to_string());
    assert_eq!(renderer.render_template(&vars, false), "content");

    // var absent -- else branch is empty, outputs nothing
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_whitespace_in_conditions() {
    // Extra whitespace around condition components should be trimmed
    let renderer = LayoutRenderer::with_format("{if  var  ==  value }content{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "value".to_string());
    assert_eq!(renderer.render_template(&vars, false), "content");
}

#[test]
fn test_template_variable_names_with_underscores_and_numbers() {
    let renderer =
        LayoutRenderer::with_format("{if gsd_phase_number}P{gsd_phase_number}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("gsd_phase_number".to_string(), "5".to_string());
    assert_eq!(renderer.render_template(&vars, false), "P5");
}

#[test]
fn test_template_variable_names_with_underscores_absent() {
    let renderer =
        LayoutRenderer::with_format("{if gsd_phase_number}P{gsd_phase_number}{endif}", "");
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_multiple_brace_escapes() {
    let renderer = LayoutRenderer::with_format("{{hello}} world {{goodbye}}", "");
    let vars = HashMap::new();
    assert_eq!(
        renderer.render_template(&vars, false),
        "{hello} world {goodbye}"
    );
}

#[test]
fn test_template_brace_escape_inside_conditional() {
    let renderer = LayoutRenderer::with_format("{if var}cost: {{12.50}}{endif}", "");
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "yes".to_string());
    assert_eq!(renderer.render_template(&vars, false), "cost: {12.50}");
}

#[test]
fn test_template_brace_escape_inside_conditional_false() {
    let renderer = LayoutRenderer::with_format("{if var}cost: {{12.50}}{endif}", "");
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_comparison_with_empty_string() {
    // {if var == } checks if var equals empty string
    let renderer = LayoutRenderer::with_format("{if var == }is empty{endif}", "");

    // var set to empty string -- should match
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "".to_string());
    assert_eq!(renderer.render_template(&vars, false), "is empty");

    // var set to non-empty -- should not match
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "hello".to_string());
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_inequality_with_empty_string() {
    let renderer = LayoutRenderer::with_format("{if var != }not empty{endif}", "");

    // var set to non-empty -- should match
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "hello".to_string());
    assert_eq!(renderer.render_template(&vars, false), "not empty");

    // var set to empty -- should not match
    let mut vars = HashMap::new();
    vars.insert("var".to_string(), "".to_string());
    assert_eq!(renderer.render_template(&vars, false), "");
}

#[test]
fn test_template_nesting_depth_exceeded() {
    // Build template with 11 nested {if} blocks -- exceeds MAX_NESTING_DEPTH of 10
    let mut template = String::new();
    for _i in 0..11 {
        template.push_str("{if x}");
    }
    template.push_str("deep");
    for _i in 0..11 {
        template.push_str("{endif}");
    }

    let renderer = LayoutRenderer::with_format(&template, "");
    let mut vars = HashMap::new();
    vars.insert("x".to_string(), "1".to_string());

    // Should produce [tmpl err] because nesting exceeds max depth
    assert_eq!(renderer.render_template(&vars, false), "[tmpl err]");
}

#[test]
fn test_template_nesting_depth_at_limit() {
    // Exactly 10 levels -- should be OK
    let mut template = String::new();
    for _i in 0..10 {
        template.push_str("{if x}");
    }
    template.push_str("ok");
    for _i in 0..10 {
        template.push_str("{endif}");
    }

    let renderer = LayoutRenderer::with_format(&template, "");
    let mut vars = HashMap::new();
    vars.insert("x".to_string(), "1".to_string());

    assert_eq!(renderer.render_template(&vars, false), "ok");
}

#[test]
fn test_template_unclosed_else() {
    // {if var}content{else} without {endif}
    let renderer = LayoutRenderer::with_format("{if var}content{else}", "");
    let vars = HashMap::new();
    assert_eq!(renderer.render_template(&vars, false), "[tmpl err]");
}

#[test]
fn test_template_extra_endif() {
    // {if var}content{endif}{endif} -- extra {endif} is a stray terminator
    let renderer = LayoutRenderer::with_format("{if var}content{endif}{endif}", "");
    let vars = HashMap::new();
    // parse_template should detect stray {endif} -> parse error
    assert_eq!(renderer.render_template(&vars, false), "[tmpl err]");
}

#[test]
fn test_template_default_template_full_data() {
    // Use the actual default.tmpl content and verify it renders correctly with full data
    let default_tmpl = include_str!("../templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");

    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("git".to_string(), "main +2".to_string());
    vars.insert("gsd_phase".to_string(), "P5: Layout".to_string());
    vars.insert("gsd_icon".to_string(), "\u{F0AE2}".to_string());
    vars.insert(
        "gsd_summary".to_string(),
        "P5\u{00b7}Layout 4/6 [2/3]".to_string(),
    );
    vars.insert("gsd_task".to_string(), "Writing tests".to_string());
    vars.insert(
        "gsd_task_full".to_string(),
        "Writing tests (2/5)".to_string(),
    );
    vars.insert("gsd_separator".to_string(), "\u{00b7}".to_string());
    vars.insert("gsd_update".to_string(), "".to_string());
    vars.insert("context".to_string(), "75%".to_string());
    vars.insert("model".to_string(), "O4.6".to_string());
    vars.insert("cost".to_string(), "$12.50".to_string());

    let result = renderer.render_template(&vars, false);
    // Verify all segments are present in order
    assert!(
        result.contains("~/proj"),
        "Should contain directory: {}",
        result
    );
    assert!(result.contains("main +2"), "Should contain git: {}", result);
    assert!(
        result.contains("\u{F0AE2}"),
        "Should contain GSD icon: {}",
        result
    );
    assert!(
        result.contains("P5\u{00b7}Layout 4/6 [2/3]"),
        "Should contain GSD summary: {}",
        result
    );
    assert!(
        result.contains("Writing tests (2/5)"),
        "Should contain task: {}",
        result
    );
    assert!(result.contains("75%"), "Should contain context: {}", result);
    assert!(result.contains("O4.6"), "Should contain model: {}", result);
    assert!(result.contains("$12.50"), "Should contain cost: {}", result);
}

#[test]
fn test_template_default_template_gsd_absent() {
    // Default template with no GSD variables -- GSD segment should be hidden
    let default_tmpl = include_str!("../templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");

    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("git".to_string(), "main".to_string());
    vars.insert("context".to_string(), "50%".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());
    vars.insert("cost".to_string(), "$5.00".to_string());
    // No gsd_phase -- GSD segment should be hidden

    let result = renderer.render_template(&vars, false);
    // Should contain all non-GSD segments
    assert!(
        result.contains("~/proj"),
        "Should contain directory: {}",
        result
    );
    assert!(result.contains("main"), "Should contain git: {}", result);
    assert!(result.contains("50%"), "Should contain context: {}", result);
    assert!(result.contains("S4.5"), "Should contain model: {}", result);
    assert!(result.contains("$5.00"), "Should contain cost: {}", result);
    // GSD icon and summary should NOT be present
    assert!(
        !result.contains("\u{F0AE2}"),
        "Should not contain GSD icon: {}",
        result
    );
    assert!(
        !result.contains("gsd"),
        "Should not contain gsd text: {}",
        result
    );
}

#[test]
fn test_template_variable_only_no_conditionals() {
    // Backward compatibility: template with only variable substitution
    let renderer = LayoutRenderer::with_format("{directory} | {git} | {cost}", "");
    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/test".to_string());
    vars.insert("git".to_string(), "main".to_string());
    vars.insert("cost".to_string(), "$10".to_string());

    let result = renderer.render_template(&vars, false);
    assert_eq!(result, "~/test | main | $10");
}

#[test]
fn test_template_default_template_partial_gsd() {
    // GSD with phase but no task -- task sub-segment should be hidden
    let default_tmpl = include_str!("../templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");

    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("gsd_phase".to_string(), "P5: Layout".to_string());
    vars.insert("gsd_icon".to_string(), "\u{F0AE2}".to_string());
    vars.insert(
        "gsd_summary".to_string(),
        "P5\u{00b7}Layout 4/6".to_string(),
    );
    vars.insert("gsd_separator".to_string(), "\u{00b7}".to_string());
    // No gsd_task, no gsd_update, no gsd_task_full
    vars.insert("model".to_string(), "S4.5".to_string());
    vars.insert("cost".to_string(), "$5.00".to_string());

    let result = renderer.render_template(&vars, false);
    assert!(
        result.contains("P5\u{00b7}Layout 4/6"),
        "Should contain summary: {}",
        result
    );
    // gsd_separator should not appear as a standalone segment since gsd_task is absent
    // (the separator between gsd_summary and gsd_task is inside the {if gsd_task} block)
}

#[test]
fn test_template_default_template_with_update() {
    // GSD with update indicator
    let default_tmpl = include_str!("../templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");

    let mut vars = HashMap::new();
    vars.insert("directory".to_string(), "~/proj".to_string());
    vars.insert("gsd_phase".to_string(), "P5: Layout".to_string());
    vars.insert("gsd_icon".to_string(), "\u{F0AE2}".to_string());
    vars.insert(
        "gsd_summary".to_string(),
        "P5\u{00b7}Layout 4/6".to_string(),
    );
    vars.insert("gsd_separator".to_string(), "\u{00b7}".to_string());
    vars.insert("gsd_update".to_string(), "\u{2191}v1.19.0".to_string());
    vars.insert("model".to_string(), "S4.5".to_string());

    let result = renderer.render_template(&vars, false);
    assert!(
        result.contains("\u{2191}v1.19.0"),
        "Should contain update indicator: {}",
        result
    );
}

// =========================================================================
// Phase 05.1 Plan 03: Provider Hardening - Template Engine Safety
// =========================================================================

#[test]
fn test_template_separator_does_not_inject_ast() {
    // B5 regression: a user-configured separator like "{else}" or "}{if git}"
    // must NOT be text-replaced into the template before AST parse, or it
    // injects template structure into the parsed AST. The fix binds {sep}
    // as a regular variable resolved at render time.
    //
    // Sub-test 1: separator "{else}" — would inject an orphan {else} keyword
    // into the AST, either parse-failing the template or wrecking the
    // surrounding conditional structure.
    let renderer = LayoutRenderer::with_format("{git}{sep}{stats}", "{else}");
    let mut vars = HashMap::new();
    vars.insert("git".to_string(), "main".to_string());
    vars.insert("stats".to_string(), "$1.23".to_string());

    let out = renderer.render_template(&vars, false);
    assert!(
        out.contains("{else}"),
        "literal separator text must appear; got {:?}",
        out
    );
    assert!(out.contains("main"), "git value must appear; got {:?}", out);
    assert!(
        out.contains("$1.23"),
        "stats value must appear; got {:?}",
        out
    );

    // Sub-test 2: canonical injection example from PR-29-REVIEW.md.
    // Separator "}{if git}" — the } would close a brace and {if git} would
    // inject a conditional block opener if pre-replaced.
    let renderer2 = LayoutRenderer::with_format("{git}{sep}{stats}", "}{if git}");
    let out2 = renderer2.render_template(&vars, false);
    assert!(
        out2.contains("}{if git}"),
        "literal separator text must appear; got {:?}",
        out2
    );
    assert!(
        out2.contains("main"),
        "git value must appear; got {:?}",
        out2
    );
    assert!(
        out2.contains("$1.23"),
        "stats value must appear; got {:?}",
        out2
    );
}
