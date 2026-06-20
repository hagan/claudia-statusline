//! Library API tests for embedding statusline in other tools

use statusline::{render_from_json, render_statusline, Model, StatuslineInput, Workspace};
use std::sync::Mutex;

// Mutex to prevent concurrent environment variable modifications
static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_statusline_basic() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Use actual home directory so path shortening works
    let home = std::env::var("HOME").unwrap_or("/tmp".to_string());
    let test_dir = format!("{}/project", home);

    let input = StatuslineInput {
        workspace: Some(Workspace {
            current_dir: Some(test_dir),
            repo: None,
        }),
        model: Some(Model {
            display_name: Some("Claude 3.5 Sonnet".to_string()),
            id: None,
        }),
        ..Default::default()
    };

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_statusline(&input, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.contains("~/project"));
    assert!(output.contains("S3.5"));

    std::env::remove_var("NO_COLOR");
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_from_json_basic() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let home = std::env::var("HOME").unwrap_or("/tmp".to_string());
    let json = format!(
        r#"{{
        "workspace": {{"current_dir": "{}/project"}},
        "model": {{"display_name": "Claude 3.5 Sonnet"}}
    }}"#,
        home
    );

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(&json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.contains("~/project"));
    assert!(output.contains("S3.5"));

    std::env::remove_var("NO_COLOR");
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_with_cost() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let json = r#"{
        "workspace": {"current_dir": "/home/user/project"},
        "model": {"display_name": "Claude 3.5 Sonnet"},
        "cost": {
            "total_cost_usd": 5.50,
            "total_lines_added": 100,
            "total_lines_removed": 50
        }
    }"#;

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.contains("$5.50"));
    assert!(output.contains("+100"));
    assert!(output.contains("-50"));

    std::env::remove_var("NO_COLOR");
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_without_stats_update() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // This test ensures we can render without updating stats
    let json = r#"{
        "workspace": {"current_dir": "/tmp/test"},
        "model": {"display_name": "Opus"},
        "session_id": "test-session-no-update",
        "cost": {"total_cost_usd": 1.0}
    }"#;

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    // Render without updating stats
    let result1 = render_from_json(json, false);
    assert!(result1.is_ok());

    // Render again - should not have updated stats
    let result2 = render_from_json(json, false);
    assert!(result2.is_ok());

    // Both results should be identical since no stats were updated
    assert_eq!(result1.unwrap(), result2.unwrap());

    std::env::remove_var("NO_COLOR");
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_with_git_repo() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Create a temporary git repo
    let temp_dir = tempfile::tempdir().unwrap();
    let repo_path = temp_dir.path().to_str().unwrap();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let input = StatuslineInput {
        workspace: Some(Workspace {
            current_dir: Some(repo_path.to_string()),
            repo: None,
        }),
        model: Some(Model {
            display_name: Some("Claude 3.5 Sonnet".to_string()),
            id: None,
        }),
        ..Default::default()
    };

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_statusline(&input, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    // Should show git branch (main or master)
    assert!(output.contains("main") || output.contains("master"));

    std::env::remove_var("NO_COLOR");
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_minimal_input() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let json = r#"{}"#;

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    // Should at least show home directory
    assert!(output.contains("~"));

    std::env::remove_var("NO_COLOR");
}

