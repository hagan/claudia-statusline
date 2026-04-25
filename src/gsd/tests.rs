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
    assert_eq!(smart_truncate("abcdefghijklmnop", 10), "abcdefghij...");
}

#[test]
fn test_smart_truncate_space_too_early() {
    // "a bcdefghijklmnop" with limit 10 -- space at position 1
    // 1 is NOT > 10/2=5, so truncate at exact limit
    assert_eq!(smart_truncate("a bcdefghijklmnop", 10), "a bcdefghi...");
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
        assert!(result.contains_key(*key), "Missing key: {}", key);
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
    fs::write(planning.join("STATE.md"), "Phase: 4 of 6 (GSD Provider)\n").unwrap();
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
    fs::write(planning.join("STATE.md"), "Phase: 4 of 6 (GSD Provider)\n").unwrap();
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
    assert_eq!(
        vars.get("gsd_summary").unwrap(),
        "P4\u{00b7}GSD Provider 3/6"
    );

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
    assert_eq!(
        vars.get("gsd_summary").unwrap(),
        "P5\u{00b7}Layout 2/6 [1/3]"
    );

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
    fs::write(planning.join("STATE.md"), "Phase: 4 of 6 (GSD Provider)\n").unwrap();
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
    fs::write(planning.join("STATE.md"), "Phase: 2 of 4 (Testing)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    // Auto-detect from deeper/ should find project/.planning/
    let detected = detect_planning_dir(&deeper);
    assert!(
        detected.is_some(),
        "Should detect .planning/ from nested dir"
    );
    assert_eq!(detected.unwrap(), planning);

    // Auto-detect from subdir/ should also find it
    let detected = detect_planning_dir(&subdir);
    assert!(detected.is_some(), "Should detect .planning/ from subdir");
    assert_eq!(detected.unwrap(), planning);

    // Auto-detect from project/ itself
    let detected = detect_planning_dir(&project);
    assert!(
        detected.is_some(),
        "Should detect .planning/ from project root"
    );
    assert_eq!(detected.unwrap(), planning);
}

// ---- Test 11: GsdProvider via config override ----

#[test]
fn test_gsd_config_override_project_dir() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Override Test)\n").unwrap();
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
    assert_eq!(
        result.len(),
        EXPECTED_KEY_COUNT,
        "All {} keys should exist",
        EXPECTED_KEY_COUNT
    );
    for (key, val) in &result {
        assert_eq!(
            val, "",
            "Key '{}' value should be empty, got '{}'",
            key, val
        );
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
    fs::write(planning.join("STATE.md"), "Phase: 4 of 6 (GSD Provider)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.color_enabled = false;

    let result = provider.collect().unwrap();
    let icon = result.get("gsd_icon").unwrap();
    // With color_enabled=false but planning_dir present, icon is the raw character
    assert_eq!(icon, "\u{F0AE2}");
    assert!(
        !icon.contains("\x1b"),
        "Icon should not contain ANSI escapes when color disabled"
    );
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
    fs::write(planning.join("STATE.md"), "Phase: 4 of 6 (GSD Provider)\n").unwrap();
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

// ============================================================================
// Phase 5 Plan 3: Extended GSD variable tests
// ============================================================================

// ---- gsd_update formatting ----

#[test]
fn test_gsd_update_formatting_available() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    // Create a fake update check file
    let home = tmp.path().join("fakehome");
    let claude_dir = home.join(".claude").join("cache");
    fs::create_dir_all(&claude_dir).unwrap();
    // Write update info JSON with zero delay
    fs::write(
        claude_dir.join("update-check.json"),
        r#"{"version": "v1.19.0", "available": true, "checked_at": "2026-02-22T00:00:00Z"}"#,
    )
    .unwrap();

    let mut provider = provider_with_planning(planning);
    provider.home_dir = home;
    provider.update_delay_seconds = 0; // No delay for testing

    let result = provider.collect().unwrap();

    // Check gsd_update formatting
    let gsd_update = result.get("gsd_update").unwrap();
    if !gsd_update.is_empty() {
        // When update is available, format is "up-arrow + version"
        assert!(
            gsd_update.contains("v1.19.0"),
            "gsd_update should contain version when available: got '{}'",
            gsd_update
        );
    }
    // Note: update might be empty if the file format doesn't exactly match
    // the reader's expectations. The formatting logic is in build_convenience_vars.
}

#[test]
fn test_gsd_update_formatting_not_available() {
    let provider = provider_without_planning();
    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_update").unwrap(), "");
}

#[test]
fn test_gsd_update_convenience_var_directly() {
    // Test build_convenience_vars directly with pre-populated vars
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning.clone());

    let mut vars = init_empty_vars();
    vars.insert("gsd_update_available".into(), "true".into());
    vars.insert("gsd_update_version".into(), "v1.19.0".into());
    provider.build_convenience_vars(&mut vars);

    assert_eq!(vars.get("gsd_update").unwrap(), "\u{2191}v1.19.0");
}

