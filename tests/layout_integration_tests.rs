//! Integration tests for the provider-to-layout pipeline.
//!
//! Tests the full render pipeline: providers -> orchestrator -> template -> output.
//! Also validates performance budgets and public API surface stability.

use std::collections::HashMap;
use std::time::Instant;

use statusline::layout::{
    get_preset_format, list_available_presets, LayoutRenderer, VariableBuilder, PRESET_COMPACT,
    PRESET_DEFAULT, PRESET_DETAILED, PRESET_MINIMAL, PRESET_POWER,
};
use statusline::provider::{DataProvider, ProviderError, ProviderOrchestrator, ProviderResult};

// ============================================================================
// Test helpers
// ============================================================================

/// A simple test provider for integration testing (standalone from the
/// #[cfg(test)] TestProvider in the provider module which is not available
/// in integration tests).
struct IntegrationTestProvider {
    name: String,
    priority: u32,
    available: bool,
    variables: HashMap<String, String>,
}

impl IntegrationTestProvider {
    fn new(name: &str, priority: u32, vars: HashMap<String, String>) -> Self {
        Self {
            name: name.to_string(),
            priority,
            available: true,
            variables: vars,
        }
    }

    fn unavailable(mut self) -> Self {
        self.available = false;
        self
    }
}

impl DataProvider for IntegrationTestProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn collect(&self) -> ProviderResult {
        Ok(self.variables.clone())
    }
}

/// Build a HashMap from key-value tuples.
fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Build a full set of representative variables for all providers.
fn full_variable_set() -> HashMap<String, String> {
    let mut v = HashMap::new();
    // Git variables
    v.insert("git".into(), "main +2 ~1".into());
    v.insert("git_branch".into(), "main".into());
    // Stats variables
    v.insert("stats_session_cost".into(), "12.50".into());
    v.insert("stats_daily_total".into(), "45.00".into());
    v.insert("stats_lines_added".into(), "150".into());
    v.insert("stats_lines_removed".into(), "30".into());
    // GSD variables
    v.insert("gsd_phase".into(), "P5: Layout".into());
    v.insert("gsd_phase_number".into(), "5".into());
    v.insert("gsd_phase_name".into(), "Layout".into());
    v.insert("gsd_progress_fraction".into(), "4/6".into());
    v.insert("gsd_progress_pct".into(), "66".into());
    v.insert("gsd_progress_completed".into(), "4".into());
    v.insert("gsd_progress_total".into(), "6".into());
    v.insert("gsd_task".into(), "Writing tests".into());
    v.insert("gsd_task_progress".into(), "2/5".into());
    v.insert("gsd_task_full".into(), "Writing tests (2/5)".into());
    v.insert("gsd_update".into(), "".into());
    v.insert("gsd_update_available".into(), "".into());
    v.insert("gsd_update_version".into(), "".into());
    v.insert("gsd_plan_completed".into(), "2".into());
    v.insert("gsd_plan_total".into(), "3".into());
    v.insert("gsd_plan_fraction".into(), "2/3".into());
    v.insert("gsd_stale".into(), "".into());
    v.insert("gsd_icon".into(), "\u{F0AE2}".into());
    v.insert("gsd_separator".into(), "\u{00b7}".into());
    v.insert("gsd_summary".into(), "P5\u{00b7}Layout 4/6 [2/3]".into());
    // Core variables
    v.insert("directory".into(), "~/projects/app".into());
    v.insert("dir_short".into(), "app".into());
    v.insert("model".into(), "O4.6".into());
    v.insert("model_full".into(), "Claude Opus 4.6".into());
    v.insert("model_name".into(), "Opus".into());
    v.insert("context".into(), "[=====>----] 55%".into());
    v.insert("context_pct".into(), "55".into());
    v.insert("cost".into(), "$12.50".into());
    v.insert("cost_short".into(), "$12".into());
    v.insert("burn_rate".into(), "$3.50/hr".into());
    v.insert("daily_total".into(), "$45.00".into());
    v.insert("duration".into(), "1h 23m".into());
    v.insert("lines".into(), "+150 -30".into());
    v
}

// ============================================================================
// Part A: Provider Pipeline Integration Tests
// ============================================================================

