//! Claude Code todo JSON reader for active task tracking.
//!
//! Scans `~/.claude/todos/` for the most recent non-stale, non-empty todo JSON
//! files to find the currently active task. Prefers `in_progress` tasks over
//! `pending` tasks (locked decision from CONTEXT.md). Uses the `activeForm`
//! field if present, falling back to `content`.
//!
//! # Caching Strategy
//!
//! Instead of caching by directory mtime (which may not update on file content
//! changes on macOS), caches by the mtime of the most recent file. If that
//! file's mtime hasn't changed, return cached data. This avoids re-reading
//! all files while still detecting new writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// A single todo item from Claude Code's todo JSON.
#[derive(Debug, serde::Deserialize)]
struct TodoItem {
    content: String,
    status: String,
    #[serde(default, rename = "activeForm")]
    active_form: Option<String>,
}

/// Cached parse result keyed by (todos_dir, most-recent file mtime).
///
/// Keying by the directory path alongside mtime is required: under test
/// isolation (and at runtime, when HOME changes), the same mtime value can
/// appear for files in different directories, and mtime alone would return
/// stale cached results from the previous directory.
struct CachedParse {
    dir: PathBuf,
    mtime: SystemTime,
    data: TodoData,
}

/// Extracted task information from todo files.
#[derive(Clone)]
struct TodoData {
    task_name: Option<String>,
    task_progress: Option<String>,
}

/// Global cache for todo directory scan results.
static TODOS_CACHE: OnceLock<Mutex<Option<CachedParse>>> = OnceLock::new();

/// Populate GSD task variables from Claude Code todo JSON files.
///
/// Sets the following keys in `vars` when an active task is found:
/// - `gsd_task` -- task name (smart-truncated), from `activeForm` or `content`
/// - `gsd_task_progress` -- e.g., "2/5" (completed/total in the todo list)
///
/// Returns without modifying `vars` if the todos directory is missing, empty,
/// contains only stale files, or no active tasks are found.
pub fn fill_vars(
    home_dir: &Path,
    task_truncation_limit: usize,
    staleness_seconds: u64,
    vars: &mut HashMap<String, String>,
) {
    let data = match find_active_task(home_dir, task_truncation_limit, staleness_seconds) {
        Some(d) => d,
        None => return,
    };

    if let Some(ref name) = data.task_name {
        vars.insert("gsd_task".into(), name.clone());
    }
    if let Some(ref progress) = data.task_progress {
        vars.insert("gsd_task_progress".into(), progress.clone());
    }
}

/// Find the active task from todo JSON files with caching.
///
/// Scans `~/.claude/todos/` for non-stale, non-empty JSON files, sorted by
/// mtime (most recent first). Checks at most 10 files. Returns the first
/// active task found (in_progress preferred over pending).
fn find_active_task(
    home_dir: &Path,
    task_truncation_limit: usize,
    staleness_seconds: u64,
) -> Option<TodoData> {
    let todos_dir = home_dir.join(".claude").join("todos");
    if !todos_dir.is_dir() {
        return None;
    }

    let now = SystemTime::now();
    let staleness_threshold = Duration::from_secs(staleness_seconds);

    // Collect non-empty, non-stale JSON files with their mtimes
    let mut files: Vec<(std::path::PathBuf, SystemTime)> = std::fs::read_dir(&todos_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            // Only process .json files
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                return None;
            }
            let meta = e.metadata().ok()?;
            let mtime = meta.modified().ok()?;
            // Skip empty files (just "[]" = 2 bytes)
            if meta.len() <= 2 {
                return None;
            }
            // Skip stale files
            if now.duration_since(mtime).ok()? > staleness_threshold {
                return None;
            }
            Some((path, mtime))
        })
        .collect();

    if files.is_empty() {
        return None;
    }

    // Sort by mtime descending (most recent first)
    files.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    // Check cache against (todos_dir, most recent file's mtime)
    let most_recent_mtime = files[0].1;
    let cache = TODOS_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(ref cached) = *guard {
        if cached.dir == todos_dir && cached.mtime == most_recent_mtime {
            return Some(cached.data.clone());
        }
    }

    // Cache miss -- scan files
    let data = scan_todo_files(&files, task_truncation_limit);
    *guard = Some(CachedParse {
        dir: todos_dir,
        mtime: most_recent_mtime,
        data: data.clone(),
    });
    Some(data)
}

