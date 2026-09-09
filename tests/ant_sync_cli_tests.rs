//! CLI failure-mode + fake-ant/fake-curl integration tests for
//! `statusline ant sync-models` (Plan 03, Task 3) and — in the clearly-marked
//! second half of this file — `statusline ant sync-pricing` (Phase 11 Plan 01,
//! the KEYLESS sync; see the section banner there for its own contract list).
//!
//! These tests prove the out-of-band sync's security and failure contract WITHOUT
//! any live network:
//!
//! - **Failure modes** exit non-zero with a clear DIFFERENTIATED, key-free stderr
//!   message and write NO cache (ANT-05 / D-13).
//! - **No key in argv** (T-07-08): a fake `ant` / `curl` records its real argv +
//!   env; the recorded argv never contains the key value.
//! - **Profile shadowing removed** (T-07-09 / D-09): the fake `ant` child env has
//!   `ANT_PROFILE` set and `ANTHROPIC_API_KEY` ABSENT on the profile path.
//! - **Pagination** (review MUST-FIX #6): a two-page fake response yields a cache
//!   with models from BOTH pages.
//! - **Leak-free curl** (review MUST-FIX #7): the fake `curl`'s argv lacks the key
//!   and the config (with `x-api-key`) arrives via STDIN.
//! - **`--quiet`** silences stdout on the failure branch.
//!
//! All PATH/env/XDG-mutating tests are `#[serial]` (the repo's global test lock,
//! review MUST-FIX #13) and use direct exit-status assertions (no
//! `cmd | tail; echo $?`).

#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant sync-models` invocation: an isolated
/// HOME + XDG dirs (so the cache lands somewhere predictable and never touches the
/// host), a config file with a chosen `[ant]` section, and a `bin` dir we control
/// to shape PATH.
struct SyncEnv {
    home: TempDir,
    bin: TempDir,
    /// Where a fake exec records its argv/env (one line per invocation).
    record: PathBuf,
    config_path: PathBuf,
}

impl SyncEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let record = home.path().join("record.log");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        SyncEnv {
            home,
            bin,
            record,
            config_path,
        }
    }

    /// PATH that contains ONLY our controlled bin dir (no real ant/curl, and no
    /// system tools — used for the "no fetch tool at all" failure branches).
    fn isolated_path(&self) -> String {
        self.bin.path().display().to_string()
    }

    /// PATH with our bin dir first, then a curated set of real system dirs so the
    /// fake shell scripts can still call `cat`/`env`/`touch`. We deliberately do
    /// NOT include any dir that contains a real `ant`, so the fetch's
    /// `tool_on_path("ant")` resolves only a fake (or nothing) — keeping the test
    /// deterministic on machines that DO have `ant` installed.
    fn system_path(&self) -> String {
        let mut dirs = vec![self.bin.path().display().to_string()];
        for d in ["/bin", "/usr/bin"] {
            if !Path::new(d).join("ant").exists() {
                dirs.push(d.to_string());
            }
        }
        dirs.join(":")
    }

    /// Install a fake executable named `name` running the given `/bin/sh` body.
    fn install_fake(&self, name: &str, body: &str) {
        let exe = self.bin.path().join(name);
        let script = format!("#!/bin/sh\n{}\n", body);
        fs::write(&exe, script).expect("write fake exec");
        let mut perms = fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    /// Run `statusline ant sync-models [--quiet]` with the given PATH and optional
    /// `ANTHROPIC_API_KEY`. Returns (status_success, stdout, stderr).
    fn run(&self, path: &str, key: Option<&str>, quiet: bool) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("sync-models");
        if quiet {
            cmd.arg("--quiet");
        }
        cmd.env("HOME", self.home.path())
            // Isolate the cache + config search to this HOME on both platforms.
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(k) = key {
            cmd.env("ANTHROPIC_API_KEY", k);
        }
        let out = cmd.output().expect("spawn statusline ant sync-models");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Find the written `models.json` (if any) under the isolated HOME tree.
    fn find_cache(&self) -> Option<PathBuf> {
        find_models_json(self.home.path())
    }

    fn record_contents(&self) -> String {
        fs::read_to_string(&self.record).unwrap_or_default()
    }
}