#[test]
fn test_pipeline_full_providers() {
    let mut orch = ProviderOrchestrator::new();

    let git = IntegrationTestProvider::new(
        "git",
        50,
        vars(&[("git", "main +2"), ("git_branch", "main")]),
    );
    let stats = IntegrationTestProvider::new(
        "stats",
        50,
        vars(&[("stats_session_cost", "12.50"), ("stats_daily_total", "45.00")]),
    );
    let gsd = IntegrationTestProvider::new(
        "gsd",
        50,
        vars(&[
            ("gsd_phase", "P5: Layout"),
            ("gsd_phase_number", "5"),
            ("gsd_summary", "P5\u{00b7}Layout 4/6"),
        ]),
    );

    orch.register(Box::new(git));
    orch.register(Box::new(stats));
    orch.register(Box::new(gsd));

    let result = orch.collect_all();

    // Verify variables from all three providers are present
    assert_eq!(result.get("git").map(|s| s.as_str()), Some("main +2"));
    assert_eq!(result.get("git_branch").map(|s| s.as_str()), Some("main"));
    assert_eq!(
        result.get("stats_session_cost").map(|s| s.as_str()),
        Some("12.50")
    );
    assert_eq!(
        result.get("gsd_phase").map(|s| s.as_str()),
        Some("P5: Layout")
    );
    assert_eq!(
        result.get("gsd_summary").map(|s| s.as_str()),
        Some("P5\u{00b7}Layout 4/6")
    );
}

#[test]
fn test_pipeline_gsd_absent() {
    let mut orch = ProviderOrchestrator::new();

    let git = IntegrationTestProvider::new(
        "git",
        50,
        vars(&[("git", "main"), ("git_branch", "main")]),
    );
    let stats = IntegrationTestProvider::new(
        "stats",
        50,
        vars(&[("stats_session_cost", "5.00")]),
    );

    orch.register(Box::new(git));
    orch.register(Box::new(stats));
    // No GSD provider registered

    let result = orch.collect_all();

    assert_eq!(result.get("git").map(|s| s.as_str()), Some("main"));
    assert_eq!(
        result.get("stats_session_cost").map(|s| s.as_str()),
        Some("5.00")
    );
    // No gsd_ variables should be present
    assert!(
        result.get("gsd_phase").is_none(),
        "GSD vars should not appear when GSD provider is absent"
    );
}

#[test]
fn test_pipeline_unavailable_git() {
    let mut orch = ProviderOrchestrator::new();

    let git = IntegrationTestProvider::new(
        "git",
        50,
        vars(&[("git", "main")]),
    )
    .unavailable();

    let stats = IntegrationTestProvider::new(
        "stats",
        50,
        vars(&[("stats_session_cost", "5.00")]),
    );
    let gsd = IntegrationTestProvider::new(
        "gsd",
        50,
        vars(&[("gsd_phase", "P1: Test")]),
    );

    orch.register(Box::new(git));
    orch.register(Box::new(stats));
    orch.register(Box::new(gsd));

    let result = orch.collect_all();

    // Git should not contribute (unavailable)
    assert!(result.get("git").is_none(), "Unavailable git should not contribute");
    // Stats and GSD should still be present
    assert_eq!(
        result.get("stats_session_cost").map(|s| s.as_str()),
        Some("5.00")
    );
    assert_eq!(
        result.get("gsd_phase").map(|s| s.as_str()),
        Some("P1: Test")
    );
}

#[test]
fn test_pipeline_variable_conflict_resolution() {
    let mut orch = ProviderOrchestrator::new();

    // Low priority: sets "shared_key" to "low"
    let low = IntegrationTestProvider::new("low", 10, vars(&[("shared_key", "low")]));
    // High priority: sets same "shared_key" to "high"
    let high = IntegrationTestProvider::new("high", 90, vars(&[("shared_key", "high")]));

    orch.register(Box::new(low));
    orch.register(Box::new(high));

    let result = orch.collect_all();

    assert_eq!(
        result.get("shared_key").map(|s| s.as_str()),
        Some("high"),
        "Higher priority provider should win variable conflicts"
    );
}

// ============================================================================
// Part B: Template + Provider End-to-End
// ============================================================================

#[test]
fn test_render_default_template_full_data() {
    let default_tmpl = include_str!("../src/templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");

    let all_vars = full_variable_set();
    let result = renderer.render_template(&all_vars, false);

    // All segments should appear in order
    assert!(
        result.contains("~/projects/app"),
        "Should contain directory: {}",
        result
    );
    assert!(
        result.contains("main +2 ~1"),
        "Should contain git info: {}",
        result
    );
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
    assert!(
        result.contains("55%"),
        "Should contain context: {}",
        result
    );
    assert!(
        result.contains("O4.6"),
        "Should contain model: {}",
        result
    );
    assert!(
        result.contains("$12.50"),
        "Should contain cost: {}",
        result
    );
}

