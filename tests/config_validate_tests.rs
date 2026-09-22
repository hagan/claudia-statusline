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
            // REMOVED, never set: plan 12-12's render-path leak regression must
            // observe the DEFAULT log level (`warn`, `src/main.rs:383`), which
            // is the level the shipped leak was reachable at. A `RUST_LOG`
            // inherited from the developer's shell would silently move that
            // test to a different level and make it prove something else.
            ("RUST_LOG".to_string(), None),
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

    /// The single spawn implementation behind all five entry points.
    fn spawn_binary(
        &self,
        args: &[&str],
        payload: Option<&str>,
        account: Option<&str>,
        unset_config: bool,
        extra: &[(&str, &str)],
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
        // Layered LAST and always named at the call site, so the shared
        // `env_block` stays the single source of truth for the baseline.
        for (key, value) in extra {
            cmd.env(key, value);
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
        self.spawn_binary(args, None, None, false, &[])
    }

    /// Run the binary with `args`, writing `payload` to its stdin (the render
    /// path).
    fn run_with_stdin(&self, args: &[&str], payload: &str) -> RunOutcome {
        self.spawn_binary(args, Some(payload), None, false, &[])
    }

    /// Like [`ConfigEnv::run`] but with `STATUSLINE_ANT_ACCOUNT` SET to
    /// `account` instead of removed.
    fn run_with_account(&self, account: &str, args: &[&str]) -> RunOutcome {
        self.spawn_binary(args, None, Some(account), false, &[])
    }

    /// Like [`ConfigEnv::run`] but with BOTH config-path env vars removed, so
    /// the binary exercises candidates 3 and 4 of its config search order.
    fn run_unset_config(&self, args: &[&str]) -> RunOutcome {
        self.spawn_binary(args, None, None, true, &[])
    }

    /// Like [`ConfigEnv::run`] but with `extra` environment variables layered on
    /// top of the shared block.
    ///
    /// Needed by plan 12-08 to make a PROBED misplacement path coincide with a
    /// real, losing search CANDIDATE (`STATUSLINE_CONFIG`), which is the only
    /// arrangement under which the probe's candidate-exclusion filter is
    /// load-bearing. It still routes through [`ConfigEnv::apply_env`], so the
    /// baseline isolation cannot drift away from the other entry points.
    fn run_with_extra_env(&self, extra: &[(&str, &str)], args: &[&str]) -> RunOutcome {
        self.spawn_binary(args, None, None, false, extra)
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

    // -------------------------------------------------------------------
    // The PATH-resolved-invocation log instrument (plan 12-12).
    // -------------------------------------------------------------------

    /// Path of the shared invocation log written by
    /// [`ConfigEnv::install_logging_fakes`].
    fn invocation_log(&self) -> PathBuf {
        self.home.path().join("invocations.log")
    }

    /// Install one fake executable per name into the controlled bin dir. Each
    /// APPENDS its own name to [`ConfigEnv::invocation_log`] if it is run.
    ///
    /// Like [`ConfigEnv::install_fake_admin_key`] the body uses only shell
    /// BUILTINS — `printf` and `>>` — because the harness REPLACES PATH with
    /// the bin dir alone, so an external utility such as `touch` or `tee` would
    /// not resolve and the instrument could never fire.
    fn install_logging_fakes(&self, names: &[&str]) {
        let log = self.invocation_log();
        for name in names {
            let exe = self.bin.path().join(name);
            let body = format!(
                "#!/bin/sh\n\
                 printf '%s\\n' {name} >> \"{log}\"\n\
                 exit 0\n",
                name = name,
                log = log.display(),
            );
            fs::write(&exe, body).expect("write fake logging program");
            let mut perms = fs::metadata(&exe).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&exe, perms).expect("chmod +x");
        }
    }

    /// The names recorded in the invocation log, in order. An absent log is an
    /// empty list — the log file is created by the first append, not up front.
    fn invocations(&self) -> Vec<String> {
        match fs::read_to_string(self.invocation_log()) {
            Ok(body) => body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn clear_invocations(&self) {
        let _ = fs::remove_file(self.invocation_log());
    }

    /// POSITIVE CONTROL for the invocation-log instrument, mirroring
    /// [`ConfigEnv::assert_marker_can_fire`].
    ///
    /// Runs the installed fake DIRECTLY under the identical environment and
    /// asserts its name lands in the log. Without this, "the invocation log is
    /// empty" is satisfied just as well by a program that cannot write.
    fn assert_logger_can_fire(&self, name: &str) {
        self.clear_invocations();
        let mut cmd = Command::new(self.bin.path().join(name));
        self.apply_env(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = cmd.output().expect("spawn the fake logging program");
        assert_eq!(
            self.invocations(),
            vec![name.to_string()],
            "POSITIVE CONTROL FAILED: the fake program {name:?} ran but wrote NO line to the \
             invocation log, so an EMPTY log proves nothing and the no-network-tool assertion \
             would be VACUOUS. status={:?} stdout={:?} stderr={:?}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        self.clear_invocations();
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
///
/// `sentinel` is interpolated into a TOML BASIC string, so a caller passing a
/// value that contains a `"` must pass it already TOML-escaped (`\\"`) while
/// asserting against the UNESCAPED form — that is exactly the CR-01 fixture.
#[allow(dead_code)]
fn config_with_secret_in_type_error(sentinel: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = \"{sentinel}\"\n")
}

/// A config whose `[pricing].source` is an unknown enum variant equal to
/// `sentinel`, so serde fails with ``unknown variant `<value>` `` — the one
/// place the offending value is echoed with BACKTICKS and NO escaping, which is
/// the delimiter `redact_quoted_runs` could not survive (CR-01, vector 2).
#[allow(dead_code)]
fn config_with_secret_in_enum_error(sentinel: &str) -> String {
    format!("[pricing]\nsource = \"{sentinel}\"\n")
}

/// A config whose `[pricing].max_age` is an unparseable duration equal to
/// `sentinel`. This one does NOT fail deserialization: it reaches the SEMANTIC
/// rule, whose parser (`ant::duration::parse_max_age`) was written for a CLI
/// flag and quotes the input back with `'…'` — CR-01 vector 3, the only one
/// that travels through `redact_value_text` rather than `redact_toml_error`.
#[allow(dead_code)]
fn config_with_secret_in_duration_error(sentinel: &str) -> String {
    format!("[pricing]\nmax_age = \"{sentinel}\"\n")
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

// ===========================================================================
// Task 2: the D-18-scoped SC3 render-stability proofs.
// ===========================================================================

/// Offset of the first differing byte, or `None` when the slices are equal.
fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    if let Some(i) = a.iter().zip(b.iter()).position(|(x, y)| x != y) {
        return Some(i);
    }
    if a.len() == b.len() {
        None
    } else {
        Some(a.len().min(b.len()))
    }
}

/// A fixed render payload whose `workspace.current_dir` is `dir`.
fn render_payload(dir: &Path) -> String {
    serde_json::json!({
        "session_id": "phase-12-sc3-fixed-session",
        "workspace": { "current_dir": dir.display().to_string() },
        "model": { "id": "claude-opus-4-8" }
    })
    .to_string()
}

/// SC3, scoped per D-18 clause 1: the rendered status line is byte-identical
/// across the four PRICE-CACHE states (absent / fresh / stale / corrupt) and
/// exits 0 in all four.
///
/// **What this establishes and what it does not.** It establishes CACHE-STATE
/// stability within a SINGLE binary; it establishes NOTHING about compatibility
/// with a pre-Phase-12 binary (D-18 clause 3 forbids labelling a four-state
/// comparison as before/after compatibility), and it makes no claim about the
/// GSD segment, whose intended change under D-17/D-18 is pinned by plan 12-03.
///
/// `workspace.current_dir` points at the TEMP HOME on purpose: that directory
/// contains no `.planning/` tree, so `gsd_phase` is empty and IDENTICAL on every
/// side of the comparison. The default template gates its entire GSD segment on
/// that variable, so holding it constant is what D-18 clause 3 requires of any
/// byte-identity assertion.
#[test]
#[serial]
fn render_unaffected_by_price_cache_state() {
    let env = ConfigEnv::new(&clean_config());
    let payload = render_payload(env.home_path());

    let render = |label: &str| -> Vec<u8> {
        let (ok, code, stdout, stderr) = env.run_with_stdin(&[], &payload);
        assert!(
            ok && code == Some(0),
            "the render must exit 0 in the {label} cache state; code={code:?} stderr={stderr}"
        );
        stdout
    };

    // --- 1. absent -------------------------------------------------------
    env.clear_caches();
    // NEGATIVE CONTROL for the consumption probe used on the fresh arm below:
    // with no cache on disk the self-throttle cannot fire, so the command falls
    // through to the fetch (which cannot spawn `curl` under the replaced PATH).
    let (_, _, throttle_absent, _) = env.run(&["ant", "sync-pricing", "--max-age", "365d"]);
    assert!(
        !String::from_utf8_lossy(&throttle_absent).contains("Price cache is fresh"),
        "negative control: with the caches cleared the price cache must NOT be reported fresh"
    );
    let absent = render("absent");

    // --- 2. fresh --------------------------------------------------------
    let seeded = env.seed_cache("ant/prices.json", &fresh_price_cache_body());

    // GUARD against the mis-seed class recurring here: prove the seeded price
    // file is the one the binary READS before comparing any bytes. Without this,
    // a future path regression silently turns the comparison below into "absent
    // four times" and it still passes.
    //
    // `ant sync-pricing --max-age` self-throttles on `read_price_cache()` BEFORE
    // any network or subprocess work, so this message is emitted if and only if
    // the seeded document was read, parsed and accepted at the real cache path.
    let (ok, code, throttle_fresh, stderr) = env.run(&["ant", "sync-pricing", "--max-age", "365d"]);
    let throttle_text = String::from_utf8_lossy(&throttle_fresh).into_owned();
    assert!(
        ok && code == Some(0),
        "the self-throttled sync must exit 0 with a fresh cache; code={code:?} stderr={stderr}"
    );
    assert!(
        throttle_text.contains("Price cache is fresh"),
        "the seeded price cache must be CONSUMED by the binary; got {throttle_text:?} \
         (seeded={seeded:?})"
    );
    // ...and it must have been read from the root the binary actually resolves:
    // all three caches share the `ant/` directory, so the models path `ant
    // doctor --json` reports has the same parent as the seeded prices file.
    let (_, _, doctor_out, _) = env.run(&["ant", "doctor", "--json"]);
    let doctor = parse_json(&doctor_out, "ant doctor --json");
    let models_path = doctor["caches"]["models"]["path"]
        .as_str()
        .expect("caches.models.path must be a string")
        .to_string();
    let live_dir = Path::new(&models_path)
        .parent()
        .expect("the models cache path has a parent")
        .to_path_buf();
    assert!(
        seeded
            .iter()
            .any(|p| p.parent() == Some(live_dir.as_path())),
        "the seeded price cache must sit in the cache directory the binary resolves \
         (live={live_dir:?} seeded={seeded:?})"
    );
    // ADDED by plan 12-02, which landed `caches.prices`: assert the FIELD too,
    // now that it exists. This does NOT replace the `ant sync-pricing` probe
    // above — the probe observes the READ (the self-throttle fires only if the
    // document was opened, parsed and accepted), whereas this field-level check
    // pins the reported path byte-for-byte. Keep both: one proves consumption,
    // the other proves identity.
    let prices_path = doctor["caches"]["prices"]["path"]
        .as_str()
        .expect("caches.prices.path must be a string (plan 12-02)")
        .to_string();
    assert!(
        seeded
            .iter()
            .any(|p| p.display().to_string() == prices_path),
        "caches.prices.path must be byte-EQUAL to a seeded path \
         (reported={prices_path:?} seeded={seeded:?})"
    );
    assert_eq!(
        doctor["caches"]["prices"]["present"],
        serde_json::Value::Bool(true),
        "the freshly seeded price cache must be reported present by `ant doctor --json`"
    );
    assert_eq!(
        doctor["caches"]["prices"]["stale"],
        serde_json::Value::Bool(false),
        "a price cache fetched just now must not be reported stale"
    );

    let fresh = render("fresh");

    // --- 3. stale --------------------------------------------------------
    env.seed_cache("ant/prices.json", &stale_price_cache_body());
    let stale = render("stale");

    // --- 4. corrupt ------------------------------------------------------
    env.seed_cache("ant/prices.json", "{not json");
    let corrupt = render("corrupt");

    let states = [
        ("absent", &absent),
        ("fresh", &fresh),
        ("stale", &stale),
        ("corrupt", &corrupt),
    ];
    for (label, bytes) in &states[1..] {
        assert!(
            first_difference(&absent, bytes).is_none(),
            "the render must be byte-identical across price-cache states, but `absent` and \
             `{label}` differ at byte offset {offset:?}: absent={absent_text:?} {label}={text:?}",
            offset = first_difference(&absent, bytes),
            absent_text = String::from_utf8_lossy(&absent),
            text = String::from_utf8_lossy(bytes),
        );
    }

    // The render must stay SILENT about cache health — "warnings never affect
    // the render" is observable here rather than merely asserted.
    for (label, bytes) in [("stale", &stale), ("corrupt", &corrupt)] {
        let text = String::from_utf8_lossy(bytes).to_lowercase();
        assert!(
            !text.contains("warn") && !text.contains("stale"),
            "the {label} render must not mention cache health: {text:?}"
        );
    }
}

/// NON-VACUITY PROOF for the byte comparator used by
/// [`render_unaffected_by_price_cache_state`]: it can report a difference.
///
/// The difference is driven from the PAYLOAD (two different
/// `workspace.current_dir` values), not from the environment, so the
/// instrument's sensitivity does not depend on `NO_COLOR` handling.
#[test]
#[serial]
fn render_byte_comparison_can_detect_a_difference() {
    let env = ConfigEnv::new(&clean_config());

    let other_dir = env.home_path().join("a-different-directory");
    fs::create_dir_all(&other_dir).expect("create the second render directory");

    let (ok_a, code_a, here, err_a) = env.run_with_stdin(&[], &render_payload(env.home_path()));
    assert!(ok_a, "render must exit 0; code={code_a:?} stderr={err_a}");
    let (ok_b, code_b, there, err_b) = env.run_with_stdin(&[], &render_payload(&other_dir));
    assert!(ok_b, "render must exit 0; code={code_b:?} stderr={err_b}");

    assert!(
        first_difference(&here, &there).is_some(),
        "the byte comparator is VACUOUS: two renders of visibly different payloads compared \
         equal ({:?} vs {:?})",
        String::from_utf8_lossy(&here),
        String::from_utf8_lossy(&there),
    );
}

// ===========================================================================
// Plan 12-08: the `config` subcommand group, the hidden `generate-config`
// alias, and the `config path` search-order / misplacement report.
// ===========================================================================

/// Detect root WITHOUT spawning `id -u`: the effective uid is not on stable
/// std, so probe the only thing that matters here — whether a permission bit
/// actually bites. Idiom copied verbatim from
/// `src/config_validation.rs::tests::running_as_root` (plan 12-06), so the two
/// permission-sensitive suites in this phase cannot disagree about what "root"
/// means.
fn running_as_root() -> bool {
    let dir = TempDir::new().expect("root-probe temp dir");
    let probe = dir.path().join("probe");
    fs::write(&probe, b"x").expect("write root probe");
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o000)).expect("chmod root probe");
    let readable = fs::read(&probe).is_ok();
    let _ = fs::set_permissions(&probe, fs::Permissions::from_mode(0o600));
    readable
}

/// The `source` token of every candidate, in the order reported.
fn candidate_sources(report: &serde_json::Value) -> Vec<String> {
    report["candidates"]
        .as_array()
        .expect("candidates must be an array")
        .iter()
        .map(|c| {
            c["source"]
                .as_str()
                .expect("candidates[].source must be a string")
                .to_string()
        })
        .collect()
}

/// The `path` of every reported misplaced config.
fn misplaced_paths(report: &serde_json::Value) -> Vec<String> {
    report["misplaced"]
        .as_array()
        .expect("misplaced must be an array")
        .iter()
        .map(|m| {
            m["path"]
                .as_str()
                .expect("misplaced[].path must be a string")
                .to_string()
        })
        .collect()
}

/// The `reason` reported for one misplaced path.
fn misplaced_reason(report: &serde_json::Value, path: &str) -> String {
    report["misplaced"]
        .as_array()
        .expect("misplaced must be an array")
        .iter()
        .find(|m| m["path"].as_str() == Some(path))
        .map(|m| {
            m["reason"]
                .as_str()
                .expect("misplaced[].reason must be a string")
                .to_string()
        })
        .unwrap_or_else(|| panic!("{path} is not in the misplaced array: {report}"))
}

/// The XDG candidate the child resolves: `ConfigEnv` sets
/// `XDG_CONFIG_HOME=<home>/config`, and `common::get_config_dir()` consults that
/// FIRST, so this is both candidate 3 and `default_config_path()`.
fn xdg_config_file(env: &ConfigEnv) -> PathBuf {
    env.home_path()
        .join("config")
        .join("claudia-statusline")
        .join("config.toml")
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("path has a parent")).expect("create parent dir");
    fs::write(path, body).expect("write file");
}

/// The `12-VALIDATION.md` contract row for `config path`: the ACTIVE file, the
/// FULL search order in order with hit/miss, and the D-10 misplacement warning —
/// in three arms that move the winner down the list.
#[test]
#[serial]
fn config_path_reports_search_order() {
    let env = ConfigEnv::new(&clean_config());

    // --- Arm 1: STATUSLINE_CONFIG_PATH wins (candidate 1). ---
    let (ok, code, stdout, stderr) = env.run(&["config", "path", "--json"]);
    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json");

    assert_eq!(
        candidate_sources(&report),
        vec![
            "env_statusline_config_path",
            "env_statusline_config",
            "xdg_config_dir",
            "home_dotfile",
        ],
        "the four candidates must be reported in SEARCH ORDER: {report}"
    );
    assert_eq!(
        report["active_source"].as_str(),
        Some("env_statusline_config_path"),
        "{report}"
    );
    assert_eq!(
        report["active"].as_str(),
        Some(env.config_path.display().to_string().as_str()),
        "{report}"
    );
    assert_eq!(report["candidates"][0]["exists"], serde_json::json!(true));
    for i in 1..4 {
        assert_eq!(
            report["candidates"][i]["exists"],
            serde_json::json!(false),
            "candidate {i} must MISS while candidate 1 wins: {report}"
        );
    }

    // --- Arm 2: with both env vars unset, the XDG dir wins (candidate 3). ---
    let xdg = xdg_config_file(&env);
    write_file(&xdg, &clean_config());

    let (ok, code, stdout, stderr) = env.run_unset_config(&["config", "path", "--json"]);
    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json (unset)");
    assert_eq!(
        report["active_source"].as_str(),
        Some("xdg_config_dir"),
        "{report}"
    );
    assert_eq!(
        report["active"].as_str(),
        Some(xdg.display().to_string().as_str()),
        "{report}"
    );

    // --- Arm 3: a config at a NEVER-CONSULTED path is warned about, while the
    // losing-but-consulted XDG candidate is NOT (it is a candidate, not a
    // misplacement — this is the filter that keeps the warning meaningful). ---
    let misplaced = env
        .home_path()
        .join(".config")
        .join("claudia-statusline")
        .join("config.toml");
    write_file(&misplaced, &clean_config());

    let (ok, code, stdout, stderr) = env.run(&["config", "path", "--json"]);
    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json (misplaced)");
    let names = misplaced_paths(&report);
    assert!(
        names.contains(&misplaced.display().to_string()),
        "the D-10 probe must name the misplaced config {}: {report}",
        misplaced.display()
    );
    // Exactly one warning, and no noise around it. (The candidate-EXCLUSION
    // filter is NOT what this arm exercises: under this fixture no probed path
    // ever coincides with a search candidate, so an assertion here that the XDG
    // candidate is absent could never fail. It is proven instead by
    // `config_path_never_warns_about_a_real_search_candidate`, which arranges
    // that coincidence deliberately.)
    assert_eq!(
        names,
        vec![misplaced.display().to_string()],
        "exactly one misplacement was planted: {report}"
    );
    // The warning's value is the destination, not the noticing.
    assert!(
        misplaced_reason(&report, &misplaced.display().to_string())
            .contains(&xdg.display().to_string()),
        "the warning must name WHERE the file has to move: {report}"
    );
}

/// `Path::exists()` and `symlink_metadata` disagree in exactly one direction
/// that matters here, and the probe was built on the measured answer rather than
/// the planned one.
///
/// A DANGLING SYMLINK at a misplaced path is a real, broken, silently-ignored
/// config: `exists()` follows the link and reports `false`, so a probe written
/// with it would go quiet on it. The POSITIVE CONTROL is the first arm — a plain
/// regular file at the same class of path — which proves the report can name a
/// misplacement at all before the symlink arm's naming counts as evidence.
///
/// The third arm covers T-12-58: a `PermissionDenied` stat is reported as
/// UNVERIFIABLE rather than collapsed to "absent". It is guarded, because root
/// traverses a `chmod 000` directory and would turn the assertion vacuous.
#[test]
#[serial]
fn config_path_probe_is_not_fooled_by_exists_semantics() {
    let env = ConfigEnv::new(&clean_config());

    // --- POSITIVE CONTROL: a plain regular file at a never-consulted path. ---
    let plain = env
        .home_path()
        .join(".config")
        .join("statusline")
        .join("config.toml");
    write_file(&plain, &clean_config());

    let (ok, code, stdout, stderr) = env.run(&["config", "path", "--json"]);
    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json (positive control)");
    assert!(
        misplaced_paths(&report).contains(&plain.display().to_string()),
        "POSITIVE CONTROL FAILED: the probe cannot name even a plain misplaced file, so every \
         other arm of this test is vacuous: {report}"
    );
    fs::remove_file(&plain).expect("remove the positive-control file");

    // --- ARM 2: a DANGLING SYMLINK, which `Path::exists()` calls absent. ---
    let dangling = env
        .home_path()
        .join(".config")
        .join("claudia-statusline")
        .join("config.toml");
    fs::create_dir_all(dangling.parent().expect("parent")).expect("create parent");
    std::os::unix::fs::symlink(env.home_path().join("moved-away.toml"), &dangling)
        .expect("create dangling symlink");
    assert!(
        !dangling.exists(),
        "the fixture is wrong: this arm is only meaningful while `exists()` answers false"
    );
    assert!(
        fs::symlink_metadata(&dangling).is_ok(),
        "the fixture is wrong: symlink_metadata must still see the link itself"
    );

    let (ok, code, stdout, stderr) = env.run(&["config", "path", "--json"]);
    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json (dangling)");
    assert!(
        misplaced_paths(&report).contains(&dangling.display().to_string()),
        "a dangling symlink at a never-consulted path is still a misplaced config, and is \
         precisely what `Path::exists()` would have missed: {report}"
    );
    fs::remove_file(&dangling).expect("remove the dangling symlink");

    // --- ARM 3 (T-12-58): a stat refused by permissions is UNVERIFIABLE. ---
    if running_as_root() {
        eprintln!("skipped: running as root, the permission bit does not bite");
        return;
    }
    let locked_dir = env.home_path().join(".claudia-statusline");
    let locked = locked_dir.join("config.toml");
    write_file(&locked, &clean_config());
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000)).expect("chmod 000");

    let (ok, code, stdout, stderr) = env.run(&["config", "path", "--json"]);
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).expect("restore mode");

    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json (permission denied)");
    let locked_name = locked.display().to_string();
    assert!(
        misplaced_paths(&report).contains(&locked_name),
        "a config the probe is REFUSED permission to stat must be reported as unverifiable, \
         never collapsed into silence (T-12-58): {report}"
    );
    assert!(
        misplaced_reason(&report, &locked_name).contains("permission denied"),
        "the reason must say the check was refused rather than claim the file is there: {report}"
    );
}