#[test]
#[serial_test::serial]
fn api_usage_renders_via_library() {
    // Proves the SINGLE display.rs api_usage wiring reaches the LIBRARY render path
    // (render_from_json): with `[ant]` enabled, STATUSLINE_ANT_ACCOUNT set, and a
    // per-account usage slice on disk, a custom layout template referencing
    // `{api_cost_today}`/`{api_account}` renders the cached values (Pitfall 6).
    let _lock = ENV_MUTEX.lock().unwrap();

    // Isolate the cache + config dirs under a tempdir HOME so the slice lands where
    // the production `read_usage_cache` (via `dirs::cache_dir()`) will read it.
    let home = tempfile::tempdir().unwrap();
    let orig_home = std::env::var_os("HOME");
    let orig_xdg_cache = std::env::var_os("XDG_CACHE_HOME");
    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));

    // Compute the cache dir the SAME way production does (dirs::cache_dir()), then
    // write a schema-v1 usage slice for account "work".
    let cache_root = dirs::cache_dir().expect("cache dir resolvable");
    let usage_dir = cache_root
        .join("claudia-statusline")
        .join("ant")
        .join("usage");
    std::fs::create_dir_all(&usage_dir).unwrap();
    let slice = r#"{
        "schema_version": 1,
        "fetched_at": "2026-06-14T00:00:00Z",
        "account": "work",
        "today_usd": 12.5,
        "mtd_usd": 340.0,
        "tz": "UTC",
        "tokens_by_model": {}
    }"#;
    std::fs::write(usage_dir.join("work.json"), slice).unwrap();

    // Config file: enable [ant] and use a custom layout referencing the api vars.
    let cfg = home.path().join("config.toml");
    std::fs::write(
        &cfg,
        "[ant]\nenabled = true\n\n[layout]\nformat = \"{api_cost_today} {api_account}\"\n",
    )
    .unwrap();

    let orig_cfg = std::env::var_os("STATUSLINE_CONFIG");
    let orig_acct = std::env::var_os("STATUSLINE_ANT_ACCOUNT");
    std::env::set_var("STATUSLINE_CONFIG", &cfg);
    std::env::set_var("STATUSLINE_ANT_ACCOUNT", "work");
    std::env::set_var("NO_COLOR", "1");
    statusline::config::reset_config();

    let json =
        r#"{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude 3.5 Sonnet"}}"#;
    let result = render_from_json(json, false);

    // Restore env before asserting so a failure cannot poison later serial tests.
    let restore = |key: &str, val: Option<std::ffi::OsString>| match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    };
    restore("HOME", orig_home);
    restore("XDG_CACHE_HOME", orig_xdg_cache);
    restore("STATUSLINE_CONFIG", orig_cfg);
    restore("STATUSLINE_ANT_ACCOUNT", orig_acct);
    std::env::remove_var("NO_COLOR");
    statusline::config::reset_config();

    let output = result.expect("render must succeed");
    assert!(
        output.contains("$12.50"),
        "library render must surface {{api_cost_today}} = $12.50, got: {output:?}"
    );
    assert!(
        output.contains("work"),
        "library render must surface {{api_account}} = work, got: {output:?}"
    );
}

#[test]
#[serial_test::serial]
fn api_age_vars_render_via_library() {
    // Proves the SINGLE display.rs api_age wiring reaches the LIBRARY render path:
    // with `[ant]` enabled, STATUSLINE_ANT_ACCOUNT set, a usage slice AND a models
    // cache on disk, a custom layout referencing `{api_usage_age}`/`{api_models_age}`
    // renders the humanized ages (Pitfall 6 / D-08/D-09).
    let _lock = ENV_MUTEX.lock().unwrap();

    let home = tempfile::tempdir().unwrap();
    let orig_home = std::env::var_os("HOME");
    let orig_xdg_cache = std::env::var_os("XDG_CACHE_HOME");
    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));

    // Compute the cache dir the SAME way production does (dirs::cache_dir()).
    let cache_root = dirs::cache_dir().expect("cache dir resolvable");
    let ant_dir = cache_root.join("claudia-statusline").join("ant");
    let usage_dir = ant_dir.join("usage");
    std::fs::create_dir_all(&usage_dir).unwrap();

    // A usage slice fetched ~10 minutes ago and a models cache ~2 hours ago, so the
    // humanized ages are deterministic ("10m" / "2h").
    let usage_when = (chrono::Utc::now() - chrono::Duration::seconds(600))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let models_when = (chrono::Utc::now() - chrono::Duration::seconds(7200))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let slice = format!(
        "{{\"schema_version\":1,\"fetched_at\":\"{usage_when}\",\"account\":\"work\",\
         \"today_usd\":1.0,\"mtd_usd\":2.0,\"tz\":\"UTC\",\"tokens_by_model\":{{}}}}"
    );
    std::fs::write(usage_dir.join("work.json"), slice).unwrap();
    let models = format!(
        "{{\"schema_version\":1,\"fetched_at\":\"{models_when}\",\
         \"models\":{{\"claude-x\":{{\"max_input_tokens\":200000}}}}}}"
    );
    std::fs::write(ant_dir.join("models.json"), models).unwrap();

    let cfg = home.path().join("config.toml");
    std::fs::write(
        &cfg,
        "[ant]\nenabled = true\n\n[layout]\nformat = \"{api_usage_age} {api_models_age}\"\n",
    )
    .unwrap();

    let orig_cfg = std::env::var_os("STATUSLINE_CONFIG");
    let orig_acct = std::env::var_os("STATUSLINE_ANT_ACCOUNT");
    std::env::set_var("STATUSLINE_CONFIG", &cfg);
    std::env::set_var("STATUSLINE_ANT_ACCOUNT", "work");
    std::env::set_var("NO_COLOR", "1");
    statusline::config::reset_config();

    let json =
        r#"{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude 3.5 Sonnet"}}"#;
    let result = render_from_json(json, false);

    let restore = |key: &str, val: Option<std::ffi::OsString>| match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    };
    restore("HOME", orig_home);
    restore("XDG_CACHE_HOME", orig_xdg_cache);
    restore("STATUSLINE_CONFIG", orig_cfg);
    restore("STATUSLINE_ANT_ACCOUNT", orig_acct);
    std::env::remove_var("NO_COLOR");
    statusline::config::reset_config();

    let output = result.expect("render must succeed");
    assert!(
        output.contains("10m"),
        "library render must surface {{api_usage_age}} = 10m, got: {output:?}"
    );
    assert!(
        output.contains("2h"),
        "library render must surface {{api_models_age}} = 2h, got: {output:?}"
    );
}

