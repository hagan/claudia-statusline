//! Regression tests for the v3.0.0 "ignored legacy" treatment of `json_backup = true`.
//!
//! Phase 6 (Option A, warn-and-continue — see 06-CONTEXT.md D-01/D-02/D-16) removed
//! the JSON backup *write* surface and replaced the originally-planned hard-error guard
//! with a one-line stderr deprecation note. These tests assert the warn-and-continue
//! contract:
//!   - a config with `json_backup = true` exits 0 (NOT a hard error),
//!   - the deprecation phrase appears on stderr exactly once,
//!   - the rendered statusline (stdout) is still produced,
//!   - configs without `json_backup = true` produce no deprecation note,
//!   - stdout is byte-for-byte identical for json_backup=true vs json_backup=false.
//!
//! The binary is spawned via `std::process::Command` + a local `get_test_binary()`
//! helper (the same pattern as `tests/sqlite_integration_tests.rs`). This test does
//! NOT depend on `assert_cmd` (it is not a dev-dependency of this project).

#[path = "test_support.rs"]
mod test_support;

use std::io::Write;
use tempfile::TempDir;

/// Run the statusline binary with an isolated XDG_CONFIG_HOME/XDG_DATA_HOME containing
/// the given config.toml body, feeding `{}` on stdin. Returns the captured output.
fn run_with_config(config_body: &str) -> std::process::Output {
    let temp_dir = TempDir::new().unwrap();
    let config_app_dir = temp_dir.path().join("claudia-statusline");
    std::fs::create_dir_all(&config_app_dir).unwrap();
    std::fs::write(config_app_dir.join("config.toml"), config_body).unwrap();

    let mut child = std::process::Command::new(test_support::test_binary())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.as_mut().unwrap().write_all(b"{}").unwrap();

    child.wait_with_output().unwrap()
}

#[test]
fn json_backup_true_in_config_warns_and_continues() {
    let output = run_with_config("[database]\njson_backup = true\n");

    assert!(
        output.status.success(),
        "binary must exit 0 with json_backup = true (warn-and-continue, NOT hard error); status was {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let count = stderr.matches("'json_backup' is ignored in v3.0.0").count();
    assert_eq!(
        count, 1,
        "deprecation phrase must appear on stderr exactly once; saw {} occurrence(s).\nstderr: {}",
        count, stderr
    );

    assert!(
        stderr.contains("MIGRATION_GUIDE.md"),
        "stderr must reference MIGRATION_GUIDE.md.\nstderr: {}",
        stderr
    );

    assert!(
        !output.stdout.is_empty(),
        "render must produce stdout output (proving render proceeded normally)"
    );
}

#[test]
fn json_backup_false_in_config_no_warning() {
    let output = run_with_config("[database]\njson_backup = false\n");

    assert!(
        output.status.success(),
        "binary must exit 0 with json_backup = false"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("'json_backup' is ignored"),
        "no deprecation note expected when json_backup = false.\nstderr: {}",
        stderr
    );
}

#[test]
fn json_backup_absent_no_warning() {
    // A config with no [database] json_backup line at all.
    let output = run_with_config("[display]\ntheme = \"dark\"\n");

    assert!(
        output.status.success(),
        "binary must exit 0 when json_backup is absent"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("'json_backup' is ignored"),
        "no deprecation note expected when json_backup is absent.\nstderr: {}",
        stderr
    );
}

#[test]
fn json_backup_stdout_identical_true_vs_false() {
    // Upgrade-safety: stdout (the rendered statusline) must be byte-for-byte identical
    // whether json_backup is true or false. Only stderr differs.
    let out_true = run_with_config("[database]\njson_backup = true\n");
    let out_false = run_with_config("[database]\njson_backup = false\n");

    assert!(out_true.status.success() && out_false.status.success());
    assert_eq!(
        out_true.stdout, out_false.stdout,
        "stdout must be byte-for-byte identical for json_backup true vs false.\ntrue: {}\nfalse: {}",
        String::from_utf8_lossy(&out_true.stdout),
        String::from_utf8_lossy(&out_false.stdout)
    );
}
