//! Comprehensive tests for the GSD data provider.
//!
//! Covers: all-keys-present, trait contract, state parsing, roadmap parsing,
//! summary assembly, smart truncation, graceful degradation, and auto-detection.

use super::*;
use crate::provider::DataProvider;
use std::fs;
use std::time::Duration;
use tempfile::TempDir;

// ============================================================================
// Smart truncation tests (moved from inline mod tests)
// ============================================================================

#[test]
fn test_smart_truncate_short_string() {
    assert_eq!(smart_truncate("hello", 10), "hello");
}

#[test]
fn test_smart_truncate_exact_limit() {
    assert_eq!(smart_truncate("hello", 5), "hello");
}

#[test]
fn test_smart_truncate_no_limit() {
    assert_eq!(smart_truncate("hello world", 0), "hello world");
}

#[test]
fn test_smart_truncate_word_boundary() {
    // "Implementing the provider trait system" with limit 30
    // At position 30: "Implementing the provider trai"
    // Last space at 25: "Implementing the provider"
    // 25 > 30/2=15, so truncate at word boundary
    assert_eq!(
        smart_truncate("Implementing the provider trait system", 30),
        "Implementing the provider..."
    );
}

#[test]
fn test_smart_truncate_no_good_boundary() {
    // "abcdefghijklmnop" with limit 10 -- no spaces at all
    assert_eq!(
        smart_truncate("abcdefghijklmnop", 10),
        "abcdefghij..."
    );
}

#[test]
fn test_smart_truncate_space_too_early() {
    // "a bcdefghijklmnop" with limit 10 -- space at position 1
    // 1 is NOT > 10/2=5, so truncate at exact limit
    assert_eq!(
        smart_truncate("a bcdefghijklmnop", 10),
        "a bcdefghi..."
    );
}

#[test]
fn test_smart_truncate_empty_string() {
    assert_eq!(smart_truncate("", 10), "");
}

#[test]
fn test_smart_truncate_single_char_over() {
    assert_eq!(smart_truncate("ab", 1), "a...");
}

// ============================================================================
// GsdProvider tests
// ============================================================================

/// Helper to create a GsdConfig with default values.
fn default_config() -> GsdConfig {
    GsdConfig::default()
}

/// Helper to create a GsdProvider pointing at a tempdir with no .planning/.
/// This tests the "absent" case where planning_dir is None.
fn provider_without_planning() -> GsdProvider {
    GsdProvider {
        planning_dir: None,
        home_dir: PathBuf::from("/tmp/nonexistent"),
        enabled: true,
        task_truncation_limit: 40,
        todo_staleness_seconds: 86400,
        update_delay_seconds: 300,
        phase_max_width: 0,
        task_max_width: 40,
        separator: "\u{00b7}".to_string(),
        phase_format: "P{n}".to_string(),
        color_enabled: false, // Disable colors in tests for predictable output
        show_phase: true,
        show_task: true,
        show_update: true,
        stale_hours: 24,
        stale_enabled: false,
    }
}

/// Helper to create a GsdProvider with a specific planning_dir.
fn provider_with_planning(planning_dir: PathBuf) -> GsdProvider {
    GsdProvider {
        planning_dir: Some(planning_dir),
        home_dir: PathBuf::from("/tmp/nonexistent"),
        enabled: true,
        task_truncation_limit: 40,
        todo_staleness_seconds: 86400,
        update_delay_seconds: 300,
        phase_max_width: 0,
        task_max_width: 40,
        separator: "\u{00b7}".to_string(),
        phase_format: "P{n}".to_string(),
        color_enabled: false,
        show_phase: true,
        show_task: true,
        show_update: true,
        stale_hours: 24,
        stale_enabled: false,
    }
}

/// Expected number of keys in the output HashMap.
/// init_empty_vars() creates 21 keys, but gsd_last_activity is removed before
/// returning, leaving 20 user-visible keys.
const EXPECTED_KEY_COUNT: usize = 20;

