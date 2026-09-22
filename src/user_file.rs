//! Bounded, metadata-first reads of user-supplied files (T-12-17).
//!
//! This is a dependency-free leaf: it uses only `std`, and it deliberately
//! imports NOTHING from the validation engine module (not even its label helpers)
//! so it never becomes a validation-engine call site. It returns raw
//! [`std::fs::FileType`] / [`std::io::ErrorKind`] values; callers do their own
//! labelling.
//!
//! Two layers, used together by the preset and theme loaders:
//!
//! 1. [`is_safe_file_stem`] — a config-supplied NAME (`layout.preset`,
//!    `display.theme`) is interpolated into `<dir>/<name>.toml`. A name with a
//!    path separator, `..`, a drive prefix or NUL could escape that directory,
//!    so it is refused before any path is built.
//! 2. [`read_regular_file_capped`] — the ONE metadata-first read ladder, shared
//!    with `config validate`'s target reader (`read_target_config` in
//!    `src/commands/config.rs`). A non-regular file (FIFO, device, directory,
//!    socket) is refused from metadata WITHOUT OPENING: `File::open` on a
//!    writer-less FIFO blocks forever, so no byte cap can bound it. `/dev/zero`
//!    is a character device and is refused the same way. A regular file is
//!    bounded twice: its `len` against the cap, then a `take(cap + 1)` read, so
//!    a file that grows after the stat still cannot exceed the cap.
//!
//! # Symlinks: `symlink_metadata` first, then FOLLOW
//!
//! A symlink is judged by its TARGET. Layer 1 already prevents a config NAME
//! from escaping the user's preset/theme directory; a symlink that sits INSIDE
//! that directory was placed there by the user who owns it — dotfiles managers
//! such as stow and chezmoi create exactly this arrangement — and rejecting it
//! would be a false failure on a working setup. Following it is safe because
//! the target must still be a regular file under the cap: a link to a FIFO or
//! to `/dev/zero` is refused from metadata. `read_target_config` already made
//! this choice, and one ladder must not give two answers.
//!
//! # Residual (NOT closed here)
//!
//! The stat -> open gap is WR-03 / residual R-2 (TOCTOU): a node swapped for a
//! FIFO between the metadata check and `File::open` can still block the open.
//! Closing it needs `O_NONBLOCK` + `fstat` on the opened descriptor and is a
//! separate item. The validation engine's `classify_cache` deliberately
//! keeps its own copy of this ladder and is not changed by this module.

use std::fs::FileType;
use std::io::ErrorKind;
use std::path::{Component, Path};

/// Why [`read_regular_file_capped`] refused or failed. Value-free: no variant
/// carries the path or any file contents.
#[derive(Debug)]
pub enum CappedReadError {
    /// `symlink_metadata` reported NotFound.
    NotFound,
    /// `symlink_metadata` failed for another reason.
    Stat(ErrorKind),
    /// The path is a symlink whose target does not exist.
    DanglingSymlink,
    /// The path is a symlink and its target could not be examined.
    TargetStat(ErrorKind),
    /// The (followed) node is not a regular file. It was NOT opened.
    NotRegular(FileType),
    /// The file is larger than the cap (by metadata or by the bounded read).
    TooLarge,
    /// Opening or reading failed (non-UTF-8 content is `InvalidData`).
    Read(ErrorKind),
}

/// Is `name` a bare file stem that cannot escape the directory it is joined to?
///
/// Rejects iff the name is empty (or whitespace-only), contains any of `/`,
/// `\`, `:` or NUL, contains `..`, or does not parse as exactly one
/// `Component::Normal` equal to itself. `\` and `:` are rejected on every OS so
/// a config file behaves identically everywhere (a Windows drive prefix such as
/// `C:evil` is refused on unix too).
pub fn is_safe_file_stem(name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    if name.contains(['/', '\\', ':', '\0']) || name.contains("..") {
        return false;
    }
    let mut comps = Path::new(name).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(c)), None) => c == std::ffi::OsStr::new(name),
        _ => false,
    }
}

/// Read `path` as UTF-8 text if and only if it is (or links to) a regular file
/// of at most `cap` bytes. Non-regular nodes are refused from metadata without
/// ever being opened. See the module docs for the ladder and its residual.
pub fn read_regular_file_capped(path: &Path, cap: u64) -> Result<String, CappedReadError> {
    let link_md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(CappedReadError::NotFound),
        Err(e) => return Err(CappedReadError::Stat(e.kind())),
    };

    let md = if link_md.file_type().is_symlink() {
        match std::fs::metadata(path) {
            Ok(md) => md,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(CappedReadError::DanglingSymlink)
            }
            Err(e) => return Err(CappedReadError::TargetStat(e.kind())),
        }
    } else {
        link_md
    };

    let ft = md.file_type();
    if !ft.is_file() {
        // MANDATORY early return: never open a FIFO or device.
        return Err(CappedReadError::NotRegular(ft));
    }
    if md.len() > cap {
        return Err(CappedReadError::TooLarge);
    }

    let mut buf = String::new();
    let read = std::fs::File::open(path).and_then(|f| {
        use std::io::Read;
        f.take(cap.saturating_add(1)).read_to_string(&mut buf)
    });
    if let Err(e) = read {
        return Err(CappedReadError::Read(e.kind()));
    }
    if buf.len() as u64 > cap {
        return Err(CappedReadError::TooLarge);
    }
    Ok(buf)
}

