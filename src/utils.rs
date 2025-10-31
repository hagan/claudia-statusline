//! Utility functions for the statusline.
//!
//! This module provides various helper functions for path manipulation,
//! time parsing, and context usage calculations.

use crate::common::validate_path_security;
use crate::config;
use crate::error::{Result, StatuslineError};
use crate::models::{ContextUsage, TranscriptEntry};
use chrono::DateTime;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// Sanitizes a string for safe terminal output by removing control characters
/// and ANSI escape sequences. This prevents malicious strings from manipulating
/// terminal state or executing unintended commands.
///
/// # Arguments
///
/// * `input` - The string to sanitize
///
/// # Returns
///
/// A sanitized string safe for terminal output
pub fn sanitize_for_terminal(input: &str) -> String {
    // Remove ANSI escape sequences (e.g., \x1b[31m for colors)
    // Pattern matches: ESC [ ... m where ... is any sequence of digits and semicolons
    let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let mut sanitized = ansi_regex.replace_all(input, "").to_string();

    // Remove control characters (0x00-0x1F and 0x7F-0x9F) except for:
    // - Tab (0x09)
    // - Line feed (0x0A)
    // - Carriage return (0x0D)
    sanitized = sanitized
        .chars()
        .filter(|c| {
            let code = *c as u32;
            // Allow printable ASCII and Unicode, tab, newline, carriage return
            (*c == '\t' || *c == '\n' || *c == '\r')
                || (code >= 0x20 && code != 0x7F && !(0x80..=0x9F).contains(&code))
        })
        .collect();

    sanitized
}

/// Parses an ISO 8601 timestamp to Unix epoch seconds.
///
/// # Arguments
///
/// * `timestamp` - An ISO 8601 formatted timestamp string
///
/// # Returns
///
/// Returns `Some(u64)` with the Unix timestamp, or `None` if parsing fails.
pub fn parse_iso8601_to_unix(timestamp: &str) -> Option<u64> {
    // Use chrono to parse ISO 8601 timestamps
    // First try parsing as RFC3339 (with timezone)
    if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) {
        return Some(dt.timestamp() as u64);
    }

    // If no timezone, try parsing as naive datetime and assume UTC
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc().timestamp() as u64);
    }

    // Try without fractional seconds
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc().timestamp() as u64);
    }

    None
}

pub fn shorten_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    if let Ok(home) = env::var("HOME") {
        if path == home {
            return "~".to_string();
        }
        if path.starts_with(&home) {
            return path.replacen(&home, "~", 1);
        }
    }
    path.to_string()
}

/// Maximum size for transcript files (10MB)
const MAX_TRANSCRIPT_SIZE: u64 = 10 * 1024 * 1024;

/// Determines the context window size for a given model
///
/// Uses intelligent defaults based on model family and version:
/// - Sonnet 3.5+, 4.5+: 200k tokens
/// - Opus 3.5+: 200k tokens
/// - Older models: 160k tokens
/// - Unknown models: Config default (200k)
///
/// Users can override any model in config.toml [context.model_windows]
///
/// # Future Enhancement
///
/// **API-based context window queries**: In a future version, we could query
/// the Anthropic API or a maintained database to get accurate, up-to-date
/// context window sizes for all models. This would eliminate the need for
/// hardcoded defaults and manual config updates.
///
/// Potential approaches:
/// - Query `/v1/models` endpoint (if available) for model metadata
/// - Maintain a remote JSON file with current context window sizes
/// - Use a caching strategy to avoid repeated API calls
/// - Fall back to intelligent defaults if query fails
///
/// Trade-offs to consider:
/// - API latency (would need caching to maintain ~5ms execution time)
/// - Offline usage (must have fallback)
/// - API availability and authentication requirements
///
/// # Arguments
///
/// * `model_name` - Optional model display name from Claude Code
/// * `config` - Configuration containing window_size defaults and overrides
///
/// # Returns
///
/// Context window size in tokens
fn get_context_window_for_model(model_name: Option<&str>, config: &config::Config) -> usize {
    if let Some(model) = model_name {
        // First check if user has configured a specific override
        if let Some(&custom_size) = config.context.model_windows.get(model) {
            return custom_size;
        }

        // Smart defaults based on model family and version
        use crate::models::ModelType;
        let model_type = ModelType::from_name(model);

        match model_type {
            ModelType::Model { family, version } => {
                // Parse version for comparison (handle formats like "3.5", "4.5", "3", etc.)
                let version_number = version
                    .split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);

                let minor_version = version
                    .split('.')
                    .nth(1)
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);

                match family.as_str() {
                    "Sonnet" => {
                        // Sonnet 3.5+, 4.x+: 200k tokens
                        if version_number >= 4 || (version_number == 3 && minor_version >= 5) {
                            200_000
                        } else {
                            160_000
                        }
                    }
                    "Opus" => {
                        // Opus 3.5+: 200k tokens
                        if version_number >= 4 || (version_number == 3 && minor_version >= 5) {
                            200_000
                        } else {
                            160_000
                        }
                    }
                    "Haiku" => {
                        // Haiku models typically have smaller windows
                        // Future versions might increase, but default to config
                        config.context.window_size
                    }
                    _ => config.context.window_size,
                }
            }
            ModelType::Unknown => config.context.window_size,
        }
    } else {
        // No model name provided, use config default
        config.context.window_size
    }
}

