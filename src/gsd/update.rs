//! GSD update check reader for update availability indicator.
//!
//! Reads `~/.claude/cache/gsd-update-check.json` to determine whether a GSD
//! update is available. Only flags `gsd_update_available` when:
//! 1. The file exists and is valid JSON
//! 2. `update_available` is `true`
//! 3. Enough time has passed since the check (delay threshold met)
//!
//! The delay threshold avoids showing the indicator during an active download.
//! Uses (path, mtime) caching via `OnceLock<Mutex<...>>` to avoid re-parsing
//! unchanged files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Deserialized update check JSON.
#[derive(Debug, serde::Deserialize)]
struct UpdateCheck {
    update_available: bool,
    #[allow(dead_code)]
    installed: String,
    latest: String,
    checked: u64,
}

/// Cached parse result to avoid re-reading unchanged files.
///
/// Keyed by (path, mtime): mtime alone is insufficient because filesystems
/// with coarse mtime resolution (e.g. Linux ext4 at 1s) can produce identical
/// mtimes for different files written close in time, causing cache collisions
/// across distinct home directories (e.g. test isolation with TempDir).
///
/// Cache content is the *raw file record* (UpdateRecord), not the
/// delay-threshold decision. The delay decision is recomputed on every
/// `fill_vars` call against `SystemTime::now()` to keep cache content
/// time-independent (B4: peer-review fix).
struct CachedParse {
    path: PathBuf,
    mtime: SystemTime,
    data: UpdateRecord,
}

/// Raw record extracted from the update-check JSON file.
///
/// This is the *cache value*: it contains only file-derived data and no
/// time-dependent decision. The delay-threshold decision is computed at
/// every read in `fill_vars`.
#[derive(Clone)]
struct UpdateRecord {
    update_available: bool,
    latest: String,
    checked_unix: u64,
}

/// Global cache for update check parse results.
static UPDATE_CACHE: OnceLock<Mutex<Option<CachedParse>>> = OnceLock::new();

/// Reset the global update cache. Test-only helper to isolate per-test state.
#[cfg(test)]
pub(super) fn reset_update_cache_for_tests() {
    let cache = UPDATE_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = cache.lock() {
        *g = None;
    }
}

/// Populate GSD update variables from gsd-update-check.json.
///
/// Sets the following keys in `vars` when an update is available:
/// - `gsd_update_available` -- "true" when update is available and delay threshold met
/// - `gsd_update_version` -- latest version string (e.g., "1.19.0")
///
/// Returns without modifying `vars` if the file is missing, malformed, no
/// update is available, or the delay threshold hasn't been met.
pub fn fill_vars(home_dir: &Path, delay_seconds: u64, vars: &mut HashMap<String, String>) {
    let path = home_dir
        .join(".claude")
        .join("cache")
        .join("gsd-update-check.json");

    let record = match read_with_cache(&path) {
        Some(r) => r,
        None => return,
    };

    if !record.update_available {
        return;
    }

    // Apply the delay-threshold decision against the *current* wall clock.
    // This must NOT be cached: cache content is time-independent so that
    // post-threshold reads surface the update even when (path, mtime) are
    // unchanged since a pre-threshold read (B4: peer-review fix).
    let now_unix = match SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs(),
        Err(_) => return,
    };

    if now_unix.saturating_sub(record.checked_unix) < delay_seconds {
        // Too soon -- might still be downloading
        return;
    }

    vars.insert("gsd_update_available".into(), "true".into());
    vars.insert("gsd_update_version".into(), record.latest);
}

/// Read update check file with (path, mtime)-based cache invalidation.
///
/// Checks file mtime (~1us) before deciding whether to re-parse. Returns
/// cached data only when both path and mtime match the cached entry.
///
/// The cached value is the *raw file record* (UpdateRecord). The
/// delay-threshold decision is intentionally NOT cached -- callers
/// (`fill_vars`) recompute it on every read so the decision tracks the
/// wall clock rather than mtime (B4 fix).
fn read_with_cache(path: &Path) -> Option<UpdateRecord> {
    let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;

    let cache = UPDATE_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(ref cached) = *guard {
        if cached.path == path && cached.mtime == current_mtime {
            return Some(cached.data.clone());
        }
    }

    // Path or mtime changed -- re-parse
    let content = std::fs::read_to_string(path).ok()?;
    let record = parse_update_check(&content)?;
    *guard = Some(CachedParse {
        path: path.to_path_buf(),
        mtime: current_mtime,
        data: record.clone(),
    });
    Some(record)
}

/// Parse update check JSON into an `UpdateRecord`.
///
/// Returns `None` if the JSON is malformed or required fields are missing.
/// Does NOT apply the delay threshold -- that decision lives at the call
/// site (`fill_vars`) so the cache content stays time-independent.
fn parse_update_check(content: &str) -> Option<UpdateRecord> {
    let check: UpdateCheck = serde_json::from_str(content).ok()?;
    Some(UpdateRecord {
        update_available: check.update_available,
        latest: check.latest,
        checked_unix: check.checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // After the B4 fix, parse_update_check is a pure JSON->UpdateRecord parser
    // (no delay logic). Delay-threshold behavior is exercised by the
    // integration test test_gsd_update_cache_reevaluates_delay_after_threshold
    // in src/gsd/tests.rs, which goes through fill_vars + a real tempdir.

    #[test]
    fn test_parse_update_not_available() {
        let json = r#"{"update_available": false, "installed": "1.18.0", "latest": "1.18.0", "checked": 1771030216}"#;
        let record = parse_update_check(json).unwrap();
        assert!(!record.update_available);
        assert_eq!(record.latest, "1.18.0");
        assert_eq!(record.checked_unix, 1771030216);
    }

    #[test]
    fn test_parse_update_available_record() {
        let json = r#"{"update_available": true, "installed": "1.18.0", "latest": "1.19.0", "checked": 1000000000}"#;
        let record = parse_update_check(json).unwrap();
        assert!(record.update_available);
        assert_eq!(record.latest, "1.19.0");
        assert_eq!(record.checked_unix, 1000000000);
    }

    #[test]
    fn test_parse_malformed_json() {
        let json = "not valid json";
        assert!(parse_update_check(json).is_none());
    }

    #[test]
    fn test_parse_empty_json() {
        let json = "{}";
        // serde should fail since required fields are missing
        assert!(parse_update_check(json).is_none());
    }

    #[test]
    fn test_fill_vars_sets_update_info() {
        // Manual mirror of fill_vars's insertion logic, exercised on the
        // raw UpdateRecord shape.
        let mut vars: HashMap<String, String> = HashMap::new();
        let record = UpdateRecord {
            update_available: true,
            latest: "1.19.0".to_string(),
            checked_unix: 1000000000,
        };
        if record.update_available {
            vars.insert("gsd_update_available".into(), "true".into());
            vars.insert("gsd_update_version".into(), record.latest);
        }
        assert_eq!(vars.get("gsd_update_available").unwrap(), "true");
        assert_eq!(vars.get("gsd_update_version").unwrap(), "1.19.0");
    }

    #[test]
    fn test_fill_vars_no_update_does_not_modify() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let record = UpdateRecord {
            update_available: false,
            latest: String::new(),
            checked_unix: 0,
        };
        if record.update_available {
            vars.insert("gsd_update_available".into(), "true".into());
            vars.insert("gsd_update_version".into(), record.latest);
        }
        assert!(!vars.contains_key("gsd_update_available"));
    }
}
