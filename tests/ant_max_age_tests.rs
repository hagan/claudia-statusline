//! Integration tests for the `--max-age` sync throttle (ANT-30, Plan 09-02).
//!
//! These tests prove the self-throttle short-circuit: with `--max-age` set and a
//! cache younger than the threshold, `ant sync-models` / `ant sync-usage` must NOT
//! invoke the fetch (no fake-exec marker), and exit 0; with `--max-age` omitted the
//! fetch must ALWAYS run even when a fresh cache exists; a cache `fetched_at` in the
//! future (negative age / clock skew) is treated as fresh (no fetch). A bad
//! `--max-age` value exits non-zero with a clear parse error.
//!
//! The harness mirrors `tests/ant_sync_cli_tests.rs`/`ant_usage_cli_tests.rs`: an
//! isolated HOME + XDG dirs (so the cache lands somewhere predictable), a config
//! file with a chosen `[ant]` section, and a controlled `bin` dir on PATH holding a
//! fake exec that records its invocation. Assertions are on recorded files / exit
//! status (no `cmd | tail` pipeline). Every PATH/env/XDG-mutating test is `#[serial]`.

#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant sync-*` invocation.
struct ThrottleEnv {
    home: TempDir,
    bin: TempDir,
    /// Where a fake exec records that it was invoked (one line per invocation).
    marker: PathBuf,
    config_path: PathBuf,
}

impl ThrottleEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let marker = home.path().join("fetch.invoked");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        ThrottleEnv {
            home,
            bin,
            marker,
            config_path,
        }
    }

    /// PATH with our bin dir first, then a curated set of real system dirs so the
    /// fake shell scripts can still call `touch`/`echo`. We deliberately do NOT
    /// include any dir that contains a real `ant` so the fetch's `tool_on_path("ant")`
    /// resolves only our fake (or nothing) — keeping the test deterministic.
    fn system_path(&self) -> String {
        let mut dirs = vec![self.bin.path().display().to_string()];
        for d in ["/bin", "/usr/bin"] {
            if !Path::new(d).join("ant").exists() {
                dirs.push(d.to_string());
            }
        }
        dirs.join(":")
    }

    /// Install a fake executable named `name` that records its invocation (drops the
    /// marker) and emits a single valid page, then exits 0. If the throttle
    /// short-circuits, this is never run and the marker stays ABSENT.
    ///
    /// The marker is written with the shell BUILTIN redirection `: >`, not `touch`:
    /// `system_path()` drops `/bin` and `/usr/bin` when they hold a real `ant` (the
    /// Ubuntu CI image ships Apache Ant there), and `touch` then fails silently.
    fn install_fake_fetch(&self, name: &str) {
        let exe = self.bin.path().join(name);
        let body = format!(
            "#!/bin/sh\n\
             : > \"{marker}\"\n\
             echo '{{\"data\":[{{\"id\":\"claude-x\",\"max_input_tokens\":200000}}],\"has_more\":false}}'\n\
             exit 0\n",
            marker = self.marker.display()
        );
        fs::write(&exe, body).expect("write fake exec");
        let mut perms = fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    /// The `XDG_CACHE_HOME` value handed to the child process.
    fn xdg_cache_home(&self) -> PathBuf {
        self.home.path().join("cache")
    }

    /// Resolve the cache root the SAME way the child's production code will via
    /// `dirs::cache_dir()`, given the child's `HOME` + `XDG_CACHE_HOME`. On Linux
    /// this honors `XDG_CACHE_HOME`; on macOS it is `$HOME/Library/Caches`. We
    /// briefly set the env in-process to reuse the exact `dirs` resolution (these
    /// tests are `#[serial]`), then restore it so no other test is affected.
    fn cache_root(&self) -> PathBuf {
        let orig_home = std::env::var_os("HOME");
        let orig_xdg = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("HOME", self.home.path());
        std::env::set_var("XDG_CACHE_HOME", self.xdg_cache_home());
        let root = dirs::cache_dir().expect("cache dir resolvable");
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match orig_xdg {
            Some(x) => std::env::set_var("XDG_CACHE_HOME", x),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        root
    }

    /// Seed a models cache whose `fetched_at` is `offset_secs` from now (negative =>
    /// in the past, positive => in the future / clock skew).
    fn seed_models_cache(&self, offset_secs: i64) {
        let dir = self.cache_root().join("claudia-statusline").join("ant");
        fs::create_dir_all(&dir).expect("mk ant cache dir");
        let when = (chrono::Utc::now() + chrono::Duration::seconds(offset_secs))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let body = format!(
            "{{\"schema_version\":1,\"fetched_at\":\"{when}\",\
             \"models\":{{\"claude-x\":{{\"max_input_tokens\":200000}}}}}}"
        );
        fs::write(dir.join("models.json"), body).expect("seed models cache");
    }

    /// Seed a per-account usage cache whose `fetched_at` is `offset_secs` from now.
    fn seed_usage_cache(&self, account: &str, offset_secs: i64) {
        let dir = self
            .cache_root()
            .join("claudia-statusline")
            .join("ant")
            .join("usage");
        fs::create_dir_all(&dir).expect("mk usage cache dir");
        let when = (chrono::Utc::now() + chrono::Duration::seconds(offset_secs))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let body = format!(
            "{{\"schema_version\":1,\"fetched_at\":\"{when}\",\"account\":\"{account}\",\
             \"today_usd\":1.0,\"mtd_usd\":2.0,\"tz\":\"UTC\",\"tokens_by_model\":{{}}}}"
        );
        fs::write(dir.join(format!("{account}.json")), body).expect("seed usage cache");
    }

    /// Run `statusline ant <args...>` with our isolated env. Returns
    /// `(status_success, stdout, stderr)`.
    fn run(&self, args: &[&str], path: &str, key: Option<&str>) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant");
        for a in args {
            cmd.arg(a);
        }
        cmd.env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.xdg_cache_home())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("STATUSLINE_ANT_ACCOUNT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(k) = key {
            cmd.env("ANTHROPIC_API_KEY", k);
        }
        let out = cmd.output().expect("spawn statusline ant");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn fetch_invoked(&self) -> bool {
        self.marker.exists()
    }
}