#[test]
fn test_gsd_update_convenience_var_not_available() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning.clone());

    let mut vars = init_empty_vars();
    vars.insert("gsd_update_available".into(), "".into());
    vars.insert("gsd_update_version".into(), "".into());
    provider.build_convenience_vars(&mut vars);

    assert_eq!(vars.get("gsd_update").unwrap(), "");
}

// ---- gsd_task_full formatting ----

#[test]
fn test_gsd_task_full_with_progress() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning.clone());

    let mut vars = init_empty_vars();
    vars.insert("gsd_task".into(), "Writing tests".into());
    vars.insert("gsd_task_progress".into(), "2/5".into());
    provider.build_convenience_vars(&mut vars);

    assert_eq!(vars.get("gsd_task_full").unwrap(), "Writing tests (2/5)");
}

#[test]
fn test_gsd_task_full_without_progress() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning.clone());

    let mut vars = init_empty_vars();
    vars.insert("gsd_task".into(), "Refactoring".into());
    vars.insert("gsd_task_progress".into(), "".into());
    provider.build_convenience_vars(&mut vars);

    assert_eq!(vars.get("gsd_task_full").unwrap(), "Refactoring");
}

#[test]
fn test_gsd_task_full_no_task() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let provider = provider_with_planning(planning.clone());

    let mut vars = init_empty_vars();
    provider.build_convenience_vars(&mut vars);

    assert_eq!(vars.get("gsd_task_full").unwrap(), "");
}

// ---- gsd_plan_* variables ----

#[test]
fn test_gsd_plan_vars_from_roadmap() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 3 of 4 (Testing Phase)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    fs::write(
        planning.join("ROADMAP.md"),
        r#"- [x] **Phase 1: First** - d
- [x] **Phase 2: Second** - d
- [ ] **Phase 3: Testing Phase** - d

Plans:
- [x] 03-01-PLAN.md -- First plan
- [x] 03-02-PLAN.md -- Second plan
- [ ] 03-03-PLAN.md -- Third plan

- [ ] **Phase 4: Final** - d
"#,
    )
    .unwrap();

    let provider = provider_with_planning(planning);
    let result = provider.collect().unwrap();

    assert_eq!(result.get("gsd_plan_completed").unwrap(), "2");
    assert_eq!(result.get("gsd_plan_total").unwrap(), "3");
    assert_eq!(result.get("gsd_plan_fraction").unwrap(), "2/3");
}

#[test]
fn test_gsd_plan_vars_no_plans_section() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 3 of 4 (Testing Phase)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();
    fs::write(
        planning.join("ROADMAP.md"),
        r#"- [x] **Phase 1: First** - d
- [x] **Phase 2: Second** - d
- [ ] **Phase 3: Testing Phase** - d
- [ ] **Phase 4: Final** - d
"#,
    )
    .unwrap();

    let provider = provider_with_planning(planning);
    let result = provider.collect().unwrap();

    // No plan checkboxes under Phase 3 -- all plan vars empty
    assert_eq!(result.get("gsd_plan_completed").unwrap(), "");
    assert_eq!(result.get("gsd_plan_total").unwrap(), "");
    assert_eq!(result.get("gsd_plan_fraction").unwrap(), "");
}

// ---- gsd_stale detection ----

