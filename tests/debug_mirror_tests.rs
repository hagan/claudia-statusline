//! PCACHE-02: the raw stdin payload mirror (`STATUSLINE_DEBUG_INPUT`) against
//! the SHIPPED BINARY.
//!
//! The mirror must be opt-in (only an ABSOLUTE `STATUSLINE_DEBUG_INPUT` turns
//! it on), must never follow a symlink, never open (and so never block on) a
//! FIFO or other non-regular node, never create parent directories, and must
//! always leave an owner-only (0600) file holding exactly the stdin bytes.
//!
//! Every env var is passed per-child via [`Command::env`]; the suite mutates no
//! process-global state and needs no `#[serial]`.

mod test_support;

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Opus"}}"#;

/// Per-test isolated HOME/XDG tree.
struct Env {
    home: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Env {
            home: tempfile::TempDir::new().expect("isolated home"),
        }
    }

    fn root(&self) -> &Path {
        self.home.path()
    }

    /// Build a child command with an isolated environment. `debug` is the
    /// value of `STATUSLINE_DEBUG_INPUT` (None = unset).
    fn command(&self, debug: Option<&std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.env("HOME", self.root())
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env_remove("STATUSLINE_CONFIG_PATH")
            .env_remove("STATUSLINE_CONFIG")
            .env_remove("STATUSLINE_THEME")
            .env_remove("STATUSLINE_TEST_MODE")
            .env_remove("STATUSLINE_DEBUG_INPUT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(v) = debug {
            cmd.env("STATUSLINE_DEBUG_INPUT", v);
        }
        cmd
    }
}

/// Spawn `cmd`, write `stdin_payload`, and poll `try_wait` every 25 ms against
/// `budget`. On timeout the child is killed and the test panics (it BLOCKED,
/// e.g. on a writer-less/reader-less FIFO). Returns the output and the elapsed
/// wall time.
fn spawn_with_budget(
    mut cmd: Command,
    stdin_payload: &str,
    budget: Duration,
    what: &str,
) -> (Output, Duration) {
    let start = Instant::now();
    let mut child = cmd.spawn().expect("spawn statusline");
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        // EPIPE just means the child already exited; the budget loop decides.
        let _ = stdin.write_all(stdin_payload.as_bytes());
    }
    let deadline = start + budget;
    let status = loop {
        match child.try_wait().expect("try_wait on the child") {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "{what}: the binary did not exit within {budget:?}: it BLOCKED \
                         (T-17-14: the mirror must never open a non-regular target)"
                    );
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let elapsed = start.elapsed();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut stdout)
        .expect("read child stdout");
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_end(&mut stderr)
        .expect("read child stderr");
    (
        Output {
            status,
            stdout,
            stderr,
        },
        elapsed,
    )
}

fn render(cmd: Command, payload: &str, what: &str) -> Output {
    let (out, _) = spawn_with_budget(cmd, payload, Duration::from_secs(10), what);
    assert!(
        out.status.success(),
        "{what}: the render must never fail: status={:?} stderr={:?}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.is_empty(),
        "{what}: the render must print a line"
    );
    out
}

/// Recursively collect every path under `dir` (not following symlinks).
fn walk(dir: &Path, acc: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        acc.push(p.clone());
        if fs::symlink_metadata(&p)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false)
        {
            walk(&p, acc);
        }
    }
}

fn assert_no_tmp_files(dir: &Path, what: &str) {
    let leftovers: Vec<_> = fs::read_dir(dir)
        .expect("read target dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "{what}: temp files left behind in {}: {leftovers:?}",
        dir.display()
    );
}

#[cfg(unix)]
fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).expect("stat target").permissions().mode() & 0o777
}

/// `mkfifo(2)` without adding a dependency (same idiom as
/// tests/config_validate_tests.rs): `mode_t` is `u16` on macOS, `u32` elsewhere.
#[cfg(unix)]
mod cfifo {
    #[cfg(target_os = "macos")]
    pub type ModeT = u16;
    #[cfg(not(target_os = "macos"))]
    pub type ModeT = u32;