/// Recursively search for a `models.json` under `root`.
fn find_models_json(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_models_json(&p) {
                return Some(found);
            }
        } else if p.file_name().map(|n| n == "models.json").unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// (a) No credential + no ant: differentiated no-credential/no-tool, no cache.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn no_credential_no_ant_fails_differentiated_no_cache() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    // PATH has neither ant nor curl.
    let (ok, stdout, stderr) = env.run(&env.isolated_path(), None, false);
    assert!(!ok, "must exit non-zero with no credential and no ant");
    assert!(
        stderr.to_lowercase().contains("credential") || stderr.to_lowercase().contains("ant"),
        "stderr must carry a clear no-credential/no-tool message, got: {stderr}"
    );
    assert!(!stderr.contains("sk-ant-"), "no key token in stderr");
    assert!(
        env.find_cache().is_none(),
        "no cache must be written on failure"
    );
    let _ = stdout;
}

// ---------------------------------------------------------------------------
// (b) No ant and no curl, but a key set: fails cleanly (no panic), no cache.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn no_ant_no_curl_with_key_fails_cleanly_no_cache() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let (ok, _stdout, stderr) = env.run(&env.isolated_path(), Some("sk-ant-dummy"), false);
    assert!(!ok, "must exit non-zero when no fetch tool is available");
    assert!(
        stderr.to_lowercase().contains("tool") || stderr.to_lowercase().contains("curl"),
        "stderr must explain no fetch tool is available, got: {stderr}"
    );
    assert!(!stderr.to_lowercase().contains("panic"), "must not panic");
    assert!(!stderr.contains("sk-ant-"), "no key token in stderr");
    assert!(env.find_cache().is_none(), "no cache on failure");
}

// ---------------------------------------------------------------------------
// (c) FAKE-ANT SUCCESS, profile mode (i): records real argv/env; assert no key
//     in argv, ANT_PROFILE set + ANTHROPIC_API_KEY removed, cache written, the
//     summary prints the profile label and no key token.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_ant_profile_mode_no_key_in_argv_profile_set_key_removed() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"work\"\n");
    // Fake `ant` records argv + env, then emits a single page and exits 0.
    let body = format!(
        "echo \"ARGV: $@\" >> \"{rec}\"\n\
         env >> \"{rec}\"\n\
         echo '{{\"data\":[{{\"id\":\"claude-x\",\"max_input_tokens\":200000}}],\"has_more\":false}}'\n\
         exit 0\n",
        rec = env.record.display()
    );
    env.install_fake("ant", &body);

    let (ok, stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-shadow"), false);
    assert!(ok, "fake-ant success must exit 0; stderr={stderr}");

    let rec = env.record_contents();
    assert!(
        !rec.contains("sk-ant-shadow"),
        "key value must NOT be in argv/env: {rec}"
    );
    assert!(
        rec.contains("ANT_PROFILE=work"),
        "ANT_PROFILE must be set in child env"
    );
    assert!(
        !rec.lines().any(|l| l.starts_with("ANTHROPIC_API_KEY=")),
        "ANTHROPIC_API_KEY must be removed from the profile child env"
    );

    assert!(
        env.find_cache().is_some(),
        "cache must be written on success"
    );
    assert!(
        stdout.contains("ant profile 'work'"),
        "summary must show the profile label"
    );
    assert!(!stdout.contains("sk-ant-"), "no key token in summary");
}