/// The hidden deprecated alias, exercised through the ALIAS PATH itself — a
/// spawned `statusline generate-config`, not a shared inner function.
///
/// Three independent facts: it is gone from `--help`, it still does the work,
/// and it announces itself exactly once on STDERR (never stdout, so a script
/// piping its output keeps parsing what it always parsed). The final arm is the
/// discriminator: `config generate` must NOT print the notice, without which
/// "the notice is on stderr" could pass for a notice printed unconditionally.
#[test]
#[serial]
fn generate_config_alias_is_hidden_but_works() {
    const NOTICE: &str = "is deprecated; use `statusline config generate`";

    let env = ConfigEnv::new_without_config();

    let (ok, code, stdout, stderr) = env.run(&["--help"]);
    assert!(ok, "--help must exit 0; code={code:?} stderr={stderr}");
    let help = String::from_utf8_lossy(&stdout);
    assert!(
        !help.contains("generate-config"),
        "the deprecated alias must be HIDDEN from --help: {help}"
    );
    assert!(
        help.contains("config"),
        "the `config` group must be listed in --help: {help}"
    );

    // ...hidden, but not removed: it still dispatches.
    let (ok, code, stdout, stderr) = env.run(&["generate-config"]);
    assert!(
        ok,
        "the hidden alias must still work; code={code:?} stderr={stderr}"
    );
    let written = xdg_config_file(&env);
    assert!(
        fs::symlink_metadata(&written).is_ok(),
        "the alias must still write the config at {}",
        written.display()
    );

    let out = String::from_utf8_lossy(&stdout);
    assert_eq!(
        stderr.matches(NOTICE).count(),
        1,
        "the deprecation notice must appear on stderr exactly ONCE: {stderr:?}"
    );
    assert!(
        !out.contains(NOTICE) && !out.contains("deprecated"),
        "the notice must never reach stdout: {out:?}"
    );

    // The discriminator: the SUPPORTED spelling stays quiet.
    let (ok, code, _stdout, stderr) = env.run(&["config", "generate"]);
    assert!(
        ok,
        "config generate must exit 0; code={code:?} stderr={stderr}"
    );
    assert!(
        !stderr.contains("deprecated"),
        "the notice belongs to the DEPRECATED path only, yet `config generate` printed it: \
         {stderr:?}"
    );
}

/// Pins the security property of the MOVED `generate` body (T-12-32) with exact
/// numeric modes, so a future rewrite cannot quietly loosen them.
#[test]
#[serial]
fn config_generate_writes_restrictive_permissions() {
    let env = ConfigEnv::new_without_config();

    let (ok, code, _stdout, stderr) = env.run(&["config", "generate"]);
    assert!(
        ok,
        "config generate must exit 0; code={code:?} stderr={stderr}"
    );

    let written = xdg_config_file(&env);
    let file_mode = fs::metadata(&written)
        .unwrap_or_else(|e| panic!("config must exist at {}: {e}", written.display()))
        .permissions()
        .mode()
        & 0o777;
    let dir_mode = fs::metadata(written.parent().expect("parent"))
        .expect("config dir metadata")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(file_mode, 0o600, "the generated config must be 0600");
    assert_eq!(dir_mode, 0o700, "the config directory must be 0700");
}

/// The root `--config <PATH>` OPTION and the `config` SUBCOMMAND coexist.
///
/// clap has to tell the two apart in one argv, and the flag is plumbed by
/// SETTING `STATUSLINE_CONFIG_PATH` (`src/main.rs`), so a successful run must
/// report the flag's file as active via candidate 1.
#[test]
#[serial]
fn global_config_flag_coexists_with_config_subcommand() {
    let env = ConfigEnv::new(&clean_config());
    let flag_target = env.config_path.display().to_string();

    let (ok, code, stdout, stderr) =
        env.run_unset_config(&["--config", &flag_target, "config", "path", "--json"]);
    assert!(
        ok,
        "`--config <PATH> config path` must be a legal invocation; code={code:?} stderr={stderr}"
    );

    let report = parse_json(&stdout, "--config <PATH> config path --json");
    assert_eq!(
        report["active_source"].as_str(),
        Some("env_statusline_config_path"),
        "the global flag is plumbed through candidate 1: {report}"
    );
    assert_eq!(
        report["active"].as_str(),
        Some(flag_target.as_str()),
        "{report}"
    );
}

/// `config path --json` is byte-reproducible.
///
/// Directory enumeration and map iteration are the usual sources of drift, so
/// the misplaced array is sorted before serialization. The `>= 2` assertion is
/// what keeps this from passing on two identical EMPTY arrays.
#[test]
#[serial]
fn config_path_json_is_deterministic() {
    let env = ConfigEnv::new(&clean_config());

    for rel in [
        ".config/claudia-statusline/config.toml",
        ".config/statusline/config.toml",
    ] {
        write_file(&env.home_path().join(rel), &clean_config());
    }

    let (ok_a, code_a, first, err_a) = env.run(&["config", "path", "--json"]);
    assert!(ok_a, "run 1 must exit 0; code={code_a:?} stderr={err_a}");
    let (ok_b, code_b, second, err_b) = env.run(&["config", "path", "--json"]);
    assert!(ok_b, "run 2 must exit 0; code={code_b:?} stderr={err_b}");

    let report = parse_json(&first, "config path --json");
    assert!(
        misplaced_paths(&report).len() >= 2,
        "both seeded misplacements must be reported, or this test compares two empty \
         arrays and proves nothing: {report}"
    );

    assert_eq!(
        first,
        second,
        "two runs in the same environment must be byte-identical: {:?} vs {:?}",
        String::from_utf8_lossy(&first),
        String::from_utf8_lossy(&second),
    );
}