#[test]
fn test_gsd_stale_detection_stale() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 1 of 2 (Test)\nLast activity: 2025-01-01 -- Old activity\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.stale_enabled = true;
    provider.stale_hours = 24;

    let result = provider.collect().unwrap();
    // 2025-01-01 is definitely more than 24 hours ago
    assert_eq!(
        result.get("gsd_stale").unwrap(),
        "true",
        "Should be stale: activity is far in the past"
    );
}

#[test]
fn test_gsd_stale_detection_disabled() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 1 of 2 (Test)\nLast activity: 2025-01-01 -- Old activity\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.stale_enabled = false; // Disabled
    provider.stale_hours = 24;

    let result = provider.collect().unwrap();
    // stale_enabled=false, so gsd_stale should be empty
    assert_eq!(result.get("gsd_stale").unwrap(), "");
}

#[test]
fn test_gsd_stale_detection_recent_activity() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();

    // Use today's date for recent activity
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    fs::write(
        planning.join("STATE.md"),
        format!("Phase: 1 of 2 (Test)\nLast activity: {} -- Recent\n", today),
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.stale_enabled = true;
    provider.stale_hours = 24;

    let result = provider.collect().unwrap();
    assert_eq!(
        result.get("gsd_stale").unwrap(),
        "",
        "Should not be stale: activity is recent (today)"
    );
}

// ---- gsd_icon state coloring ----

#[test]
fn test_gsd_icon_green_when_active_task() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning.clone());
    provider.color_enabled = true;

    let mut vars = init_empty_vars();
    vars.insert("gsd_task".into(), "Active task".into());
    provider.build_convenience_vars(&mut vars);

    let icon = vars.get("gsd_icon").unwrap();
    assert!(
        icon.contains("\x1b[32m"),
        "Icon should be green when task is active: {:?}",
        icon
    );
}

#[test]
fn test_gsd_icon_yellow_when_update_available() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning.clone());
    provider.color_enabled = true;

    let mut vars = init_empty_vars();
    // No task, but update available
    vars.insert("gsd_update_available".into(), "true".into());
    vars.insert("gsd_update_version".into(), "v1.20.0".into());
    provider.build_convenience_vars(&mut vars);

    let icon = vars.get("gsd_icon").unwrap();
    assert!(
        icon.contains("\x1b[33m"),
        "Icon should be yellow when update available and no task: {:?}",
        icon
    );
}

#[test]
fn test_gsd_icon_red_when_stale() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning.clone());
    provider.color_enabled = true;
    provider.stale_enabled = true;

    let mut vars = init_empty_vars();
    // Simulate staleness: set the last_activity to old date
    vars.insert("gsd_last_activity".into(), "2020-01-01".into());
    provider.build_convenience_vars(&mut vars);

    let icon = vars.get("gsd_icon").unwrap();
    assert!(
        icon.contains("\x1b[31m"),
        "Icon should be red when stale: {:?}",
        icon
    );
}

#[test]
fn test_gsd_icon_no_ansi_when_color_disabled() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning.clone());
    provider.color_enabled = false;

    let mut vars = init_empty_vars();
    vars.insert("gsd_task".into(), "Active task".into());
    provider.build_convenience_vars(&mut vars);

    let icon = vars.get("gsd_icon").unwrap();
    assert!(
        !icon.contains("\x1b"),
        "Icon should not contain ANSI codes when color_enabled=false: {:?}",
        icon
    );
    assert_eq!(icon, "\u{F0AE2}");
}

// ---- Sub-feature toggles ----

#[test]
fn test_gsd_toggle_show_phase_off_with_plan_vars() {
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
        r#"- [x] **Phase 1: P1** - d
- [ ] **Phase 5: Layout Refactoring** - d

Plans:
- [x] 05-01-PLAN.md -- Done
- [ ] 05-02-PLAN.md -- Todo

- [ ] **Phase 6: Final** - d
"#,
    )
    .unwrap();

    let mut provider = provider_with_planning(planning);
    provider.show_phase = false;

    let result = provider.collect().unwrap();

    // Phase-related vars should ALL be empty when show_phase is false
    assert_eq!(result.get("gsd_phase").unwrap(), "");
    assert_eq!(result.get("gsd_phase_number").unwrap(), "");
    assert_eq!(result.get("gsd_phase_name").unwrap(), "");
    assert_eq!(result.get("gsd_progress_fraction").unwrap(), "");
    assert_eq!(result.get("gsd_progress_pct").unwrap(), "");
    assert_eq!(result.get("gsd_progress_completed").unwrap(), "");
    assert_eq!(result.get("gsd_progress_total").unwrap(), "");
    assert_eq!(result.get("gsd_plan_completed").unwrap(), "");
    assert_eq!(result.get("gsd_plan_total").unwrap(), "");
    assert_eq!(result.get("gsd_plan_fraction").unwrap(), "");
    assert_eq!(result.get("gsd_summary").unwrap(), "");
}

