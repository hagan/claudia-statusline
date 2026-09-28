//! Wave 0 integration-test scaffold for `statusline ant sync-usage` (Phase 08).
//!
//! Cloned from the `SyncEnv` harness in `ant_sync_cli_tests.rs` (renamed
//! `UsageEnv`) so 08-02 can fill in the assertions without re-deriving the
//! fake-exec / isolated-HOME / PATH-shaping plumbing. Per VALIDATION.md
//! §"Per-Task Verification Map", the Wave 0 integration test fns declared here
//! are `sync_usage_writes_after_both`, `degrades_401_403_differentiated`,
//! `per_account_no_bleed`, and `no_key_in_argv`. They are wired against the
//! harness but their assertions are `todo!("implemented in 08-02")` behind
//! `#[ignore]` so the file compiles and the suite stays green until 08-02.
//!
//! Like the models harness, all PATH/env/XDG-mutating tests are `#[serial]` and
//! assert on exit status / recorded files directly (no `cmd | tail` pipeline).

#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant sync-usage` invocation: an isolated
/// HOME + XDG dirs (so the per-account usage cache lands somewhere predictable
/// and never touches the host), a config file with a chosen `[ant]` section, and
/// a `bin` dir we control to shape PATH (fake `admin_key_command` + fake `curl`).
#[allow(dead_code)]
struct UsageEnv {
    home: TempDir,
    bin: TempDir,
    /// Where a fake exec records its argv/env (one line per invocation).
    record: PathBuf,
    config_path: PathBuf,
}

#[allow(dead_code)]
impl UsageEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let record = home.path().join("record.log");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        UsageEnv {
            home,
            bin,
            record,
            config_path,
        }
    }

    /// PATH that contains ONLY our controlled bin dir (no real curl/ant, and no
    /// system tools — used for the "no fetch tool at all" failure branches).
    fn isolated_path(&self) -> String {
        self.bin.path().display().to_string()
    }

    /// PATH with our bin dir first, then a curated set of real system dirs so the
    /// fake shell scripts can still call `cat`/`env`/`touch`. We deliberately do
    /// NOT include any dir that contains a real `ant`, so the fetch resolves only
    /// a fake (or nothing) — keeping the test deterministic on machines that DO
    /// have `ant` installed.
    /// When `/bin` or `/usr/bin` holds a real `ant` (the Ubuntu CI image ships
    /// Apache Ant) that directory is dropped, so fake scripts must use shell
    /// builtins or absolute paths (`/bin/cat`, `/usr/bin/env`), never bare tools.
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

    /// Install a fake `admin_key_command` target: an executable that prints a
    /// canned org-admin key on stdout (so the sync can resolve a credential
    /// without a real keychain). 08-02 points the config's `admin_key_command`
    /// at this script.
    fn install_fake_admin_key(&self, name: &str, key: &str) {
        let body = format!("printf '%s' '{key}'\nexit 0\n");
        self.install_fake(name, &body);
    }

    /// Install a fake `curl` that records argv, slurps its stdin config to
    /// `stdin_capture`, and emits the given JSON body on stdout.
    fn install_fake_curl(&self, stdin_capture: &Path, json_body: &str) {
        let body = format!(
            "echo \"ARGV: $@\" >> \"{rec}\"\n\
             /bin/cat >> \"{cap}\"\n\
             echo '{json}'\n\
             exit 0\n",
            rec = self.record.display(),
            cap = stdin_capture.display(),
            json = json_body,
        );
        self.install_fake("curl", &body);
    }

    /// Run `statusline ant sync-usage [--quiet]` with the given PATH, optional
    /// `ANTHROPIC_API_KEY`, and optional `STATUSLINE_ANT_ACCOUNT`. Returns
    /// (status_success, stdout, stderr).
    fn run(
        &self,
        path: &str,
        key: Option<&str>,
        account: Option<&str>,
        quiet: bool,
    ) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("sync-usage");
        if quiet {
            cmd.arg("--quiet");
        }
        cmd.env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
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
        if let Some(a) = account {
            cmd.env("STATUSLINE_ANT_ACCOUNT", a);
        }
        let out = cmd.output().expect("spawn statusline ant sync-usage");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Find a written per-account `usage/<account>.json` (if any) under HOME.
    fn find_usage_cache(&self) -> Option<PathBuf> {
        find_usage_json(self.home.path())
    }

    fn record_contents(&self) -> String {
        fs::read_to_string(&self.record).unwrap_or_default()
    }
}

/// Recursively search for a specifically-named `*.json` under an `ant/usage/`
/// directory below `root` (e.g. `a.json`, `b.json`).
fn find_named_usage_json(root: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_named_usage_json(&p, file_name) {
                return Some(found);
            }
        } else if p.file_name().map(|n| n == file_name).unwrap_or(false)
            && p.parent()
                .and_then(|d| d.file_name())
                .map(|n| n == "usage")
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