/// The probe must never warn about a path the resolver DOES consult.
///
/// This is the one arrangement under which that filter is load-bearing: a file
/// that is simultaneously a PROBED misplacement location and a real, LOSING
/// search candidate. `STATUSLINE_CONFIG` (candidate 2) is pointed at
/// `~/.config/statusline/config.toml` — probe 3 — while candidate 1 still wins,
/// so neither the active-file check nor mere absence can account for the
/// exclusion.
///
/// The POSITIVE CONTROL is a second file at a different probed path: it must be
/// reported in the SAME run, otherwise "the candidate is absent from the list"
/// would be satisfied by a report that had simply gone silent.
#[test]
#[serial]
fn config_path_never_warns_about_a_real_search_candidate() {
    let env = ConfigEnv::new(&clean_config());

    let as_candidate = env
        .home_path()
        .join(".config")
        .join("statusline")
        .join("config.toml");
    write_file(&as_candidate, &clean_config());

    let control = env
        .home_path()
        .join(".config")
        .join("claudia-statusline")
        .join("config.toml");
    write_file(&control, &clean_config());

    let (ok, code, stdout, stderr) = env.run_with_extra_env(
        &[("STATUSLINE_CONFIG", &as_candidate.display().to_string())],
        &["config", "path", "--json"],
    );
    assert!(ok, "config path must exit 0; code={code:?} stderr={stderr}");
    let report = parse_json(&stdout, "config path --json (candidate coincidence)");

    // The arrangement itself, asserted rather than assumed: candidate 2 IS the
    // probed path, it exists, and it is NOT the active file.
    assert_eq!(
        report["candidates"][1]["path"].as_str(),
        Some(as_candidate.display().to_string().as_str()),
        "the fixture is wrong: candidate 2 must be the probed path: {report}"
    );
    assert_eq!(
        report["candidates"][1]["exists"],
        serde_json::json!(true),
        "the fixture is wrong: that candidate must exist: {report}"
    );
    assert_eq!(
        report["active_source"].as_str(),
        Some("env_statusline_config_path"),
        "the fixture is wrong: candidate 1 must still win, so the file under test is a \
         LOSING candidate: {report}"
    );

    let names = misplaced_paths(&report);
    assert!(
        names.contains(&control.display().to_string()),
        "POSITIVE CONTROL FAILED: the report went silent in this run, so the exclusion \
         below proves nothing: {report}"
    );
    assert!(
        !names.contains(&as_candidate.display().to_string()),
        "a file the resolver DOES consult is a losing candidate, not a misplacement, and \
         warning about it would make the warning noise: {report}"
    );
}

// ===========================================================================
// Plan 12-09: `config validate` — the QUAL-01 contract.
// ===========================================================================

/// Parse `stdout` as EXACTLY ONE JSON value.
///
/// `serde_json::from_slice` already rejects trailing non-whitespace, but the
/// streaming count is what makes the claim explicit: the `--json` document is a
/// SINGLE self-contained document, and a stray `println!` (the notice-as-a-line
/// mistake this plan exists to avoid) would make this two values or zero.
fn parse_exactly_one_json(stdout: &[u8], what: &str) -> serde_json::Value {
    let stream = serde_json::Deserializer::from_slice(stdout).into_iter::<serde_json::Value>();
    let values: Vec<serde_json::Value> = stream
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap_or_else(|e| {
            panic!(
                "{what} stdout must be exactly one JSON document ({e}): {:?}",
                String::from_utf8_lossy(stdout)
            )
        });
    assert_eq!(
        values.len(),
        1,
        "{what} must print exactly ONE JSON value and nothing else — a stray line before or \
         after the document breaks the single-document contract: {:?}",
        String::from_utf8_lossy(stdout)
    );
    values.into_iter().next().expect("one value")
}

/// Every `findings[].kind` the `--json` contract allows. A consumer filters on
/// this closed vocabulary instead of string-matching prose.
const FINDING_KINDS: &[&str] = &[
    "syntax_error",
    "unknown_key",
    "type_error",
    "invalid_value",
    "stale_cache",
    "unreadable_cache",
    "misplaced_config",
    "config_io",
];

/// Assert the document's shape, then hand back the findings array.
///
/// Called by every test below so the schema is re-checked on every fixture
/// rather than in one place a later change could route around.
fn validate_findings(report: &serde_json::Value) -> &Vec<serde_json::Value> {
    for field in [
        "schema_version",
        "valid",
        "strict",
        "exit_code",
        "target_path",
        "target_source",
        "active_path",
        "active_source",
        "counts",
        "notices",
        "findings",
        "caches",
    ] {
        assert!(
            report.get(field).is_some(),
            "`config validate --json` must carry `{field}`: {report}"
        );
    }
    assert_eq!(report["schema_version"], 1, "schema_version: {report}");
    assert!(
        report["counts"]["errors"].is_u64(),
        "counts.errors: {report}"
    );
    assert!(
        report["counts"]["warnings"].is_u64(),
        "counts.warnings: {report}"
    );

    let findings = report["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("`findings` must be an array: {report}"));
    for finding in findings {
        let kind = finding["kind"].as_str().unwrap_or("<missing>");
        assert!(
            FINDING_KINDS.contains(&kind),
            "every finding kind must come from the closed vocabulary {FINDING_KINDS:?}, got \
             {kind:?}: {report}"
        );
    }
    // `valid` is DEFINED as "no errors" and is INDEPENDENT of `--strict`.
    assert_eq!(
        report["valid"].as_bool(),
        Some(report["counts"]["errors"].as_u64() == Some(0)),
        "`valid` must mean `counts.errors == 0`: {report}"
    );
    findings
}

/// The findings whose `key` is exactly `key`.
fn findings_at<'a>(findings: &'a [serde_json::Value], key: &str) -> Vec<&'a serde_json::Value> {
    findings
        .iter()
        .filter(|f| f["key"].as_str() == Some(key))
        .collect()
}