// ---------------------------------------------------------------------------
// Phase 10-02: api_equiv cost-from-tokens render-path tests
// ---------------------------------------------------------------------------

/// Render `json` through the LIBRARY path with a tempdir HOME and a custom config
/// TOML, restoring all mutated env afterwards. Returns the rendered string.
fn render_with_config(config_toml: &str, json: &str) -> String {
    // Recover from a poisoned lock: pre-existing suite tests can panic while
    // holding ENV_MUTEX, which would otherwise cascade-poison these tests. The
    // guard only protects env-var ordering, so a poisoned guard is still safe.
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    let home = tempfile::tempdir().unwrap();
    let orig_home = std::env::var_os("HOME");
    let orig_xdg_cache = std::env::var_os("XDG_CACHE_HOME");
    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));

    let cfg = home.path().join("config.toml");
    std::fs::write(&cfg, config_toml).unwrap();

    let orig_cfg = std::env::var_os("STATUSLINE_CONFIG");
    let orig_acct = std::env::var_os("STATUSLINE_ANT_ACCOUNT");
    std::env::set_var("STATUSLINE_CONFIG", &cfg);
    // Ensure no stray account leaks an ant usage slice into these tests.
    std::env::remove_var("STATUSLINE_ANT_ACCOUNT");
    std::env::set_var("NO_COLOR", "1");
    statusline::config::reset_config();

    let result = render_from_json(json, false);

    let restore = |key: &str, val: Option<std::ffi::OsString>| match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    };
    restore("HOME", orig_home);
    restore("XDG_CACHE_HOME", orig_xdg_cache);
    restore("STATUSLINE_CONFIG", orig_cfg);
    restore("STATUSLINE_ANT_ACCOUNT", orig_acct);
    std::env::remove_var("NO_COLOR");
    statusline::config::reset_config();

    result.expect("render must succeed")
}

/// A payload carrying `context_window.current_usage` token counts and a model id.
/// input 100k -> $1.50, output 10k -> $0.75, cache_creation 20k -> $0.37,
/// cache_read 200k -> $0.30, total = $2.92 with the bundled `claude-opus-4-8` row.
fn token_payload(model_id: &str) -> String {
    format!(
        r#"{{"workspace":{{"current_dir":"/tmp"}},"model":{{"id":"{model_id}"}},
        "context_window":{{"current_usage":{{
            "input_tokens":100000,"output_tokens":10000,
            "cache_creation_input_tokens":20000,"cache_read_input_tokens":200000}}}}}}"#
    )
}

#[test]
#[serial_test::serial]
fn api_equiv_cost_renders_dollar_via_library() {
    let out = render_with_config(
        "[layout]\nformat = \"{api_equiv_cost}\"\n",
        &token_payload("claude-opus-4-8"),
    );
    assert!(
        out.contains("$2.92"),
        "library render of {{api_equiv_cost}} must surface the additive total, got: {out:?}"
    );
}

#[test]
#[serial_test::serial]
fn api_equiv_cost_labeled_renders_marker_via_library() {
    let out = render_with_config(
        "[layout]\nformat = \"{api_equiv_cost_labeled}\"\n",
        &token_payload("claude-opus-4-8"),
    );
    assert!(
        out.contains("API-equiv") && out.contains("$2.92"),
        "{{api_equiv_cost_labeled}} must carry the API-equiv marker + figure, got: {out:?}"
    );
}

#[test]
#[serial_test::serial]
fn default_render_is_byte_identical_with_and_without_token_data() {
    // The DEFAULT layout references no api_equiv_* var. A token-bearing payload
    // and a token-free payload must render byte-identically (SC2): the new vars
    // never leak into the default output.
    let plain = r#"{"workspace":{"current_dir":"/tmp"},"model":{"id":"claude-opus-4-8"}}"#;
    let with_tokens = token_payload("claude-opus-4-8");

    // No [layout] section -> default layout.
    let out_plain = render_with_config("", plain);
    let out_tokens = render_with_config("", &with_tokens);

    assert_eq!(
        out_plain, out_tokens,
        "default render must be byte-identical regardless of token data (SC2)"
    );
    assert!(
        !out_tokens.contains("api_equiv") && !out_tokens.contains("API-equiv"),
        "default render must carry NO api_equiv output, got: {out_tokens:?}"
    );
}