/// All expected keys in the output HashMap.
const EXPECTED_KEYS: &[&str] = &[
    "gsd_phase",
    "gsd_phase_number",
    "gsd_phase_name",
    "gsd_progress_fraction",
    "gsd_progress_pct",
    "gsd_progress_completed",
    "gsd_progress_total",
    "gsd_task",
    "gsd_task_progress",
    "gsd_update_available",
    "gsd_update_version",
    "gsd_summary",
    // New in Phase 5 Plan 02
    "gsd_update",
    "gsd_task_full",
    "gsd_plan_completed",
    "gsd_plan_total",
    "gsd_plan_fraction",
    "gsd_stale",
    "gsd_icon",
    "gsd_separator",
    // Note: gsd_last_activity is internal-only and removed before returning
];

// ---- Test 1: All keys present ----

#[test]
fn test_gsd_all_keys_present() {
    let provider = provider_without_planning();
    let result = provider.collect().expect("collect should not error");

    // All expected keys must be present
    for key in EXPECTED_KEYS {
        assert!(
            result.contains_key(*key),
            "Missing key: {}",
            key
        );
    }

    // gsd_separator is also present
    assert!(
        result.contains_key("gsd_separator"),
        "Missing key: gsd_separator"
    );

    assert_eq!(
        result.len(),
        EXPECTED_KEY_COUNT,
        "Expected exactly {} keys, got {} (keys: {:?})",
        EXPECTED_KEY_COUNT,
        result.len(),
        result.keys().collect::<Vec<_>>()
    );

    // All values should be empty strings when no .planning/ dir
    for key in EXPECTED_KEYS {
        assert_eq!(
            result.get(*key).unwrap(),
            "",
            "Key '{}' should be empty string when no planning dir, got '{}'",
            key,
            result.get(*key).unwrap()
        );
    }
}

// ---- Test 2: Trait contract ----

#[test]
fn test_gsd_trait_contract() {
    let provider = provider_without_planning();

    assert_eq!(provider.name(), "gsd");
    assert_eq!(provider.priority(), 50);
    assert_eq!(provider.timeout(), Duration::from_millis(10));

    // No planning_dir -> is_available() should be false
    assert!(
        !provider.is_available(),
        "Provider with no planning_dir should not be available"
    );
}

#[test]
fn test_gsd_available_with_planning_dir() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut config = default_config();
    config.project_dir = tmp.path().to_string_lossy().to_string();

    let provider = GsdProvider::new(&config, tmp.path());
    assert!(
        provider.is_available(),
        "Provider with valid planning_dir should be available"
    );
}

// ---- Test 3: Unavailable when disabled ----

#[test]
fn test_gsd_unavailable_when_disabled() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.enabled = false;

    assert!(
        !provider.is_available(),
        "Disabled provider should not be available even with planning_dir"
    );
}

// ---- Test 4: State parsing ----

#[test]
fn test_gsd_state_parsing() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "## Current Position\n\nPhase: 4 of 6 (GSD Provider)\nPlan: 1 of 3\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning);

    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_phase").unwrap(), "P4: GSD Provider");
    assert_eq!(result.get("gsd_phase_number").unwrap(), "4");
    assert_eq!(result.get("gsd_phase_name").unwrap(), "GSD Provider");
}

// ---- Test 5: Roadmap parsing ----

#[test]
fn test_gsd_roadmap_parsing() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "# Minimal\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    fs::write(
        planning.join("ROADMAP.md"),
        r#"## Phases

- [x] **Phase 1: Provider Architecture** - desc
- [x] **Phase 2: Database Refactoring** - desc
- [x] **Phase 3: Stats Refactoring** - desc
- [ ] **Phase 4: GSD Provider** - desc
- [ ] **Phase 5: Layout** - desc
- [ ] **Phase 6: Release** - desc
"#,
    )
    .unwrap();

    let provider = provider_with_planning(planning);

    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_progress_fraction").unwrap(), "3/6");
    assert_eq!(result.get("gsd_progress_pct").unwrap(), "50");
    assert_eq!(result.get("gsd_progress_completed").unwrap(), "3");
    assert_eq!(result.get("gsd_progress_total").unwrap(), "6");
}

// ---- Test 6: Summary assembly ----

