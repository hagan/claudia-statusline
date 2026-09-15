//! ANT-32 integration tests for `statusline ant doctor` (Plan 09-03).
//!
//! These prove the diagnostics command's three load-bearing guarantees:
//!
//! 1. **`--json` schema** — `ant doctor --json` emits valid JSON carrying the
//!    documented top-level keys (`ant_on_path`, `caches`, `credentials`,
//!    `audit`, ...).
//! 2. **Passive == no credential exec** (D-13 / T-09-01) — a passive (no
//!    `--probe`) run with a configured account whose `admin_key_command` would
//!    drop a marker file does NOT create the marker: the credential command is
//!    never spawned. Mirrors the no-spawn marker proof in
//!    `ant_invariant_tests.rs`.
//! 3. **No secret in output** (D-13 / T-09-D-01) — neither the human nor the
//!    JSON report contains an `sk-ant-` substring, and the credential SOURCE
//!    labels are present.
//! 4. **The price cache is reported** (plan 12-02 / Phase 11 WR-08) —
//!    `caches.prices` carries `present`/`age`/`stale`/`path`, the reported path
//!    is byte-equal to a path this test seeded, and a 400-day-old cache reads
//!    `stale` under the DEFAULT `[pricing].max_age` (proving the row is wired to
//!    the real threshold rather than hardcoded).
//!
//! **Instrument repair (plan 12-02).** Guarantee 2 was VACUOUS until 12-02: the
//! fake credential program built its marker with the external `touch` utility,
//! but [`DoctorEnv::run`] REPLACES PATH with the controlled bin dir alone.
//! Reproduced live under exactly that environment:
//! `fake-admin-key-cmd: line 2: touch: command not found`, exit 0, no marker —
//! so "the marker is absent" proved nothing. The marker is now written with the
//! shell BUILTIN redirection `: > "<marker>"`, and
//! [`DoctorEnv::assert_marker_can_fire`] is a POSITIVE CONTROL, run under the
//! identical environment, that the no-spawn test calls BEFORE its passive run.
//!
//! Conventions (copied from `ant_usage_cli_tests.rs`): `#![cfg(unix)]`, every
//! PATH/env/XDG-mutating test is `#[serial]`, assertions are on captured output /
//! recorded files (no `cmd | tail` pipeline).

#![cfg(unix)]