/// The codes in the report's `notices` array.
fn notice_codes(report: &serde_json::Value) -> Vec<String> {
    report["notices"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n["code"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A typo'd key is an ERROR (D-09) and it names the dotted path.
///
/// MUTATION PROOF 2 is recorded against this test: with the unknown-key pass
/// removed from `src/config_validation.rs`, it fails.
#[test]
#[serial]
fn unknown_key_is_error() {
    let env = ConfigEnv::new(&config_with_unknown_key());
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(
        !ok,
        "an unknown key must exit NON-ZERO (D-09); code={code:?} stderr={stderr}"
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    let hits = findings_at(findings, "display.show_gti");
    assert_eq!(
        hits.len(),
        1,
        "exactly one finding at `display.show_gti`: {report}"
    );
    assert_eq!(hits[0]["kind"], "unknown_key", "{report}");
    assert_eq!(hits[0]["severity"], "error", "{report}");
    assert_eq!(report["valid"], false, "{report}");
}

/// A type error in ONE section must not silence the next section's rules.
///
/// This is the whole point of D-02's per-section deserialization: a whole-file
/// `Config::load()` aborts on the first type error, so a config with several
/// distinct defects would report exactly one of them.
///
/// Deliberate contrast with `config_validation::tests::
/// failed_section_reports_exactly_one_finding`: WITHIN one failed section a
/// single finding is correct (its semantic pass never ran, because its input
/// never existed). ACROSS sections, isolation is required. This test therefore
/// asserts two findings in two DIFFERENT sections and never demands two from one.
#[test]
#[serial]
fn type_error_isolated_per_section() {
    let body = "[display]\nprogress_bar_width = \"wide\"\n\n[layout]\npreset = \"compakt\"\n";
    let env = ConfigEnv::new(body);
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(!ok, "code={code:?} stderr={stderr}");
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    let display = findings_at(findings, "display");
    assert_eq!(
        display.len(),
        1,
        "the failed `[display]` section reports exactly one finding, keyed at the section: {report}"
    );
    assert_eq!(display[0]["kind"], "type_error", "{report}");

    let preset = findings_at(findings, "layout.preset");
    assert_eq!(
        preset.len(),
        1,
        "`[layout]` parsed, so its semantic rules MUST still have run — a single finding for a \
         config with defects in two different sections is the whole-document abort this design \
         exists to avoid: {report}"
    );
    assert_eq!(preset[0]["kind"], "invalid_value", "{report}");

    let kinds: std::collections::BTreeSet<&str> =
        findings.iter().filter_map(|f| f["kind"].as_str()).collect();
    assert!(
        kinds.len() >= 2,
        "at least two DIFFERENT finding kinds across the two sections: {report}"
    );
}

/// Every unknown enum value names the LEGAL SET, never echoing the user's value.
#[test]
#[serial]
fn invalid_enum_values() {
    let body = "[layout]\n\
                preset = \"compakt\"\n\n\
                [layout.components.git]\n\
                format = \"fancy\"\n\n\
                [burn_rate]\n\
                mode = \"wallclock\"\n";
    let env = ConfigEnv::new(body);
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(!ok, "code={code:?} stderr={stderr}");
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    // Assert the message CONTAINS each legal value, not the exact prose — the
    // wording is Claude's Discretion, the legal set is the contract.
    for (key, legal) in [
        (
            "layout.preset",
            vec!["default", "compact", "detailed", "minimal", "power"],
        ),
        (
            "layout.components.git.format",
            vec!["full", "branch", "status"],
        ),
        (
            "burn_rate.mode",
            vec!["wall_clock", "active_time", "auto_reset"],
        ),
    ] {
        let hits = findings_at(findings, key);
        assert_eq!(hits.len(), 1, "exactly one finding at `{key}`: {report}");
        assert_eq!(hits[0]["kind"], "invalid_value", "at `{key}`: {report}");
        let message = hits[0]["message"].as_str().unwrap_or_default();
        for value in legal {
            assert!(
                message.contains(value),
                "the message at `{key}` must name the legal value {value:?}: {message:?}"
            );
        }
        assert!(
            !message.contains("compakt")
                && !message.contains("fancy")
                && !message.contains("wallclock"),
            "the message at `{key}` must name the LEGAL set, never echo the user's value: \
             {message:?}"
        );
    }
}

/// The shared clean fixture passes with an EMPTY findings array and exit 0.
#[test]
#[serial]
fn clean_config_exits_zero() {
    let env = ConfigEnv::new(&clean_config());
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(
        ok,
        "a clean config must exit 0 (SC1); code={code:?} stdout={:?} stderr={stderr}",
        String::from_utf8_lossy(&stdout)
    );
    assert_eq!(code, Some(0), "stderr={stderr}");
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    assert!(
        findings.is_empty(),
        "a clean config must produce NO findings at all — not even warnings: {report}"
    );
    assert_eq!(report["valid"], true, "{report}");
    assert_eq!(report["exit_code"], 0, "{report}");
    assert_eq!(report["strict"], false, "{report}");
}

/// The SHIPPED example config must satisfy the validator.
///
/// Cheap insurance against a future `Config::example_toml()` edit introducing a
/// key or value the validator rejects — the two would otherwise drift silently
/// and the first user to run `config generate && config validate` would find it.
#[test]
#[serial]
fn generated_config_validates_clean() {
    let env = ConfigEnv::new(&clean_config());

    let (ok, code, stdout, stderr) = env.run(&["config", "generate"]);
    assert!(
        ok,
        "config generate must succeed; code={code:?} stderr={stderr}"
    );
    let generated = env
        .home_path()
        .join("config")
        .join("claudia-statusline")
        .join("config.toml");
    assert!(
        fs::metadata(&generated).is_ok(),
        "config generate must have written {generated:?}; stdout={:?}",
        String::from_utf8_lossy(&stdout)
    );

    let target = generated.display().to_string();
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", &target, "--json"]);
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    validate_findings(&report);
    assert!(
        ok,
        "the SHIPPED example config must validate clean; code={code:?} stderr={stderr} \
         report={report}"
    );
    assert_eq!(report["valid"], true, "{report}");
}

/// `[sync]` is a recognized section in EVERY build.
///
/// It is `#[cfg(feature = "turso-sync")]` on `Config`, so a fourteen-element
/// known-sections list that omitted it would fail a legitimate turso user's
/// config in a default build. Must pass under BOTH feature sets.
#[test]
#[serial]
fn sync_section_cfg_aware() {
    let env = ConfigEnv::new(&config_with_sync_section());
    let (_ok, _code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    let sync_findings: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|f| {
            f["kind"] == "unknown_key"
                && f["key"]
                    .as_str()
                    .map(|k| k == "sync" || k.starts_with("sync."))
                    .unwrap_or(false)
        })
        .collect();
    assert!(
        sync_findings.is_empty(),
        "`[sync]` must never be an unknown key, in either feature set: {report} stderr={stderr}"
    );
}

/// MUTATION PROOF 4: `--strict` moves the EXIT CODE and nothing else (D-11).
///
/// The identical-`findings` half is what makes this prove the warning is REAL
/// rather than cosmetic; the identical-`valid` half pins the semantics the
/// renderer defines. Both runs must agree on both.
#[test]
#[serial]
fn strict_promotes_warnings() {
    // Warnings, no errors: `enabled = false` with an account staged, and a
    // program that is not on the harness's replaced PATH.
    let body = "[ant]\nenabled = false\n\n[ant.accounts.work]\n\
                admin_key_command = [\"definitely-not-on-path\"]\n";
    let env = ConfigEnv::new(body);

    let (lax_ok, lax_code, lax_out, lax_err) = env.run(&["config", "validate", "--json"]);
    let lax = parse_exactly_one_json(&lax_out, "config validate --json");
    let lax_findings = validate_findings(&lax);

    assert!(
        !lax_findings.is_empty(),
        "the fixture must actually PRODUCE warnings, or this proof is vacuous: {lax}"
    );
    assert_eq!(lax["counts"]["errors"], 0, "warnings only: {lax}");
    assert!(
        lax["counts"]["warnings"].as_u64().unwrap_or(0) > 0,
        "warnings only: {lax}"
    );
    assert!(
        lax_ok && lax_code == Some(0),
        "warnings alone must NOT affect the exit code (D-11/SC3); code={lax_code:?} \
         stderr={lax_err}"
    );
    assert_eq!(lax["exit_code"], 0, "{lax}");
    assert_eq!(lax["strict"], false, "{lax}");

    let (strict_ok, strict_code, strict_out, strict_err) =
        env.run(&["config", "validate", "--json", "--strict"]);
    let strict = parse_exactly_one_json(&strict_out, "config validate --json --strict");
    validate_findings(&strict);

    assert!(
        !strict_ok && strict_code == Some(1),
        "--strict must promote warnings to a non-zero exit; code={strict_code:?} \
         stderr={strict_err}"
    );
    assert_eq!(strict["exit_code"], 1, "{strict}");
    assert_eq!(strict["strict"], true, "{strict}");

    // The three assertions that make this a proof rather than a tautology.
    assert_eq!(
        lax["findings"], strict["findings"],
        "--strict must not change WHICH findings are produced or their severities"
    );
    assert_eq!(
        lax["valid"], strict["valid"],
        "`valid` means `counts.errors == 0` and is INDEPENDENT of --strict"
    );
    assert_eq!(lax["valid"], true, "no errors, so still valid: {lax}");
    assert_eq!(
        lax["counts"], strict["counts"],
        "--strict must not change the counts"
    );
}

/// The `--json` document is byte-identical across processes.
///
/// The fixture carries THREE `[pricing.aliases]` entries with bad targets and
/// THREE `[ant.accounts.*]` tables with illegal names, so BOTH `HashMap`
/// iteration orders are genuinely exercised. With fewer than three entries per
/// map a two-element shuffle can come back in the original order and the test
/// passes by luck.
///
/// NON-VACUITY, stated honestly: `impl Validate for PricingConfig` and
/// `impl Validate for AntConfig` each ALREADY sort their map keys before
/// emitting, and `toml::Table` is a `BTreeMap`, so on this code base the five
/// raw outputs would match even without `Report::sort_findings`. What
/// `sort_findings` uniquely guarantees is the TOTAL ORDER — errors before
/// warnings, then by key — and that is asserted separately below, so removing
/// the sort fails this test.
#[test]
#[serial]
fn validate_json_is_deterministic_across_runs() {
    let body = "[pricing]\n\
                source = \"auto\"\n\n\
                [pricing.aliases]\n\
                alias-one = \"no-such-model-one\"\n\
                alias-two = \"no-such-model-two\"\n\
                alias-three = \"no-such-model-three\"\n\n\
                [ant]\n\
                enabled = true\n\n\
                [ant.accounts.\"bad one\"]\n\
                admin_key_command = [\"nope-one\"]\n\n\
                [ant.accounts.\"bad+two\"]\n\
                admin_key_command = [\"nope-two\"]\n\n\
                [ant.accounts.\"bad:three\"]\n\
                admin_key_command = [\"nope-three\"]\n";
    let env = ConfigEnv::new(body);

    let mut runs: Vec<Vec<u8>> = Vec::new();
    for _ in 0..5 {
        let (_ok, _code, stdout, _stderr) = env.run(&["config", "validate", "--json"]);
        runs.push(stdout);
    }
    for (i, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(
            run,
            &runs[0],
            "run {i} differed from run 0 at byte {:?}; the --json document must be \
             byte-identical across processes (T-12-60)",
            first_difference(run, &runs[0])
        );
    }

    let report = parse_exactly_one_json(&runs[0], "config validate --json");
    let findings = validate_findings(&report);
    assert!(
        findings.len() >= 6,
        "the fixture must actually exercise both maps (3 aliases + 3 accounts): {report}"
    );

    // The property `Report::sort_findings` uniquely provides: a TOTAL order,
    // errors first, then by key. Removing the sort leaves the findings in
    // section-emission order, which interleaves the `[ant]` errors and warnings.
    let order: Vec<(u8, String)> = findings
        .iter()
        .map(|f| {
            let rank = match f["severity"].as_str() {
                Some("error") => 0u8,
                _ => 1u8,
            };
            (rank, f["key"].as_str().unwrap_or_default().to_string())
        })
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(
        order, sorted,
        "findings must be in `sort_findings`'s total order — errors first, then by key: {report}"
    );
}

/// A positional PATH is validated, and BOTH files are named — as FIELDS.
#[test]
#[serial]
fn positional_path_reports_both_files() {
    let env = ConfigEnv::new(&clean_config());
    let other = env.home_path().join("other-config.toml");
    write_file(&other, &config_with_unknown_key());
    let other_text = other.display().to_string();

    let (ok, code, stdout, stderr) = env.run(&["config", "validate", &other_text, "--json"]);
    assert!(
        !ok && code == Some(1),
        "the POSITIONAL file carries the unknown key, so it must decide the exit code; \
         code={code:?} stderr={stderr}"
    );

    // The notice must live INSIDE the document. A stray stdout line would make
    // this parse as two values (or none).
    let report = parse_exactly_one_json(&stdout, "config validate <path> --json");
    validate_findings(&report);

    assert_eq!(
        report["target_path"].as_str(),
        Some(other_text.as_str()),
        "the positional PATH wins: {report}"
    );
    assert_eq!(report["target_source"], "positional", "{report}");
    assert_eq!(
        report["active_path"].as_str(),
        Some(env.config_path.display().to_string().as_str()),
        "the resolver's answer must still be reported, so \"names both files\" is \
         machine-readable: {report}"
    );
    assert!(
        report["active_source"].is_string(),
        "active_source must accompany active_path: {report}"
    );
    assert!(
        notice_codes(&report).contains(&"positional_target_differs_from_active".to_string()),
        "a positional target that is not the active config must say so: {report}"
    );
}

// ===========================================================================
// Plan 12-10 Task 1: the QUAL-02 `[pricing]` contract, END TO END.
//
// The library-level tests in plans 12-04 through 12-06 prove the mechanics.
// These prove the COMMAND: what a user actually sees when they run
// `statusline config validate` against each shape of `[pricing]` defect.
//
// The fixtures are deliberately SPLIT. Plan 12-04 records exactly ONE
// `type_error` for a section whose deserialization fails and deliberately SKIPS
// that section's semantic pass, so a single fixture carrying an invalid `source`
// AND a bad `max_age` AND a bad alias target can only ever produce ONE finding.
// `failed_section_suppresses_its_own_semantic_findings` pins that rule rather
// than contradicting it; `pricing_semantic_knobs_validated` covers the semantic
// rules on a section that PARSES.
// ===========================================================================

/// `[pricing]` unknown keys reach the report — the `deserialize_lenient`
/// blindness cannot regress (SC2 / QUAL-02).
///
/// `Config::pricing` is the ONE field carrying a custom lenient
/// `deserialize_with` (`src/config.rs:61`), and that deserializer consumes the
/// whole sub-table itself and silently returns the default on any error. A
/// whole-`Config` `serde_ignored` pass therefore CANNOT SEE INSIDE `[pricing]` —
/// the one section QUAL-02 names — which is why the engine walks sections.
///
/// The fixture carries a typo in BOTH `[display]` and `[pricing]` so the
/// asymmetry is observable in ONE run. That matters: a test asserting only the
/// `[display]` typo passes happily under the blind design, and "the same typo is
/// reported in `[display]` and silent in `[pricing]`" is precisely the warning
/// sign this row exists to raise.
///
/// MUTATION PROOF 1 of the phase, in its end-to-end form, is recorded against
/// this test.
#[test]
#[serial]
fn pricing_unknown_key_is_not_swallowed() {
    let body = format!(
        "{}\n{}",
        config_with_unknown_key(),
        config_with_pricing_typo()
    );
    let env = ConfigEnv::new(&body);
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(
        !ok,
        "an unknown key must exit NON-ZERO (D-09); code={code:?} stderr={stderr}"
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    // The CONTROL arm: a section with an ordinary derived deserializer.
    let control = findings_at(findings, "display.show_gti");
    assert_eq!(
        control.len(),
        1,
        "CONTROL: the `[display]` typo must be reported; if this arm fails the fixture is broken \
         rather than the pricing pass: {report}"
    );
    assert_eq!(control[0]["kind"], "unknown_key", "{report}");

    // The arm that only the per-section engine can satisfy.
    let hits = findings_at(findings, "pricing.aliasess");
    assert_eq!(
        hits.len(),
        1,
        "the `[pricing]` typo must be reported TOO. A report where the `[display]` typo above IS \
         present and this one is NOT is the `deserialize_lenient` blindness returning: a \
         whole-`Config` `serde_ignored` pass never reaches inside the one section QUAL-02 \
         names: {report}"
    );
    assert_eq!(hits[0]["kind"], "unknown_key", "{report}");
    assert_eq!(
        hits[0]["severity"], "error",
        "D-09: an unknown key is an ERROR, not advice: {report}"
    );
}

/// An invalid `[pricing].source` is EXACTLY ONE `type_error` keyed at the
/// section — and it still names the legal set.
///
/// `source` is an enum with no semantic rule of its own, and a section whose
/// deserialization fails never runs its semantic pass, so this one finding is
/// the ONLY thing the user is ever told. The message must therefore be useful:
/// it names `auto` / `bundled` / `synced` and never echoes the typo. See
/// `src/config_validation.rs::legal_variants` for why naming the legal set does
/// not weaken the redaction boundary.
#[test]
#[serial]
fn pricing_source_typo_is_one_type_error() {
    let env = ConfigEnv::new("[pricing]\nsource = \"bunlded\"\n");
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(
        !ok,
        "an unusable `[pricing]` section must exit NON-ZERO; code={code:?} stderr={stderr}"
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    assert_eq!(
        findings.len(),
        1,
        "a section whose deserialization FAILS yields EXACTLY ONE finding (D-02): {report}"
    );
    assert_eq!(
        findings[0]["key"], "pricing",
        "the finding is keyed at the SECTION, not at `pricing.source` — the section's typed value \
         never existed, so no field-level key can be attributed: {report}"
    );
    assert_eq!(findings[0]["kind"], "type_error", "{report}");

    let message = findings[0]["message"].as_str().unwrap_or_default();
    for legal in ["auto", "bundled", "synced"] {
        assert!(
            message.contains(legal),
            "this is the ONLY finding the user gets for a `source` typo, so it must name the \
             legal value {legal:?}: {message:?}"
        );
    }
    assert!(
        !message.contains("bunlded"),
        "the message must name the LEGAL set, never echo the user's value: {message:?}"
    );
}

/// The semantic rules on a `[pricing]` section that PARSES.
///
/// `source = "bundled"` is chosen so the unresolvable alias is an ERROR rather
/// than the conservative WARNING — see `pricing_alias_severity_follows_source`.
/// It also makes `max_age` an ignored knob, which is a THIRD finding: the
/// section carries both a malformed `max_age` (error) and a `max_age` that would
/// be ignored even if it parsed (warning). The count is asserted exactly so the
/// row cannot quietly acquire a fourth.
#[test]
#[serial]
fn pricing_semantic_knobs_validated() {
    let env = ConfigEnv::new(
        "[pricing]\n\
         source = \"bundled\"\n\
         max_age = \"30x\"\n\
         \n\
         [pricing.aliases]\n\
         \"my-model\" = \"not-a-real-model\"\n",
    );
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(
        !ok,
        "a malformed duration is an ERROR, so the run must exit NON-ZERO; code={code:?} \
         stderr={stderr}"
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    assert_eq!(
        findings.len(),
        3,
        "exactly three findings: the malformed `max_age`, the unresolvable alias, and the \
         `max_age`-is-ignored-under-`bundled` advisory: {report}"
    );
    assert_eq!(report["counts"]["errors"], 2, "{report}");
    assert_eq!(report["counts"]["warnings"], 1, "{report}");

    // 1. The malformed duration, at its own key, naming the grammar.
    let max_age = findings_at(findings, "pricing.max_age");
    let errors: Vec<_> = max_age
        .iter()
        .filter(|f| f["severity"] == "error")
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "exactly one ERROR at `pricing.max_age`: {report}"
    );
    assert_eq!(errors[0]["kind"], "invalid_value", "{report}");
    let message = errors[0]["message"].as_str().unwrap_or_default();
    for unit in ["s", "m", "h", "d"] {
        assert!(
            message.contains(unit),
            "the message must name the {unit:?} unit of the s/m/h/d grammar: {message:?}"
        );
    }
    assert!(
        message.contains("s/m/h/d"),
        "the message must spell the grammar out: {message:?}"
    );
    assert!(
        !message.contains("30x"),
        "the message must never echo the user's value: {message:?}"
    );

    // 2. The unresolvable alias, at a DISTINCT key naming the offending source.
    let alias = findings_at(findings, "pricing.aliases.my-model");
    assert_eq!(
        alias.len(),
        1,
        "exactly one finding at `pricing.aliases.my-model` — the key names WHICH alias is \
         broken, which is the whole point of a per-alias key: {report}"
    );
    assert_eq!(alias[0]["kind"], "invalid_value", "{report}");
    assert_eq!(
        alias[0]["severity"], "error",
        "under `source = bundled` the resolution union IS the bundled table, so the alias \
         provably cannot resolve: {report}"
    );

    // 3. The advisory, which is a WARNING and therefore exit-neutral (D-11).
    let advisory: Vec<_> = max_age
        .iter()
        .filter(|f| f["severity"] == "warning")
        .collect();
    assert_eq!(
        advisory.len(),
        1,
        "`max_age` under `source = bundled` is ignored, and saying so is advice, not a defect: \
         {report}"
    );
}

/// The alias severity follows `[pricing].source` — and the run's exit code
/// follows the severity.
///
/// `lookup_in` resolves an alias target against the UNION of the synced cache
/// and the bundled table, so under `source = auto` the bundled table alone
/// cannot prove the alias is broken: a `statusline ant sync-pricing` away, it
/// may resolve perfectly. Plan 12-06 made that arm a conservative WARNING.
/// Under `source = bundled` no cache is read at all, the union IS the bundled
/// table, and the same alias is provably dead — an ERROR.
///
/// Both configs run HERE, in one test, so the difference is asserted rather
/// than merely asserted-about across two test bodies.
#[test]
#[serial]
fn pricing_alias_severity_follows_source() {
    const ALIASES: &str = "\n[pricing.aliases]\n\"my-model\" = \"not-a-real-model\"\n";
    const KEY: &str = "pricing.aliases.my-model";

    let auto = ConfigEnv::new(&format!("[pricing]\nsource = \"auto\"\n{ALIASES}"));
    let (ok, code, stdout, stderr) = auto.run(&["config", "validate", "--json"]);
    assert!(
        ok,
        "a conservative alias WARNING must not fail validation (D-11); code={code:?} \
         stderr={stderr}"
    );
    assert_eq!(code, Some(0), "stderr={stderr}");
    let auto_report = parse_exactly_one_json(&stdout, "config validate --json");
    let auto_findings = validate_findings(&auto_report);
    let auto_hits = findings_at(auto_findings, KEY);
    assert_eq!(
        auto_hits.len(),
        1,
        "exactly one finding at `{KEY}` under `source = auto`: {auto_report}"
    );
    assert_eq!(
        auto_hits[0]["severity"], "warning",
        "the bundled table alone cannot prove the alias is broken under `auto`: {auto_report}"
    );
    assert_eq!(auto_report["counts"]["errors"], 0, "{auto_report}");

    let bundled = ConfigEnv::new(&format!("[pricing]\nsource = \"bundled\"\n{ALIASES}"));
    let (ok, code, stdout, stderr) = bundled.run(&["config", "validate", "--json"]);
    assert!(
        !ok,
        "a provably dead alias is an ERROR and must fail validation; code={code:?} \
         stderr={stderr}"
    );
    let bundled_report = parse_exactly_one_json(&stdout, "config validate --json");
    let bundled_findings = validate_findings(&bundled_report);
    let bundled_hits = findings_at(bundled_findings, KEY);
    assert_eq!(bundled_hits.len(), 1, "{bundled_report}");

    assert_ne!(
        auto_hits[0]["severity"], bundled_hits[0]["severity"],
        "the SAME alias target must be judged differently under the two sources — if these ever \
         agree, the conservative-policy downgrade has collapsed and one of the two answers is \
         wrong: auto={auto_hits:?} bundled={bundled_hits:?}"
    );
    assert_eq!(bundled_hits[0]["severity"], "error", "{bundled_report}");
}

/// A section whose deserialization FAILS reports exactly one finding and its
/// semantic rules are SKIPPED — pinned END TO END, not only in the library.
///
/// This is D-02's deliberate behaviour, not a gap: the semantic pass takes the
/// section's TYPED value as input, and for a failed section that value never
/// existed. `src/config_validation.rs::failed_section_reports_exactly_one_finding`
/// pins it at the library level; this row pins it through the shipped binary so
/// no future acceptance test can demand three findings from one failed section.
///
/// The fixture below carries THREE defects — an invalid `source`, a malformed
/// `max_age` and an unresolvable alias — and can only ever produce ONE finding.
/// `pricing_semantic_knobs_validated` is the row that covers the semantic rules,
/// on a section that parses.
#[test]
#[serial]
fn failed_section_suppresses_its_own_semantic_findings() {
    let env = ConfigEnv::new(
        "[pricing]\n\
         source = \"bunlded\"\n\
         max_age = \"30x\"\n\
         \n\
         [pricing.aliases]\n\
         \"my-model\" = \"not-a-real-model\"\n",
    );
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(!ok, "code={code:?} stderr={stderr}");
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    let section = findings_at(findings, "pricing");
    assert_eq!(
        section.len(),
        1,
        "EXACTLY ONE finding for a failed section (D-02): {report}"
    );
    assert_eq!(section[0]["kind"], "type_error", "{report}");

    let leaked: Vec<&str> = findings
        .iter()
        .filter_map(|f| f["key"].as_str())
        .filter(|k| k.starts_with("pricing."))
        .collect();
    assert!(
        leaked.is_empty(),
        "a failed section's semantic pass is SKIPPED, so NO `pricing.*` key may appear — its \
         typed input never existed. Found {leaked:?} in {report}"
    );
    assert_eq!(
        report["counts"]["errors"], 1,
        "three defects in one failed section still count ONCE: {report}"
    );
}

// ===========================================================================
// Plan 12-10 Task 2: the `[ant]` threshold rows and SC3, through the binary.
// ===========================================================================

/// A `ModelsCache` document dated `fetched_at`. Field names from
/// `src/ant/cache.rs::ModelsCache` / `ModelEntry`.
fn models_cache_body(fetched_at: chrono::DateTime<chrono::Utc>) -> String {
    serde_json::json!({
        "schema_version": 1,
        "fetched_at": fetched_at.to_rfc3339(),
        "models": { "claude-opus-4-8": { "max_input_tokens": 200_000 } }
    })
    .to_string()
}

/// A `UsageCache` document for `account`, dated `fetched_at`. Field names from
/// `src/ant/cache.rs::UsageCache`; `schema_version` equals
/// `USAGE_CACHE_SCHEMA_VERSION` (1).
fn usage_cache_body(account: &str, fetched_at: chrono::DateTime<chrono::Utc>) -> String {
    serde_json::json!({
        "schema_version": 1,
        "fetched_at": fetched_at.to_rfc3339(),
        "account": account,
        "today_usd": 1.25,
        "mtd_usd": 42.5,
        "tz": "UTC",
        "tokens_by_model": {}
    })
    .to_string()
}

/// `chrono::Utc::now()` minus `minutes`.
fn minutes_ago(minutes: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::minutes(minutes)
}

/// The `caches.<name>` row of a `config validate --json` document.
fn cache_row<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["caches"]
        .get(name)
        .filter(|v| !v.is_null())
        .unwrap_or_else(|| panic!("`caches.{name}` must be reported: {report}"))
}

/// The findings keyed at `caches.<name>`.
fn cache_findings<'a>(findings: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    findings_at(findings, &format!("caches.{name}"))
}

/// THE consumption proof, and the reason every seeded arm below calls it.
///
/// Asserts the path the binary REPORTED BACK equals one of the paths
/// [`ConfigEnv::seed_cache`] actually wrote. Without this a mis-seed silently
/// exercises the `absent` branch and the whole row passes vacuously — which is
/// how the round-1 cache fixtures would have failed, and why a
/// `starts_with(home)` or `contains("prices")` substring check is NOT an
/// acceptable substitute: both survive writing to a directory nothing reads.
fn assert_reported_path_is_seeded(report: &serde_json::Value, name: &str, seeded: &[PathBuf]) {
    let reported = cache_row(report, name)["path"]
        .as_str()
        .unwrap_or_else(|| panic!("`caches.{name}.path` must be a string: {report}"));
    let seeded_strings: Vec<String> = seeded.iter().map(|p| p.display().to_string()).collect();
    assert!(
        seeded_strings.iter().any(|s| s == reported),
        "the binary must have read a path this test SEEDED — if it reports a path we never wrote, \
         the arm is exercising the `absent` branch and proves nothing. reported={reported:?} \
         seeded={seeded_strings:?}"
    );
}

/// Both `[ant]` staleness thresholds round-trip `parse_max_age` through the CLI.
///
/// `"soon"` has no number at all; `"48"` is the subtler defect — a bare number
/// with NO unit, which a permissive parser would happily read as seconds (or
/// hours, or days) and silently mean something the user did not write.
#[test]
#[serial]
fn ant_thresholds_validated() {
    let env = ConfigEnv::new(
        "[ant]\n\
         enabled = true\n\
         usage_stale_after = \"soon\"\n\
         models_stale_after = \"48\"\n",
    );
    let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);

    assert!(
        !ok,
        "an unparseable threshold is an ERROR; code={code:?} stderr={stderr}"
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    for (key, bad) in [
        ("ant.usage_stale_after", "soon"),
        ("ant.models_stale_after", "48"),
    ] {
        let hits = findings_at(findings, key);
        assert_eq!(
            hits.len(),
            1,
            "exactly one finding at `{key}` — each threshold is judged on its own key, so a user \
             with two defects is told about both: {report}"
        );
        assert_eq!(hits[0]["kind"], "invalid_value", "at `{key}`: {report}");
        assert_eq!(hits[0]["severity"], "error", "at `{key}`: {report}");
        let message = hits[0]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("s/m/h/d"),
            "the message at `{key}` must name the grammar: {message:?}"
        );
        assert!(
            !message.contains(&format!("\"{bad}\"")) && !message.contains(&format!("`{bad}`")),
            "the message at `{key}` must not echo the user's value: {message:?}"
        );
    }
    assert_eq!(report["counts"]["errors"], 2, "{report}");
}

/// SC3, through the shipped binary, across all FIVE cache states and all THREE
/// caches.
///
/// **Stale, unreadable and unparseable WARN and still exit 0. Fresh and absent
/// emit NOTHING.**
///
/// The absent-is-silent half is the SETTLED policy, and it supersedes the
/// round-1 must-haves and documentation, which said absent caches warn — a
/// missing cache is the normal state for a user who has never run a sync, so
/// warning about it would make the default experience noisy. The resolution in
/// favour of SILENCE is recorded at
/// `src/config_validation.rs::report_cache_findings`.
///
/// Every SEEDED arm calls [`assert_reported_path_is_seeded`]. That assertion is
/// the row's foundation, not decoration: a mis-seed would otherwise exercise the
/// `absent` branch five times over and pass.
#[test]
#[serial]
fn stale_cache_warns_exit_zero() {
    let env = ConfigEnv::new(&clean_config());
    let validate = &["config", "validate", "--json"];

    // -- helper: run, require exit EXACTLY 0, and hand back the document ----
    let run_expecting_zero = |outcome: RunOutcome, arm: &str| -> serde_json::Value {
        let (ok, code, stdout, stderr) = outcome;
        assert!(
            ok,
            "{arm}: a cache defect is a WARNING and must never fail validation (SC3/D-11); \
             code={code:?} stderr={stderr}"
        );
        assert_eq!(
            code,
            Some(0),
            "{arm}: exit code must be EXACTLY 0; stderr={stderr}"
        );
        parse_exactly_one_json(&stdout, "config validate --json")
    };

    // =================== PRICES: fresh ====================================
    env.clear_caches();
    let seeded = env.seed_cache("ant/prices.json", &price_cache_body(minutes_ago(1)));
    let report = run_expecting_zero(env.run(validate), "prices/fresh");
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "prices", &seeded);
    assert_eq!(cache_row(&report, "prices")["state"], "fresh", "{report}");
    assert!(
        cache_findings(findings, "prices").is_empty(),
        "a FRESH cache is healthy and says nothing: {report}"
    );

    // =================== PRICES: stale ====================================
    // 400 days, against `[pricing].max_age = "30d"`.
    env.clear_caches();
    let seeded = env.seed_cache(
        "ant/prices.json",
        &price_cache_body(chrono::Utc::now() - chrono::Duration::days(400)),
    );
    let report = run_expecting_zero(env.run(validate), "prices/stale");
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "prices", &seeded);
    assert_eq!(cache_row(&report, "prices")["state"], "stale", "{report}");
    let hits = cache_findings(findings, "prices");
    assert_eq!(
        hits.len(),
        1,
        "exactly one finding for a stale cache: {report}"
    );
    assert_eq!(hits[0]["kind"], "stale_cache", "{report}");
    assert_eq!(hits[0]["severity"], "warning", "{report}");
    assert_eq!(
        report["counts"]["errors"], 0,
        "a stale cache contributes NO errors: {report}"
    );
    assert_eq!(report["exit_code"], 0, "{report}");

    // =================== PRICES: unparseable (not JSON) ===================
    env.clear_caches();
    let seeded = env.seed_cache("ant/prices.json", "{not json");
    let report = run_expecting_zero(env.run(validate), "prices/unparseable");
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "prices", &seeded);
    assert_eq!(
        cache_row(&report, "prices")["state"],
        "unparseable",
        "{report}"
    );
    let hits = cache_findings(findings, "prices");
    assert_eq!(hits.len(), 1, "{report}");
    assert_eq!(hits[0]["kind"], "unreadable_cache", "{report}");
    assert_eq!(hits[0]["severity"], "warning", "{report}");
    assert_eq!(report["counts"]["errors"], 0, "{report}");

    // =================== PRICES: unparseable BY SCHEMA ====================
    // Valid JSON, wrong `schema_version`. This arm is what proves the
    // classifier checks the VERSION and not merely JSON validity — the typed
    // reader rejects this document, so reporting it healthy would be exactly
    // the silent fallback this command exists to end.
    env.clear_caches();
    let mut wrong: serde_json::Value =
        serde_json::from_str(&price_cache_body(minutes_ago(1))).expect("fixture is JSON");
    wrong["schema_version"] = serde_json::json!(99);
    let seeded = env.seed_cache("ant/prices.json", &wrong.to_string());
    let report = run_expecting_zero(env.run(validate), "prices/schema-mismatch");
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "prices", &seeded);
    assert_eq!(
        cache_row(&report, "prices")["state"],
        "unparseable",
        "a schema_version this build does not accept is UNPARSEABLE, not fresh: {report}"
    );
    let hits = cache_findings(findings, "prices");
    assert_eq!(hits.len(), 1, "{report}");
    assert_eq!(hits[0]["kind"], "unreadable_cache", "{report}");
    assert_eq!(report["counts"]["errors"], 0, "{report}");

    // =================== PRICES: unreadable (permission) ==================
    // Skipped as root, where the permission bit does not bite and the arm would
    // silently pass by classifying the cache `fresh`.
    if running_as_root() {
        eprintln!("[stale_cache_warns_exit_zero] running as root: permission arm SKIPPED");
    } else {
        env.clear_caches();
        let seeded = env.seed_cache("ant/prices.json", &price_cache_body(minutes_ago(1)));
        for path in &seeded {
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod 000");
        }
        let report = run_expecting_zero(env.run(validate), "prices/unreadable");
        let findings = validate_findings(&report);
        assert_reported_path_is_seeded(&report, "prices", &seeded);
        assert_eq!(
            cache_row(&report, "prices")["state"],
            "unreadable",
            "{report}"
        );
        let hits = cache_findings(findings, "prices");
        assert_eq!(hits.len(), 1, "{report}");
        assert_eq!(hits[0]["kind"], "unreadable_cache", "{report}");
        assert_eq!(hits[0]["severity"], "warning", "{report}");
        assert_eq!(report["counts"]["errors"], 0, "{report}");
        for path in &seeded {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("chmod back");
        }
    }

    // =================== PRICES: absent ===================================
    env.clear_caches();
    let (ok, code, stdout, stderr) = env.run(validate);
    assert!(ok, "code={code:?} stderr={stderr}");
    assert_eq!(code, Some(0), "stderr={stderr}");
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);
    assert_eq!(cache_row(&report, "prices")["state"], "absent", "{report}");
    assert!(
        cache_findings(findings, "prices").is_empty(),
        "an ABSENT cache emits NOTHING — not a stale_cache finding, not an unreadable_cache \
         finding, nothing. This is the SETTLED policy (report_cache_findings): a missing cache \
         is the normal state for a user who has never synced. Found: {:?} in {report}",
        cache_findings(findings, "prices")
    );
    assert!(
        findings.is_empty(),
        "the clean fixture with no caches at all must produce an EMPTY findings array — which is \
         also the negative control for every arm above: {report}"
    );

    // =================== MODELS: stale ====================================
    // A separate cache with a separate threshold: `[ant].models_stale_after`
    // is `48h` in the clean fixture, so 72 hours is beyond it.
    env.clear_caches();
    let seeded = env.seed_cache("ant/models.json", &models_cache_body(minutes_ago(72 * 60)));
    let report = run_expecting_zero(env.run(validate), "models/stale");
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "models", &seeded);
    assert_eq!(cache_row(&report, "models")["state"], "stale", "{report}");
    let hits = cache_findings(findings, "models");
    assert_eq!(hits.len(), 1, "{report}");
    assert_eq!(hits[0]["kind"], "stale_cache", "{report}");
    assert_eq!(hits[0]["severity"], "warning", "{report}");
    assert_eq!(report["counts"]["errors"], 0, "{report}");
    assert_eq!(
        cache_row(&report, "prices")["state"],
        "absent",
        "the models arm must not be reading the prices cache: {report}"
    );

    // =================== USAGE: positive (account SET) ====================
    // The per-account cache, which no `prices` arm can reach: `ConfigEnv::run`
    // deliberately `env_remove`s STATUSLINE_ANT_ACCOUNT. 2 hours against the
    // clean fixture's `usage_stale_after = "30m"`.
    env.clear_caches();
    let usage_seeded = env.seed_cache(
        "ant/usage/work.json",
        &usage_cache_body("work", minutes_ago(120)),
    );
    let report = run_expecting_zero(
        env.run_with_account("work", validate),
        "usage/stale (account set)",
    );
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "usage", &usage_seeded);
    assert_eq!(cache_row(&report, "usage")["state"], "stale", "{report}");
    let hits = cache_findings(findings, "usage");
    assert_eq!(hits.len(), 1, "{report}");
    assert_eq!(hits[0]["kind"], "stale_cache", "{report}");
    assert_eq!(hits[0]["severity"], "warning", "{report}");
    assert_eq!(report["counts"]["errors"], 0, "{report}");
    assert_eq!(report["exit_code"], 0, "{report}");

    // =================== USAGE: negative (account UNSET) ==================
    // The SAME file is still on disk. Without an active account there is no
    // usage cache to speak of, so the row is simply absent — and this arm is
    // what proves the positive arm above was driven by the env var rather than
    // by a row the classifier reports unconditionally.
    let (ok, code, stdout, stderr) = env.run(validate);
    assert!(ok, "code={code:?} stderr={stderr}");
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);
    assert!(
        report["caches"].get("usage").is_none_or(|v| v.is_null()),
        "with no active account the usage cache is NOT CLASSIFIED AT ALL, mirroring the render \
         path, which inserts nothing: {report}"
    );
    assert!(
        cache_findings(findings, "usage").is_empty(),
        "no usage finding without an active account: {report}"
    );
    assert!(
        report["caches"].get("prices").is_some(),
        "prices must STILL be reported — the missing row is specific to `usage`, not a collapsed \
         caches object: {report}"
    );

    // =================== USAGE: `[ant].enabled` is not a filter ===========
    // The clean fixture above runs with `enabled = false`, so the positive arm
    // already proves the row is reported while enrichment is OFF. This arm is
    // its other half: turning enrichment ON changes nothing about the row.
    // Gating here would hide a stale cache from precisely the user who just
    // toggled enrichment and is trying to work out why.
    let enabled_on = ConfigEnv::new(&clean_config().replace("enabled = false", "enabled = true"));
    let on_seeded = enabled_on.seed_cache(
        "ant/usage/work.json",
        &usage_cache_body("work", minutes_ago(120)),
    );
    let report = run_expecting_zero(
        enabled_on.run_with_account("work", validate),
        "usage/stale (enabled = true)",
    );
    let findings = validate_findings(&report);
    assert_reported_path_is_seeded(&report, "usage", &on_seeded);
    assert_eq!(
        cache_row(&report, "usage")["state"],
        "stale",
        "the classifier gates on the ACCOUNT, never on `[ant].enabled`: {report}"
    );
    assert_eq!(cache_findings(findings, "usage").len(), 1, "{report}");
    assert_eq!(report["counts"]["errors"], 0, "{report}");
}

