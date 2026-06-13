//! Generic (path, mtime)-keyed cache for GSD file readers.
//!
//! The roadmap, state, and update readers all share the same caching shape:
//! check a file's mtime (~1us), reuse the cached parse when both path and mtime
//! match, otherwise re-read and re-parse. This module factors that logic into a
//! single generic helper so the three readers carry only their concrete data
//! type and parse step.
//!
//! Keyed by (path, mtime): mtime alone is insufficient because filesystems with
//! coarse mtime resolution (e.g. Linux ext4 at 1s) can produce identical mtimes
//! for different files written close in time, causing cache collisions across
//! distinct planning/home directories.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Cached parse result to avoid re-reading unchanged files.
///
/// Generic over the cached data type `T`. The cache value must be the
/// time-independent file-derived data (no wall-clock-dependent decisions), so
/// that callers can recompute any time-sensitive logic on every read.
pub(crate) struct CachedParse<T> {
    path: PathBuf,
    mtime: SystemTime,
    data: T,
}

/// Read a file with (path, mtime)-based cache invalidation.
///
/// Checks the file mtime before deciding whether to re-parse. Returns cached
/// data only when both path and mtime match the cached entry. On a miss, reads
/// the file and applies `parse`; the parse closure returns `Option<T>` to cover
/// both infallible parsers (wrap in `Some(...)`) and fallible ones.
pub(crate) fn read_with_cache<T, F>(
    cache: &OnceLock<Mutex<Option<CachedParse<T>>>>,
    path: &Path,
    parse: F,
) -> Option<T>
where
    T: Clone,
    F: FnOnce(&str) -> Option<T>,
{
    let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;

    let cache = cache.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(ref cached) = *guard {
        if cached.path == path && cached.mtime == current_mtime {
            return Some(cached.data.clone());
        }
    }

    // Path or mtime changed -- re-parse
    let content = std::fs::read_to_string(path).ok()?;
    let data = parse(&content)?;
    *guard = Some(CachedParse {
        path: path.to_path_buf(),
        mtime: current_mtime,
        data: data.clone(),
    });
    Some(data)
}