/// Recursively search for any `*.json` under an `ant/usage/` directory below `root`.
fn find_usage_json(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_usage_json(&p) {
                return Some(found);
            }
        } else if p.extension().map(|e| e == "json").unwrap_or(false)
            && p.parent()
                .and_then(|d| d.file_name())
                .map(|n| n == "usage")
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

/// A canned body valid as BOTH a cost envelope (`results[].amount`) and a usage
/// envelope (`results[].model` + token fields). The fake curl returns it for
/// every endpoint; unknown fields are ignored by each parser.
const FAKE_BODY: &str = "{\"data\":[{\"results\":[{\"amount\":\"500\",\"model\":\"opus\",\"uncached_input_tokens\":100,\"output_tokens\":40}]}],\"has_more\":false}";

/// Build an `[ant]` config that maps account `name` to a fake `admin_key_command`
/// (the fake key-printer installed under our controlled bin dir).
fn ant_config_with_account(name: &str, key_cmd: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.{name}]\nadmin_key_command = [\"{key_cmd}\"]\n")
}

// ---------------------------------------------------------------------------
// REAL (08-02): both endpoints succeed -> the per-account usage cache is written
// (all-or-nothing two-endpoint gate). VALIDATION.md row `sync_usage_writes_after_both`.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn sync_usage_writes_after_both() {
    let env = UsageEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
    env.install_fake_admin_key("fake-admin-key-cmd", "sk-ant-admin01-FAKE");
    let stdin_capture = env.home.path().join("curl_stdin.log");
    env.install_fake_curl(&stdin_capture, FAKE_BODY);

    let (ok, stdout, stderr) = env.run(&env.system_path(), None, Some("work"), false);
    assert!(ok, "sync must exit 0; stderr={stderr}");

    let cache = env
        .find_usage_cache()
        .expect("usage/work.json must be written after both endpoints");
    assert!(
        cache.ends_with("usage/work.json"),
        "wrong cache path: {cache:?}"
    );
    let content = fs::read_to_string(&cache).expect("read cache");
    // amount "500" cents summed (today + MTD both fetch the fake) => $5.00 per window.
    assert!(
        content.contains("\"today_usd\""),
        "today_usd present: {content}"
    );
    assert!(
        content.contains("\"mtd_usd\""),
        "mtd_usd present: {content}"
    );
    assert!(
        content.contains("opus"),
        "by-model tokens present: {content}"
    );
    assert!(content.contains("\"tz\""), "tz label present: {content}");
    // The summary prints totals but never the key.
    assert!(!stdout.contains("sk-ant-"), "no key in summary: {stdout}");
    assert!(
        stdout.contains("work"),
        "summary names the account: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// REAL (08-02): 401 (bad key) vs 403 (non-admin/unsupported account) produce
// DIFFERENTIATED, key-free messages + non-zero exit + no cache.
// VALIDATION.md row `degrades_401_403_differentiated`.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn degrades_401_403_differentiated() {
    // Case 401: fake curl exits 22 (--fail-with-body) and writes a trailing
    // "\n401" (the --write-out %{http_code}).
    {
        let env = UsageEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
        env.install_fake_admin_key("fake-admin-key-cmd", "sk-ant-admin01-FAKE");
        // Emit a body then the trailing http_code, then exit 22.
        env.install_fake(
            "curl",
            &format!(
                "echo \"ARGV: $@\" >> \"{rec}\"\n\
                 /bin/cat > /dev/null\n\
                 printf '%s\\n401' '{{\"error\":\"unauthorized\"}}'\n\
                 exit 22\n",
                rec = env.record.display(),
            ),
        );

        let (ok, _stdout, stderr) = env.run(&env.system_path(), None, Some("work"), false);
        assert!(!ok, "401 must exit non-zero");
        assert!(
            stderr.contains("invalid or expired") || stderr.contains("401"),
            "401 wording: {stderr}"
        );
        assert!(
            !stderr.contains("lacks Admin API access"),
            "must not be the 403 wording"
        );
        assert!(
            !stderr.contains("sk-ant-"),
            "no key in 401 message: {stderr}"
        );
        assert!(
            env.find_usage_cache().is_none(),
            "no cache may be written on 401 (all-or-nothing)"
        );
    }

    // Case 403: same shape, trailing "\n403".
    {
        let env = UsageEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
        env.install_fake_admin_key("fake-admin-key-cmd", "sk-ant-admin01-FAKE");
        env.install_fake(
            "curl",
            &format!(
                "echo \"ARGV: $@\" >> \"{rec}\"\n\
                 /bin/cat > /dev/null\n\
                 printf '%s\\n403' '{{\"error\":\"forbidden\"}}'\n\
                 exit 22\n",
                rec = env.record.display(),
            ),
        );

        let (ok, _stdout, stderr) = env.run(&env.system_path(), None, Some("work"), false);
        assert!(!ok, "403 must exit non-zero");
        assert!(
            stderr.contains("lacks Admin API access"),
            "403 wording: {stderr}"
        );
        assert!(
            !stderr.contains("invalid or expired"),
            "must not be the 401 wording"
        );
        assert!(
            !stderr.contains("sk-ant-"),
            "no key in 403 message: {stderr}"
        );
        assert!(
            env.find_usage_cache().is_none(),
            "no cache may be written on 403 (all-or-nothing)"
        );
    }
}