/// Validates that a path is a valid transcript file
fn validate_transcript_file(path: &str) -> Result<PathBuf> {
    // Use common validation first
    let canonical_path = validate_path_security(path)?;

    // Ensure the path is a file (not a directory)
    if !canonical_path.is_file() {
        return Err(StatuslineError::invalid_path(format!(
            "Path is not a file: {}",
            path
        )));
    }

    // Check file extension (case-insensitive)
    if let Some(ext) = canonical_path.extension() {
        // Case-insensitive check for jsonl extension
        if !ext
            .to_str()
            .map(|s| s.eq_ignore_ascii_case("jsonl"))
            .unwrap_or(false)
        {
            return Err(StatuslineError::invalid_path(
                "Only .jsonl files are allowed for transcripts",
            ));
        }
    } else {
        return Err(StatuslineError::invalid_path(
            "File must have .jsonl extension",
        ));
    }

    // Check file size to prevent DoS
    if let Ok(metadata) = canonical_path.metadata() {
        if metadata.len() > MAX_TRANSCRIPT_SIZE {
            return Err(StatuslineError::invalid_path(format!(
                "Transcript file too large (max {}MB)",
                MAX_TRANSCRIPT_SIZE / 1024 / 1024
            )));
        }
    }

    Ok(canonical_path)
}

pub fn calculate_context_usage(
    transcript_path: &str,
    model_name: Option<&str>,
) -> Option<ContextUsage> {
    // Validate and canonicalize the file path
    let safe_path = validate_transcript_file(transcript_path).ok()?;

    // Efficiently read only the last 50 lines using a circular buffer
    let file = File::open(&safe_path).ok()?;
    let reader = BufReader::new(file);

    // Use a circular buffer to keep only the configured number of lines in memory
    let config = config::get_config();
    let buffer_size = config.transcript.buffer_lines;
    let mut circular_buffer = std::collections::VecDeque::with_capacity(buffer_size);
    for line in reader.lines().map_while(|l| l.ok()) {
        if circular_buffer.len() == buffer_size {
            circular_buffer.pop_front();
        }
        circular_buffer.push_back(line);
    }

    let lines: Vec<String> = circular_buffer.into_iter().collect();

    // Find the most recent assistant message with usage data
    let mut total_tokens = 0u32;

    for line in lines {
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
            if entry.message.role == "assistant" {
                if let Some(usage) = entry.message.usage {
                    // Add up all token types
                    let input = usage.input_tokens.unwrap_or(0);
                    let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
                    let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
                    let output = usage.output_tokens.unwrap_or(0);
                    let current_total = input + cache_read + cache_creation + output;
                    total_tokens = total_tokens.max(current_total);
                }
            }
        }
    }

    if total_tokens > 0 {
        // Get context window size based on model (intelligent detection + config overrides)
        let context_window = get_context_window_for_model(model_name, config);
        let percentage = (total_tokens as f64 / context_window as f64) * 100.0;

        return Some(ContextUsage {
            percentage: percentage.min(100.0),
        });
    }

    None
}