#[test]
fn test_gsd_summary_assembly() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 4 of 6 (GSD Provider)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    fs::write(
        planning.join("ROADMAP.md"),
        r#"- [x] **Phase 1: Provider Architecture** - d
- [x] **Phase 2: Database Refactoring** - d
- [x] **Phase 3: Stats Refactoring** - d
- [ ] **Phase 4: GSD Provider** - d
- [ ] **Phase 5: Layout** - d
- [ ] **Phase 6: Release** - d
"#,
    )
    .unwrap();

    let provider = provider_with_planning(planning);

    let result = provider.collect().unwrap();
    // With default separator (middle dot) and format "P{n}"
    assert_eq!(
        result.get("gsd_summary").unwrap(),
        "P4\u{00b7}GSD Provider 3/6"
    );
}

// ---- Test 7: Summary phase only (no roadmap) ----

#[test]
fn test_gsd_summary_phase_only() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 4 of 6 (GSD Provider)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    // No ROADMAP.md

    let provider = provider_with_planning(planning);

    let result = provider.collect().unwrap();
    assert_eq!(
        result.get("gsd_summary").unwrap(),
        "P4\u{00b7}GSD Provider",
        "Summary should be phase only when no ROADMAP.md"
    );
    // Progress vars should be empty
    assert_eq!(result.get("gsd_progress_fraction").unwrap(), "");
}

// ---- Test 8: build_summary directly ----

#[test]
fn test_build_summary_directly() {
    // Phase + progress (using new signature with format and separator)
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "4".into());
    vars.insert("gsd_phase_name".into(), "GSD Provider".into());
    vars.insert("gsd_progress_fraction".into(), "3/6".into());
    GsdProvider::build_summary(&mut vars, "P{n}", "\u{00b7}");
    assert_eq!(vars.get("gsd_summary").unwrap(), "P4\u{00b7}GSD Provider 3/6");

    // Phase only
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "4".into());
    vars.insert("gsd_phase_name".into(), "GSD Provider".into());
    GsdProvider::build_summary(&mut vars, "P{n}", "\u{00b7}");
    assert_eq!(vars.get("gsd_summary").unwrap(), "P4\u{00b7}GSD Provider");

    // No phase
    let mut vars = init_empty_vars();
    GsdProvider::build_summary(&mut vars, "P{n}", "\u{00b7}");
    assert_eq!(
        vars.get("gsd_summary").unwrap(),
        "",
        "Summary should stay empty when no phase data"
    );

    // With plan fraction
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "5".into());
    vars.insert("gsd_phase_name".into(), "Layout".into());
    vars.insert("gsd_progress_fraction".into(), "2/6".into());
    vars.insert("gsd_plan_fraction".into(), "1/3".into());
    GsdProvider::build_summary(&mut vars, "P{n}", "\u{00b7}");
    assert_eq!(vars.get("gsd_summary").unwrap(), "P5\u{00b7}Layout 2/6 [1/3]");

    // Custom format
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "5".into());
    vars.insert("gsd_phase_name".into(), "Layout".into());
    GsdProvider::build_summary(&mut vars, "Phase {n}", " - ");
    assert_eq!(vars.get("gsd_summary").unwrap(), "Phase 5 - Layout");
}

// ---- Test 9: Graceful degradation with missing files ----

#[test]
fn test_gsd_graceful_degradation_missing_files() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    // Only STATE.md and config.json -- no ROADMAP.md
    fs::write(
        planning.join("STATE.md"),
        "Phase: 4 of 6 (GSD Provider)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning);

    let result = provider.collect().unwrap();

    // State vars should be populated
    assert_eq!(result.get("gsd_phase").unwrap(), "P4: GSD Provider");
    assert_eq!(result.get("gsd_phase_number").unwrap(), "4");
    assert_eq!(result.get("gsd_phase_name").unwrap(), "GSD Provider");

    // Progress vars should be empty (no ROADMAP.md)
    assert_eq!(result.get("gsd_progress_fraction").unwrap(), "");
    assert_eq!(result.get("gsd_progress_pct").unwrap(), "");
    assert_eq!(result.get("gsd_progress_completed").unwrap(), "");
    assert_eq!(result.get("gsd_progress_total").unwrap(), "");

    // Task vars should be empty (no todos directory)
    assert_eq!(result.get("gsd_task").unwrap(), "");
    assert_eq!(result.get("gsd_task_progress").unwrap(), "");

    // Update vars should be empty (no update check file)
    assert_eq!(result.get("gsd_update_available").unwrap(), "");
    assert_eq!(result.get("gsd_update_version").unwrap(), "");

    // All keys must still be present
    assert_eq!(result.len(), EXPECTED_KEY_COUNT);
}