    extern "C" {
        pub fn mkfifo(path: *const std::os::raw::c_char, mode: ModeT) -> std::os::raw::c_int;
    }
}

/// Create a FIFO at `path` and PROVE it is a FIFO, otherwise the test would
/// face no hazard at all.
#[cfg(unix)]
fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
    let rc = unsafe { cfifo::mkfifo(c_path.as_ptr(), 0o644 as cfifo::ModeT) };
    assert_eq!(
        rc,
        0,
        "mkfifo({}) failed: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    let ft = fs::symlink_metadata(path).expect("stat fifo").file_type();
    assert!(ft.is_fifo(), "{} must really be a FIFO", path.display());
}

#[test]
fn mirror_unset_writes_nothing() {
    let env = Env::new();
    // The data directory is created by the stats update AFTER the mirror runs,
    // so the first render in a fresh HOME cannot reveal a default mirror. A
    // second render is the steady state a real user is in.
    render(env.command(None), PAYLOAD, "unset (prime data dir)");
    render(env.command(None), PAYLOAD, "unset");
    let mut all = Vec::new();
    walk(env.root(), &mut all);
    let hits: Vec<_> = all
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "last-input.json"))
        .collect();
    assert!(
        hits.is_empty(),
        "without STATUSLINE_DEBUG_INPUT no mirror file may be written: {hits:?}"
    );
}

#[test]
fn mirror_relative_path_writes_nothing() {
    for value in ["rel.json", "off", "0"] {
        let env = Env::new();
        let cwd = tempfile::TempDir::new().expect("cwd");
        // Prime the data directory first (see mirror_unset_writes_nothing).
        render(env.command(None), PAYLOAD, "prime data dir");
        let mut cmd = env.command(Some(std::ffi::OsStr::new(value)));
        cmd.current_dir(cwd.path());
        render(cmd, PAYLOAD, value);

        let cwd_entries: Vec<_> = fs::read_dir(cwd.path())
            .expect("read cwd")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(
            cwd_entries.is_empty(),
            "STATUSLINE_DEBUG_INPUT={value:?} must write nothing into CWD: {cwd_entries:?}"
        );
        let mut all = Vec::new();
        walk(env.root(), &mut all);
        let hits: Vec<_> = all
            .iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n == "last-input.json" || n == value)
            })
            .collect();
        assert!(
            hits.is_empty(),
            "STATUSLINE_DEBUG_INPUT={value:?} must write no mirror anywhere: {hits:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn mirror_absolute_path_writes_exact_stdin_0600() {
    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("mirror.json");

    let with = render(
        env.command(Some(target.as_os_str())),
        PAYLOAD,
        "absolute path",
    );
    assert_eq!(
        fs::read(&target).expect("mirror file exists"),
        PAYLOAD.as_bytes(),
        "the mirror must hold exactly the stdin bytes"
    );
    assert_eq!(mode_of(&target), 0o600, "the mirror must be owner-only");

    let env2 = Env::new();
    let without = render(env2.command(None), PAYLOAD, "no mirror");
    assert_eq!(
        String::from_utf8_lossy(&with.stdout),
        String::from_utf8_lossy(&without.stdout),
        "the mirror must not change the rendered line"
    );
    assert_no_tmp_files(dir.path(), "absolute path");
}

#[cfg(unix)]
#[test]
fn mirror_replaces_existing_0644_file_with_0600() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("mirror.json");
    fs::write(
        &target,
        b"old content that is longer than the payload ......................................",
    )
    .expect("pre-create target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).expect("chmod 0644");
    assert_eq!(mode_of(&target), 0o644);

    render(env.command(Some(target.as_os_str())), PAYLOAD, "0644 file");
    assert_eq!(fs::read(&target).expect("read mirror"), PAYLOAD.as_bytes());
    assert_eq!(
        mode_of(&target),
        0o600,
        "a pre-existing 0644 file must end up 0600"
    );
    assert_no_tmp_files(dir.path(), "0644 file");
}

