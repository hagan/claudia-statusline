//! Doc ↔ render drift guard for the cost examples in `docs/CONFIGURATION.md`
//! and `README.md` (phase 13, D-03 / D-10).
//!
//! The documented `{cost}` example and the "Cost-Focused Power User" example
//! are rendered through the REAL binary under a fully isolated environment
//! (HOME + all three XDG dirs in a temp dir, empty PATH, `NO_COLOR=1`), and the
//! docs are asserted to show exactly what the binary prints.
#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// The documented numbers: `$12.50` over 12857 s (3h 34m 17s) → `$3.50/hr`.
const TOTAL_COST_USD: f64 = 12.5;
const TOTAL_DURATION_MS: u64 = 12_857_000;

/// What `{cost}` renders for the documented numbers with the default
/// `[layout.components.cost] format = "full"`.
const EXPECTED_COST_RENDER: &str = "$12.50 ($3.50/hr)";

fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// An isolated environment for one render.
struct RenderEnv {
    home: TempDir,
    bin: TempDir,
    config_path: PathBuf,
}

impl RenderEnv {
    fn new(config_body: &str) -> RenderEnv {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let config_path = home.path().join("statusline.toml");
        fs::write(&config_path, config_body).expect("write config");
        RenderEnv {
            home,
            bin,
            config_path,
        }
    }

    /// Render the fixed cost payload; returns trimmed stdout. Panics on a
    /// non-zero exit so a broken config never masquerades as a doc mismatch.
    fn render(&self) -> String {
        // No `session_id` on purpose: with one, burn-rate duration comes from the
        // session's first-seen time in the (fresh, isolated) stats DB — about 0 s
        // here — so no `(…/hr)` would appear. A real 3.5-hour session has a DB
        // duration matching its wall clock; omitting `session_id` makes the
        // render fall back to the payload's `total_duration_ms`, reproducing that
        // documented scenario deterministically.
        let payload = serde_json::json!({
            "workspace": { "current_dir": self.home.path().display().to_string() },
            "model": { "id": "claude-opus-4-8" },
            "cost": {
                "total_cost_usd": TOTAL_COST_USD,
                "total_duration_ms": TOTAL_DURATION_MS
            }
        })
        .to_string();

        let home = self.home.path();
        let mut child = Command::new(test_support::test_binary())
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", self.bin.path())
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
            .write_all(payload.as_bytes())
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
}

/// Extract the ```toml block that follows `heading` in `doc`.
fn toml_block_after(doc: &str, heading: &str) -> String {
    let start = doc
        .find(heading)
        .unwrap_or_else(|| panic!("heading {heading:?} not found in doc"));
    let rest = &doc[start..];
    let open = rest.find("```toml\n").expect("```toml fence after heading") + "```toml\n".len();
    let body = &rest[open..];
    let close = body.find("```").expect("closing fence");
    body[..close].to_string()
}

#[test]
#[serial]
fn documented_cost_row_matches_render() {
    let rendered = RenderEnv::new("[layout]\nformat = \"{cost}\"\n").render();
    assert_eq!(
        rendered, EXPECTED_COST_RENDER,
        "the `{{cost}}` render changed; update docs/CONFIGURATION.md to match"
    );

    let doc = repo_file("docs/CONFIGURATION.md");
    let row_prefix = format!("| `{{cost}}` | `{rendered}`");
    assert!(
        doc.lines().any(|l| l.starts_with(&row_prefix)),
        "docs/CONFIGURATION.md has no `{{cost}}` table row starting with {row_prefix:?}"
    );
}

#[test]
#[serial]
fn power_user_example_renders_burn_rate_once() {
    let doc = repo_file("docs/CONFIGURATION.md");
    let toml = toml_block_after(&doc, "#### Cost-Focused Power User");
    let format_line = toml
        .lines()
        .find(|l| l.starts_with("format = \"{directory}"))
        .expect("Power User example has a [layout] format line");
    assert!(
        doc.contains(format_line),
        "format line must be verbatim in doc"
    );

    let rendered = RenderEnv::new(&toml).render();
    assert_eq!(
        rendered.matches("/hr").count(),
        1,
        "Power User example must show the burn rate exactly once; rendered {rendered:?}"
    );
}

/// The built-in `power` preset must show the burn rate once: under the default
/// `[layout.components.cost] format = "full"`, `{cost}` already carries
/// `($X.XX/hr)`, so a trailing `({burn_rate})` would duplicate it (CR-01).
#[test]
#[serial]
fn power_preset_renders_burn_rate_once() {
    let rendered = RenderEnv::new("[layout]\npreset = \"power\"\n").render();
    assert_eq!(
        rendered.matches("/hr").count(),
        1,
        "built-in power preset must show the burn rate exactly once; rendered {rendered:?}"
    );
}

#[test]
fn docs_have_no_stale_cost_examples() {
    for rel in ["docs/CONFIGURATION.md", "README.md"] {
        let doc = repo_file(rel);
        assert!(
            !doc.contains("${cost_short}"),
            "{rel} still contains `${{cost_short}}` (renders `$$12`)"
        );
        for line in doc.lines().filter(|l| l.contains("S4.5 • $12.50")) {
            assert!(
                line.contains(EXPECTED_COST_RENDER),
                "{rel}: preset example does not end in the real render {EXPECTED_COST_RENDER:?}: {line}"
            );
        }
    }
}