// ---------------------------------------------------------------------------
// REAL (08-02): syncing one account writes ONLY its own usage/<name>.json and
// never clobbers another account's slice (D-10 single-file write).
// VALIDATION.md row `per_account_no_bleed`.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn per_account_no_bleed() {
    // One config carries two accounts, each with its own (fake) key command.
    let config = "[ant]\nenabled = true\n\n\
         [ant.accounts.a]\nadmin_key_command = [\"fake-key-a\"]\n\n\
         [ant.accounts.b]\nadmin_key_command = [\"fake-key-b\"]\n";
    let env = UsageEnv::new(config);
    env.install_fake_admin_key("fake-key-a", "sk-ant-admin01-A");
    env.install_fake_admin_key("fake-key-b", "sk-ant-admin01-B");
    let stdin_capture = env.home.path().join("curl_stdin.log");
    env.install_fake_curl(&stdin_capture, FAKE_BODY);

    // Sync account a.
    let (ok_a, _o, e_a) = env.run(&env.system_path(), None, Some("a"), true);
    assert!(ok_a, "sync a must succeed; stderr={e_a}");
    let path_a = find_named_usage_json(env.home.path(), "a.json")
        .expect("a.json must exist after syncing a");
    let a_before = fs::read_to_string(&path_a).expect("read a.json");

    // Sync account b.
    let (ok_b, _o2, e_b) = env.run(&env.system_path(), None, Some("b"), true);
    assert!(ok_b, "sync b must succeed; stderr={e_b}");
    let path_b = find_named_usage_json(env.home.path(), "b.json")
        .expect("b.json must exist after syncing b");

    // a.json is unchanged (syncing b did not clobber a's slice — D-10).
    let a_after = fs::read_to_string(&path_a).expect("re-read a.json");
    assert_eq!(a_before, a_after, "syncing b must not modify a.json");
    // Each slice records its own account label.
    assert!(
        a_after.contains("\"account\": \"a\""),
        "a.json names a: {a_after}"
    );
    let b_content = fs::read_to_string(&path_b).expect("read b.json");
    assert!(
        b_content.contains("\"account\": \"b\""),
        "b.json names b: {b_content}"
    );
}

// ---------------------------------------------------------------------------
// REAL (08-02): the org-admin key never appears in curl argv NOR the credential
// command argv (it arrives via the leak-free STDIN config). VALIDATION.md row
// `no_key_in_argv` / T-08-KEY.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn no_key_in_argv() {
    let env = UsageEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
    let fake_key = "sk-ant-admin01-LEAKCHECK";
    // The fake admin_key_command records its OWN argv too, so we can assert the
    // key never appears there (the key is the command's OUTPUT, not its argv).
    env.install_fake(
        "fake-admin-key-cmd",
        &format!(
            "echo \"KEYCMD_ARGV: $@\" >> \"{rec}\"\n\
             printf '%s' '{key}'\n\
             exit 0\n",
            rec = env.record.display(),
            key = fake_key,
        ),
    );
    let stdin_capture = env.home.path().join("curl_stdin.log");
    env.install_fake_curl(&stdin_capture, FAKE_BODY);

    let (ok, stdout, stderr) = env.run(&env.system_path(), None, Some("work"), false);
    assert!(ok, "sync must succeed; stderr={stderr}");

    // The recorded argv (curl + credential command) must NOT contain the key.
    let rec = env.record_contents();
    assert!(rec.contains("ARGV:"), "curl must have been invoked: {rec}");
    assert!(
        !rec.contains(fake_key),
        "the key must NOT appear in any recorded argv (curl or credential cmd): {rec}"
    );

    // The key reached curl only via the stdin config (x-api-key header).
    let stdin_cfg = fs::read_to_string(&stdin_capture).expect("read curl stdin capture");
    assert!(
        stdin_cfg.contains(&format!("x-api-key: {fake_key}")),
        "the x-api-key config must arrive via curl STDIN: {stdin_cfg}"
    );

    // The key never leaks to stdout/stderr.
    assert!(!stdout.contains(fake_key), "no key in summary");
    assert!(!stderr.contains(fake_key), "no key in stderr");
}