// ===========================================================================
// Plan 12-10 Task 3: cross-surface agreement, and the FIFO termination proof.
// ===========================================================================

/// `mkfifo(2)` without adding a dependency, and without a PATH-resolved utility.
///
/// Copied from the idiom `src/config_validation.rs` established for its own
/// classifier tests: `libc` is already linked into every Rust binary through
/// `std` and is only an INDIRECT entry in `Cargo.lock`, so declaring the one
/// symbol keeps `Cargo.toml` / `Cargo.lock` byte-unchanged. The plan also
/// allowed shelling out to the `mkfifo` utility; that would resolve against the
/// DEVELOPER's PATH (the restricted PATH in [`ConfigEnv::env_block`] applies to
/// the child under test, not to this process), which is exactly the class of
/// PATH-dependent test setup this suite was rewritten to avoid.
///
/// `mode_t` is `u16` on macOS and `u32` on Linux; declaring the wrong width
/// would be an ABI mismatch, so it is `cfg`-selected.
mod cfifo {
    #[cfg(target_os = "macos")]
    pub type ModeT = u16;
    #[cfg(not(target_os = "macos"))]
    pub type ModeT = u32;

    extern "C" {
        pub fn mkfifo(path: *const std::os::raw::c_char, mode: ModeT) -> std::os::raw::c_int;
    }
}