// ---------------------------------------------------------------------------
// (c2) FAKE-ANT SUCCESS, env-key mode (ii): the env key is inherited (present in
//      the recorded child env) and the label is "ANTHROPIC_API_KEY (env)".
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_ant_env_key_mode_inherits_key_and_labels_env() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let body = format!(
        "env >> \"{rec}\"\n\
         echo '{{\"data\":[{{\"id\":\"claude-y\",\"max_input_tokens\":100000}}],\"has_more\":false}}'\n\
         exit 0\n",
        rec = env.record.display()
    );
    env.install_fake("ant", &body);

    let (ok, stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-envkey"), false);
    assert!(ok, "fake-ant env-key success must exit 0; stderr={stderr}");

    let rec = env.record_contents();
    assert!(
        rec.contains("ANTHROPIC_API_KEY=sk-ant-envkey"),
        "env-key mode must inherit the key into the child env: {rec}"
    );
    assert!(
        stdout.contains("ANTHROPIC_API_KEY (env)"),
        "summary must show the env-key label, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// (d) FAKE-ANT PAGINATION: page 1 has_more=true,last_id=X; page 2 has_more=false.
//     The written cache contains models from BOTH pages.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_ant_pagination_accumulates_both_pages() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let counter = env.home.path().join("count");
    // First invocation (no --after-id): emit page 1; second: page 2. We key off a
    // counter file so we don't depend on argv parsing in the shell.
    let body = format!(
        "if [ -f \"{cnt}\" ]; then\n\
         echo '{{\"data\":[{{\"id\":\"model-b\",\"max_input_tokens\":200}}],\"has_more\":false}}'\n\
         else\n\
         touch \"{cnt}\"\n\
         echo '{{\"data\":[{{\"id\":\"model-a\",\"max_input_tokens\":100}}],\"has_more\":true,\"last_id\":\"model-a\"}}'\n\
         fi\n\
         exit 0\n",
        cnt = counter.display()
    );
    env.install_fake("ant", &body);

    let (ok, _stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-x"), false);
    assert!(ok, "paginated fake-ant must exit 0; stderr={stderr}");

    let cache_path = env.find_cache().expect("cache must be written");
    let body = fs::read_to_string(&cache_path).expect("read cache");
    assert!(
        body.contains("model-a"),
        "page 1 model must be present: {body}"
    );
    assert!(
        body.contains("model-b"),
        "page 2 model must be present: {body}"
    );
}

// ---------------------------------------------------------------------------
// (e) FAKE-CURL fallback (mode ii): NO ant on PATH, a fake curl records argv +
//     reads its stdin config and emits JSON. Assert no key in curl argv, the
//     config (x-api-key) arrived via stdin, the cache is written, and no key
//     token leaks to output.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_curl_fallback_leakfree_stdin_config() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let stdin_capture = env.home.path().join("curl_stdin.log");
    // Fake `curl`: record argv, slurp stdin (the config) to a file, emit JSON.
    let body = format!(
        "echo \"ARGV: $@\" >> \"{rec}\"\n\
         cat >> \"{cap}\"\n\
         echo '{{\"data\":[{{\"id\":\"curl-model\",\"max_input_tokens\":150000}}],\"has_more\":false}}'\n\
         exit 0\n",
        rec = env.record.display(),
        cap = stdin_capture.display()
    );
    env.install_fake("curl", &body);
    // NOTE: no fake `ant` installed and isolated PATH excludes the real ant, so
    // the EnvKey-mode fetch must fall back to curl.

    let (ok, stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-curlkey"), false);
    assert!(ok, "fake-curl fallback must exit 0; stderr={stderr}");

    let rec = env.record_contents();
    assert!(
        rec.starts_with("ARGV:"),
        "curl must have been invoked: {rec}"
    );
    assert!(
        !rec.contains("sk-ant-curlkey"),
        "key must NOT be in curl argv: {rec}"
    );

    let stdin_cfg = fs::read_to_string(&stdin_capture).expect("read curl stdin capture");
    assert!(
        stdin_cfg.contains("x-api-key: sk-ant-curlkey"),
        "the x-api-key config must arrive via curl STDIN: {stdin_cfg}"
    );

    assert!(
        env.find_cache().is_some(),
        "cache must be written via curl fallback"
    );
    assert!(!stdout.contains("sk-ant-"), "no key token in summary");
    assert!(!stderr.contains("sk-ant-"), "no key token in stderr");
}

// ---------------------------------------------------------------------------
// (f) --quiet suppresses the summary on the failure branch (stdout empty, stderr
//     still carries the differentiated error).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn quiet_suppresses_stdout_on_failure_branch() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let (ok, stdout, stderr) = env.run(&env.isolated_path(), None, true);
    assert!(!ok, "failure branch must exit non-zero");
    assert!(
        stdout.is_empty(),
        "--quiet must produce empty stdout, got: {stdout}"
    );
    assert!(!stderr.is_empty(), "stderr must still carry the error");
}

// ===========================================================================
// `statusline ant sync-pricing` (Phase 11 / PRICE-04)
// ---------------------------------------------------------------------------
// The pricing sync is the ONE KEYLESS member of `ant`: upstream is a public
// raw-GitHub URL. These fake-`curl` tests prove — with no live network — the
// SC1 success contract and the SC4/D-04 failure contract:
//
//   * success writes a versioned cache stamped source / version / fetched_at,
//   * a failing fetch exits non-zero and writes NOTHING (a pre-existing cache
//     survives byte-identical; the compiled-in bundled table is untouched),
//   * a Claude-less upstream payload is a FAILURE, never an empty cache,
//   * `--max-age` self-throttles before any subprocess is spawned,
//   * `--quiet` silences the summary,
//   * the child argv is keyless even when a key is present in the environment.
// ===========================================================================

/// An isolated environment for one `ant sync-pricing` invocation: an isolated
/// HOME + XDG tree (so the price cache lands somewhere predictable and never
/// touches the host) plus a controlled `bin` dir holding a fake `curl`.
struct PricingEnv {
    home: TempDir,
    bin: TempDir,
    /// Where the fake `curl` records its argv (one invocation per line).
    argv_log: PathBuf,
}

/// A minimal upstream payload: two usable Anthropic rows, one unusable
/// (missing `output_cost_per_token`), one non-Anthropic `claude-*` decoy, and
/// one non-Claude row. Selection must keep exactly the two, and COUNT one skip.
const FAKE_UPSTREAM: &str = r#"{
  "claude-test-sonnet": {
    "litellm_provider": "anthropic",
    "input_cost_per_token": 3e-06,
    "output_cost_per_token": 1.5e-05,
    "cache_creation_input_token_cost": 3.75e-06,
    "cache_read_input_token_cost": 3e-07,
    "cache_creation_input_token_cost_above_1hr": 6e-06
  },
  "claude-test-haiku": {
    "litellm_provider": "anthropic",
    "input_cost_per_token": 2.5e-07,
    "output_cost_per_token": 1.25e-06,
    "cache_creation_input_token_cost": 3e-07,
    "cache_read_input_token_cost": 3e-08
  },
  "claude-test-broken": {
    "litellm_provider": "anthropic",
    "input_cost_per_token": 3e-06,
    "cache_creation_input_token_cost": 3.75e-06,
    "cache_read_input_token_cost": 3e-07
  },
  "claude-test-bedrock": {
    "litellm_provider": "bedrock",
    "input_cost_per_token": 3e-06,
    "output_cost_per_token": 1.5e-05,
    "cache_creation_input_token_cost": 3.75e-06,
    "cache_read_input_token_cost": 3e-07
  },
  "gpt-test": {
    "litellm_provider": "openai",
    "input_cost_per_token": 1e-06,
    "output_cost_per_token": 2e-06
  }
}"#;