#[cfg(unix)]
#[test]
fn mirror_writes_malformed_payload() {
    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("mirror.json");
    render(
        env.command(Some(target.as_os_str())),
        "{not json",
        "malformed payload",
    );
    assert_eq!(fs::read(&target).expect("read mirror"), b"{not json");
    assert_no_tmp_files(dir.path(), "malformed payload");
}

#[cfg(unix)]
#[test]
fn mirror_refuses_symlink() {
    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let victim = dir.path().join("victim.txt");
    let victim_bytes = b"precious victim bytes";
    fs::write(&victim, victim_bytes).expect("write victim");
    let target = dir.path().join("mirror.json");
    std::os::unix::fs::symlink(&victim, &target).expect("symlink");

    render(env.command(Some(target.as_os_str())), PAYLOAD, "symlink");

    assert!(
        fs::symlink_metadata(&target)
            .expect("stat link")
            .file_type()
            .is_symlink(),
        "the symlink must be left in place"
    );
    assert_eq!(
        fs::read_link(&target).expect("read link"),
        victim,
        "the link target must be unchanged"
    );
    assert_eq!(
        fs::read(&victim).expect("read victim"),
        victim_bytes,
        "the victim file must not be written through the symlink"
    );
    assert_no_tmp_files(dir.path(), "symlink");
}

#[cfg(unix)]
#[test]
fn mirror_refuses_fifo_and_never_blocks() {
    use std::os::unix::fs::FileTypeExt;

    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("mirror.fifo");
    make_fifo(&target);

    let (out, elapsed) = spawn_with_budget(
        env.command(Some(target.as_os_str())),
        PAYLOAD,
        Duration::from_secs(10),
        "fifo",
    );
    assert!(
        out.status.success(),
        "fifo: render failed: {:?}",
        out.status
    );
    assert!(!out.stdout.is_empty(), "fifo: the render must print a line");
    assert!(
        elapsed < Duration::from_secs(2),
        "fifo: the render took {elapsed:?}; it must not block on the FIFO"
    );
    assert!(
        fs::symlink_metadata(&target)
            .expect("stat fifo")
            .file_type()
            .is_fifo(),
        "the FIFO must be left in place"
    );
    assert_no_tmp_files(dir.path(), "fifo");
}

#[cfg(unix)]
#[test]
fn mirror_refuses_directory() {
    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("adir");
    fs::create_dir(&target).expect("mkdir target");
    fs::write(target.join("keep.txt"), b"keep").expect("seed dir");

    render(env.command(Some(target.as_os_str())), PAYLOAD, "directory");

    assert!(fs::symlink_metadata(&target)
        .expect("stat dir")
        .file_type()
        .is_dir());
    let entries: Vec<_> = fs::read_dir(&target)
        .expect("read dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["keep.txt".to_string()]);
    assert_eq!(fs::read(target.join("keep.txt")).expect("read"), b"keep");
    assert_no_tmp_files(dir.path(), "directory");
}

#[cfg(unix)]
#[test]
fn mirror_missing_parent_not_created() {
    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("nope").join("out.json");

    render(
        env.command(Some(target.as_os_str())),
        PAYLOAD,
        "missing parent",
    );
    assert!(
        !dir.path().join("nope").exists(),
        "the mirror must never create parent directories"
    );
    assert_no_tmp_files(dir.path(), "missing parent");
}

#[cfg(unix)]
#[test]
fn mirror_leaves_no_temp_files() {
    // Success then overwrite: both runs must leave only the target behind.
    let env = Env::new();
    let dir = tempfile::TempDir::new().expect("target dir");
    let target = dir.path().join("mirror.json");
    render(env.command(Some(target.as_os_str())), PAYLOAD, "first");
    render(env.command(Some(target.as_os_str())), "{not json", "second");
    let entries: Vec<_> = fs::read_dir(dir.path())
        .expect("read dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["mirror.json".to_string()]);
    assert_eq!(fs::read(&target).expect("read"), b"{not json");
}