/// Create a FIFO at `path`, creating parents, and PROVE it is a FIFO.
///
/// The proof is not ceremony: if the node silently failed to be created as a
/// FIFO the classifier would report `absent` and the termination budget below
/// would be met trivially, by a run that never faced the hazard.
fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;

    fs::create_dir_all(path.parent().expect("fifo path has a parent")).expect("create fifo parent");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
    let rc = unsafe { cfifo::mkfifo(c_path.as_ptr(), 0o644 as cfifo::ModeT) };
    assert_eq!(
        rc,
        0,
        "mkfifo({}) failed: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    let ft = fs::symlink_metadata(path)
        .expect("stat the new fifo")
        .file_type();
    assert!(
        ft.is_fifo(),
        "the node at {} must really be a FIFO, or this test faces no hazard at all",
        path.display()
    );
}

/// `ant doctor` and `config validate` reach the SAME verdict on the SAME cache
/// at a NON-DEFAULT threshold (D-12).
///
/// # What this establishes
///
/// That the two DIAGNOSTIC surfaces read the same configured threshold and
/// decide staleness through the same helper, `crate::ant::duration::is_stale`.
/// The threshold matters: at the defaults (`48h` for models, `30d` for prices)
/// two surfaces with independently drifting logic would still agree about most
/// ages by coincidence, so an agreement test at the defaults proves almost
/// nothing. `5m` against those defaults puts a 10-minute-old cache on OPPOSITE
/// sides of the default and configured answers, so reading the wrong threshold
/// — or no threshold — is immediately visible. Both directions are asserted, so
/// a surface that simply always says "stale" fails too.
///
/// # What this establishes about `crate::pricing::select_synced_at`: NOTHING
///
/// That function is deliberately NOT routed through `is_stale`. It is a price
/// SOURCE-SELECTION decision, not a diagnostic one; plan 12-02 documented the
/// divergence (they differ on a malformed threshold), pinned it with its own
/// test, and declined to reroute it because doing so would change what the
/// statusline RENDERS. This test must not be read as claiming otherwise.
#[test]
#[serial]
fn doctor_and_validate_agree_on_staleness_at_a_non_default_threshold() {
    // `5m` where the defaults are `48h` and `30d`.
    let env = ConfigEnv::new(
        "[pricing]\n\
         source = \"auto\"\n\
         max_age = \"5m\"\n\
         \n\
         [ant]\n\
         enabled = false\n\
         models_stale_after = \"5m\"\n",
    );

    let read_both = |minutes: i64| -> (serde_json::Value, serde_json::Value) {
        env.clear_caches();
        let prices_seeded =
            env.seed_cache("ant/prices.json", &price_cache_body(minutes_ago(minutes)));
        let models_seeded =
            env.seed_cache("ant/models.json", &models_cache_body(minutes_ago(minutes)));

        let (_, code, stdout, stderr) = env.run(&["ant", "doctor", "--json"]);
        assert_eq!(
            code,
            Some(0),
            "ant doctor --json must exit 0; stderr={stderr}"
        );
        let doctor = parse_exactly_one_json(&stdout, "ant doctor --json");

        let (ok, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);
        assert!(
            ok,
            "agreement must not cost the SC3 exit contract: a cache verdict is a WARNING and \
             `config validate` must still exit 0; code={code:?} stderr={stderr}"
        );
        assert_eq!(code, Some(0), "stderr={stderr}");
        let validate = parse_exactly_one_json(&stdout, "config validate --json");
        validate_findings(&validate);

        // Both surfaces must be looking at the file this test wrote.
        assert_reported_path_is_seeded(&validate, "prices", &prices_seeded);
        assert_reported_path_is_seeded(&validate, "models", &models_seeded);
        for (name, seeded) in [("prices", &prices_seeded), ("models", &models_seeded)] {
            let reported = doctor["caches"][name]["path"].as_str().unwrap_or_default();
            assert!(
                seeded.iter().any(|p| p.display().to_string() == reported),
                "ant doctor must report a path this test seeded for `{name}`, or the two \
                 surfaces are not being compared on the SAME cache: reported={reported:?} \
                 seeded={seeded:?}"
            );
            assert_eq!(
                doctor["caches"][name]["present"], true,
                "ant doctor must see the seeded `{name}` cache: {doctor}"
            );
        }
        (doctor, validate)
    };

    // -- 10 minutes old: FRESH under both defaults, STALE under both `5m`s --
    let (doctor, validate) = read_both(10);
    for name in ["models", "prices"] {
        assert_eq!(
            doctor["caches"][name]["stale"], true,
            "ant doctor must read the CONFIGURED `{name}` threshold, not the default: {doctor}"
        );
        assert_eq!(
            cache_row(&validate, name)["state"],
            "stale",
            "config validate must reach the same verdict for `{name}`: {validate}"
        );
    }
    assert_eq!(
        validate["exit_code"], 0,
        "the stale run must still exit 0 (SC3): {validate}"
    );
    assert_eq!(validate["counts"]["errors"], 0, "{validate}");

    // -- 1 minute old: FRESH under the configured `5m` too ------------------
    // The other direction. Without it a surface hardwired to "stale" passes.
    let (doctor, validate) = read_both(1);
    for name in ["models", "prices"] {
        assert_eq!(
            doctor["caches"][name]["stale"], false,
            "a 1-minute-old `{name}` cache is inside a `5m` threshold: {doctor}"
        );
        assert_eq!(
            cache_row(&validate, name)["state"],
            "fresh",
            "config validate must reach the same verdict for `{name}`: {validate}"
        );
    }
}

/// `config validate` TERMINATES on a FIFO at a cache path — plan 12-06's
/// metadata-first guard, proven through the shipped binary.
///
/// `std::fs::File::open` on a writer-less FIFO blocks indefinitely: reproduced
/// at the shell against this very layout, where a 3-second `timeout` returned
/// 124. A byte cap bounds how much is read, not how long the open waits, so the
/// only defence is deciding from `symlink_metadata` and returning WITHOUT
/// opening. This is therefore a TERMINATION proof, not a size proof.
///
/// The budget is explicit — a 5-second `try_wait` poll with a `kill()` on expiry
/// — because relying on the harness timeout cannot distinguish "returned
/// unreadable" from "hung": it would fail the whole test binary minutes later
/// with no attribution.
#[test]
#[serial]
fn validate_terminates_on_a_fifo_cache_path() {
    use std::io::Read as _;

    let env = ConfigEnv::new(&clean_config());
    env.clear_caches();

    let mut fifos: Vec<PathBuf> = Vec::new();
    for root in ConfigEnv::cache_roots(&env) {
        let path = root
            .join("claudia-statusline")
            .join("ant")
            .join("prices.json");
        make_fifo(&path);
        fifos.push(path);
    }

    let mut cmd = Command::new(test_support::test_binary());
    cmd.args(["config", "validate", "--json"]);
    env.apply_env(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn statusline");

    // The document is ~1 KiB, far below the pipe buffer, so leaving stdout
    // unread while polling cannot itself deadlock the child.
    let budget = std::time::Duration::from_secs(5);
    let deadline = std::time::Instant::now() + budget;
    let status = loop {
        match child.try_wait().expect("try_wait on the child") {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "`config validate` BLOCKED on a non-regular cache file: it did not exit \
                         within {budget:?} with a writer-less FIFO at the prices cache path. \
                         `File::open` on such a FIFO waits forever, so the classifier MUST decide \
                         from metadata and return without opening."
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    };

    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut stdout)
        .expect("read child stdout");

    assert_eq!(
        status.code(),
        Some(0),
        "an unreadable cache is a WARNING, so the run exits 0 (SC3); stdout={:?}",
        String::from_utf8_lossy(&stdout)
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);

    let row = cache_row(&report, "prices");
    assert_eq!(
        row["state"], "unreadable",
        "a FIFO is present but not obtainable: {report}"
    );
    let reported = row["path"].as_str().unwrap_or_default();
    assert!(
        fifos.iter().any(|p| p.display().to_string() == reported),
        "the binary must have classified one of the FIFOs this test created: reported={reported:?} \
         fifos={fifos:?}"
    );
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("fifo"),
        "the detail must name the FILE TYPE, so a user can tell this apart from a permission \
         problem: {detail:?}"
    );

    let hits = cache_findings(findings, "prices");
    assert_eq!(hits.len(), 1, "{report}");
    assert_eq!(hits[0]["kind"], "unreadable_cache", "{report}");
    assert_eq!(hits[0]["severity"], "warning", "{report}");
    assert_eq!(report["counts"]["errors"], 0, "{report}");

    for path in &fifos {
        let _ = fs::remove_file(path);
    }
}

// ===========================================================================
// Plan 12-12 Task 1: the phase's SECURITY proofs.
//
// Every instrument below is paired with a control that proves it can OBSERVE
// the event it denies, and every test name is scoped to what its mechanism
// actually establishes. A security test that cannot fail certifies a guarantee
// nobody checked, which is worse than no test at all — this phase has six live
// examples of exactly that, including the `touch`-based marker this file's
// fixture replaced.
// ===========================================================================

/// ESC, the lead byte of every ANSI control sequence.
const ESC_BYTE: u8 = 0x1b;
/// BEL, the other byte an injected config value uses to reach the terminal.
const BEL_BYTE: u8 = 0x07;

/// Count non-overlapping-safe occurrences of `needle` in raw `haystack` bytes.
///
/// Counts EVERY occurrence, not just the first: the TOML deserialization path
/// leaks its value TWICE in an unredacted binary (once in the echoed source
/// line, once in `invalid type: string "…"`), and an assertion that stopped at
/// the first hit would still pass against a half-fixed redaction.
fn occurrences(haystack: &[u8], needle: &str) -> usize {
    let n = needle.as_bytes();
    if n.is_empty() || haystack.len() < n.len() {
        return 0;
    }
    haystack.windows(n.len()).filter(|w| *w == n).count()
}