/// Test-only `mkfifo(2)` shim, shared by the preset and theme unit tests so the
/// FIFO fixtures don't each re-declare the FFI.
#[cfg(all(test, unix))]
pub(crate) mod test_fifo {
    #[cfg(target_os = "macos")]
    type ModeT = u16;
    #[cfg(not(target_os = "macos"))]
    type ModeT = u32;

    extern "C" {
        fn mkfifo(path: *const std::os::raw::c_char, mode: ModeT) -> std::os::raw::c_int;
    }

    /// Create a FIFO at `path` and PROVE it is one, so a test that "refuses a
    /// FIFO" cannot pass by facing a missing file instead.
    pub(crate) fn make_fifo(path: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::FileTypeExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("no NUL");
        let rc = unsafe { mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        assert!(std::fs::symlink_metadata(path)
            .expect("stat fifo")
            .file_type()
            .is_fifo());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn safe_stem_accepts_bare_names() {
        for ok in ["mine", "my-preset", "dark_v2", "Compact", "a.b"] {
            assert!(is_safe_file_stem(ok), "{ok:?} should be accepted");
        }
    }

    #[test]
    fn safe_stem_rejects_escapes() {
        for bad in [
            "",
            "   ",
            "../evil",
            "..",
            ".",
            "a/b",
            "a\\b",
            "/etc/passwd",
            "evil\0",
            "foo..bar",
            "C:evil",
            "evil/",
        ] {
            assert!(!is_safe_file_stem(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn reads_small_regular_file() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("a.toml");
        std::fs::write(&p, "format = \"x\"").unwrap();
        assert_eq!(read_regular_file_capped(&p, 64).unwrap(), "format = \"x\"");
    }

    #[test]
    fn exactly_cap_is_ok_cap_plus_one_is_too_large() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("a.toml");
        std::fs::write(&p, vec![b'a'; 16]).unwrap();
        assert!(read_regular_file_capped(&p, 16).is_ok());
        std::fs::write(&p, vec![b'a'; 17]).unwrap();
        assert!(matches!(
            read_regular_file_capped(&p, 16),
            Err(CappedReadError::TooLarge)
        ));
    }

    #[test]
    fn missing_is_not_found() {
        let d = TempDir::new().unwrap();
        assert!(matches!(
            read_regular_file_capped(&d.path().join("nope"), 16),
            Err(CappedReadError::NotFound)
        ));
    }

    #[test]
    fn directory_is_not_regular() {
        let d = TempDir::new().unwrap();
        match read_regular_file_capped(d.path(), 16) {
            Err(CappedReadError::NotRegular(ft)) => assert!(ft.is_dir()),
            other => panic!("expected NotRegular(dir), got {other:?}"),
        }
    }

    #[test]
    fn non_utf8_is_invalid_data() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("bin");
        std::fs::write(&p, [0xff, 0xfe, 0xfd]).unwrap();
        assert!(matches!(
            read_regular_file_capped(&p, 16),
            Err(CappedReadError::Read(ErrorKind::InvalidData))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_regular_is_followed() {
        let d = TempDir::new().unwrap();
        let target = d.path().join("real.toml");
        std::fs::write(&target, "ok").unwrap();
        let link = d.path().join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(read_regular_file_capped(&link, 16).unwrap(), "ok");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_reported() {
        let d = TempDir::new().unwrap();
        let link = d.path().join("link.toml");
        std::os::unix::fs::symlink(d.path().join("gone"), &link).unwrap();
        assert!(matches!(
            read_regular_file_capped(&link, 16),
            Err(CappedReadError::DanglingSymlink)
        ));
    }

    /// `/dev/zero` must be refused from metadata as a character device. Run on
    /// a worker thread with a 5 s join budget: a regression that reads it would
    /// otherwise grow memory without bound instead of failing.
    #[cfg(unix)]
    #[test]
    fn dev_zero_is_refused_as_char_device() {
        use std::os::unix::fs::FileTypeExt;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = read_regular_file_capped(Path::new("/dev/zero"), 16);
            let _ =
                tx.send(matches!(r, Err(CappedReadError::NotRegular(ft)) if ft.is_char_device()));
        });
        let refused = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("read_regular_file_capped did not return promptly on /dev/zero");
        assert!(
            refused,
            "/dev/zero must be refused as NotRegular(char device)"
        );
    }

    /// A writer-less FIFO must be refused from metadata. Run on a worker thread
    /// with a 5 s join budget so a regression FAILS instead of hanging the
    /// test runner (a blocked thread is leaked, not joined).
    #[cfg(unix)]
    #[test]
    fn fifo_is_refused_promptly_without_opening() {
        use std::os::unix::fs::FileTypeExt;
        let d = TempDir::new().unwrap();
        let p = d.path().join("fifo.toml");
        test_fifo::make_fifo(&p);

        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = p.clone();
        std::thread::spawn(move || {
            let r = read_regular_file_capped(&p2, 16);
            let _ = tx.send(matches!(r, Err(CappedReadError::NotRegular(ft)) if ft.is_fifo()));
        });
        let refused = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("read_regular_file_capped BLOCKED on a FIFO: it must decide from metadata");
        assert!(refused, "a FIFO must be refused as NotRegular(fifo)");
    }
}
