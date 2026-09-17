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