/// An upstream payload carrying no usable Claude row at all (D-04 / Pitfall 5).
const CLAUDE_LESS_UPSTREAM: &str = r#"{
  "gpt-test": {
    "litellm_provider": "openai",
    "input_cost_per_token": 1e-06,
    "output_cost_per_token": 2e-06
  }
}"#;

impl PricingEnv {
    fn new() -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let argv_log = home.path().join("curl-argv.log");
        PricingEnv {
            home,
            bin,
            argv_log,
        }
    }

    /// Install a fake `curl` that records its argv then emits `payload` on
    /// stdout and exits 0.
    fn install_curl_serving(&self, payload: &str) {
        let payload_path = self.home.path().join("upstream.json");
        fs::write(&payload_path, payload).expect("write fake upstream payload");
        self.install_fake_curl(&format!(
            "echo \"$@\" >> '{log}'\nexec /bin/cat '{payload}'",
            log = self.argv_log.display(),
            payload = payload_path.display()
        ));
    }

    /// Install a fake `curl` that records its argv then fails like a real
    /// connection failure (exit 7).
    fn install_curl_failing(&self) {
        self.install_fake_curl(&format!(
            "echo \"$@\" >> '{log}'\necho 'curl: (7) Failed to connect to host' >&2\nexit 7",
            log = self.argv_log.display()
        ));
    }

    fn install_fake_curl(&self, body: &str) {
        let exe = self.bin.path().join("curl");
        fs::write(&exe, format!("#!/bin/sh\n{body}\n")).expect("write fake curl");
        let mut perms = fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    /// PATH containing ONLY our controlled bin dir, so the real `curl` can never
    /// be reached (and with no fake installed, no fetch tool exists at all).
    fn path(&self) -> String {
        self.bin.path().display().to_string()
    }

    /// Where the binary will resolve the price cache under the isolated HOME.
    /// `dirs::cache_dir()` honors `XDG_CACHE_HOME` on Linux and
    /// `$HOME/Library/Caches` on macOS; `run()` sets both consistently.
    fn cache_path(&self) -> PathBuf {
        let base = if cfg!(target_os = "macos") {
            self.home.path().join("Library").join("Caches")
        } else {
            self.home.path().join("cache")
        };
        base.join("claudia-statusline")
            .join("ant")
            .join("prices.json")
    }

    /// Seed a pre-existing price cache with `fetched_at` `age_secs` in the past.
    /// Returns the bytes written so a test can assert byte-identity later.
    fn seed_cache(&self, age_secs: i64) -> Vec<u8> {
        let path = self.cache_path();
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir cache dir");
        let fetched_at = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        let body = format!(
            r#"{{
  "schema_version": 1,
  "fetched_at": "{ts}",
  "source": "https://seeded.invalid/prices.json",
  "version": "seededseeded01",
  "prices": {{
    "claude-seeded": {{
      "input": 1e-06,
      "output": 2e-06,
      "cache_creation": 1.25e-06,
      "cache_read": 1e-07
    }}
  }}
}}"#,
            ts = fetched_at.to_rfc3339()
        );
        fs::write(&path, &body).expect("seed price cache");
        body.into_bytes()
    }

    /// Run `statusline ant sync-pricing [--quiet] [--max-age DUR]`.
    /// Returns (success, stdout, stderr).
    fn run(&self, quiet: bool, max_age: Option<&str>, key: Option<&str>) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("sync-pricing");
        if quiet {
            cmd.arg("--quiet");
        }
        if let Some(spec) = max_age {
            cmd.arg("--max-age").arg(spec);
        }
        cmd.env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("PATH", self.path())
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("STATUSLINE_CONFIG_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(k) = key {
            cmd.env("ANTHROPIC_API_KEY", k);
        }
        let out = cmd.output().expect("spawn statusline ant sync-pricing");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn recorded_argv(&self) -> String {
        fs::read_to_string(&self.argv_log).unwrap_or_default()
    }

    fn find_price_cache(&self) -> Option<PathBuf> {
        find_named(self.home.path(), "prices.json")
    }
}