// ---- Test 10: Detect planning dir ----

#[test]
fn test_gsd_detect_planning_dir() {
    let tmp = TempDir::new().unwrap();
    // Create nested structure: project/subdir/deeper/
    let project = tmp.path().join("project");
    let subdir = project.join("subdir");
    let deeper = subdir.join("deeper");
    fs::create_dir_all(&deeper).unwrap();

    // Put .planning/ at project level
    let planning = project.join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 2 of 4 (Testing)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    // Auto-detect from deeper/ should find project/.planning/
    let detected = detect_planning_dir(&deeper);
    assert!(detected.is_some(), "Should detect .planning/ from nested dir");
    assert_eq!(detected.unwrap(), planning);

    // Auto-detect from subdir/ should also find it
    let detected = detect_planning_dir(&subdir);
    assert!(detected.is_some(), "Should detect .planning/ from subdir");
    assert_eq!(detected.unwrap(), planning);

    // Auto-detect from project/ itself
    let detected = detect_planning_dir(&project);
    assert!(detected.is_some(), "Should detect .planning/ from project root");
    assert_eq!(detected.unwrap(), planning);
}

// ---- Test 11: GsdProvider via config override ----

#[test]
fn test_gsd_config_override_project_dir() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 1 of 2 (Override Test)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut config = default_config();
    config.project_dir = tmp.path().to_string_lossy().to_string();

    // CWD is /tmp (no .planning/ there), but config override points to tempdir
    let provider = GsdProvider::new(&config, std::path::Path::new("/tmp"));
    assert!(provider.is_available());

    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_phase").unwrap(), "P1: Override Test");
}

// ---- Test 12: No .planning/ returns all empty ----

#[test]
fn test_gsd_no_planning_all_empty() {
    let tmp = TempDir::new().unwrap();
    // CWD with no .planning/ anywhere
    let config = default_config();
    let provider = GsdProvider::new(&config, tmp.path());

    assert!(
        !provider.is_available(),
        "Should not be available without .planning/"
    );

    let result = provider.collect().unwrap();
    assert_eq!(result.len(), EXPECTED_KEY_COUNT, "All {} keys should exist", EXPECTED_KEY_COUNT);
    for (key, val) in &result {
        assert_eq!(val, "", "Key '{}' value should be empty, got '{}'", key, val);
    }
}

// ---- Test 13: New convenience variables ----

#[test]
fn test_gsd_convenience_vars_empty_when_no_data() {
    let provider = provider_without_planning();
    let result = provider.collect().unwrap();

    // gsd_update should be empty (no update data)
    assert_eq!(result.get("gsd_update").unwrap(), "");
    // gsd_task_full should be empty (no task data)
    assert_eq!(result.get("gsd_task_full").unwrap(), "");
    // gsd_stale should be empty (stale_enabled is false)
    assert_eq!(result.get("gsd_stale").unwrap(), "");
    // gsd_plan_fraction should be empty (no roadmap)
    assert_eq!(result.get("gsd_plan_fraction").unwrap(), "");
    assert_eq!(result.get("gsd_plan_completed").unwrap(), "");
    assert_eq!(result.get("gsd_plan_total").unwrap(), "");
}

// ---- Test 14: Icon output ----

#[test]
fn test_gsd_icon_without_color() {
    // Without planning_dir, gsd_icon is empty (no data to display)
    let provider = provider_without_planning();
    let result = provider.collect().unwrap();
    let icon = result.get("gsd_icon").unwrap();
    assert_eq!(icon, "", "Icon should be empty when no planning dir");
}

#[test]
fn test_gsd_icon_with_planning_no_color() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 4 of 6 (GSD Provider)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.color_enabled = false;

    let result = provider.collect().unwrap();
    let icon = result.get("gsd_icon").unwrap();
    // With color_enabled=false but planning_dir present, icon is the raw character
    assert_eq!(icon, "\u{F0AE2}");
    assert!(!icon.contains("\x1b"), "Icon should not contain ANSI escapes when color disabled");
}

