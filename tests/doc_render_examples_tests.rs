//! Doc ↔ render drift guard for the context-bar and `{api_equiv_cost_labeled}`
//! examples in `README.md`, `docs/CONFIGURATION.md` and `docs/USAGE.md`
//! (phase 13, W-7). Sibling of `tests/doc_cost_examples_tests.rs`.
//!
//! Every example is rendered through the REAL binary under a fully isolated
//! environment (HOME + all three XDG dirs in a temp dir, `NO_COLOR=1`), and the
//! docs are asserted to show exactly what the binary prints.
#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// The docs that carry rendered examples.
const DOCS: &[&str] = &["README.md", "docs/CONFIGURATION.md", "docs/USAGE.md"];

fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn assert_doc_contains(rel: &str, needle: &str) {
    assert!(
        repo_file(rel).contains(needle),
        "{rel} must show the real render {needle:?}"
    );
}

/// An isolated environment for renders.
struct RenderEnv {
    home: TempDir,
    config_path: PathBuf,
}

impl RenderEnv {
    fn new(config_body: &str) -> RenderEnv {
        let home = TempDir::new().expect("home temp dir");
        let config_path = home.path().join("statusline.toml");
        fs::write(&config_path, config_body).expect("write config");
        RenderEnv { home, config_path }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    /// Render `payload`; returns trimmed stdout. `path` is the child's PATH (an
    /// empty dir unless the example needs `git`). Panics on a non-zero exit so a
    /// broken config never masquerades as a doc mismatch.
    fn render(&self, payload: &serde_json::Value, path: &std::ffi::OsStr) -> String {
        let home = self.home.path();
        let mut child = Command::new(test_support::test_binary())
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("STATUSLINE_ANT_ACCOUNT")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn statusline");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.to_string().as_bytes())
            .expect("write payload");
        let out = child.wait_with_output().expect("wait statusline");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "statusline exited {:?}; stdout={stdout:?} stderr={:?}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        stdout.trim().to_string()
    }

    /// Render a context payload at `pct`% of a 200k window (no git on PATH).
    fn render_context(&self, pct: u64) -> String {
        let empty_bin = self.home.path().join("bin");
        fs::create_dir_all(&empty_bin).expect("bin dir");
        let payload = serde_json::json!({
            "workspace": { "current_dir": self.home.path().display().to_string() },
            "model": { "id": "claude-sonnet-4-5" },
            "context_window": {
                "total_input_tokens": pct * 2_000,
                "context_window_size": 200_000,
                "used_percentage": pct
            },
            "cost": {
                "total_cost_usd": 12.5,
                "total_duration_ms": 12_857_000,
                "total_lines_added": 123,
                "total_lines_removed": 45
            }
        });
        self.render(&payload, empty_bin.as_os_str())
    }
}

/// Every `NN% [bar]` in the docs shows the bar the binary really draws for NN%.
#[test]
#[serial]
fn every_documented_context_bar_matches_render() {
    let pattern = regex::Regex::new(r"(\d{1,3})% (\[[=>-]+\])").expect("regex");
    let env = RenderEnv::new("[layout]\nformat = \"{context}\"\n");
    let mut checked = 0;
    for rel in DOCS {
        let doc = repo_file(rel);
        for caps in pattern.captures_iter(&doc) {
            let pct: u64 = caps[1].parse().expect("percentage");
            let documented = &caps[0];
            let rendered = env.render_context(pct);
            assert!(
                rendered.starts_with(documented),
                "{rel}: documented {documented:?}, but the binary renders {rendered:?} for {pct}%"
            );
            checked += 1;
        }
    }
    assert!(checked >= 10, "the scan found only {checked} context bars");
}

/// The built-in `default` renderer appends `⚠` at 75% (the auto-compact
/// warning threshold); the preset tables must show it.
#[test]
#[serial]
fn default_preset_row_shows_the_warning_marker() {
    let rendered = RenderEnv::new("").render_context(75);
    // The payload also carries line counts (for the developer-focus example),
    // which the default renderer shows between model and cost; the doc rows
    // omit them, so pin the two halves around them.
    assert!(
        rendered.contains(" • 75% [========>-] ⚠ • S4.5 • ")
            && rendered.ends_with(" • $12.50 ($3.50/hr)"),
        "default render changed: {rendered:?}"
    );
    let expected = "75% [========>-] ⚠ • S4.5 • $12.50 ($3.50/hr)";
    assert_doc_contains("README.md", &format!("• main +2 • {expected}`"));
    assert_doc_contains("docs/CONFIGURATION.md", &format!("• main +2 • {expected}`"));
}

/// The layout `{context}` (default `show_tokens = true`) and its format options.
#[test]
#[serial]
fn context_variable_rows_match_render() {
    let full = RenderEnv::new("[layout]\nformat = \"{context}\"\n").render_context(75);
    assert_eq!(full, "75% [========>-] 150k/200k");
    assert_doc_contains(
        "docs/CONFIGURATION.md",
        &format!("| `{{context}}` | `{full}` |"),
    );
    assert_doc_contains("docs/CONFIGURATION.md", &format!("| `full` | `{full}` |"));

    let detailed = RenderEnv::new("[layout]\npreset = \"detailed\"\n").render_context(75);
    let line2 = detailed.lines().nth(1).unwrap_or_default();
    assert!(
        line2.starts_with("75% [========>-] 150k/200k • S4.5 • "),
        "detailed render changed: {detailed:?}"
    );
    assert_doc_contains(
        "docs/CONFIGURATION.md",
        "`75% [========>-] 150k/200k • S4.5 • ",
    );

    let no_tokens = RenderEnv::new(
        "[layout]\nformat = \"{context}\"\n\n[layout.components.context]\nshow_tokens = false\n",
    )
    .render_context(75);
    assert_doc_contains(
        "docs/CONFIGURATION.md",
        &format!("| `full` + `show_tokens = false` | `{no_tokens}` |"),
    );

    let bar = RenderEnv::new(
        "[layout]\nformat = \"{context}\"\n\n[layout.components.context]\nformat = \"bar\"\n",
    )
    .render_context(75);
    assert_doc_contains("docs/CONFIGURATION.md", &format!("| `bar` | `{bar}` |"));
}

/// The "Developer Focus" `[display]` example's context + lines segment.
#[test]
#[serial]
fn developer_focus_example_matches_render() {
    let rendered = RenderEnv::new(
        "[display]\nshow_directory = true\nshow_git = true\nshow_context = true\n\
         show_model = false\nshow_duration = false\nshow_lines_changed = true\n\
         show_cost = false\n",
    )
    .render_context(42);
    let tail = "42% [====>-----] • +123 -45";
    assert!(
        rendered.ends_with(tail),
        "developer-focus render changed: {rendered:?}"
    );
    assert_doc_contains("docs/CONFIGURATION.md", &format!("~1 • {tail}`"));
}

/// The README `{api_equiv_cost_labeled}` example, rendered in a real git repo
/// at `~/projects/app` on `main` with two staged files.
#[test]
#[serial]
fn readme_api_equiv_example_matches_render() {
    let format = "{directory} {git} {model} {api_equiv_cost_labeled}";
    assert_doc_contains("README.md", &format!("format = \"{format}\""));
    let env = RenderEnv::new(&format!("[layout]\nformat = \"{format}\"\n"));

    let repo = env.home().join("projects").join("app");
    fs::create_dir_all(&repo).expect("repo dir");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(&repo)
            .env("HOME", env.home())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    fs::write(repo.join("a"), "a").expect("write a");
    fs::write(repo.join("b"), "b").expect("write b");
    git(&["add", "a", "b"]);

    // One realistic API call: a warm prompt cache, a small cache write and a
    // short response.
    let payload = serde_json::json!({
        "workspace": { "current_dir": repo.display().to_string() },
        "model": { "id": "claude-opus-4-8", "display_name": "Opus 4.8" },
        "context_window": {
            "total_input_tokens": 123_050,
            "context_window_size": 200_000,
            "current_usage": {
                "input_tokens": 50,
                "output_tokens": 800,
                "cache_creation_input_tokens": 3_000,
                "cache_read_input_tokens": 120_000
            }
        }
    });
    let path = std::env::var_os("PATH").unwrap_or_default();
    let rendered = env.render(&payload, &path);
    assert_eq!(rendered, "~/projects/app main +2 O4.8 ~$0.10 API-equiv");
    assert_doc_contains("README.md", &format!("\n{rendered}\n"));
}