/// Recursively search for a file named `name` under `root`.
fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_named(&p, name) {
                return Some(found);
            }
        } else if p.file_name().map(|n| n == name).unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// (p1) SUCCESS: writes a versioned cache stamped source/version/fetched_at and
//      reports the skipped row (SC1 / D-03 / D-12).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_success_writes_versioned_cache_and_reports_skips() {
    let env = PricingEnv::new();
    env.install_curl_serving(FAKE_UPSTREAM);

    let (ok, stdout, stderr) = env.run(false, None, None);
    assert!(ok, "success path must exit 0; stderr: {stderr}");

    let path = env.find_price_cache().expect("prices.json must be written");
    assert_eq!(
        path,
        env.cache_path(),
        "cache must land at the resolved path"
    );

    let raw = fs::read_to_string(&path).expect("read cache");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("cache is valid JSON");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(
        v["source"],
        "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
    );
    let version = v["version"].as_str().expect("version is a string");
    assert_eq!(version.len(), 16, "version is a 16-hex content hash");
    assert!(version.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        v["fetched_at"].as_str().is_some_and(|s| !s.is_empty()),
        "fetched_at must be stamped"
    );

    let prices = v["prices"].as_object().expect("prices is an object");
    assert_eq!(prices.len(), 2, "exactly the two usable Anthropic rows");
    assert!(prices.contains_key("claude-test-sonnet"));
    assert!(prices.contains_key("claude-test-haiku"));
    assert!(
        !prices.contains_key("claude-test-bedrock"),
        "a non-anthropic provider row must not be cached"
    );
    assert_eq!(
        prices["claude-test-sonnet"]["cache_creation_1h"], 6e-06,
        "a sane 1h rate is carried through to the cache"
    );

    // Summary: counts + provenance, and NO credential label (keyless).
    assert!(
        stdout.contains("Synced 2 model prices"),
        "summary must report the count, got: {stdout}"
    );
    assert!(
        stdout.contains("Skipped 1 unusable upstream row."),
        "summary must report the skipped row (D-03), got: {stdout}"
    );
    assert!(
        stdout.contains("Cache:"),
        "summary must show the cache path"
    );
    assert!(
        stdout.contains("Source:"),
        "summary must show the source URL"
    );
    assert!(
        stdout.contains("Snapshot:"),
        "summary must show the version"
    );
    assert!(
        !stdout.contains("Credential source"),
        "the keyless sync must NOT print a credential label, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// (p2) The child argv is KEYLESS even with a key in the environment (T-11-01).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_child_argv_is_keyless() {
    let env = PricingEnv::new();
    env.install_curl_serving(FAKE_UPSTREAM);

    let (ok, _stdout, stderr) = env.run(true, None, Some("sk-ant-should-never-be-used"));
    assert!(ok, "success path must exit 0; stderr: {stderr}");

    let argv = env.recorded_argv();
    assert!(
        argv.contains("raw.githubusercontent.com/BerriAI/litellm"),
        "the fetch must target the public upstream URL, got argv: {argv}"
    );
    assert!(
        !argv.contains("sk-ant-"),
        "no credential may reach the child argv, got: {argv}"
    );
    assert!(
        !argv.contains("--config"),
        "keyless fetch needs no stdin-config dance, got: {argv}"
    );
    assert!(
        !argv.to_lowercase().contains("api-key"),
        "no api-key header may reach the child, got: {argv}"
    );
}

