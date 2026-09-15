#![cfg(unix)]

//! Phase 12 config-validation suite: the shared spawned-binary fixture
//! (`ConfigEnv`) plus the D-18-scoped SC3 render-stability proofs.
//!
//! Every later plan in Phase 12 builds on `ConfigEnv`, so this file's first job
//! is to make sure the fixture's INSTRUMENTS actually observe what they claim.
//! The round-1 cross-AI review found two independent ways the originally-planned
//! fixture would have been VACUOUS, both reproduced live against this repo:
//!
//! 1. **Wrong cache paths.** `price_cache_path()` resolves to
//!    `<dirs::cache_dir()>/claudia-statusline/ant/prices.json` — the `ant/`
//!    component is mandatory — and `dirs::cache_dir()` is `$HOME/Library/Caches`
//!    on macOS (where `XDG_CACHE_HOME` is IGNORED) and `$XDG_CACHE_HOME` on
//!    Linux. Seeding the wrong root means the binary reads NOTHING, and a "four
//!    cache states" test then exercises "absent" four times and still passes.
//!    Verified live on this machine: seeding ONLY `home/Library/Caches/...` makes
//!    `ant sync-pricing --max-age 365d` report the cache fresh; seeding ONLY the
//!    `XDG_CACHE_HOME` root does not. Hence [`ConfigEnv::seed_cache`] writes BOTH
//!    roots and every seeding test proves CONSUMPTION rather than mere presence.
//! 2. **A dead marker instrument.** `tests/ant_doctor_tests.rs` builds its fake
//!    credential program around the external `touch` utility, but the harness
//!    replaces PATH with the temp bin dir ONLY. Reproduced here: `/bin/sh`
//!    cannot resolve it, prints `touch: command not found`, exits 0, and the
//!    marker is never created — an instrument that can never fire, so every
//!    no-spawn assertion built on it is vacuous. This fixture's marker uses the
//!    shell BUILTIN redirection `: > "<marker>"` and ships
//!    [`ConfigEnv::assert_marker_can_fire`], a positive control that runs under
//!    the IDENTICAL restricted-PATH environment.
//!
//! Conventions (copied from `tests/ant_doctor_tests.rs`): `#![cfg(unix)]`, every
//! test is `#[serial]` (they all spawn a child against shared temp state and
//! several drive process-global cache paths), and assertions are on captured
//! output, never on a `cmd | tail` pipeline.
//!
//! Note on `Self::cache_roots(self)`: the roots accessor is deliberately called
//! in UFCS form so that this file contains NO occurrence of the literal
//! substring `<dot>cache`. The mis-seed defect above wrote to a dot-prefixed
//! cache dir under HOME — a directory nothing in this harness ever points at —
//! so a grep for that substring is this file's own tripwire against the
//! regression it exists to prevent.

mod test_support;

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// One spawned-binary invocation: `(success, exit code, RAW stdout bytes,
/// stderr as a lossy String)`.
///
/// stdout is returned as BYTES on purpose: plan 12-12's terminal-escape proof
/// asserts on raw bytes, and a lossy `String` conversion would destroy exactly
/// the evidence it needs.
type RunOutcome = (bool, Option<i32>, Vec<u8>, String);

/// A fully isolated environment for one `statusline` invocation: an isolated
/// HOME + all three XDG dirs (so config/cache/data never reach the developer's
/// real files), a config file at a known path, and a `bin` dir we control that
/// is the ONLY entry on PATH.
struct ConfigEnv {
    home: TempDir,
    bin: TempDir,
    /// The marker a fake `admin_key_command` writes IF (and only if) it is run.
    marker: PathBuf,
    config_path: PathBuf,
}

#[allow(dead_code)]
impl ConfigEnv {
    /// Build an environment whose active config file contains `config_body`.
    fn new(config_body: &str) -> ConfigEnv {
        let env = ConfigEnv::new_without_config();
        fs::write(&env.config_path, config_body).expect("write config body");
        env
    }