#[test]
#[serial_test::serial]
fn unpriceable_model_renders_unknown_marker_via_library() {
    let out = render_with_config(
        "[layout]\nformat = \"{api_equiv_cost}\"\n",
        &token_payload("totally-unknown-model-xyz"),
    );
    assert!(
        out.contains("unknown") && !out.contains('$'),
        "an unpriceable model must render the literal `unknown` marker (SC5), got: {out:?}"
    );
}

#[test]
#[serial_test::serial]
fn pricing_independent_of_ant_disabled() {
    // With [ant].enabled = false AND {api_equiv_cost} referenced, a known model +
    // token data still renders the figure: the offline pricing path is INDEPENDENT
    // of the Admin-key [ant] subsystem (D-11).
    let out = render_with_config(
        "[ant]\nenabled = false\n\n[layout]\nformat = \"{api_equiv_cost}\"\n",
        &token_payload("claude-opus-4-8"),
    );
    assert!(
        out.contains("$2.92"),
        "pricing must render with [ant] disabled (D-11), got: {out:?}"
    );
}

#[test]
#[serial_test::serial]
fn pricing_aliases_resolve_through_render_path() {
    // A [pricing.aliases] mapping a proxy id -> a known table id, with a payload
    // whose model IS the proxy id, must render the aliased price through the REAL
    // render path (not only the Plan 01 lookup unit test).
    let out = render_with_config(
        "[pricing.aliases]\n\"my-proxy-opus\" = \"claude-opus-4-8\"\n\n\
         [layout]\nformat = \"{api_equiv_cost}\"\n",
        &token_payload("my-proxy-opus"),
    );
    assert!(
        out.contains("$2.92"),
        "a [pricing.aliases] proxy id must render the aliased price, got: {out:?}"
    );
}

#[test]
fn variables_rs_has_no_network_or_subprocess_tokens() {
    // Offline static-scan guard over the new render-vars path: the cost-from-tokens
    // builder must perform pure local arithmetic. Forbidden tokens are built by
    // concatenation so this assertion cannot self-match.
    let src = include_str!("../src/layout/variables.rs");
    let forbidden = [
        concat!("Comm", "and"),
        concat!("std::", "net"),
        concat!("req", "west"),
        concat!("ur", "eq"),
        concat!("cu", "rl"),
        concat!("tok", "io"),
        concat!("TcpStr", "eam"),
    ];
    for tok in forbidden {
        assert!(
            !src.contains(tok),
            "src/layout/variables.rs must not reference `{tok}` (offline invariant)"
        );
    }
}

#[test]
fn test_render_invalid_json() {
    let json = r#"{ invalid json }"#;

    let result = render_from_json(json, false);
    assert!(result.is_err());

    let error = result.unwrap_err();
    let error_msg = format!("{}", error);
    assert!(error_msg.contains("Failed to parse JSON"));
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_no_color_environment() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let json = r#"{
        "workspace": {"current_dir": "/home/user/project"},
        "model": {"display_name": "Claude 3.5 Sonnet"}
    }"#;

    // Test with NO_COLOR set
    std::env::set_var("NO_COLOR", "1");
    let result_no_color = render_from_json(json, false).unwrap();
    assert!(!result_no_color.contains("\x1b[")); // No ANSI codes

    // Test without NO_COLOR
    std::env::remove_var("NO_COLOR");
    let result_with_color = render_from_json(json, false).unwrap();
    assert!(result_with_color.contains("\x1b[")); // Has ANSI codes
}

#[test]
#[serial_test::serial] // Run serially to avoid NO_COLOR env var conflicts
fn test_render_with_context_usage() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Create a temporary transcript file
    let temp_dir = tempfile::tempdir().unwrap();
    let transcript_path = temp_dir.path().join("transcript.jsonl");

    // Write some sample JSONL content
    std::fs::write(&transcript_path, r#"{"message":{"role":"user","content":"Hello"},"timestamp":"2025-08-31T10:00:00.000Z"}
{"message":{"role":"assistant","content":"World","usage":{"input_tokens":5000,"output_tokens":1000,"cache_read_input_tokens":2000}},"timestamp":"2025-08-31T10:00:01.000Z"}"#).unwrap();

    let json = format!(
        r#"{{
        "workspace": {{"current_dir": "/home/user/project"}},
        "model": {{"display_name": "Claude 3.5 Sonnet"}},
        "transcript": "{}"
    }}"#,
        transcript_path.to_str().unwrap()
    );

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(&json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    println!("DEBUG: Context usage test output: '{}'", output);
    // Should show context usage percentage
    assert!(output.contains("%"));

    std::env::remove_var("NO_COLOR");
}