// ---------------------------------------------------------------------------
// sync-models
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn models_skip_if_fresh_does_not_invoke_fetch() {
    let env = ThrottleEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    env.install_fake_fetch("ant");
    // Cache fetched 1 minute ago; --max-age 24h => fresh => NO fetch.
    env.seed_models_cache(-60);

    let (ok, _stdout, stderr) = env.run(
        &["sync-models", "--max-age", "24h"],
        &env.system_path(),
        None,
    );
    assert!(ok, "fresh-skip must exit 0; stderr={stderr}");
    assert!(
        !env.fetch_invoked(),
        "throttle must short-circuit BEFORE fetch (marker present => fetched)"
    );
}

#[test]
#[serial]
fn models_stale_cache_invokes_fetch() {
    let env = ThrottleEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    env.install_fake_fetch("ant");
    // Cache fetched 1 hour ago; --max-age 1s => stale => DO fetch.
    env.seed_models_cache(-3600);

    let (ok, _stdout, stderr) = env.run(
        &["sync-models", "--max-age", "1s"],
        &env.system_path(),
        Some("sk-ant-x"),
    );
    assert!(ok, "stale fetch must exit 0; stderr={stderr}");
    assert!(
        env.fetch_invoked(),
        "a cache older than --max-age must fall through to the fetch"
    );
}