// ---------------------------------------------------------------------------
// (p3) FETCH FAILURE: non-zero exit, nothing written, pre-existing cache and
//      the bundled table both intact (SC4).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_fetch_failure_writes_nothing_and_preserves_cache() {
    let env = PricingEnv::new();
    // Seed an OLD cache (older than any --max-age we pass) so the failure path
    // is genuinely reached and we can prove the cache survives untouched.
    let seeded = env.seed_cache(86_400);
    env.install_curl_failing();

    let (ok, _stdout, stderr) = env.run(false, None, None);
    assert!(!ok, "a failed fetch must exit non-zero");
    assert!(
        stderr.to_lowercase().contains("network")
            || stderr.to_lowercase().contains("curl")
            || stderr.to_lowercase().contains("price"),
        "stderr must explain the fetch failure, got: {stderr}"
    );
    assert!(!stderr.to_lowercase().contains("panic"), "must not panic");

    let after = fs::read(env.cache_path()).expect("pre-existing cache must survive");
    assert_eq!(
        after, seeded,
        "a failed fetch must leave the existing cache byte-identical"
    );

    // And no temp file was left behind by the aborted publish.
    assert!(
        find_named(env.home.path(), "prices.json.tmp").is_none(),
        "a failed fetch must leave no partial temp cache"
    );
}

#[test]
#[serial]
fn sync_pricing_fetch_failure_with_no_cache_writes_nothing() {
    let env = PricingEnv::new();
    env.install_curl_failing();

    let (ok, _stdout, stderr) = env.run(false, None, None);
    assert!(!ok, "a failed fetch must exit non-zero");
    assert!(
        env.find_price_cache().is_none(),
        "no cache may be created on the failure path; stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// (p4) NO FETCH TOOL: curl absent entirely -> non-zero, nothing written.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_without_curl_fails_cleanly_no_cache() {
    let env = PricingEnv::new(); // no fake curl installed; PATH has only the empty bin dir
    let (ok, _stdout, stderr) = env.run(false, None, None);
    assert!(!ok, "a missing fetch tool must exit non-zero");
    assert!(
        stderr.to_lowercase().contains("curl"),
        "stderr must name the missing tool, got: {stderr}"
    );
    assert!(!stderr.to_lowercase().contains("panic"), "must not panic");
    assert!(env.find_price_cache().is_none(), "no cache on failure");
}

// ---------------------------------------------------------------------------
// (p5) CLAUDE-LESS UPSTREAM: a well-formed payload with zero usable Claude rows
//      is a FAILURE, never an empty cache (D-04 / T-11-04).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_claude_less_upstream_writes_nothing() {
    let env = PricingEnv::new();
    let seeded = env.seed_cache(86_400);
    env.install_curl_serving(CLAUDE_LESS_UPSTREAM);

    let (ok, _stdout, stderr) = env.run(false, None, None);
    assert!(!ok, "zero usable Claude rows must exit non-zero");
    assert!(
        stderr.contains("zero usable"),
        "stderr must name the empty-result cause, got: {stderr}"
    );

    let after = fs::read(env.cache_path()).expect("pre-existing cache must survive");
    assert_eq!(
        after, seeded,
        "an empty upstream must never overwrite a good cache"
    );
}