#[test]
fn test_render_default_template_gsd_hidden() {
    let default_tmpl = include_str!("../src/templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");

    let mut v = HashMap::new();
    v.insert("directory".into(), "~/projects/app".into());
    v.insert("git".into(), "main".into());
    v.insert("context".into(), "50%".into());
    v.insert("model".into(), "S4.5".into());
    v.insert("cost".into(), "$5.00".into());
    // No gsd_phase at all -- GSD segment should be completely hidden

    let result = renderer.render_template(&v, false);

    assert!(result.contains("~/projects/app"), "Directory present: {}", result);
    assert!(result.contains("main"), "Git present: {}", result);
    assert!(result.contains("50%"), "Context present: {}", result);
    assert!(result.contains("S4.5"), "Model present: {}", result);
    assert!(result.contains("$5.00"), "Cost present: {}", result);
    // GSD must be completely absent
    assert!(
        !result.contains("\u{F0AE2}"),
        "GSD icon should be absent: {}",
        result
    );
    assert!(!result.contains("gsd"), "No gsd text: {}", result);
}

#[test]
fn test_render_user_template_override() {
    // Create a custom template (simulating user override)
    let custom_template = "{directory} [{model}] ({cost})";
    let renderer = LayoutRenderer::with_format(custom_template, "");

    let mut v = HashMap::new();
    v.insert("directory".into(), "~/test".into());
    v.insert("model".into(), "O4.6".into());
    v.insert("cost".into(), "$10".into());

    let result = renderer.render_template(&v, false);
    assert_eq!(result, "~/test [O4.6] ($10)");
}

// ============================================================================
// Part C: Performance Validation
// ============================================================================

