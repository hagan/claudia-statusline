#![cfg(unix)]
//! Closed-stdout behaviour for subcommands (INTEG-02, phase 14 plan 03).
//!
//! `statusline <cmd> | head` must end cleanly: when the reader of stdout goes
//! away, subcommand output (routed through the bin-only `cli_print!` /
//! `cli_println!` macros in `src/cli_output.rs`) is dropped without a message
//! and the command runs to completion, keeping its own exit status: 0 for a
//! read-only command, non-zero for a failure.
//!
//! The exit status is asserted as `Some(0)`, not merely "success": a process
//! killed by a signal reports `code() == None`, so `Some(0)` is the behavioural
//! proof that no global SIGPIPE disposition (`SIG_DFL`) is installed. The
//! `no_global_sigpipe_disposition` source scan keeps it from coming back.

mod test_support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

struct PipeEnv {
    home: TempDir,
}

impl PipeEnv {
    fn new() -> Self {
        PipeEnv {
            home: TempDir::new().expect("temp home"),
        }
    }

    /// Seeded env: one default render so the isolated stats.db has a session
    /// and `sessions` / `stats` have something to print.
    fn seeded() -> Self {
        let env = Self::new();
        env.seed();
        env
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let home = self.home.path();
        let mut cmd = Command::new(test_support::test_binary());
        cmd.args(args)
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("NO_COLOR", "1");
        for k in ["ANTHROPIC_API_KEY", "RUST_LOG"] {
            cmd.env_remove(k);
        }
        // Drop every inherited STATUSLINE_*/CLAUDE_* var (as test_support::init
        // does): e.g. a host STATUSLINE_DEBUG_INPUT=<path> would make the seed
        // render overwrite that real file, outside this env.
        for (k, _) in std::env::vars_os() {
            let name = k.to_string_lossy();
            if name.starts_with("STATUSLINE_") || name.starts_with("CLAUDE_") {
                cmd.env_remove(&k);
            }
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn seed(&self) {
        let payload = r#"{"session_id":"pipe-test-1","workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet 4.5"},"cost":{"total_cost_usd":0.5,"total_lines_added":1,"total_lines_removed":0}}"#;
        let mut cmd = self.cmd(&[]);
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn seed render");
        child
            .stdin
            .take()
            .expect("child stdin")
            .write_all(payload.as_bytes())
            .expect("write seed payload");
        let out = child.wait_with_output().expect("wait for seed render");
        assert!(
            out.status.success(),
            "seed render failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Spawn with the read end of stdout dropped immediately, so every write
    /// the child makes hits a broken pipe (the `| head` case).
    fn run_closed(&self, args: &[&str]) -> (Option<i32>, String) {
        let mut child = self.cmd(args).spawn().expect("spawn statusline");
        drop(child.stdout.take());
        let out = child.wait_with_output().expect("wait for statusline");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Normal run with stdout captured.
    fn run_open(&self, args: &[&str]) -> (Option<i32>, String) {
        let out = self.cmd(args).output().expect("run statusline");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }
}

/// Sanity-check that `args` writes to stdout, then assert that with stdout
/// closed it exits `Some(0)` with no broken-pipe or panic noise.
fn assert_clean_on_closed_stdout(env: &PipeEnv, args: &[&str]) {
    let (_, stdout) = env.run_open(args);
    assert!(
        !stdout.trim().is_empty(),
        "{:?} printed nothing to stdout, so the closed-pipe check proves nothing",
        args
    );

    let (code, stderr) = env.run_closed(args);
    assert_eq!(
        code,
        Some(0),
        "{:?} with closed stdout must exit 0 (None = killed by a signal); stderr: {}",
        args,
        stderr
    );
    assert!(
        !stderr.contains("Broken pipe"),
        "{:?} reported a broken pipe: {}",
        args,
        stderr
    );
    assert!(
        !stderr.contains("panicked"),
        "{:?} panicked: {}",
        args,
        stderr
    );
}

#[test]
fn sessions_list_exits_cleanly_on_broken_pipe() {
    let env = PipeEnv::seeded();
    assert_clean_on_closed_stdout(&env, &["sessions", "list"]);
}

#[test]
fn stats_exits_cleanly_on_broken_pipe() {
    let env = PipeEnv::seeded();
    assert_clean_on_closed_stdout(&env, &["stats"]);
}

#[test]
fn health_exits_cleanly_on_broken_pipe() {
    let env = PipeEnv::seeded();
    assert_clean_on_closed_stdout(&env, &["health"]);
}

#[test]
fn version_full_exits_cleanly_on_broken_pipe() {
    let env = PipeEnv::new();
    assert_clean_on_closed_stdout(&env, &["--version-full"]);
}

#[test]
fn help_exits_cleanly_on_broken_pipe() {
    let env = PipeEnv::new();
    assert_clean_on_closed_stdout(&env, &["--help"]);
}

/// A closed stdout must not turn a failure into success: `config validate` on a
/// broken config prints its report, then exits 1. With the reader gone the
/// report is dropped, but the command must still finish and exit 1 (CR-01).
#[test]
fn failing_command_keeps_nonzero_exit_on_broken_pipe() {
    let env = PipeEnv::new();
    let bad = env.home.path().join("broken.toml");
    std::fs::write(&bad, "this is = = not toml\n[unterminated\n").expect("write bad config");
    let bad = bad.to_str().expect("utf-8 temp path");
    let args = ["config", "validate", bad];

    let (open_code, stdout) = env.run_open(&args);
    assert_eq!(open_code, Some(1), "broken config must fail validation");
    assert!(
        !stdout.trim().is_empty(),
        "validate printed nothing, so the closed-pipe check proves nothing"
    );

    let (code, stderr) = env.run_closed(&args);
    assert_eq!(
        code,
        Some(1),
        "closed stdout must keep validate's failure exit (None = killed by a signal); stderr: {}",
        stderr
    );
    assert!(
        !stderr.contains("panicked"),
        "validate panicked: {}",
        stderr
    );
}

/// A closed stdout must not stop a command midway: `migrate --dump-schema`
/// prints the schema and only then deletes its temp database. With the reader
/// gone it must still run to the end and remove the file (CR-01 / WR-02).
#[test]
fn dump_schema_completes_cleanup_on_broken_pipe() {
    let env = PipeEnv::new();
    let tmp = env.home.path().join("tmp");
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    let mut child = env
        .cmd(&["migrate", "--dump-schema"])
        .env("TMPDIR", &tmp)
        .spawn()
        .expect("spawn statusline");
    drop(child.stdout.take());
    let out = child.wait_with_output().expect("wait for statusline");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(0),
        "dump-schema with closed stdout must exit 0; stderr: {}",
        stderr
    );
    let leftovers: Vec<_> = std::fs::read_dir(&tmp)
        .expect("read temp dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("statusline_schema_"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "dump-schema stopped before cleanup, temp DB left behind: {:?}",
        leftovers
    );
}

/// A closed stderr must not kill a command: `migrate --dump-schema` writes its
/// progress note to stderr. Rust's runtime ignores SIGPIPE, so a plain
/// `eprintln!` would panic there (abort under the release profile). Bin-side
/// notes go through `cli_eprintln!`, which ignores the error (WR-01).
#[test]
fn closed_stderr_does_not_panic() {
    let env = PipeEnv::new();
    let mut child = env
        .cmd(&["migrate", "--dump-schema"])
        .spawn()
        .expect("spawn statusline");
    drop(child.stderr.take());
    let out = child.wait_with_output().expect("wait for statusline");
    assert_eq!(
        out.status.code(),
        Some(0),
        "dump-schema with closed stderr must exit 0 (None = killed by a signal, 101 = panic)"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("CREATE TABLE"),
        "dump-schema did not finish writing the schema to stdout"
    );
}

/// Bin-side code must print diagnostics with `cli_eprintln!`, never a bare
/// `eprintln!`/`eprint!`, which panics on a closed stderr (WR-01). Test
/// modules (`#[cfg(test)]` onward) are not scanned.
#[test]
fn no_bare_eprintln_in_bin_code() {
    let needles = [concat!("eprint", "ln!("), concat!("eprint", "!(")];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("src/main.rs")];
    collect_rs_files(&root.join("src/commands"), &mut files);
    for file in &files {
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("read {}: {}", file.display(), e));
        for (i, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            let code = code_portion(line);
            for needle in needles {
                let bare = code
                    .match_indices(needle)
                    .any(|(idx, _)| !code[..idx].ends_with("cli_"));
                assert!(
                    !bare,
                    "bare `{}` in {} line {} (use cli_eprintln!): {}",
                    needle,
                    file.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
    }
}

/// Copy of the `ant_invariant_tests` helper: only code before a `//` counts,
/// and comment-only lines are ignored.
fn code_portion(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
        return "";
    }
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {:?}: {}", dir, e)) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_global_sigpipe_disposition() {
    // Needles are assembled so this file never matches itself if scanned.
    let needles = [concat!("SIG", "PIPE"), concat!("libc::", "signal")];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut files = Vec::new();
    collect_rs_files(&root.join("src"), &mut files);
    assert!(!files.is_empty(), "no .rs files found under src/");
    for file in &files {
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("read {}: {}", file.display(), e));
        for (i, line) in source.lines().enumerate() {
            let code = code_portion(line);
            for needle in needles {
                assert!(
                    !code.contains(needle),
                    "global signal disposition token `{}` in {} line {}: {}",
                    needle,
                    file.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
    }

    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    let mut in_deps = false;
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_deps = trimmed.starts_with("[dependencies]");
            continue;
        }
        if in_deps {
            assert!(
                !trimmed.starts_with("libc"),
                "Cargo.toml [dependencies] must not list libc directly: {}",
                line
            );
        }
    }
}