// ---------------------------------------------------------------------------
// (p6) `--max-age` self-throttles BEFORE any subprocess is spawned (D-10).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_max_age_skips_fetch_when_cache_is_fresh() {
    let env = PricingEnv::new();
    let seeded = env.seed_cache(60); // 1 minute old
                                     // A curl that would FAIL if it were ever spawned — reaching it means the
                                     // throttle did not work.
    env.install_curl_failing();

    let (ok, stdout, stderr) = env.run(false, Some("24h"), None);
    assert!(ok, "a fresh cache must short-circuit to exit 0; {stderr}");
    assert!(
        stdout.contains("fresh") && stdout.contains("skipping"),
        "the throttle must say why it skipped, got: {stdout}"
    );
    assert!(
        env.recorded_argv().is_empty(),
        "the throttle must run BEFORE any subprocess is spawned"
    );
    assert_eq!(
        fs::read(env.cache_path()).expect("cache still there"),
        seeded,
        "the throttled run must not touch the cache"
    );
}

#[test]
#[serial]
fn sync_pricing_max_age_fetches_when_cache_is_stale() {
    let env = PricingEnv::new();
    env.seed_cache(86_400); // 1 day old
    env.install_curl_serving(FAKE_UPSTREAM);

    let (ok, stdout, stderr) = env.run(false, Some("10m"), None);
    assert!(ok, "a stale cache must fetch and succeed; {stderr}");
    assert!(
        stdout.contains("Synced 2 model prices"),
        "the stale path must actually refresh, got: {stdout}"
    );
    assert!(
        !env.recorded_argv().is_empty(),
        "a stale cache must reach the fetch"
    );
}

#[test]
#[serial]
fn sync_pricing_future_dated_cache_counts_as_fresh() {
    let env = PricingEnv::new();
    // Clock skew: a cache stamped in the FUTURE must be treated as fresh, never
    // as infinitely stale (Pitfall 1).
    env.seed_cache(-3_600);
    env.install_curl_failing();

    let (ok, stdout, _stderr) = env.run(false, Some("10m"), None);
    assert!(ok, "a future-dated cache must count as fresh");
    assert!(stdout.contains("fresh"), "got: {stdout}");
    assert!(
        env.recorded_argv().is_empty(),
        "no fetch may be attempted for a future-dated cache"
    );
}

#[test]
#[serial]
fn sync_pricing_invalid_max_age_is_rejected_before_fetching() {
    let env = PricingEnv::new();
    env.install_curl_serving(FAKE_UPSTREAM);

    let (ok, _stdout, stderr) = env.run(false, Some("banana"), None);
    assert!(!ok, "a malformed --max-age must exit non-zero");
    assert!(
        stderr.contains("max-age"),
        "stderr must name the bad flag, got: {stderr}"
    );
    assert!(
        env.recorded_argv().is_empty(),
        "a malformed --max-age must be rejected BEFORE any fetch"
    );
    assert!(env.find_price_cache().is_none(), "no cache may be written");
}

// ---------------------------------------------------------------------------
// (p7) `--quiet` suppresses the summary on success (errors still print).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_pricing_quiet_suppresses_the_summary() {
    let env = PricingEnv::new();
    env.install_curl_serving(FAKE_UPSTREAM);

    let (ok, stdout, stderr) = env.run(true, None, None);
    assert!(ok, "success path must exit 0; stderr: {stderr}");
    assert!(
        stdout.is_empty(),
        "--quiet must produce empty stdout, got: {stdout}"
    );
    assert!(
        env.find_price_cache().is_some(),
        "--quiet still publishes the cache"
    );
}