// ---- Test 15: Separator in output ----

#[test]
fn test_gsd_separator_in_output() {
    // Without planning_dir, separator is empty
    let provider = provider_without_planning();
    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_separator").unwrap(), "");
}

#[test]
fn test_gsd_separator_with_planning() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning);
    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_separator").unwrap(), "\u{00b7}");
}

// ---- Test 16: Sub-feature toggles ----

#[test]
fn test_gsd_toggle_show_phase_off() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 4 of 6 (GSD Provider)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    fs::write(
        planning.join("ROADMAP.md"),
        "- [x] **Phase 1: P1** - d\n- [ ] **Phase 4: GSD Provider** - d\n",
    )
    .unwrap();

    let mut provider = provider_with_planning(planning);
    provider.show_phase = false;

    let result = provider.collect().unwrap();

    // Phase-related vars should be empty when show_phase is false
    assert_eq!(result.get("gsd_phase").unwrap(), "");
    assert_eq!(result.get("gsd_phase_number").unwrap(), "");
    assert_eq!(result.get("gsd_phase_name").unwrap(), "");
    assert_eq!(result.get("gsd_progress_fraction").unwrap(), "");
    assert_eq!(result.get("gsd_summary").unwrap(), "");
    assert_eq!(result.get("gsd_plan_fraction").unwrap(), "");
}

#[test]
fn test_gsd_toggle_show_task_off() {
    let mut provider = provider_without_planning();
    provider.show_task = false;

    let result = provider.collect().unwrap();

    assert_eq!(result.get("gsd_task").unwrap(), "");
    assert_eq!(result.get("gsd_task_progress").unwrap(), "");
    assert_eq!(result.get("gsd_task_full").unwrap(), "");
}

#[test]
fn test_gsd_toggle_show_update_off() {
    let mut provider = provider_without_planning();
    provider.show_update = false;

    let result = provider.collect().unwrap();

    assert_eq!(result.get("gsd_update").unwrap(), "");
    assert_eq!(result.get("gsd_update_available").unwrap(), "");
    assert_eq!(result.get("gsd_update_version").unwrap(), "");
}

// ---- Test 17: Plan-level progress in summary ----

#[test]
fn test_gsd_plan_progress_in_summary() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 5 of 6 (Layout Refactoring)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    fs::write(
        planning.join("ROADMAP.md"),
        r#"- [x] **Phase 1: Provider Architecture** - d
- [x] **Phase 2: Database Refactoring** - d
- [x] **Phase 3: Stats Refactoring** - d
- [x] **Phase 4: GSD Provider** - d

Plans:
- [x] 04-01-PLAN.md -- State parser
- [x] 04-02-PLAN.md -- Roadmap parser

- [ ] **Phase 5: Layout Refactoring** - d

Plans:
- [x] 05-01-PLAN.md -- Template engine
- [ ] 05-02-PLAN.md -- Orchestrator wiring
- [ ] 05-03-PLAN.md -- Default template

- [ ] **Phase 6: Release** - d
"#,
    )
    .unwrap();

    let provider = provider_with_planning(planning);

    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_plan_completed").unwrap(), "1");
    assert_eq!(result.get("gsd_plan_total").unwrap(), "3");
    assert_eq!(result.get("gsd_plan_fraction").unwrap(), "1/3");
    // Summary should include plan fraction
    assert_eq!(
        result.get("gsd_summary").unwrap(),
        "P5\u{00b7}Layout Refactoring 4/6 [1/3]"
    );
}

// ---- Test 18: Width truncation ----

#[test]
fn test_gsd_phase_max_width_truncation() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 5 of 6 (Layout Refactoring)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.phase_max_width = 6; // "Layout" fits, "Refactoring" gets cut

    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_phase_name").unwrap(), "Layout...");
}

// ---- Test 19: Last activity not exposed ----

#[test]
fn test_gsd_last_activity_not_in_output() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 5 of 6 (Layout Refactoring)\nLast activity: 2026-02-22 -- Plan 05-01 complete\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning);
    let result = provider.collect().unwrap();

    // gsd_last_activity is internal-only and must not appear in output
    assert!(
        !result.contains_key("gsd_last_activity"),
        "gsd_last_activity should be removed before returning"
    );
}