/// Assert that NO `fragment` appears in either stream.
///
/// WR-02: [`assert_no_sentinel`] looks for the WHOLE sentinel and for the
/// `sk-ant-` prefix, and a DELIMITER-SPLIT leak carries neither — the pre-fix
/// `redact_quoted_runs` cut `sk-ant-SEN"TINEL-JJJ` at the interior quote and
/// printed only the tail, `TINEL-JJJ`. A sentinel is only observable through
/// its fragments once it carries the delimiter its diagnostic renders it with,
/// so every delimiter-bearing arm asserts on fragments as well.
fn assert_no_fragments(what: &str, fragments: &[&str], stdout: &[u8], stderr: &[u8]) {
    for (stream, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        for fragment in fragments {
            let hits = occurrences(bytes, fragment);
            assert_eq!(
                hits,
                0,
                "{what}: the secret FRAGMENT {fragment:?} appeared {hits} time(s) on {stream}: \
                 {:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}

/// Assert `sentinel` (and the generic `sk-ant-` prefix) appear ZERO times in
/// both streams, naming the count and the offending stream on failure.
fn assert_no_sentinel(what: &str, sentinel: &str, stdout: &[u8], stderr: &[u8]) {
    for (stream, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        let hits = occurrences(bytes, sentinel);
        assert_eq!(
            hits,
            0,
            "{what}: the secret sentinel {sentinel:?} appeared {hits} time(s) on {stream}: {:?}",
            String::from_utf8_lossy(bytes)
        );
        let prefix_hits = occurrences(bytes, "sk-ant-");
        assert_eq!(
            prefix_hits,
            0,
            "{what}: a secret-shaped `sk-ant-` substring appeared {prefix_hits} time(s) on \
             {stream}: {:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

/// An `[ant]` config with TWO accounts: `on_path` names a program installed in
/// the controlled bin dir, `off_path` names one that is deliberately absent.
///
/// The second account is what makes the no-spawn proof NON-VACUOUS from the
/// other direction: its `admin_key_command` produces a finding, which is only
/// possible if the validator walked the accounts table and INSPECTED the argv
/// it must never execute.
fn ant_config_with_two_accounts(on_path: &str, off_path: &str) -> String {
    format!(
        "[ant]\nenabled = true\n\n\
         [ant.accounts.work]\nadmin_key_command = [\"{on_path}\"]\n\n\
         [ant.accounts.other]\nadmin_key_command = [\"{off_path}\"]\n"
    )
}

/// An `admin_key_command` in the real `security find-generic-password -w <key>`
/// shape, with the SECRET at index 3.
fn config_with_secret_in_argv(sentinel: &str) -> String {
    format!(
        "[ant]\nenabled = true\n\n[ant.accounts.work]\n\
         admin_key_command = [\"security\", \"find-generic-password\", \"-w\", \"{sentinel}\"]\n"
    )
}

/// A config whose KEY and whose VALUE both carry ESC and BEL.
///
/// The control bytes are written as TOML `\u` escapes on purpose. A RAW ESC
/// inside a TOML basic string is a SYNTAX error (`invalid basic string`,
/// verified live), so a fixture carrying raw bytes never reaches key or value
/// validation at all and the escapes could not possibly reach stdout — the test
/// would pass for the wrong reason.
///
/// Both halves are echoed by the shipped report, which is what makes the
/// positive arms below possible:
///
/// * the unknown KEY is echoed as the finding's `key`;
/// * the `admin_key_command` program NAME is echoed inside the finding's
///   message (`` `admin_key_command` starts with `…` ``).
fn config_with_terminal_escapes() -> String {
    concat!(
        "[display]\n",
        "\"bad\\u001B[31mkey\" = 1\n",
        "\n",
        "[ant]\n",
        "enabled = true\n",
        "\n",
        "[ant.accounts.\"ev\\u001Bil\"]\n",
        "admin_key_command = [\"NOPE\\u001B[31mPROG\\u0007\"]\n",
    )
    .to_string()
}

/// Spawn the binary and return `(exit code, RAW stdout, RAW stderr)`.
///
/// `ConfigEnv::run` hands stderr back as a lossy `String`, which would silently
/// rewrite exactly the bytes the escape proof is looking for. `color = true`
/// REMOVES `NO_COLOR` from the otherwise identical block.
fn run_raw(
    env: &ConfigEnv,
    args: &[&str],
    payload: Option<&str>,
    color: bool,
) -> (Option<i32>, Vec<u8>, Vec<u8>) {
    let mut cmd = Command::new(test_support::test_binary());
    cmd.args(args);
    env.apply_env(&mut cmd);
    if color {
        cmd.env_remove("NO_COLOR");
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = match payload {
        Some(body) => {
            cmd.stdin(Stdio::piped());
            let mut child = cmd.spawn().expect("spawn statusline");
            child
                .stdin
                .as_mut()
                .expect("child stdin")
                .write_all(body.as_bytes())
                .expect("write payload");
            child.wait_with_output().expect("wait for statusline")
        }
        None => {
            cmd.stdin(Stdio::null());
            cmd.output().expect("spawn statusline")
        }
    };
    (out.status.code(), out.stdout, out.stderr)
}

/// T-12-45: `config validate` never EXECUTES an `admin_key_command`.
///
/// Both halves are mandatory and both are here:
///
/// 1. **Positive control.** [`ConfigEnv::assert_marker_can_fire`] runs the fake
///    credential program directly under the IDENTICAL environment block and
///    asserts the marker appears. The instrument this replaces — the shipped
///    `tests/ant_doctor_tests.rs` fake, built on the external `touch` — could
///    never fire under a REPLACED PATH (`/bin/sh` reports
///    `touch: command not found`, exits 0, no marker), so every no-spawn
///    assertion resting on it was vacuous. Plan 12-01 repaired the mechanism
///    with builtin redirection; plan 12-02 rewired the doctor test.
/// 2. **The negative assertion.** After `config validate` in both output modes
///    the marker is still absent.
///
/// A third arm bounds the other direction: a SECOND account names a program
/// that is NOT on PATH, and the report must carry a finding about it. That
/// proves the validator walked the accounts table and READ the argv — so
/// "no marker" means "inspected without executing", not "never looked".
#[test]
#[serial]
fn validate_never_execs_credential_command() {
    let env = ConfigEnv::new(&ant_config_with_two_accounts(
        "fake-admin-key",
        "definitely-not-on-path-12-12",
    ));
    env.install_fake_admin_key("fake-admin-key");

    // 1. POSITIVE CONTROL — before anything else.
    env.assert_marker_can_fire("fake-admin-key");

    // 2. The negative assertion, in both output modes.
    let (_, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);
    assert!(
        !env.marker_fired(),
        "`config validate --json` EXECUTED admin_key_command (marker present). \
         code={code:?} stderr={stderr}"
    );
    let (_, code_h, stdout_h, stderr_h) = env.run(&["config", "validate"]);
    assert!(
        !env.marker_fired(),
        "`config validate` EXECUTED admin_key_command (marker present). \
         code={code_h:?} stderr={stderr_h}"
    );

    // 3. NON-VACUITY from the other direction: the accounts table really was
    //    traversed and the argv really was inspected.
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let findings = validate_findings(&report);
    let hits = findings_at(findings, "ant.accounts.other.admin_key_command");
    assert_eq!(
        hits.len(),
        1,
        "the validator must INSPECT every account's `admin_key_command` — without a finding for \
         the off-PATH account, `marker absent` could simply mean the accounts table was never \
         reached: {report}"
    );
    assert_eq!(hits[0]["severity"], "warning", "{report}");
    assert!(
        String::from_utf8_lossy(&stdout_h).contains("definitely-not-on-path-12-12"),
        "the human report must name the off-PATH program too: {:?}",
        String::from_utf8_lossy(&stdout_h)
    );
}

/// T-12-46: `config validate` invokes NO PATH-RESOLVED network tool.
///
/// # What this establishes — and what it does NOT
///
/// It establishes that no PATH-resolved `curl` and no PATH-resolved `ant` was
/// invoked during a full `config validate --json` run: both names are the FIRST
/// (and, under this fixture, ONLY) entries on the child's PATH, each appends
/// its own name to a shared log through shell builtins, and the log is empty.
///
/// It does **not** establish that zero sockets were opened — a fake-`curl`
/// invocation log cannot observe a socket. It would **not** detect a program
/// executed by ABSOLUTE path, which bypasses PATH resolution entirely. The
/// round-1 name for this test, `validate_opens_no_socket`, claimed both of
/// those and is deliberately not used.
///
/// The complementary evidence that actually bounds network use is structural
/// and lives in this phase's earlier plans: plan 12-06 greps
/// `src/config_validation.rs` and plan 12-09 greps `src/commands/config.rs` for
/// `reqwest`, `TcpStream`, `UdpSocket`, `Command::new` and `.spawn(`, and both
/// require — and record — zero hits. Process-level network tracing is
/// deliberately out of scope for this phase; it is not cross-platform and would
/// not run under this suite's macOS gate.
#[test]
#[serial]
fn validate_invokes_no_path_resolved_network_tool() {
    let env = ConfigEnv::new(&clean_config());
    env.clear_caches();
    env.install_logging_fakes(&["curl", "ant"]);

    // POSITIVE CONTROL, both programs: an empty log must mean "not invoked",
    // never "could not write".
    env.assert_logger_can_fire("curl");
    env.assert_logger_can_fire("ant");

    let (_, code, stdout, stderr) = env.run(&["config", "validate", "--json"]);
    assert_eq!(
        code,
        Some(0),
        "a clean config with absent caches must exit 0; stderr={stderr}"
    );
    let report = parse_exactly_one_json(&stdout, "config validate --json");
    let _ = validate_findings(&report);
    // The run really did classify the caches it would have had to fetch.
    assert_eq!(
        cache_row(&report, "prices")["state"],
        "absent",
        "the caches must have been classified — otherwise the command did nothing and could \
         hardly have invoked a fetch tool: {report}"
    );

    assert_eq!(
        env.invocations(),
        Vec::<String>::new(),
        "`config validate --json` invoked a PATH-resolved network tool: {:?}",
        env.invocations()
    );
}

/// T-12-47 (argv arm): a secret carried in an `admin_key_command` ARGUMENT
/// never reaches stdout or stderr, in either output mode.
///
/// The program NAME (`security`) MAY appear and does — that is the actionable
/// half of the diagnostic, and asserting its presence is what stops a validator
/// that simply printed nothing from passing this test.
#[test]
#[serial]
fn report_leaks_no_secret_material() {
    const SENTINEL: &str = "sk-ant-SENTINEL-GGG";
    let env = ConfigEnv::new(&config_with_secret_in_argv(SENTINEL));

    for args in [
        vec!["config", "validate"],
        vec!["config", "validate", "--json"],
    ] {
        let label = args.join(" ");
        let (code, stdout, stderr) = run_raw(&env, &args, None, false);
        assert_eq!(
            code,
            Some(0),
            "{label}: an unavailable credential program is a WARNING, so the run exits 0; \
             stdout={:?} stderr={:?}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr),
        );
        assert_no_sentinel(&label, SENTINEL, &stdout, &stderr);
        assert!(
            occurrences(&stdout, "security") >= 1,
            "{label}: the program NAME must still be reported — it is the useful diagnostic, and \
             a report that printed nothing would pass the redaction assertion vacuously: {:?}",
            String::from_utf8_lossy(&stdout),
        );
        assert!(
            occurrences(&stdout, "admin_key_command") >= 1,
            "{label}: the finding must name the field: {:?}",
            String::from_utf8_lossy(&stdout),
        );
    }
}

/// T-12-47 (parser arms): NEITHER TOML failure path leaks the config value,
/// while BOTH still report the failure.
///
/// Two arms, both required:
///
/// * **A — syntax.** An unclosed `admin_key_command` array. The unfixed binary
///   echoes the offending source line verbatim: ONE occurrence.
/// * **B — deserialization.** `admin_key_command` as a bare STRING. The unfixed
///   binary leaks TWICE — the source-line echo PLUS
///   `invalid type: string "sk-ant-…", expected a sequence`.
///
/// Arm B is the correction to this phase's round-1 grading, which was told the
/// type path leaked nothing. That disproof used a fixture placing
/// `admin_key_command` at `[ant]` level, where it is an ignored unknown key that
/// never reaches the deserializer. At `[ant.accounts.<name>]` — the only level
/// at which it is a real field — it leaks, twice. [`occurrences`] counts EVERY
/// hit for that reason.
#[test]
#[serial]
fn parser_diagnostics_leak_no_secret_material() {
    let arms: [(&str, &[&str], String, &str); 4] = [
        (
            "sk-ant-SENTINEL-HHH",
            &["SENTINEL-HHH"],
            config_with_secret_in_syntax_error("sk-ant-SENTINEL-HHH"),
            "syntax_error",
        ),
        (
            "sk-ant-SENTINEL-III",
            &["SENTINEL-III"],
            config_with_secret_in_type_error("sk-ant-SENTINEL-III"),
            "type_error",
        ),
        // WR-02, delimiter arm 1 — DOUBLE QUOTE. The TOML value is
        // `sk-ant-SEN"TINEL-JJJ`, so the fixture body carries `\"` while the
        // assertions use the unescaped form. serde renders it with `{:?}`,
        // escaping the interior quote; the pre-fix scanner closed its run there
        // and printed `TINEL-JJJ` in the clear.
        (
            "sk-ant-SEN\"TINEL-JJJ",
            &["TINEL-JJJ", "sk-ant-SEN"],
            config_with_secret_in_type_error("sk-ant-SEN\\\"TINEL-JJJ"),
            "type_error",
        ),
        // WR-02, delimiter arm 2 — BACKTICK, which serde does NOT escape at all
        // in an unknown-variant message, so no escape-aware scanner could have
        // closed this one either.
        (
            "sk-ant-SEN`TINEL-KKK",
            &["TINEL-KKK", "sk-ant-SEN"],
            config_with_secret_in_enum_error("sk-ant-SEN`TINEL-KKK"),
            "type_error",
        ),
    ];

    for (sentinel, fragments, body, expected_kind) in arms {
        let env = ConfigEnv::new(&body);

        // Human mode.
        let (code, stdout, stderr) = run_raw(&env, &["config", "validate"], None, false);
        let label = format!("config validate [{expected_kind}]");
        assert_eq!(
            code,
            Some(1),
            "{label}: a malformed config is an ERROR, so the run exits 1; stdout={:?}",
            String::from_utf8_lossy(&stdout),
        );
        assert_no_sentinel(&label, sentinel, &stdout, &stderr);
        assert_no_fragments(&label, fragments, &stdout, &stderr);
        assert!(
            occurrences(&stdout, "1 error(s)") >= 1,
            "{label}: the failure must still be REPORTED — a command that printed nothing would \
             satisfy the redaction assertion vacuously: {:?}",
            String::from_utf8_lossy(&stdout),
        );

        // JSON mode.
        let (code, stdout, stderr) = run_raw(&env, &["config", "validate", "--json"], None, false);
        let label = format!("config validate --json [{expected_kind}]");
        assert_eq!(code, Some(1), "{label}: exit 1");
        assert_no_sentinel(&label, sentinel, &stdout, &stderr);
        assert_no_fragments(&label, fragments, &stdout, &stderr);
        let report = parse_exactly_one_json(&stdout, &label);
        let findings = validate_findings(&report);
        assert!(
            findings
                .iter()
                .any(|f| f["kind"].as_str() == Some(expected_kind)),
            "{label}: the report must carry a `{expected_kind}` finding, so redaction cannot be \
             confused with suppression: {report}"
        );
    }
}

/// WR-02 / CR-01 vector 3: the SINGLE-QUOTE delimiter, which reaches output
/// through `redact_value_text` rather than `redact_toml_error`.
///
/// `[pricing].max_age` does NOT fail deserialization — it is a `String` — so it
/// reaches the SEMANTIC rule, whose parser (`ant::duration::parse_max_age`) was
/// written for a CLI flag and quotes the offending input back with `'…'`. A
/// value carrying a `'` therefore split the pre-fix pairing scanner exactly as
/// the other two delimiters did, on a DIFFERENT helper:
///
/// ```text
/// max_age = "a'LEAKED_MAXAGE'b"
///   → invalid --max-age number in "<redacted>"LEAKED_MAXAGE"<redacted>"
/// ```
///
/// No sentinel anywhere else in this phase contains a `'`, so this axis was
/// unobservable.
#[test]
#[serial]
fn semantic_rule_diagnostics_leak_no_fragment_of_a_quote_bearing_value() {
    const SENTINEL: &str = "sk-ant-SEN'TINEL-LLL";
    let fragments = ["TINEL-LLL", "sk-ant-SEN"];
    let env = ConfigEnv::new(&config_with_secret_in_duration_error(SENTINEL));

    for json in [false, true] {
        let args: &[&str] = if json {
            &["config", "validate", "--json"]
        } else {
            &["config", "validate"]
        };
        let (code, stdout, stderr) = run_raw(&env, args, None, false);
        let label = format!(
            "config validate{} [duration_error]",
            if json { " --json" } else { "" }
        );

        assert_eq!(
            code,
            Some(1),
            "{label}: an unparseable duration is an ERROR, so the run exits 1; stdout={:?}",
            String::from_utf8_lossy(&stdout),
        );
        assert_no_sentinel(&label, SENTINEL, &stdout, &stderr);
        assert_no_fragments(&label, &fragments, &stdout, &stderr);

        if json {
            // Non-vacuity: redaction must not be confused with suppression —
            // the rule really did fire, on the key it was supposed to fire on.
            let report = parse_exactly_one_json(&stdout, &label);
            let findings = validate_findings(&report);
            assert!(
                findings.iter().any(|f| {
                    f["key"].as_str() == Some("pricing.max_age")
                        && f["kind"].as_str() == Some("invalid_value")
                }),
                "{label}: the report must carry an `invalid_value` finding at \
                 `pricing.max_age`: {report}"
            );
        }
    }
}

/// T-12-71: the PRE-EXISTING RENDER-PATH credential leak stays closed.
///
/// This is not a leak this phase introduced — it was live on the SHIPPED binary
/// at the DEFAULT log level. `Config::load_from_file`'s parse error flowed into
/// `build_config`'s `warn!`, so merely rendering a status line with a malformed
/// config printed the offending TOML source line, secret and all, to stderr.
/// Plan 12-04 routed that error through
/// `crate::config_validation::redact_toml_error`; this test fails if that
/// rewiring is ever reverted.
///
/// Pre-fix stderr, reproduced live during plan 12-04 (deserialization arm):
///
/// ```text
/// invalid type: string "sk-ant-SENTINEL-BBB", expected a sequence
/// ```
///
/// Post-fix, verified against the shipped binary:
///
/// ```text
/// [... WARN statusline::config] Failed to load config: Configuration error:
/// Failed to parse config file: invalid type: string "<redacted>", expected a
/// sequence (line 5, column 21). Using defaults.
/// ```
///
/// `RUST_LOG` is REMOVED by `ConfigEnv::env_block`, so this runs at the default
/// `warn` level — the level the leak was reachable at. `sanitize_for_terminal`
/// is NOT a mitigation here: it strips control bytes and PRESERVES every
/// printable character, so a printable API key passes straight through it.
#[test]
#[serial]
fn render_path_toml_error_leaks_no_secret_material() {
    let arms: [(&str, &[&str], String, &str); 3] = [
        (
            "sk-ant-SENTINEL-DDD",
            &["SENTINEL-DDD"],
            config_with_secret_in_syntax_error("sk-ant-SENTINEL-DDD"),
            "syntax",
        ),
        (
            "sk-ant-SENTINEL-CCC",
            &["SENTINEL-CCC"],
            config_with_secret_in_type_error("sk-ant-SENTINEL-CCC"),
            "deserialization",
        ),
        // WR-02 / CR-01: the SAME leak, on the SAME render path, from a value
        // that carries the delimiter its diagnostic renders it with. The
        // pre-fix binary printed `TINEL-MMM` here at the DEFAULT log level.
        // Double quote is the only one of CR-01's three delimiters reachable on
        // this path: `Config::load_from_file` fails on a whole-document
        // deserialization, where serde renders strings with `{:?}`. The
        // backtick vector is `[pricing]`, whose field is `deserialize_lenient`
        // and never aborts the document load; the single-quote vector belongs
        // to a SEMANTIC rule that only `config validate` runs.
        (
            "sk-ant-SEN\"TINEL-MMM",
            &["TINEL-MMM", "sk-ant-SEN"],
            config_with_secret_in_type_error("sk-ant-SEN\\\"TINEL-MMM"),
            "deserialization, delimiter-bearing",
        ),
    ];

    for (sentinel, fragments, body, arm) in arms {
        let env = ConfigEnv::new(&body);
        let payload = render_payload(env.home_path());
        let (code, stdout, stderr) = run_raw(&env, &[], Some(&payload), false);
        let label = format!("render [{arm}]");

        assert_eq!(
            code,
            Some(0),
            "{label}: a malformed config must never fail the render; stderr={:?}",
            String::from_utf8_lossy(&stderr),
        );
        assert_no_sentinel(&label, sentinel, &stdout, &stderr);
        assert_no_fragments(&label, fragments, &stdout, &stderr);

        // Redacted, NOT suppressed: the user is still told the config failed.
        let err = String::from_utf8_lossy(&stderr);
        assert!(
            err.contains("Failed to load config"),
            "{label}: the render must still REPORT the config failure at the default log level, \
             otherwise this test would also pass against a binary that silently swallowed it: \
             {err:?}"
        );
        assert!(
            err.contains("<redacted>") || err.contains("invalid array"),
            "{label}: the diagnostic must survive redaction with its shape intact: {err:?}"
        );
        assert!(
            !stdout.is_empty(),
            "{label}: the status line must still render on built-in defaults"
        );
    }
}

/// The OTHER render-path credential leak: `[pricing]`'s LENIENT deserializer.
///
/// `crate::pricing::deserialize_lenient` buffers the `[pricing]` subtree through
/// a `toml::Value` so a bad `[pricing]` cannot abort the whole document, then
/// `warn!`s about the failure. It interpolated serde's RAW message, which quotes
/// the offending value back:
///
/// ```text
/// [pricing] source = "sk-ant-PLAINSECRET-999"
///   → [WARN statusline::pricing] Invalid [pricing] config: unknown variant
///     `sk-ant-PLAINSECRET-999`, expected one of `auto`, `bundled`, `synced`
///     in `source`. Using pricing defaults (rest of config kept).
/// ```
///
/// That is a WHOLE-value leak, not a split one, so it is not CR-01 and it is
/// not observed by [`render_path_toml_error_leaks_no_secret_material`] either:
/// this path never reaches `Config::load_from_file`'s error, which is the only
/// render-path error that test drives. It was found while reproducing CR-01 and
/// is strictly worse than CR-01 — no delimiter in the value is required.
///
/// Both arms below fail against the pre-fix `deserialize_lenient`; the first
/// needs no delimiter at all.
#[test]
#[serial]
fn render_path_lenient_pricing_error_leaks_no_secret_material() {
    let arms: [(&str, &[&str], String); 2] = [
        (
            "sk-ant-SENTINEL-NNN",
            &["SENTINEL-NNN"],
            config_with_secret_in_enum_error("sk-ant-SENTINEL-NNN"),
        ),
        (
            "sk-ant-SEN`TINEL-OOO",
            &["TINEL-OOO", "sk-ant-SEN"],
            config_with_secret_in_enum_error("sk-ant-SEN`TINEL-OOO"),
        ),
    ];

    for (sentinel, fragments, body) in arms {
        let env = ConfigEnv::new(&body);
        let payload = render_payload(env.home_path());
        let (code, stdout, stderr) = run_raw(&env, &[], Some(&payload), false);
        let label = format!("render [lenient pricing: {sentinel}]");

        assert_eq!(
            code,
            Some(0),
            "{label}: a bad [pricing] must never fail the render; stderr={:?}",
            String::from_utf8_lossy(&stderr),
        );
        assert_no_sentinel(&label, sentinel, &stdout, &stderr);
        assert_no_fragments(&label, fragments, &stdout, &stderr);

        // Redacted, NOT suppressed. Without this the test would also pass
        // against a binary that simply stopped warning.
        let err = String::from_utf8_lossy(&stderr);
        assert!(
            err.contains("Invalid [pricing] config"),
            "{label}: the render must still REPORT the bad section at the default log level: \
             {err:?}"
        );
        assert!(
            err.contains("<redacted>"),
            "{label}: the diagnostic must survive redaction with its shape intact: {err:?}"
        );
        assert!(
            !stdout.is_empty(),
            "{label}: the status line must still render on pricing defaults"
        );
    }
}

/// T-12-48: no terminal escape byte reaches stdout or stderr from a config KEY
/// or VALUE — asserted on the RAW bytes of a spawned process.
///
/// Deliberately NOT written the way `test_sanitized_output` was: that unit test
/// called `sanitize_for_terminal` itself and compared the result to its own
/// output, a tautology that missed R5-CR-01 for five review rounds and was
/// DELETED rather than supplemented. This file never CALLS
/// `sanitize_for_terminal` — the only occurrences of that identifier anywhere in
/// it are inside doc comments such as this one, which plan 12-12 requires in
/// order to record why the tautology is avoided and (in
/// [`render_path_toml_error_leaks_no_secret_material`]) why the helper is not a
/// leak mitigation. `grep -n 'sanitize_for_terminal' tests/config_validate_tests.rs`
/// therefore shows prose lines and NO call site; the plan's literal
/// "`grep -c` returns 0" acceptance criterion is unsatisfiable alongside the
/// prose the same plan mandates, and the CALL-SITE reading is the one that
/// carries the guarantee.
///
/// # Defense in depth, and what a single-layer mutation proves
///
/// Two independent layers strip control bytes before stdout, verified by
/// mutation during plan 12-12:
///
/// * UPSTREAM — `config_validation::redact_key_path` and `ant::config::program_label`
///   drop `char::is_control()` bytes while KEEPING the printable remainder, so
///   `bad<ESC>[31mkey` becomes `bad[31mkey`;
/// * AT THE PRINT SITE — `commands::config`'s `sanitize_for_terminal`, which
///   removes the WHOLE SGR sequence, so the same key would become `badkey`.
///
/// Removing EITHER layer alone therefore leaves no ESC byte on stdout, and this
/// test's escape assertion does not fire. That is a real property of the code,
/// not a vacuous test: removing the print-site layer alone leaves the test
/// green, removing the upstream layer alone trips the POSITIVE ARM below
/// (`[31mkey` no longer survives), and removing BOTH trips the escape assertion
/// itself. All three outcomes are recorded in this plan's SUMMARY.
///
/// Three controls keep this from passing vacuously:
///
/// 1. **The byte instrument is shown able to SEE an ESC.** A plain render with
///    `NO_COLOR` removed emits the status line's own SGR sequences, and this
///    test asserts they are present — the same `Command::output()` byte path
///    that reports zero escapes for `config validate`. This is also the
///    legitimate-color-survives half: sanitizing the COMPOSED coloured string
///    instead of the untrusted inner value would ship a colourless status line,
///    which a bare "no ESC anywhere" assertion would score as a PASS.
/// 2. **The printable remnants are asserted present.** `display.bad<ESC>[31mkey`
///    must still be reported as `display.bad[31mkey`, and the injected program
///    name as `NOPE[31mPROG` — so a validator that printed nothing fails.
/// 3. **Both colour modes are exercised.** `config validate` emits no colour at
///    all (verified live: zero ESC bytes even with `NO_COLOR` unset), so the
///    absence of escapes is a property of the report, not an artifact of the
///    fixture setting `NO_COLOR=1`.
#[test]
#[serial]
fn report_does_not_emit_terminal_escapes() {
    let env = ConfigEnv::new(&config_with_terminal_escapes());

    // --- CONTROL 1: the byte instrument can observe an ESC, and the status
    //     line's own colours survive the sanitization boundary. ------------
    let payload = render_payload(env.home_path());
    let (code, coloured, _) = run_raw(&env, &[], Some(&payload), true);
    assert_eq!(code, Some(0), "the control render must exit 0");
    assert!(
        coloured.contains(&ESC_BYTE),
        "CONTROL FAILED: a coloured render emitted NO ESC byte, so this test's byte instrument \
         cannot observe an escape and every assertion below is vacuous: {:?}",
        String::from_utf8_lossy(&coloured)
    );
    assert!(
        occurrences(&coloured, "\u{1b}[0m") >= 1,
        "CONTROL FAILED: the renderer's own colour RESET is missing, so legitimate colour did \
         not survive sanitization: {:?}",
        String::from_utf8_lossy(&coloured)
    );

    // --- The proof, in both output modes and both colour modes. ----------
    for args in [
        vec!["config", "validate"],
        vec!["config", "validate", "--json"],
    ] {
        for colour in [false, true] {
            let label = format!("{} (colour={colour})", args.join(" "));
            let (code, stdout, stderr) = run_raw(&env, &args, None, colour);
            assert_eq!(
                code,
                Some(1),
                "{label}: the fixture carries unknown-key and invalid-value ERRORS, so the run \
                 exits 1; stdout={:?}",
                String::from_utf8_lossy(&stdout)
            );

            for (stream, bytes) in [("stdout", &stdout), ("stderr", &stderr)] {
                assert!(
                    !bytes.contains(&ESC_BYTE),
                    "{label}: an ESC byte reached {stream}: {:?}",
                    String::from_utf8_lossy(bytes)
                );
                assert!(
                    !bytes.contains(&BEL_BYTE),
                    "{label}: a BEL byte reached {stream}: {:?}",
                    String::from_utf8_lossy(bytes)
                );
            }

            // POSITIVE ARM: the untrusted text really did reach the report,
            // minus its control bytes.
            for needle in ["display.bad", "[31mkey", "NOPE", "[31mPROG"] {
                assert!(
                    occurrences(&stdout, needle) >= 1,
                    "{label}: the printable remainder {needle:?} must survive — without this a \
                     report that printed nothing would pass: {:?}",
                    String::from_utf8_lossy(&stdout)
                );
            }
        }
    }
}
