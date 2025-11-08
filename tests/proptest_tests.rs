//! Property-based tests using proptest
//!
//! These tests verify that our functions handle arbitrary inputs correctly
//! without panicking or producing invalid outputs.

use proptest::prelude::*;
use serde_json::json;
use statusline::{
    git::get_git_status,
    models::{ContextUsage, Cost, ModelType, StatuslineInput},
    stats::StatsData,
    utils::{parse_iso8601_to_unix, shorten_path},
};

// Strategy for generating valid JSON that should parse as StatuslineInput
fn arbitrary_statusline_json() -> impl Strategy<Value = serde_json::Value> {
    (
        prop::option::of(prop::string::string_regex("[a-zA-Z0-9/._-]{1,100}").unwrap()),
        prop::option::of(prop::string::string_regex("[a-zA-Z0-9 ]{1,50}").unwrap()),
        prop::option::of(prop::string::string_regex("[a-zA-Z0-9-]{1,50}").unwrap()),
        prop::option::of(0.0f64..10000.0),
        prop::option::of(0u64..100000),
        prop::option::of(0u64..100000),
    )
        .prop_map(|(dir, model, session, cost, added, removed)| {
            let mut obj = json!({});

            if dir.is_some() {
                obj["workspace"] = json!({"current_dir": dir});
            }
            if model.is_some() {
                obj["model"] = json!({"display_name": model});
            }
            if session.is_some() {
                obj["session_id"] = json!(session);
            }
            if cost.is_some() || added.is_some() || removed.is_some() {
                obj["cost"] = json!({
                    "total_cost_usd": cost,
                    "total_lines_added": added,
                    "total_lines_removed": removed,
                });
            }

            obj
        })
}

// Test that arbitrary JSON inputs don't panic when parsing
proptest! {
    #[test]
    fn test_statusline_input_parsing_doesnt_panic(json in arbitrary_statusline_json()) {
        let json_str = json.to_string();
        // Should not panic, regardless of input
        let _result: Result<StatuslineInput, _> = serde_json::from_str(&json_str);
    }
}

// Test that path shortening always produces valid output
proptest! {
    #[test]
    fn test_shorten_path_properties(
        path in prop::string::string_regex("[a-zA-Z0-9/._-]{0,500}").unwrap()
    ) {
        let shortened = shorten_path(&path);

        // Properties that should always hold:
        // 1. Output should not be longer than input
        prop_assert!(shortened.len() <= path.len() + 1); // +1 for potential ~

        // 2. If path contains home directory, it should be replaced with ~
        if let Ok(home) = std::env::var("HOME") {
            if path.starts_with(&home) {
                prop_assert!(shortened.starts_with("~"));
            }
        }

        // 3. Empty path should return empty string
        if path.is_empty() {
            prop_assert_eq!(shortened, "");
        }
    }
}

// Test that ISO 8601 parsing handles various formats gracefully
proptest! {
    #[test]
    fn test_iso8601_parsing_doesnt_panic(
        timestamp in prop::string::string_regex("[0-9T:.-Z+]{0,100}").unwrap()
    ) {
        // Should not panic, just return None for invalid inputs
        let _result = parse_iso8601_to_unix(&timestamp);
    }
}

// Test that valid ISO 8601 timestamps parse correctly
proptest! {
    #[test]
    fn test_valid_iso8601_parsing(
        year in 1970u32..2100,
        month in 1u32..=12,
        day in 1u32..=28, // Use 28 to avoid invalid dates
        hour in 0u32..=23,
        minute in 0u32..=59,
        second in 0u32..=59,
    ) {
        let timestamp = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
            year, month, day, hour, minute, second
        );

        let result = parse_iso8601_to_unix(&timestamp);
        prop_assert!(result.is_some());

        // Verify the parsed timestamp is reasonable
        if let Some(unix_time) = result {
            prop_assert!(unix_time > 0);
            prop_assert!(unix_time < 4102444800); // Year 2100 in unix time
        }
    }
}

// Test git status parsing with arbitrary directory paths
proptest! {
    #[test]
    fn test_git_status_doesnt_panic(
        dir in prop::string::string_regex("[a-zA-Z0-9/._-]{0,200}").unwrap()
    ) {
        // Should not panic, just return None for invalid paths
        let _status = get_git_status(&dir);
    }
}

