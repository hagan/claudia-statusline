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
struct CachedParse {
    path: PathBuf,
    mtime: SystemTime,
    data: UpdateData,
}

/// Extracted update information.
#[derive(Clone)]
struct UpdateData {
    update_available: bool,
    latest_version: String,
}

/// Global cache for update check parse results.
static UPDATE_CACHE: OnceLock<Mutex<Option<CachedParse>>> = OnceLock::new();

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

    let data = match read_with_cache(&path, delay_seconds) {
        Some(d) => d,
        None => return,
    };

    if data.update_available {
        vars.insert("gsd_update_available".into(), "true".into());
        vars.insert("gsd_update_version".into(), data.latest_version);
    }
}

/// Read update check file with (path, mtime)-based cache invalidation.
///
/// Checks file mtime (~1us) before deciding whether to re-parse. Returns
/// cached data only when both path and mtime match the cached entry.
fn read_with_cache(path: &Path, delay_seconds: u64) -> Option<UpdateData> {
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
    let data = parse_update_check(&content, delay_seconds)?;
    *guard = Some(CachedParse {
        path: path.to_path_buf(),
        mtime: current_mtime,
        data: data.clone(),
    });
    Some(data)
}

/// Parse update check JSON and apply delay threshold.
///
/// Returns `None` if the JSON is malformed. Returns `UpdateData` with
/// `update_available = false` if the update flag is false or the delay
/// threshold hasn't been met.
fn parse_update_check(content: &str, delay_seconds: u64) -> Option<UpdateData> {
    let check: UpdateCheck = serde_json::from_str(content).ok()?;

    if !check.update_available {
        return Some(UpdateData {
            update_available: false,
            latest_version: String::new(),
        });
    }

    // Check time threshold: enough time must have passed since the check
    let now_unix = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    if now_unix.saturating_sub(check.checked) < delay_seconds {
        // Too soon -- might be downloading
        return Some(UpdateData {
            update_available: false,
            latest_version: String::new(),
        });
    }

    Some(UpdateData {
        update_available: true,
        latest_version: check.latest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_update_not_available() {
        let json = r#"{"update_available": false, "installed": "1.18.0", "latest": "1.18.0", "checked": 1771030216}"#;
        let data = parse_update_check(json, 300).unwrap();
        assert!(!data.update_available);
        assert!(data.latest_version.is_empty());
    }

    #[test]
    fn test_parse_update_available_after_delay() {
        // Use a checked timestamp far in the past so delay is always met
        let json = r#"{"update_available": true, "installed": "1.18.0", "latest": "1.19.0", "checked": 1000000000}"#;
        let data = parse_update_check(json, 300).unwrap();
        assert!(data.update_available);
        assert_eq!(data.latest_version, "1.19.0");
    }

    #[test]
    fn test_parse_update_available_within_delay() {
        // Use a checked timestamp in the future so delay is never met
        let json = r#"{"update_available": true, "installed": "1.18.0", "latest": "1.19.0", "checked": 9999999999}"#;
        let data = parse_update_check(json, 300).unwrap();
        assert!(!data.update_available);
    }

    #[test]
    fn test_parse_malformed_json() {
        let json = "not valid json";
        let data = parse_update_check(json, 300);
        assert!(data.is_none());
    }

    #[test]
    fn test_parse_empty_json() {
        let json = "{}";
        // serde should fail since required fields are missing
        let data = parse_update_check(json, 300);
        assert!(data.is_none());
    }

    #[test]
    fn test_fill_vars_sets_update_info() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let data = UpdateData {
            update_available: true,
            latest_version: "1.19.0".to_string(),
        };
        if data.update_available {
            vars.insert("gsd_update_available".into(), "true".into());
            vars.insert("gsd_update_version".into(), data.latest_version);
        }
        assert_eq!(vars.get("gsd_update_available").unwrap(), "true");
        assert_eq!(vars.get("gsd_update_version").unwrap(), "1.19.0");
    }

    #[test]
    fn test_fill_vars_no_update_does_not_modify() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let data = UpdateData {
            update_available: false,
            latest_version: String::new(),
        };
        if data.update_available {
            vars.insert("gsd_update_available".into(), "true".into());
            vars.insert("gsd_update_version".into(), data.latest_version);
        }
        assert!(!vars.contains_key("gsd_update_available"));
    }

    #[test]
    fn test_zero_delay_shows_immediately() {
        let json = r#"{"update_available": true, "installed": "1.18.0", "latest": "1.19.0", "checked": 9999999999}"#;
        let data = parse_update_check(json, 0).unwrap();
        // With delay of 0, even future timestamps should show
        assert!(data.update_available);
    }
}