/// Scan at most 10 todo files for the first active task.
fn scan_todo_files(
    files: &[(std::path::PathBuf, SystemTime)],
    task_truncation_limit: usize,
) -> TodoData {
    for (path, _) in files.iter().take(10) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let todos: Vec<TodoItem> = match serde_json::from_str(&content) {
            Ok(t) => t,
            Err(_) => continue,
        };

        // Priority: in_progress > pending (skip completed-only lists)
        let active = todos
            .iter()
            .find(|t| t.status == "in_progress")
            .or_else(|| todos.iter().find(|t| t.status == "pending"));

        if let Some(task) = active {
            let raw_name = task.active_form.as_deref().unwrap_or(&task.content);

            let name = super::smart_truncate(raw_name, task_truncation_limit);

            // Count progress: completed / total
            let completed = todos.iter().filter(|t| t.status == "completed").count();
            let total = todos.len();
            let progress = format!("{}/{}", completed, total);

            return TodoData {
                task_name: Some(name),
                task_progress: Some(progress),
            };
        }
    }

    TodoData {
        task_name: None,
        task_progress: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_todo_item() {
        let json = r#"[
            {"content": "Implement auth", "status": "completed"},
            {"content": "Write tests", "status": "in_progress", "activeForm": "Writing unit tests"},
            {"content": "Deploy", "status": "pending"}
        ]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        assert_eq!(todos.len(), 3);
        assert_eq!(todos[0].status, "completed");
        assert_eq!(todos[1].active_form.as_deref(), Some("Writing unit tests"));
        assert_eq!(todos[2].active_form, None);
    }

    #[test]
    fn test_parse_todo_item_without_active_form() {
        let json = r#"[{"content": "Task A", "status": "pending"}]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "Task A");
        assert_eq!(todos[0].active_form, None);
    }

    #[test]
    fn test_parse_empty_todo_array() {
        let json = "[]";
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        assert!(todos.is_empty());
    }

    #[test]
    fn test_in_progress_preferred_over_pending() {
        let json = r#"[
            {"content": "Pending task", "status": "pending"},
            {"content": "Active task", "status": "in_progress"}
        ]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        let active = todos
            .iter()
            .find(|t| t.status == "in_progress")
            .or_else(|| todos.iter().find(|t| t.status == "pending"));
        assert_eq!(active.unwrap().content, "Active task");
    }

    #[test]
    fn test_falls_back_to_pending_when_no_in_progress() {
        let json = r#"[
            {"content": "Completed task", "status": "completed"},
            {"content": "Pending task", "status": "pending"}
        ]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        let active = todos
            .iter()
            .find(|t| t.status == "in_progress")
            .or_else(|| todos.iter().find(|t| t.status == "pending"));
        assert_eq!(active.unwrap().content, "Pending task");
    }

    #[test]
    fn test_no_active_when_all_completed() {
        let json = r#"[
            {"content": "Done 1", "status": "completed"},
            {"content": "Done 2", "status": "completed"}
        ]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        let active = todos
            .iter()
            .find(|t| t.status == "in_progress")
            .or_else(|| todos.iter().find(|t| t.status == "pending"));
        assert!(active.is_none());
    }

    #[test]
    fn test_active_form_preferred_over_content() {
        let json = r#"[{"content": "Write tests", "status": "in_progress", "activeForm": "Writing unit tests for auth module"}]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        let task = &todos[0];
        let name = task.active_form.as_deref().unwrap_or(&task.content);
        assert_eq!(name, "Writing unit tests for auth module");
    }

    #[test]
    fn test_content_used_when_no_active_form() {
        let json = r#"[{"content": "Write tests", "status": "in_progress"}]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        let task = &todos[0];
        let name = task.active_form.as_deref().unwrap_or(&task.content);
        assert_eq!(name, "Write tests");
    }

    #[test]
    fn test_progress_counting() {
        let json = r#"[
            {"content": "A", "status": "completed"},
            {"content": "B", "status": "completed"},
            {"content": "C", "status": "in_progress"},
            {"content": "D", "status": "pending"},
            {"content": "E", "status": "pending"}
        ]"#;
        let todos: Vec<TodoItem> = serde_json::from_str(json).unwrap();
        let completed = todos.iter().filter(|t| t.status == "completed").count();
        let total = todos.len();
        assert_eq!(format!("{}/{}", completed, total), "2/5");
    }
}