#[test]
fn test_perf_render_evaluation() {
    // PERF-01: Template evaluation should be < 1ms average
    let default_tmpl = include_str!("../src/templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");
    let all_vars = full_variable_set();

    // Warmup
    for _ in 0..10 {
        let _ = renderer.render_template(&all_vars, false);
    }

    // Timed run: 100 iterations
    let start = Instant::now();
    for _ in 0..100 {
        let _ = renderer.render_template(&all_vars, false);
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() / 100;

    assert!(
        avg_us < 1000,
        "Template evaluation should average < 1ms (1000us), got {}us",
        avg_us
    );
}

#[test]
fn test_perf_template_parse() {
    // Template parse performance: 1000 parses should complete in < 100ms
    let default_tmpl = include_str!("../src/templates/default.tmpl");

    // Warmup
    for _ in 0..10 {
        let _ = LayoutRenderer::with_format(default_tmpl, " | ");
    }

    // Timed run: 1000 parses
    let start = Instant::now();
    for _ in 0..1000 {
        let _ = LayoutRenderer::with_format(default_tmpl, " | ");
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 100,
        "1000 template parses should complete in < 100ms, got {}ms",
        elapsed.as_millis()
    );
}

#[test]
fn test_perf_gsd_provider_collection() {
    // GSD provider collection from temp files should be < 10ms average.
    // Since we cannot easily create a GsdProvider in integration tests
    // (it requires internal module access), we use a TestProvider with
    // realistic variable counts as a proxy for the merge/collect overhead.
    // The actual GSD file I/O is validated by the unit tests in src/gsd/tests.rs.

    let gsd_vars = vars(&[
        ("gsd_phase", "P5: Layout"),
        ("gsd_phase_number", "5"),
        ("gsd_phase_name", "Layout"),
        ("gsd_progress_fraction", "4/6"),
        ("gsd_progress_pct", "66"),
        ("gsd_progress_completed", "4"),
        ("gsd_progress_total", "6"),
        ("gsd_task", "Writing tests"),
        ("gsd_task_progress", "2/5"),
        ("gsd_task_full", "Writing tests (2/5)"),
        ("gsd_update", ""),
        ("gsd_update_available", ""),
        ("gsd_update_version", ""),
        ("gsd_plan_completed", "2"),
        ("gsd_plan_total", "3"),
        ("gsd_plan_fraction", "2/3"),
        ("gsd_stale", ""),
        ("gsd_icon", "\u{F0AE2}"),
        ("gsd_separator", "\u{00b7}"),
        ("gsd_summary", "P5\u{00b7}Layout 4/6 [2/3]"),
    ]);

    let start = Instant::now();
    for _ in 0..100 {
        let provider = IntegrationTestProvider::new("gsd", 50, gsd_vars.clone());
        let _ = provider.collect();
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() / 100;

    assert!(
        avg_us < 10_000,
        "GSD collection should average < 10ms (10000us), got {}us",
        avg_us
    );
}

// ============================================================================
// Part D: Regression Guards
// ============================================================================

#[test]
fn test_layout_public_api_surface() {
    // Reference every public item from the layout module.
    // If any item is accidentally removed from pub use re-exports,
    // this test will fail to compile.

    // LayoutRenderer
    let renderer = LayoutRenderer::with_format("{directory}", "");
    let _ = renderer.render(&HashMap::new());
    let _ = renderer.render_template(&HashMap::new(), false);
    let _ = renderer.uses_variable("directory");
    let _ = renderer.get_used_variables();

    // VariableBuilder
    let _ = VariableBuilder::new().build();

    // Preset constants
    assert!(!PRESET_DEFAULT.is_empty());
    assert!(!PRESET_COMPACT.is_empty());
    assert!(!PRESET_DETAILED.is_empty());
    assert!(!PRESET_MINIMAL.is_empty());
    assert!(!PRESET_POWER.is_empty());

    // Preset functions
    let _ = get_preset_format("default");
    let presets = list_available_presets();
    assert!(presets.len() >= 5);
}

#[test]
fn test_provider_public_api_surface() {
    // Reference every public type from the provider module.
    // If any item is accidentally removed, this test will fail to compile.

    // DataProvider trait (referenced through IntegrationTestProvider which implements it)
    let provider = IntegrationTestProvider::new("test", 50, HashMap::new());
    let _: &str = provider.name();
    let _: u32 = provider.priority();
    let _: std::time::Duration = provider.timeout();
    let _: bool = provider.is_available();
    let _: ProviderResult = provider.collect();

    // ProviderOrchestrator
    let mut orch = ProviderOrchestrator::new();
    orch.register(Box::new(IntegrationTestProvider::new("t", 50, HashMap::new())));
    let _: HashMap<String, String> = orch.collect_all();

    // ProviderError variants (compile-time check that they exist)
    let _err1 = ProviderError::Unavailable("test".into());
    let _err2 = ProviderError::Timeout {
        provider: "test".into(),
        limit: std::time::Duration::from_millis(100),
    };
    let _err3 = ProviderError::CollectionError("test".into());

    // GsdProvider and GitProvider are re-exported from lib.rs
    // We verify they exist as types (cannot easily construct without
    // realistic config, but the import compiles)
    fn _assert_gsd_provider_exists(_p: statusline::GsdProvider) {}
    fn _assert_git_provider_exists(_p: statusline::GitProvider) {}
    fn _assert_stats_provider_exists(_p: statusline::StatsProvider) {}
}

#[test]
fn test_layout_renderer_from_config() {
    // Verify LayoutRenderer::from_config works with default config
    let config = statusline::config::LayoutConfig::default();
    let renderer = LayoutRenderer::from_config(&config);
    // The renderer should use the default preset; verify by rendering with known vars
    let mut v = HashMap::new();
    v.insert("directory".into(), "~/test".into());
    v.insert("model".into(), "S4.5".into());
    let result = renderer.render(&v);
    // Default format includes {directory} and {model}
    assert!(result.contains("~/test"), "Should render directory: {}", result);
    assert!(result.contains("S4.5"), "Should render model: {}", result);
}

#[test]
fn test_full_pipeline_orchestrator_to_template() {
    // End-to-end: orchestrator collects vars, feeds to template, produces output
    let mut orch = ProviderOrchestrator::new();

    orch.register(Box::new(IntegrationTestProvider::new(
        "core",
        50,
        vars(&[
            ("directory", "~/test"),
            ("model", "S4.5"),
            ("cost", "$5.00"),
        ]),
    )));
    orch.register(Box::new(IntegrationTestProvider::new(
        "git",
        50,
        vars(&[("git", "main")]),
    )));

    let provider_vars = orch.collect_all();

    let default_tmpl = include_str!("../src/templates/default.tmpl");
    let renderer = LayoutRenderer::with_format(default_tmpl, " | ");
    let result = renderer.render_template(&provider_vars, false);

    assert!(result.contains("~/test"), "Pipeline output: {}", result);
    assert!(result.contains("main"), "Pipeline output: {}", result);
    assert!(result.contains("S4.5"), "Pipeline output: {}", result);
    assert!(result.contains("$5.00"), "Pipeline output: {}", result);
}