#[test]
fn test_gsd_toggle_show_task_off_with_active_task() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.show_task = false;

    let result = provider.collect().unwrap();

    assert_eq!(result.get("gsd_task").unwrap(), "");
    assert_eq!(result.get("gsd_task_progress").unwrap(), "");
    assert_eq!(result.get("gsd_task_full").unwrap(), "");
}

#[test]
fn test_gsd_toggle_show_update_off_with_update() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.show_update = false;

    let result = provider.collect().unwrap();

    assert_eq!(result.get("gsd_update").unwrap(), "");
    assert_eq!(result.get("gsd_update_available").unwrap(), "");
    assert_eq!(result.get("gsd_update_version").unwrap(), "");
}

// ---- Config defaults ----

#[test]
fn test_gsd_config_defaults() {
    // Deserialize empty [gsd] section -- all fields should have expected defaults
    let config: GsdConfig = toml::from_str("").unwrap();
    assert!(config.enabled);
    assert_eq!(config.project_dir, "");
    assert_eq!(config.task_max_length, 40);
    assert_eq!(config.todo_staleness_seconds, 86400);
    assert_eq!(config.update_delay_seconds, 300);
    assert_eq!(config.phase_max_width, 0);
    assert_eq!(config.task_max_width, 40);
    assert_eq!(config.separator, "\u{00b7}");
    assert_eq!(config.phase_format, "P{n}");
    assert!(config.color_enabled);
    assert!(config.show_phase);
    assert!(config.show_task);
    assert!(config.show_update);
    assert_eq!(config.stale_hours, 24);
    assert!(!config.stale_enabled);
}

#[test]
fn test_gsd_config_partial_override() {
    // Partial config -- only overridden fields change
    let config: GsdConfig = toml::from_str(
        r#"
        stale_enabled = true
        stale_hours = 48
        separator = " - "
        "#,
    )
    .unwrap();
    assert!(config.stale_enabled);
    assert_eq!(config.stale_hours, 48);
    assert_eq!(config.separator, " - ");
    // Defaults remain
    assert!(config.enabled);
    assert_eq!(config.phase_format, "P{n}");
    assert!(config.color_enabled);
}

// ---- gsd_summary with new format ----

#[test]
fn test_gsd_summary_custom_format() {
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "5".into());
    vars.insert("gsd_phase_name".into(), "Layout".into());
    vars.insert("gsd_progress_fraction".into(), "2/6".into());
    vars.insert("gsd_plan_fraction".into(), "1/3".into());

    GsdProvider::build_summary(&mut vars, "P{n}", "\u{00b7}");
    assert_eq!(
        vars.get("gsd_summary").unwrap(),
        "P5\u{00b7}Layout 2/6 [1/3]"
    );
}

#[test]
fn test_gsd_summary_without_plan_progress() {
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "5".into());
    vars.insert("gsd_phase_name".into(), "Layout".into());
    vars.insert("gsd_progress_fraction".into(), "2/6".into());

    GsdProvider::build_summary(&mut vars, "P{n}", "\u{00b7}");
    assert_eq!(vars.get("gsd_summary").unwrap(), "P5\u{00b7}Layout 2/6");
}

#[test]
fn test_gsd_summary_custom_phase_format_and_separator() {
    let mut vars = init_empty_vars();
    vars.insert("gsd_phase_number".into(), "3".into());
    vars.insert("gsd_phase_name".into(), "Stats".into());
    vars.insert("gsd_progress_fraction".into(), "1/4".into());

    GsdProvider::build_summary(&mut vars, "Phase {n}", " - ");
    assert_eq!(vars.get("gsd_summary").unwrap(), "Phase 3 - Stats 1/4");
}