    /// Build an environment with NO config file on disk: `config_path` names a
    /// file that does not exist. Needed by the D-10 "no config loaded" cases.
    fn new_without_config() -> ConfigEnv {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let marker = home.path().join("spawned.marker");
        let config_path = home.path().join("statusline.toml");
        ConfigEnv {
            home,
            bin,
            marker,
            config_path,
        }
    }

    fn home_path(&self) -> &Path {
        self.home.path()
    }

    // -------------------------------------------------------------------
    // The ONE shared environment definition.
    // -------------------------------------------------------------------

    /// The single source of truth for the child environment. `Some(v)` sets the
    /// variable, `None` removes it.
    ///
    /// `run`, `run_with_stdin`, `run_with_account`, `run_unset_config` AND the
    /// [`ConfigEnv::assert_marker_can_fire`] positive control all apply this, so
    /// the entry points cannot drift apart — a positive control that ran under a
    /// different environment than the assertion it guards would prove nothing.
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

    /// The single spawn implementation behind all four entry points.
    fn spawn_binary(
        &self,
        args: &[&str],
        payload: Option<&str>,
        account: Option<&str>,
        unset_config: bool,
    ) -> RunOutcome {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.args(args);
        self.apply_env(&mut cmd);
        if let Some(a) = account {
            cmd.env("STATUSLINE_ANT_ACCOUNT", a);
        }
        if unset_config {
            cmd.env_remove("STATUSLINE_CONFIG_PATH");
            cmd.env_remove("STATUSLINE_CONFIG");
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match payload {
            Some(_) => {
                cmd.stdin(Stdio::piped());
            }
            None => {
                cmd.stdin(Stdio::null());
            }
        }

        let out = match payload {
            Some(body) => {
                let mut child = cmd.spawn().expect("spawn statusline");
                child
                    .stdin
                    .as_mut()
                    .expect("child stdin")
                    .write_all(body.as_bytes())
                    .expect("write payload to child stdin");
                child.wait_with_output().expect("wait for statusline")
            }
            None => cmd.output().expect("spawn statusline"),
        };

        (
            out.status.success(),
            out.status.code(),
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Run the binary with `args` and no stdin.
    fn run(&self, args: &[&str]) -> RunOutcome {
        self.spawn_binary(args, None, None, false)
    }

    /// Run the binary with `args`, writing `payload` to its stdin (the render
    /// path).
    fn run_with_stdin(&self, args: &[&str], payload: &str) -> RunOutcome {
        self.spawn_binary(args, Some(payload), None, false)
    }

    /// Like [`ConfigEnv::run`] but with `STATUSLINE_ANT_ACCOUNT` SET to
    /// `account` instead of removed.
    fn run_with_account(&self, account: &str, args: &[&str]) -> RunOutcome {
        self.spawn_binary(args, None, Some(account), false)
    }

    /// Like [`ConfigEnv::run`] but with BOTH config-path env vars removed, so
    /// the binary exercises candidates 3 and 4 of its config search order.
    fn run_unset_config(&self, args: &[&str]) -> RunOutcome {
        self.spawn_binary(args, None, None, true)
    }

    // -------------------------------------------------------------------
    // Cache seeding — the REAL roots and the REAL layout.
    // -------------------------------------------------------------------

    /// Both roots the binary could resolve `dirs::cache_dir()` to under this
    /// harness:
    ///
    /// * `$HOME/Library/Caches` — the macOS answer (macOS IGNORES
    ///   `XDG_CACHE_HOME`, so only the isolated HOME redirects it);
    /// * `home/cache` — the value this harness assigns to `XDG_CACHE_HOME`,
    ///   which is what `dirs::cache_dir()` returns on Linux.
    ///
    /// Both are written by [`ConfigEnv::seed_cache`] so a seeded cache is live
    /// on either platform. The in-repo statement of this divergence is the
    /// `isolate_cache()` helper in `src/utils.rs`.
    fn cache_roots(&self) -> Vec<PathBuf> {
        vec![
            self.home.path().join("Library").join("Caches"),
            self.home.path().join("cache"),
        ]
    }

    /// Seed a cache file under BOTH roots and return every path written.
    ///
    /// `rel` is the path BELOW the `claudia-statusline` directory and therefore
    /// ALWAYS begins with `ant/` — all three caches live in that subdirectory.
    /// The three real spellings are:
    ///
    /// * `"ant/prices.json"`
    /// * `"ant/models.json"`
    /// * `"ant/usage/<account>.json"`
    ///
    /// A caller passing `"prices.json"` (no `ant/` component) is seeding a path
    /// the binary NEVER reads: the file lands under the isolated HOME, a
    /// `starts_with(home)` assertion still passes, and the test silently becomes
    /// vacuous. That is the exact defect this fixture was rewritten to prevent.
    fn seed_cache(&self, rel: &str, body: &str) -> Vec<PathBuf> {
        assert!(
            rel.starts_with("ant/"),
            "seed_cache: `rel` must begin with the mandatory `ant/` component \
             (got {rel:?}); the binary reads nothing outside it"
        );
        let mut written = Vec::new();
        for root in Self::cache_roots(self) {
            let path = root.join("claudia-statusline").join(rel);
            let parent = path.parent().expect("seeded path has a parent");
            fs::create_dir_all(parent).expect("create seeded cache dir");
            fs::write(&path, body).expect("write seeded cache body");
            written.push(path);
        }
        written
    }

    /// Remove the whole `claudia-statusline` tree under both roots.
    fn clear_caches(&self) {
        for root in Self::cache_roots(self) {
            let _ = fs::remove_dir_all(root.join("claudia-statusline"));
        }
    }

    // -------------------------------------------------------------------
    // The no-spawn marker instrument.
    // -------------------------------------------------------------------

    /// Install a fake `admin_key_command` target into the controlled bin dir:
    /// an executable that, IF run, drops the marker AND prints a fake key.
    ///
    /// The marker line is the shell BUILTIN redirection `: > "<marker>"`, and
    /// the key line uses the `printf` builtin. Both resolve with PATH set to the
    /// bin dir alone. The external `touch` utility does NOT: verified live under
    /// this harness's replaced PATH, `/bin/sh` reports it as not found, exits 0,
    /// and the marker is never created — which is why the shipped
    /// `tests/ant_doctor_tests.rs` instrument can never fire.
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

    fn marker_fired(&self) -> bool {
        self.marker.exists()
    }

    fn clear_marker(&self) {
        let _ = fs::remove_file(&self.marker);
    }

    /// POSITIVE CONTROL for the marker instrument.
    ///
    /// Invokes the installed fake program DIRECTLY under the identical
    /// environment [`ConfigEnv::env_block`] gives the binary (same replaced
    /// PATH, same HOME, same XDG dirs) and asserts the marker appears. Every
    /// no-spawn test in Phase 12 must call this, because a silent instrument
    /// makes every no-spawn assertion in the phase vacuous.
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
             marker, so the instrument can never fire and EVERY no-spawn assertion in Phase 12 \
             built on it is VACUOUS. status={:?} stdout={:?} stderr={:?}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        self.clear_marker();
    }
}

// ---------------------------------------------------------------------------
// Cache bodies.
// ---------------------------------------------------------------------------

/// A `PriceCache` document at the CURRENT schema version whose `fetched_at` is
/// now: within any plausible `[pricing].max_age` window (default `30d`).
#[allow(dead_code)]
fn fresh_price_cache_body() -> String {
    price_cache_body(chrono::Utc::now())
}

/// The same document dated 400 days ago — far beyond the default `30d`
/// `[pricing].max_age`, so a synced table built from it is demoted as stale.
#[allow(dead_code)]
fn stale_price_cache_body() -> String {
    price_cache_body(chrono::Utc::now() - chrono::Duration::days(400))
}

/// Field names taken from `src/pricing/cache.rs::PriceCache` and
/// `src/pricing/mod.rs::PriceEntry` (`cache_creation_1h` is the ONE optional
/// dimension). `schema_version` equals `PRICE_CACHE_SCHEMA_VERSION` (1); any
/// other value is rejected on read and the cache is treated as absent.
#[allow(dead_code)]
fn price_cache_body(fetched_at: chrono::DateTime<chrono::Utc>) -> String {
    serde_json::json!({
        "schema_version": 1,
        "fetched_at": fetched_at.to_rfc3339(),
        "source": "https://example.invalid/phase-12-fixture.json",
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

/// A `ModelsCache` document at the current schema version, dated now. Field
/// names from `src/ant/cache.rs::ModelsCache` / `ModelEntry`.
#[allow(dead_code)]
fn fresh_models_cache_body() -> String {
    serde_json::json!({
        "schema_version": 1,
        "fetched_at": chrono::Utc::now().to_rfc3339(),
        "models": {
            "claude-opus-4-8": { "max_input_tokens": 200_000 }
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// TOML fixture bodies. Named here because later plans assert against them.
// ---------------------------------------------------------------------------

/// A valid config exercising several sections, expected to validate clean.
#[allow(dead_code)]
fn clean_config() -> String {
    "[display]\n\
     show_git = true\n\
     progress_bar_width = 10\n\
     \n\
     [layout]\n\
     preset = \"default\"\n\
     \n\
     [burn_rate]\n\
     mode = \"wall_clock\"\n\
     \n\
     [pricing]\n\
     source = \"auto\"\n\
     max_age = \"30d\"\n\
     \n\
     [ant]\n\
     enabled = false\n\
     usage_stale_after = \"30m\"\n\
     models_stale_after = \"48h\"\n"
        .to_string()
}

/// `display.show_gti` — a typo for `show_git`, i.e. an unknown key.
#[allow(dead_code)]
fn config_with_unknown_key() -> String {
    "[display]\nshow_git = true\nshow_gti = true\n".to_string()
}

/// `[pricing].aliasess` — an unknown key in the section whose lenient
/// deserializer currently swallows it (the QUAL-02 blindness regression).
#[allow(dead_code)]
fn config_with_pricing_typo() -> String {
    "[pricing]\nsource = \"auto\"\naliasess = { foo = \"claude-opus-4-8\" }\n".to_string()
}

/// `display.progress_bar_width` is a `usize`; a string is a type error.
#[allow(dead_code)]
fn config_with_type_error() -> String {
    "[display]\nprogress_bar_width = \"wide\"\n".to_string()
}

/// Two bad enum values in two different sections.
#[allow(dead_code)]
fn config_with_bad_enum() -> String {
    "[layout]\npreset = \"compakt\"\n\n[burn_rate]\nmode = \"wallclock\"\n".to_string()
}

/// Two unparseable single-unit durations, in two different sections.
#[allow(dead_code)]
fn config_with_bad_durations() -> String {
    "[pricing]\nmax_age = \"30x\"\n\n[ant]\nenabled = true\nusage_stale_after = \"soon\"\n"
        .to_string()
}

/// An `[ant]` config mapping account `name` to a fake `admin_key_command`.
/// Shape copied verbatim from `tests/ant_doctor_tests.rs`.
#[allow(dead_code)]
fn ant_config_with_account(name: &str, key_cmd: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.{name}]\nadmin_key_command = [\"{key_cmd}\"]\n")
}

/// A `[sync]` table — a section that exists ONLY in a `turso-sync` build, and
/// must therefore not be reported as an unknown key there.
#[allow(dead_code)]
fn config_with_sync_section() -> String {
    "[sync]\nenabled = false\n".to_string()
}

/// A config whose `admin_key_command` ARRAY is left UNCLOSED, so the TOML
/// parser fails at the SYNTAX level with `sentinel` as the last element.
///
/// `admin_key_command` sits under `[ant.accounts.<name>]` because that is the
/// ONLY level at which it is a real field; at `[ant]` level it is merely an
/// unknown key that never reaches the deserializer, so a fixture placed there
/// leaks nothing and proves nothing.
#[allow(dead_code)]
fn config_with_secret_in_syntax_error(sentinel: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = [\"{sentinel}\"\n")
}

/// A config whose `admin_key_command` is a bare STRING equal to `sentinel`,
/// so the parser fails at the TYPE level (it expects a sequence).
#[allow(dead_code)]
fn config_with_secret_in_type_error(sentinel: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = \"{sentinel}\"\n")
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn parse_json(stdout: &[u8], what: &str) -> serde_json::Value {
    serde_json::from_slice(stdout).unwrap_or_else(|e| {
        panic!(
            "{what} must emit valid JSON ({e}): {:?}",
            String::from_utf8_lossy(stdout)
        )
    })
}

// ===========================================================================
// Task 1 smoke tests: the fixture's own non-vacuity proofs.
// ===========================================================================

/// The fixture isolates HOME / XDG / PATH, the binary resolves its caches inside
/// the isolated HOME, a passive run spawns no credential command — AND the
/// marker instrument that last assertion rests on is proven able to fire.
#[test]
#[serial]
fn fixture_env_is_isolated() {
    let env = ConfigEnv::new(&ant_config_with_account("work", "fake-admin-key"));
    env.install_fake_admin_key("fake-admin-key");

    let (ok, code, stdout, stderr) = env.run(&["ant", "doctor", "--json"]);
    assert!(
        ok,
        "doctor --json must exit 0; code={code:?} stderr={stderr}"
    );

    let v = parse_json(&stdout, "ant doctor --json");
    let models_path = v["caches"]["models"]["path"]
        .as_str()
        .expect("caches.models.path must be a string")
        .to_string();
    let home = env.home_path().display().to_string();
    assert!(
        models_path.starts_with(&home),
        "the reported models cache path must live inside the isolated HOME: \
         path={models_path} home={home}"
    );

    assert!(
        !env.marker_fired(),
        "a passive doctor run must NOT spawn admin_key_command (marker present)"
    );

    // ...and the assertion above is only meaningful if the marker CAN appear.
    env.assert_marker_can_fire("fake-admin-key");
}

/// NON-VACUITY PROOF for the seeder: the seeded file is what the binary READS.
///
/// The `present` flag flipping `false` -> `true` proves CONSUMPTION; the
/// path EQUALITY against the seeded list proves the correct root was chosen. A
/// `starts_with(home)` check proves neither — a file seeded at a path the binary
/// never reads still lives under HOME.
#[test]
#[serial]
fn seeded_cache_is_actually_consumed() {
    let env = ConfigEnv::new(&clean_config());

    env.clear_caches();
    let (ok, code, stdout, stderr) = env.run(&["ant", "doctor", "--json"]);
    assert!(ok, "doctor must exit 0; code={code:?} stderr={stderr}");
    let before = parse_json(&stdout, "ant doctor --json");
    assert_eq!(
        before["caches"]["models"]["present"].as_bool(),
        Some(false),
        "with caches cleared the models cache must be reported ABSENT: {before}"
    );

    let seeded = env.seed_cache("ant/models.json", &fresh_models_cache_body());
    let (ok, code, stdout, stderr) = env.run(&["ant", "doctor", "--json"]);
    assert!(ok, "doctor must exit 0; code={code:?} stderr={stderr}");
    let after = parse_json(&stdout, "ant doctor --json");
    assert_eq!(
        after["caches"]["models"]["present"].as_bool(),
        Some(true),
        "the seeded models cache must be CONSUMED (present=true). If this fails the seeder is \
         writing a path the binary never reads, and every cache test in Phase 12 is vacuous. \
         seeded={seeded:?} report={after}"
    );

    let reported = after["caches"]["models"]["path"]
        .as_str()
        .expect("caches.models.path must be a string");
    assert!(
        seeded.iter().any(|p| p.as_path() == Path::new(reported)),
        "the binary-reported cache path must EQUAL one of the seeded paths \
         (reported={reported} seeded={seeded:?})"
    );
}