#[test]
#[serial]
fn models_omitted_max_age_always_fetches() {
    let env = ThrottleEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    env.install_fake_fetch("ant");
    // Even with a brand-new cache, omitting --max-age always fetches.
    env.seed_models_cache(-1);

    let (ok, _stdout, stderr) = env.run(&["sync-models"], &env.system_path(), Some("sk-ant-x"));
    assert!(ok, "omitted-max-age fetch must exit 0; stderr={stderr}");
    assert!(
        env.fetch_invoked(),
        "with --max-age omitted the fetch must ALWAYS run (manual runs never throttled)"
    );
}

#[test]
#[serial]
fn models_future_dated_cache_is_treated_as_fresh() {
    let env = ThrottleEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    env.install_fake_fetch("ant");
    // fetched_at 1h in the FUTURE (clock skew) => negative age => treated as fresh.
    env.seed_models_cache(3600);

    let (ok, _stdout, stderr) = env.run(
        &["sync-models", "--max-age", "1s"],
        &env.system_path(),
        None,
    );
    assert!(ok, "future-dated skip must exit 0; stderr={stderr}");
    assert!(
        !env.fetch_invoked(),
        "a future-dated cache (negative age) must be treated as fresh under --max-age"
    );
}

#[test]
#[serial]
fn models_bad_max_age_exits_nonzero() {
    let env = ThrottleEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    env.install_fake_fetch("ant");
    env.seed_models_cache(-60);

    let (ok, _stdout, stderr) = env.run(
        &["sync-models", "--max-age", "bogus"],
        &env.system_path(),
        None,
    );
    assert!(!ok, "a malformed --max-age must exit non-zero");
    assert!(
        stderr.to_lowercase().contains("max-age"),
        "stderr must explain the bad --max-age, got: {stderr}"
    );
    assert!(
        !env.fetch_invoked(),
        "a parse error must abort before any fetch"
    );
}

// ---------------------------------------------------------------------------
// sync-usage
// ---------------------------------------------------------------------------

/// `[ant]` config enabling an account `work` with an admin_key_command that, if
/// reached, would be exec'd by the usage fetch (the credential command). We point
/// it at a marker-dropping fake so a fetch attempt is observable.
fn usage_config(marker: &Path) -> String {
    format!(
        "[ant]\nenabled = true\nprofile = \"\"\n\n\
         [ant.accounts.work]\nadmin_key_command = [\"sh\", \"-c\", \"touch '{m}'; echo sk-ant-admin01-x\"]\n",
        m = marker.display()
    )
}

#[test]
#[serial]
fn usage_skip_if_fresh_does_not_invoke_fetch() {
    let env = ThrottleEnv::new("placeholder");
    // Rewrite the config to a usage-account config whose credential command drops
    // the same marker the fake fetch would.
    fs::write(&env.config_path, usage_config(&env.marker)).expect("write usage config");
    env.seed_usage_cache("work", -60);

    let (ok, _stdout, stderr) = env.run(
        &["sync-usage", "--account", "work", "--max-age", "30m"],
        &env.system_path(),
        None,
    );
    assert!(ok, "fresh usage skip must exit 0; stderr={stderr}");
    assert!(
        !env.fetch_invoked(),
        "usage throttle must short-circuit BEFORE resolving/exec'ing the credential command"
    );
}

#[test]
#[serial]
fn usage_omitted_max_age_attempts_fetch() {
    let env = ThrottleEnv::new("placeholder");
    fs::write(&env.config_path, usage_config(&env.marker)).expect("write usage config");
    // A fresh cache exists, but with --max-age omitted the fetch must be attempted
    // (the credential command runs, dropping the marker).
    env.seed_usage_cache("work", -1);

    let (_ok, _stdout, _stderr) = env.run(
        &["sync-usage", "--account", "work"],
        &env.system_path(),
        None,
    );
    // We don't assert exit status (the downstream Admin fetch will fail without a
    // real endpoint); we only assert the throttle did NOT short-circuit — the
    // credential command was reached.
    assert!(
        env.fetch_invoked(),
        "with --max-age omitted the usage fetch must always be attempted"
    );
}