mod test_support;

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant doctor` invocation: an isolated
/// HOME + XDG dirs (so caches/config never touch the host), a config file with a
/// chosen `[ant]` section, and a `bin` dir we control for the fake
/// `admin_key_command` (whose only job is to drop a marker file IF run).
struct DoctorEnv {
    home: TempDir,
    bin: TempDir,
    /// The marker a fake `admin_key_command` writes IF (and only if) it is run.
    marker: PathBuf,
    config_path: PathBuf,
}

impl DoctorEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let marker = home.path().join("cred_was_run.marker");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        DoctorEnv {
            home,
            bin,
            marker,
            config_path,
        }
    }

    /// Install a fake `admin_key_command` target: an executable that, IF run,
    /// drops the marker file AND prints a (fake) key on stdout. A passive doctor
    /// must NEVER spawn it (so the marker stays absent).
    ///
    /// The marker line is the shell BUILTIN redirection `: > "<marker>"` and the
    /// key line uses the `printf` builtin, because [`DoctorEnv::run`] replaces
    /// PATH with the controlled bin dir ALONE. The external `touch` utility this
    /// instrument used until plan 12-02 cannot resolve there — verified live,
    /// `/bin/sh` reports `touch: command not found`, exits 0, and no marker is
    /// ever created, which made every no-spawn assertion built on it vacuous.
    /// [`DoctorEnv::assert_marker_can_fire`] is the standing proof that this
    /// cannot silently regress.
    fn install_fake_admin_key(&self, name: &str) {
        let exe = self.bin.path().join(name);
        let body = format!(
            "#!/bin/sh\n\
             : > \"{marker}\"\n\
             printf '%s' 'sk-ant-admin01-FAKE'\n\
             exit 0\n",
            marker = self.marker.display(),
        );
        fs::write(&exe, body).expect("write fake admin key cmd");
        let mut perms = fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    /// The SINGLE definition of the child environment. `Some(v)` sets the
    /// variable, `None` removes it.
    ///
    /// Both [`DoctorEnv::run`] and the [`DoctorEnv::assert_marker_can_fire`]
    /// positive control apply this, so they cannot drift apart — a positive
    /// control that ran under a different environment than the assertion it
    /// guards would prove nothing (in particular, the marker instrument's whole
    /// failure mode was the REPLACED PATH set here).
    fn env_block(&self) -> Vec<(String, Option<OsString>)> {
        vec![
            (
                "HOME".to_string(),
                Some(self.home.path().as_os_str().to_os_string()),
            ),
            (
                "XDG_CACHE_HOME".to_string(),
                Some(self.home.path().join("cache").into_os_string()),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                Some(self.home.path().join("config").into_os_string()),
            ),
            (
                "XDG_DATA_HOME".to_string(),
                Some(self.home.path().join("data").into_os_string()),
            ),
            (
                "STATUSLINE_CONFIG_PATH".to_string(),
                Some(self.config_path.clone().into_os_string()),
            ),
            (
                "PATH".to_string(),
                Some(self.bin.path().as_os_str().to_os_string()),
            ),
            ("NO_COLOR".to_string(), Some(OsString::from("1"))),
            ("ANTHROPIC_API_KEY".to_string(), None),
            ("STATUSLINE_ANT_ACCOUNT".to_string(), None),
        ]
    }

    fn apply_env(&self, cmd: &mut Command) {
        for (key, value) in self.env_block() {
            match value {
                Some(v) => {
                    cmd.env(key, v);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
    }

    fn marker_fired(&self) -> bool {
        self.marker.exists()
    }

    fn clear_marker(&self) {
        let _ = fs::remove_file(&self.marker);
    }

    /// POSITIVE CONTROL for the no-spawn marker instrument.
    ///
    /// Invokes the installed fake credential program DIRECTLY under the
    /// IDENTICAL environment [`DoctorEnv::env_block`] gives the binary (same
    /// replaced PATH, same HOME, same XDG dirs) and asserts the marker appears,
    /// then clears it. Every no-spawn test must call this first: the shipped
    /// instrument could not fire at all until plan 12-02, so its absence proof
    /// was vacuous for the whole of Phases 09-11.
    fn assert_marker_can_fire(&self, name: &str) {
        self.clear_marker();
        let mut cmd = Command::new(self.bin.path().join(name));
        self.apply_env(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = cmd.output().expect("spawn the fake credential program");
        assert!(
            self.marker_fired(),
            "POSITIVE CONTROL FAILED: the fake credential program {name:?} ran but created NO \
             marker, so the instrument can never fire and the no-spawn assertion it backs is \
             VACUOUS. status={:?} stdout={:?} stderr={:?}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        self.clear_marker();
    }

    /// Both roots the binary could resolve `dirs::cache_dir()` to under this
    /// harness:
    ///
    /// * `$HOME/Library/Caches` — the macOS answer (macOS IGNORES
    ///   `XDG_CACHE_HOME`, so only the isolated HOME redirects it);
    /// * `home/cache` — the value [`DoctorEnv::env_block`] assigns to
    ///   `XDG_CACHE_HOME`, which is what `dirs::cache_dir()` returns on Linux.
    ///
    /// Both are seeded so a seeded cache is live on either platform. Seeding
    /// only one makes a cache test read NOTHING and pass vacuously (verified
    /// live in plan 12-01).
    fn cache_roots(&self) -> Vec<PathBuf> {
        vec![
            self.home.path().join("Library").join("Caches"),
            self.home.path().join("cache"),
        ]
    }

    /// Seed `prices.json` under BOTH cache roots and return every path written.
    ///
    /// The `ant/` component is MANDATORY — `price_cache_path()` resolves to
    /// `<cache_dir>/claudia-statusline/ant/prices.json`, and a file written one
    /// level up is a file the binary never reads.
    fn seed_price_cache(&self, body: &str) -> Vec<PathBuf> {
        let mut written = Vec::new();
        for root in self.cache_roots() {
            let path = root
                .join("claudia-statusline")
                .join("ant")
                .join("prices.json");
            let parent = path.parent().expect("seeded path has a parent");
            fs::create_dir_all(parent).expect("create seeded cache dir");
            fs::write(&path, body).expect("write seeded price cache");
            written.push(path);
        }
        written
    }

    /// Run `statusline ant doctor [--json]` with the given optional active
    /// account. Returns `(success, stdout, stderr)`. PATH contains ONLY our
    /// controlled bin dir so the only resolvable `admin_key_command` is the fake.
    fn run(&self, json: bool, account: Option<&str>) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("doctor");
        if json {
            cmd.arg("--json");
        }
        self.apply_env(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(a) = account {
            cmd.env("STATUSLINE_ANT_ACCOUNT", a);
        }
        let out = cmd.output().expect("spawn statusline ant doctor");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// An `[ant]` config mapping account `name` to a fake `admin_key_command`.
fn ant_config_with_account(name: &str, key_cmd: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.{name}]\nadmin_key_command = [\"{key_cmd}\"]\n")
}

// ---------------------------------------------------------------------------
// 1) --json emits the documented schema.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn doctor_json_emits_expected_schema() {
    let env = DoctorEnv::new("[ant]\nenabled = true\n");
    let (ok, stdout, stderr) = env.run(true, None);
    assert!(ok, "doctor --json must exit 0; stderr={stderr}");

    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("doctor --json must emit valid JSON");
    let obj = v.as_object().expect("top-level JSON object");

    for key in [
        "ant_on_path",
        "active_account",
        "caches",
        "credentials",
        "audit",
    ] {
        assert!(
            obj.contains_key(key),
            "missing top-level key {key}: {stdout}"
        );
    }
    // caches has models + usage sub-objects.
    let caches = obj["caches"].as_object().expect("caches object");
    assert!(caches.contains_key("models"), "caches.models present");
    assert!(caches.contains_key("usage"), "caches.usage present");
    // audit reports a clean flag + a scanned count.
    let audit = obj["audit"].as_object().expect("audit object");
    assert!(audit.contains_key("clean"), "audit.clean present");
    assert!(audit.contains_key("scanned"), "audit.scanned present");
}

// ---------------------------------------------------------------------------
// 2) A passive (no --probe) run NEVER executes the credential command.
//    The fake admin_key_command would drop a marker file if run; it stays absent.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn doctor_passive_does_not_exec_credential_command() {
    let env = DoctorEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
    env.install_fake_admin_key("fake-admin-key-cmd");

    // POSITIVE CONTROL FIRST. Without this the test below proves only that
    // SOMETHING did not happen — which was literally true for the wrong reason
    // until plan 12-02 (the `touch`-based instrument could not resolve under the
    // replaced PATH, so the marker never appeared whether or not the credential
    // command ran). Running it here makes this test prove BOTH halves: the
    // instrument CAN fire, and the passive run did not fire it.
    env.assert_marker_can_fire("fake-admin-key-cmd");

    // Passive run (no --probe) with the account active.
    let (ok, stdout, stderr) = env.run(false, Some("work"));
    assert!(ok, "passive doctor must exit 0; stderr={stderr}");

    assert!(
        !env.marker.exists(),
        "passive doctor must NOT run admin_key_command (marker present): {stdout}"
    );
    // And it should still report the usage credential SOURCE label (no exec).
    assert!(
        stdout.contains("admin_key_command (credential command)"),
        "doctor must report the usage credential source label: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 3) The report (human + JSON) shows a credential SOURCE label and NEVER an
//    sk-ant- secret.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn doctor_reports_source_labels_without_secrets() {
    let env = DoctorEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
    env.install_fake_admin_key("fake-admin-key-cmd");

    // Human report.
    let (ok_h, human, err_h) = env.run(false, Some("work"));
    assert!(ok_h, "human doctor must exit 0; stderr={err_h}");
    assert!(
        human.contains("Credentials") && human.contains("admin_key_command (credential command)"),
        "human report must show a credential SOURCE label: {human}"
    );
    assert!(
        !human.contains("sk-ant-"),
        "human report must contain NO sk-ant- secret: {human}"
    );

    // JSON report.
    let (ok_j, jsonout, err_j) = env.run(true, Some("work"));
    assert!(ok_j, "json doctor must exit 0; stderr={err_j}");
    assert!(
        jsonout.contains("admin_key_command (credential command)"),
        "json report must show a credential SOURCE label: {jsonout}"
    );
    assert!(
        !jsonout.contains("sk-ant-"),
        "json report must contain NO sk-ant- secret: {jsonout}"
    );
}

// ---------------------------------------------------------------------------
// 4) `caches.prices` — Phase 11's deferred WR-08, closed by plan 12-02.
// ---------------------------------------------------------------------------

/// A `PriceCache` document at the CURRENT schema version, stamped `fetched_at`.
///
/// Field names taken from `src/pricing/cache.rs::PriceCache` and
/// `src/pricing/mod.rs::PriceEntry`. `schema_version` must equal
/// `PRICE_CACHE_SCHEMA_VERSION` (1); any other value is rejected on read and the
/// cache is reported ABSENT.
fn price_cache_body(fetched_at: chrono::DateTime<chrono::Utc>) -> String {
    serde_json::json!({
        "schema_version": 1,
        "fetched_at": fetched_at.to_rfc3339(),
        "source": "https://example.invalid/12-02-doctor-fixture.json",
        "version": "0123456789abcdef",
        "prices": {
            "claude-4-sonnet-20250514": {
                "input": 3e-6,
                "output": 1.5e-5,
                "cache_creation": 3.75e-6,
                "cache_read": 3e-7,
                "cache_creation_1h": 6e-6
            }
        }
    })
    .to_string()
}

/// `ant doctor --json` reports the price cache as a third `caches` row, driven
/// by the REAL `[pricing].max_age` threshold.
///
/// Two arms, because either alone would be weak:
///
/// * FRESH — `present` is true, `stale` is false, and `caches.prices.path` is
///   byte-EQUAL to one of the paths this test seeded. The equality is the
///   non-vacuity assertion: a `stdout.contains("prices")` substring check would
///   pass against a hardcoded empty row, and a path that merely *looks* right
///   would not prove the binary read the file this test wrote.
/// * STALE — the same document dated 400 days back reads `stale: true` with NO
///   `[pricing]` section in the config, i.e. against the DEFAULT `30d` window.
///   That is what proves the row consults the configured threshold instead of
///   carrying a hardcoded verdict.
#[test]
#[serial]
fn doctor_json_reports_price_cache() {
    let env = DoctorEnv::new("[ant]\nenabled = true\n");

    // --- Arm 1: a FRESH price cache -----------------------------------------
    let seeded = env.seed_price_cache(&price_cache_body(chrono::Utc::now()));
    let (ok, stdout, stderr) = env.run(true, None);
    assert!(ok, "doctor --json must exit 0; stderr={stderr}");
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("doctor --json must emit valid JSON");

    let prices = v["caches"]["prices"]
        .as_object()
        .unwrap_or_else(|| panic!("caches.prices must be an object: {stdout}"));
    assert_eq!(
        prices.get("present"),
        Some(&serde_json::Value::Bool(true)),
        "a seeded, current-schema price cache must be reported present: {stdout}"
    );
    assert_eq!(
        prices.get("stale"),
        Some(&serde_json::Value::Bool(false)),
        "a price cache fetched just now must NOT be stale: {stdout}"
    );
    assert!(
        prices.get("age").map(|a| a.is_string()).unwrap_or(false),
        "caches.prices.age must be a humanized string when the cache is present: {stdout}"
    );

    let reported = prices
        .get("path")
        .and_then(|p| p.as_str())
        .unwrap_or_else(|| panic!("caches.prices.path must be a string: {stdout}"))
        .to_string();
    assert!(
        seeded.iter().any(|p| p.display().to_string() == reported),
        "caches.prices.path must be byte-EQUAL to a path this test seeded — otherwise the \
         row describes a file the test never wrote and the arm is vacuous. \
         reported={reported:?} seeded={seeded:?}"
    );

    // --- Arm 2: the SAME document, 400 days old -----------------------------
    env.seed_price_cache(&price_cache_body(
        chrono::Utc::now() - chrono::Duration::days(400),
    ));
    let (ok2, stdout2, stderr2) = env.run(true, None);
    assert!(ok2, "doctor --json must exit 0; stderr={stderr2}");
    let v2: serde_json::Value =
        serde_json::from_str(stdout2.trim()).expect("doctor --json must emit valid JSON");
    let prices2 = v2["caches"]["prices"]
        .as_object()
        .unwrap_or_else(|| panic!("caches.prices must be an object: {stdout2}"));
    assert_eq!(
        prices2.get("present"),
        Some(&serde_json::Value::Bool(true)),
        "a 400-day-old cache is still PRESENT (staleness is not absence): {stdout2}"
    );
    assert_eq!(
        prices2.get("stale"),
        Some(&serde_json::Value::Bool(true)),
        "a cache 400 days older than the DEFAULT [pricing].max_age (30d) must read stale — \
         if this fails while arm 1 passes, the row is not wired to the real threshold: {stdout2}"
    );
}

/// The human report carries a `Prices:` line alongside `Models:` and `Usage:`.
#[test]
#[serial]
fn doctor_human_report_shows_a_prices_line() {
    let env = DoctorEnv::new("[ant]\nenabled = true\n");

    // Absent first: the row must exist even with nothing on disk.
    let (ok_absent, absent, err_absent) = env.run(false, None);
    assert!(ok_absent, "human doctor must exit 0; stderr={err_absent}");
    assert!(
        absent.contains("Prices: absent"),
        "the human Caches block must report an absent price cache: {absent}"
    );

    env.seed_price_cache(&price_cache_body(chrono::Utc::now()));
    let (ok, human, err) = env.run(false, None);
    assert!(ok, "human doctor must exit 0; stderr={err}");
    assert!(
        human.contains("Prices: present, age "),
        "the human Caches block must report a present price cache with its age: {human}"
    );
}
