//! Integration tests for `scripts/changelog-section.sh` (Phase 19 D-14).
//!
//! The release workflow builds each GitHub Release body from the tagged
//! version's CHANGELOG.md section. These tests prove the extraction locally,
//! with no tag or workflow run, and run inside `make test` isolation via
//! `test_support::init()`.

#![cfg(unix)]

mod test_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/changelog-section.sh")
}

fn run(args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script_path())
        .args(args)
        .output()
        .expect("failed to run scripts/changelog-section.sh")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write_fixture(dir: &TempDir, body: &str) -> PathBuf {
    let path = dir.path().join("CHANGELOG.md");
    fs::write(&path, body).expect("failed to write fixture changelog");
    path
}

#[test]
fn current_version_section_is_extracted() {
    let _guard = test_support::init();
    let version = include_str!("../VERSION").trim();
    let output = run(&[version]);
    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output);
    let text = stdout(&output);
    assert!(!text.trim().is_empty(), "section for {version} is empty");
    assert!(
        !text.lines().any(|l| l.starts_with("## [")),
        "output must not include any version heading"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        !lines.first().unwrap().trim().is_empty(),
        "leading blank line"
    );
    assert!(
        !lines.last().unwrap().trim().is_empty(),
        "trailing blank line"
    );
    assert!(text.contains("### "), "section has no ### subsections");
}

/// Pinned to 3.4.0: documents that the 3.4.0 release carries the PR #76 credit.
#[test]
fn v3_4_0_section_credits_contributor() {
    let _guard = test_support::init();
    let output = run(&["3.4.0"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("Piotr Wąsiewicz (@pwasiewi)"));
    assert!(text.contains("### Added"));
    assert!(text.contains("### Fixed"));
    assert!(
        !text.contains("Cost Accuracy & Honesty"),
        "3.4.0 section leaked into 3.3.0"
    );
}

#[test]
fn v3_3_0_section_is_bounded() {
    let _guard = test_support::init();
    let output = run(&["3.3.0"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("Cost Accuracy & Honesty"));
    assert!(!text.contains("@pwasiewi"));
    assert!(!text.contains("## [3.2.0]"));
    assert!(!text.contains("## [3.3.0]"));
}

#[test]
fn unknown_version_fails() {
    let _guard = test_support::init();
    let output = run(&["9.9.9"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn empty_unreleased_section_fails() {
    let _guard = test_support::init();
    let output = run(&["Unreleased"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn version_dot_is_literal() {
    let _guard = test_support::init();
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "# Changelog\n\n## [Unreleased]\n\n## [3x4x0] - 2026-01-01\n\nwrong\n\n\
         ## [3.4.0] - 2026-01-02\n\nright\n\n## [3.3.0] - 2025-12-01\n\nolder\n",
    );
    let output = run(&["3.4.0", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("right"));
    assert!(!text.contains("wrong"));
    assert!(!text.contains("older"));

    let dir2 = TempDir::new().unwrap();
    let path2 = write_fixture(&dir2, "# Changelog\n\n## [3x4x0] - 2026-01-01\n\nwrong\n");
    let output2 = run(&["3.4.0", path2.to_str().unwrap()]);
    assert_eq!(output2.status.code(), Some(1));
}

#[test]
fn provisional_or_final_date_both_match() {
    let _guard = test_support::init();
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "# Changelog\n\n## [1.2.3] - 2099-12-31\n\n### Added\n- thing\n",
    );
    let output = run(&["1.2.3", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("- thing"));
}

#[test]
fn usage_errors_exit_2() {
    let _guard = test_support::init();
    assert_eq!(run(&[]).status.code(), Some(2), "no arguments");
    assert_eq!(run(&["3.4.0;id"]).status.code(), Some(2), "bad version");
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("does-not-exist.md");
    assert_eq!(
        run(&["3.4.0", missing.to_str().unwrap()]).status.code(),
        Some(2),
        "unreadable changelog"
    );
}

#[test]
fn script_is_executable() {
    let _guard = test_support::init();
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(script_path())
        .expect("script missing")
        .permissions()
        .mode();
    assert!(
        mode & 0o111 != 0,
        "scripts/changelog-section.sh must be executable"
    );
}