pub fn parse_duration(transcript_path: &str) -> Option<u64> {
    // Validate and canonicalize the file path
    let safe_path = validate_transcript_file(transcript_path).ok()?;

    // Read first and last timestamps from transcript efficiently
    let file = File::open(&safe_path).ok()?;
    let reader = BufReader::new(file);

    let mut first_timestamp = None;
    let mut last_timestamp = None;
    let mut first_line = None;

    // Read lines one at a time, keeping track of first and updating last
    for line in reader.lines().map_while(|l| l.ok()) {
        if first_line.is_none() {
            first_line = Some(line.clone());
            // Parse first line
            if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
                first_timestamp = parse_iso8601_to_unix(&entry.timestamp);
            }
        }

        // Always try to parse the current line as the last one
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
            last_timestamp = parse_iso8601_to_unix(&entry.timestamp);
        }
    }

    if first_timestamp.is_none() || last_timestamp.is_none() {
        return None;
    }

    // Calculate duration in seconds
    match (first_timestamp, last_timestamp) {
        (Some(first), Some(last)) if last > first => Some(last - first),
        _ => None, // Can't calculate duration without valid timestamps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_validate_transcript_file_security() {
        // Test null byte injection
        assert!(validate_transcript_file("/tmp/test\0.jsonl").is_err());
        assert!(validate_transcript_file("/tmp\0/test.jsonl").is_err());

        // Test non-existent files
        assert!(validate_transcript_file("/definitely/does/not/exist.jsonl").is_err());

        // Test directory instead of file
        let temp_dir = std::env::temp_dir();
        assert!(validate_transcript_file(temp_dir.to_str().unwrap()).is_err());

        // Test non-jsonl file
        let temp_file = std::env::temp_dir().join("test.txt");
        fs::write(&temp_file, "test").ok();
        assert!(validate_transcript_file(temp_file.to_str().unwrap()).is_err());
        fs::remove_file(temp_file).ok();

        // Test case-insensitive extension (should accept .JSONL, .JsonL, etc.)
        use tempfile::NamedTempFile;
        let temp_file = NamedTempFile::new().unwrap();
        let path_upper = temp_file.path().with_extension("JSONL");
        fs::write(&path_upper, "test").ok();
        assert!(validate_transcript_file(path_upper.to_str().unwrap()).is_ok());
        fs::remove_file(path_upper).ok();
    }

    #[test]
    fn test_malicious_transcript_paths() {
        // Directory traversal attempts
        assert!(calculate_context_usage("../../../etc/passwd", None).is_none());
        assert!(parse_duration("../../../../../../etc/shadow").is_none());

        // Command injection attempts
        assert!(calculate_context_usage("/tmp/test.jsonl; rm -rf /", None).is_none());
        assert!(parse_duration("/tmp/test.jsonl && echo hacked").is_none());
        assert!(calculate_context_usage("/tmp/test.jsonl | cat /etc/passwd", None).is_none());
        assert!(parse_duration("/tmp/test.jsonl`whoami`").is_none());
        assert!(calculate_context_usage("/tmp/test.jsonl$(whoami)", None).is_none());

        // Null byte injection
        assert!(calculate_context_usage("/tmp/test\0.jsonl", None).is_none());
        assert!(parse_duration("/tmp\0/test.jsonl").is_none());

        // Special characters that might cause issues
        assert!(calculate_context_usage("/tmp/test\n.jsonl", None).is_none());
        assert!(parse_duration("/tmp/test\r.jsonl").is_none());
    }

    #[test]
    fn test_sanitize_for_terminal() {
        // Test removal of ANSI escape codes
        assert_eq!(sanitize_for_terminal("\x1b[31mRed Text\x1b[0m"), "Red Text");
        assert_eq!(
            sanitize_for_terminal("\x1b[1;32mBold Green\x1b[0m"),
            "Bold Green"
        );

        // Test removal of control characters
        assert_eq!(
            sanitize_for_terminal("Hello\x00World"), // Null byte
            "HelloWorld"
        );
        assert_eq!(
            sanitize_for_terminal("Text\x1bEscape"), // Escape character alone
            "TextEscape"
        );
        assert_eq!(
            sanitize_for_terminal("Bell\x07Sound"), // Bell character
            "BellSound"
        );

        // Test preservation of allowed control characters
        assert_eq!(
            sanitize_for_terminal("Line1\nLine2\tTabbed"),
            "Line1\nLine2\tTabbed"
        );
        assert_eq!(
            sanitize_for_terminal("Windows\r\nLineEnd"),
            "Windows\r\nLineEnd"
        );

        // Test complex mixed input
        assert_eq!(
            sanitize_for_terminal("\x1b[31mDanger\x00\x07\x1b[0m\nSafe"),
            "Danger\nSafe"
        );

        // Test Unicode characters are preserved
        assert_eq!(
            sanitize_for_terminal("Unicode: 🚀 日本語"),
            "Unicode: 🚀 日本語"
        );

        // Test removal of non-printable Unicode control characters
        assert_eq!(
            sanitize_for_terminal("Text\u{0080}\u{009F}More"), // C1 control characters
            "TextMore"
        );
    }

    #[test]
    fn test_shorten_path() {
        let home = env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());

        // Test home directory substitution
        let path = format!("{}/projects/test", home);
        assert_eq!(shorten_path(&path), "~/projects/test");

        // Test path that doesn't start with home
        assert_eq!(shorten_path("/usr/local/bin"), "/usr/local/bin");

        // Test exact home directory
        assert_eq!(shorten_path(&home), "~");

        // Test empty path
        assert_eq!(shorten_path(""), "");
    }

    #[test]
    fn test_context_usage_levels() {
        // Test various percentage levels
        let low = ContextUsage { percentage: 10.0 };
        let medium = ContextUsage { percentage: 55.0 };
        let high = ContextUsage { percentage: 75.0 };
        let critical = ContextUsage { percentage: 95.0 };

        assert_eq!(low.percentage, 10.0);
        assert_eq!(medium.percentage, 55.0);
        assert_eq!(high.percentage, 75.0);
        assert_eq!(critical.percentage, 95.0);
    }

    #[test]
    fn test_calculate_context_usage() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with non-existent file
        assert!(calculate_context_usage("/tmp/nonexistent.jsonl", None).is_none());

        // Test with valid transcript (string timestamp and string content)
        let mut file = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test","usage":{{"input_tokens":120000,"output_tokens":5000}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"question"}},"timestamp":"2025-08-22T18:33:00.000Z"}}"#).unwrap();

        let result = calculate_context_usage(file.path().to_str().unwrap(), None);
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 62.5).abs() < 0.01); // 125000/200000 * 100 (updated for 200k default)
    }

    #[test]
    fn test_calculate_context_usage_with_cache_tokens() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with cache tokens
        let mut file = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test","usage":{{"input_tokens":100,"cache_read_input_tokens":30000,"cache_creation_input_tokens":200,"output_tokens":500}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();

        let result = calculate_context_usage(file.path().to_str().unwrap(), None);
        assert!(result.is_some());
        let usage = result.unwrap();
        // Total: 100 + 30000 + 200 + 500 = 30800
        assert!((usage.percentage - 15.4).abs() < 0.01); // 30800/200000 * 100 (updated for 200k default)
    }

    #[test]
    fn test_calculate_context_usage_with_array_content() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with array content (assistant messages often have this)
        let mut file = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":[{{"type":"text","text":"response"}}],"usage":{{"input_tokens":50000,"output_tokens":1000}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();

        let result = calculate_context_usage(file.path().to_str().unwrap(), None);
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 25.5).abs() < 0.01); // 51000/200000 * 100 (updated for 200k default)
    }

    #[test]
    fn test_parse_iso8601_to_unix() {
        // Test valid ISO 8601 timestamps
        assert_eq!(
            parse_iso8601_to_unix("2025-08-25T10:00:00.000Z").unwrap(),
            parse_iso8601_to_unix("2025-08-25T10:00:00.000Z").unwrap()
        );

        // Test that timestamps 5 minutes apart give 300 seconds difference
        let t1 = parse_iso8601_to_unix("2025-08-25T10:00:00.000Z").unwrap();
        let t2 = parse_iso8601_to_unix("2025-08-25T10:05:00.000Z").unwrap();
        assert_eq!(t2 - t1, 300);

        // Test that timestamps 1 hour apart give 3600 seconds difference
        let t3 = parse_iso8601_to_unix("2025-08-25T10:00:00.000Z").unwrap();
        let t4 = parse_iso8601_to_unix("2025-08-25T11:00:00.000Z").unwrap();
        assert_eq!(t4 - t3, 3600);

        // Test with milliseconds
        assert!(parse_iso8601_to_unix("2025-08-25T10:00:00.123Z").is_some());

        // Test invalid formats
        assert!(parse_iso8601_to_unix("2025-08-25 10:00:00").is_none()); // No T separator
        assert!(parse_iso8601_to_unix("2025-08-25T10:00:00").is_some()); // No Z suffix - should still parse
        assert!(parse_iso8601_to_unix("not a timestamp").is_none());
    }

    #[test]
    fn test_parse_duration() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with non-existent file
        assert!(parse_duration("/tmp/nonexistent.jsonl").is_none());

        // Test with valid transcript (using string timestamps)
        let mut file = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test"}},"timestamp":"2025-08-22T18:00:00.000Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"question"}},"timestamp":"2025-08-22T19:00:00.000Z"}}"#).unwrap();

        let result = parse_duration(file.path().to_str().unwrap());
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 3600); // 1 hour between 18:00:00 and 19:00:00

        // Test with single line (should return None)
        let mut file2 = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file2, r#"{{"message":{{"role":"assistant","content":"test"}},"timestamp":"2025-08-22T18:00:00.000Z"}}"#).unwrap();

        let result2 = parse_duration(file2.path().to_str().unwrap());
        assert!(result2.is_none());
    }

    #[test]
    fn test_parse_duration_with_realistic_timestamps() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test 5-minute session (the case that was showing $399/hr)
        let mut file = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"Hello"}},"timestamp":"2025-08-25T10:00:00.000Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"Hi","usage":{{"input_tokens":100,"output_tokens":50}}}},"timestamp":"2025-08-25T10:05:00.000Z"}}"#).unwrap();

        let result = parse_duration(file.path().to_str().unwrap());
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 300); // 5 minutes = 300 seconds

        // Test 10-minute session
        let mut file2 = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file2, r#"{{"message":{{"role":"user","content":"Start"}},"timestamp":"2025-08-25T10:00:00.000Z"}}"#).unwrap();
        writeln!(file2, r#"{{"message":{{"role":"assistant","content":"Working"}},"timestamp":"2025-08-25T10:10:00.000Z"}}"#).unwrap();

        let result2 = parse_duration(file2.path().to_str().unwrap());
        assert!(result2.is_some());
        assert_eq!(result2.unwrap(), 600); // 10 minutes = 600 seconds
    }

    #[test]
    fn test_model_based_context_window() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a test file with 100k tokens
        let mut file = NamedTempFile::with_suffix(".jsonl").unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test","usage":{{"input_tokens":100000,"output_tokens":0}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();

        // Test Sonnet 4.5 (should use 200k window)
        let result =
            calculate_context_usage(file.path().to_str().unwrap(), Some("Claude Sonnet 4.5"));
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 50.0).abs() < 0.01); // 100000/200000 * 100 = 50%

        // Test Sonnet 3.5 (should use 200k window)
        let result =
            calculate_context_usage(file.path().to_str().unwrap(), Some("Claude 3.5 Sonnet"));
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 50.0).abs() < 0.01); // 100000/200000 * 100 = 50%

        // Test Opus 3.5 (should use 200k window)
        let result =
            calculate_context_usage(file.path().to_str().unwrap(), Some("Claude 3.5 Opus"));
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 50.0).abs() < 0.01); // 100000/200000 * 100 = 50%

        // Test unknown model (should use default 200k window)
        let result = calculate_context_usage(file.path().to_str().unwrap(), None);
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 50.0).abs() < 0.01); // 100000/200000 * 100 = 50%
    }
}