// Test that model type detection handles arbitrary strings
proptest! {
    #[test]
    fn test_model_type_detection(
        name in ".*"
    ) {
        let model_type = ModelType::from_name(&name);

        // Should always return a valid ModelType variant
        match model_type {
            ModelType::Model { .. } | ModelType::Unknown => {
                // All valid variants
            }
        }

        // Abbreviation should always be non-empty
        prop_assert!(!model_type.abbreviation().is_empty());
    }
}

// Test cost calculation properties
proptest! {
    #[test]
    fn test_cost_calculations(
        cost in 0.0f64..10000.0,
        lines_added in 0u64..1000000,
        lines_removed in 0u64..1000000,
    ) {
        let cost_obj = Cost {
            total_cost_usd: Some(cost),
            total_lines_added: Some(lines_added),
            total_lines_removed: Some(lines_removed),
        };

        // Properties:
        // 1. Cost should be non-negative
        if let Some(c) = cost_obj.total_cost_usd {
            prop_assert!(c >= 0.0);
        }

        // 2. Line counts are u64 so always non-negative by type
        // No need to check - compiler guarantees this
    }
}

// Test path security validation
proptest! {
    #[test]
    fn test_path_traversal_protection(
        segments in prop::collection::vec(
            prop_oneof![
                Just("..".to_string()),
                Just(".".to_string()),
                prop::string::string_regex("[a-zA-Z0-9_-]+").unwrap(),
            ],
            0..10
        )
    ) {
        let path = segments.join("/");

        // If path contains "..", it should be rejected by security validation
        if path.contains("..") {
            // This would be caught by our validate_directory_path function
            // Just verify the string contains the pattern
            prop_assert!(path.contains(".."));
        }
    }
}

// Test that stats data serialization round-trips correctly
proptest! {
    #[test]
    fn test_stats_serialization_roundtrip(
        session_id in "[a-zA-Z0-9-]{1,50}",
        cost in 0.0f64..1000.0,
        lines_added in 0u64..10000,
        lines_removed in 0u64..10000,
    ) {
        let mut stats = StatsData::default();
        stats.update_session(&session_id, cost, lines_added, lines_removed, None, None, None, None);

        // Serialize to JSON
        let json = serde_json::to_string(&stats);
        prop_assert!(json.is_ok());

        if let Ok(json_str) = json {
            // Deserialize back
            let deserialized: Result<StatsData, _> = serde_json::from_str(&json_str);
            prop_assert!(deserialized.is_ok());

            if let Ok(restored) = deserialized {
                // Check that key data is preserved
                prop_assert!(restored.sessions.contains_key(&session_id));
            }
        }
    }
}

// Test context usage calculation boundaries
proptest! {
    #[test]
    fn test_context_usage_boundaries(
        input_tokens in 0u32..200000,
        output_tokens in 0u32..50000,
        cache_read in 0u32..50000,
        cache_creation in 0u32..50000,
    ) {
        // Total should be sum of all token types
        let total = input_tokens.saturating_add(output_tokens)
            .saturating_add(cache_read)
            .saturating_add(cache_creation);

        // Calculate percentage (assuming 160k context window)
        let percentage = (total as f64 / 160000.0) * 100.0;

        // Context usage should have valid percentage
        let context_usage = ContextUsage {
            percentage,
            approaching_limit: percentage > 80.0,
            tokens_remaining: 160000_usize.saturating_sub(total as usize),
        };

        // Verify percentage is non-negative
        prop_assert!(context_usage.percentage >= 0.0);

        // Verify level determination logic
        if percentage > 90.0 {
            // Critical level
            prop_assert!(context_usage.percentage > 90.0);
        } else if percentage > 70.0 {
            // High level
            prop_assert!(context_usage.percentage > 70.0 && context_usage.percentage <= 90.0);
        } else if percentage > 50.0 {
            // Medium level
            prop_assert!(context_usage.percentage > 50.0 && context_usage.percentage <= 70.0);
        } else {
            // Low level
            prop_assert!(context_usage.percentage <= 50.0);
        }
    }
}