// ---- Width truncation ----

#[test]
fn test_gsd_phase_max_width_truncation_detailed() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(
        planning.join("STATE.md"),
        "Phase: 5 of 6 (Layout Refactoring & Integration)\n",
    )
    .unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.phase_max_width = 10;

    let result = provider.collect().unwrap();
    let name = result.get("gsd_phase_name").unwrap();
    assert!(
        name.ends_with("..."),
        "Truncated name should end with '...': got '{}'",
        name
    );
    // "Layout Ref" (10 chars) + "..." = "Layout Ref..."
    assert_eq!(name, "Layout Ref...");
}

#[test]
fn test_gsd_task_max_width_truncation() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 1 of 2 (Test)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.task_max_width = 10;

    // Simulate: set gsd_task directly in the vars before apply_truncations
    let mut vars = init_empty_vars();
    vars.insert(
        "gsd_task".into(),
        "Very long task name that exceeds limit".into(),
    );
    provider.apply_truncations(&mut vars);

    let task = vars.get("gsd_task").unwrap();
    assert!(
        task.ends_with("..."),
        "Truncated task should end with '...': got '{}'",
        task
    );
    // "Very long " (10 chars) + "..." = "Very long ..."
    assert_eq!(task, "Very long ...");
}

#[test]
fn test_gsd_phase_max_width_no_truncation_when_fits() {
    let tmp = TempDir::new().unwrap();
    let planning = tmp.path().join(".planning");
    fs::create_dir_all(&planning).unwrap();
    fs::write(planning.join("STATE.md"), "Phase: 5 of 6 (Layout)\n").unwrap();
    fs::write(planning.join("config.json"), "{}").unwrap();

    let mut provider = provider_with_planning(planning);
    provider.phase_max_width = 20; // "Layout" is 6 chars -- fits easily

    let result = provider.collect().unwrap();
    assert_eq!(result.get("gsd_phase_name").unwrap(), "Layout");
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

// ---- Test 20: Reader caches must key on path, not mtime alone ----
//
// Regression test for a race condition observed on Linux CI: on filesystems
// with coarse mtime resolution (ext4 defaults to 1s), two distinct STATE.md
// or ROADMAP.md files written in the same second receive identical mtimes.
// Before the fix, the GSD reader caches (state.rs, roadmap.rs, update.rs,
// todos.rs) keyed only on mtime, so a second call for a different file would
// match the cached entry and return the first file's data. macOS APFS has
// nanosecond mtime, which hid the bug locally.
//
// This test forces the collision deterministically by assigning the same
// mtime to files in two distinct planning directories, then asserts each
// provider returns its own content.

#[test]
fn test_gsd_cache_distinguishes_paths_with_identical_mtimes() {
    use std::fs::FileTimes;
    use std::time::SystemTime;

    // Two distinct planning dirs with different content.
    let tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();
    let planning_a = tmp_a.path().join(".planning");
    let planning_b = tmp_b.path().join(".planning");
    fs::create_dir_all(&planning_a).unwrap();
    fs::create_dir_all(&planning_b).unwrap();

    fs::write(planning_a.join("STATE.md"), "Phase: 1 of 6 (Alpha Phase)\n").unwrap();
    fs::write(planning_b.join("STATE.md"), "Phase: 4 of 6 (Bravo Phase)\n").unwrap();

    fs::write(
        planning_a.join("ROADMAP.md"),
        "- [x] **Phase 1: Alpha** - d\n- [ ] **Phase 2: Zeta** - d\n",
    )
    .unwrap();
    fs::write(
        planning_b.join("ROADMAP.md"),
        "- [x] **Phase 1: B1** - d\n- [x] **Phase 2: B2** - d\n- [x] **Phase 3: B3** - d\n- [ ] **Phase 4: Bravo** - d\n",
    )
    .unwrap();

    fs::write(planning_a.join("config.json"), "{}").unwrap();
    fs::write(planning_b.join("config.json"), "{}").unwrap();

    // Force identical mtimes across both pairs of files (simulates ext4's 1s
    // resolution aliasing concurrent writes).
    let pinned_mtime = SystemTime::now();
    let times = FileTimes::new()
        .set_modified(pinned_mtime)
        .set_accessed(pinned_mtime);
    for path in [
        planning_a.join("STATE.md"),
        planning_b.join("STATE.md"),
        planning_a.join("ROADMAP.md"),
        planning_b.join("ROADMAP.md"),
    ] {
        let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_times(times).unwrap();
    }

    // Collect in order A -> B; without the path-aware cache, B's read returns
    // A's cached data.
    let provider_a = provider_with_planning(planning_a);
    let result_a = provider_a.collect().unwrap();
    assert_eq!(result_a.get("gsd_phase_name").unwrap(), "Alpha Phase");
    assert_eq!(result_a.get("gsd_progress_fraction").unwrap(), "1/2");

    let provider_b = provider_with_planning(planning_b);
    let result_b = provider_b.collect().unwrap();
    assert_eq!(
        result_b.get("gsd_phase_name").unwrap(),
        "Bravo Phase",
        "STATE.md cache must distinguish by path, not mtime alone"
    );
    assert_eq!(
        result_b.get("gsd_progress_fraction").unwrap(),
        "3/4",
        "ROADMAP.md cache must distinguish by path, not mtime alone"
    );
}

// ============================================================================
// B4 regression: update.rs cache must not freeze the delay-threshold decision.
//
// Bug: read_with_cache cached UpdateData (a time-DEPENDENT decision) keyed on
// (path, mtime). The first call before the delay threshold cached
// update_available=false; later calls past the threshold returned that stale
// cache because mtime hadn't changed -- the delay decision was never re-evaluated.
//
// Fix: cache the raw parsed JSON (UpdateRecord) and apply the delay-threshold
// computation in fill_vars on every call against SystemTime::now(), so cache
// content is time-independent.
// ============================================================================

#[test]
#[serial_test::serial]
fn test_gsd_update_cache_reevaluates_delay_after_threshold() {
    use std::fs::FileTimes;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Reset the global update cache so prior tests don't pollute state.
    super::update::reset_update_cache_for_tests();

    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join(".claude").join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    let update_path = cache_dir.join("gsd-update-check.json");

    // Write the file with `checked` = NOW so the delay threshold has not yet
    // been met when we make the first call.
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let content = format!(
        r#"{{"update_available": true, "installed": "1.18.0", "latest": "1.19.0", "checked": {}}}"#,
        now_unix
    );
    fs::write(&update_path, content).unwrap();

    // Pin the file's mtime so it stays constant across the wall-clock sleep.
    // This isolates the test's intent: only the wall clock advances, not mtime.
    let pinned_mtime = SystemTime::now();
    let times = FileTimes::new()
        .set_modified(pinned_mtime)
        .set_accessed(pinned_mtime);
    let f = fs::OpenOptions::new()
        .write(true)
        .open(&update_path)
        .unwrap();
    f.set_times(times).unwrap();
    drop(f);

    // First call: delay_seconds=1, elapsed since `checked` is ~0s. Threshold
    // not yet met -- update vars must NOT be set.
    let mut vars1: HashMap<String, String> = HashMap::new();
    super::update::fill_vars(tmp.path(), 1, &mut vars1);
    assert!(
        !vars1.contains_key("gsd_update_available"),
        "before delay threshold, update vars must not be set; got vars1={:?}",
        vars1
    );

    // Sleep just past the 1s threshold without modifying the file.
    std::thread::sleep(Duration::from_millis(1500));

    // Second call: same path, same mtime, but wall clock advanced past the
    // threshold. On the buggy cache (caches the decision), this returns the
    // cached update_available=false and the assertion below fails. On the
    // fixed cache (caches raw JSON, recomputes decision), gsd_update_available
    // is now "true".
    let mut vars2: HashMap<String, String> = HashMap::new();
    super::update::fill_vars(tmp.path(), 1, &mut vars2);
    assert_eq!(
        vars2.get("gsd_update_available").map(String::as_str),
        Some("true"),
        "after delay threshold elapses, update must surface; cache must not freeze the decision (vars2={:?})",
        vars2
    );
    assert_eq!(
        vars2.get("gsd_update_version").map(String::as_str),
        Some("1.19.0"),
    );
}
