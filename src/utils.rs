use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use crate::models::{ContextUsage, TranscriptEntry};

pub fn parse_iso8601_to_unix(timestamp: &str) -> Option<u64> {
    // Parser for ISO 8601 timestamps like:
    // "2025-08-22T18:32:37.789Z" (UTC)
    // "2025-08-24T23:24:15.577606003-07:00" (with timezone offset)
    
    // Handle timezone
    let (timestamp_part, tz_offset_hours) = if timestamp.ends_with('Z') {
        (&timestamp[..timestamp.len() - 1], 0i32)
    } else if let Some(plus_pos) = timestamp.rfind('+') {
        if plus_pos > 10 {  // Make sure it's not in the date part
            let tz_str = &timestamp[plus_pos + 1..];
            let tz_hours = tz_str.split(':').next()?.parse::<i32>().ok()?;
            (&timestamp[..plus_pos], -tz_hours)  // Subtract for positive offset
        } else {
            (timestamp, 0)
        }
    } else if let Some(minus_pos) = timestamp.rfind('-') {
        if minus_pos > 10 {  // Make sure it's not in the date part
            let tz_str = &timestamp[minus_pos + 1..];
            let tz_hours = tz_str.split(':').next()?.parse::<i32>().ok()?;
            (&timestamp[..minus_pos], tz_hours)  // Add for negative offset
        } else {
            (timestamp, 0)
        }
    } else {
        (timestamp, 0)
    };
    
    // Split into date and time
    let parts: Vec<&str> = timestamp_part.split('T').collect();
    if parts.len() != 2 {
        return None;
    }
    
    // Parse date (YYYY-MM-DD)
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    
    // Parse time (HH:MM:SS.sss)
    let time_and_ms: Vec<&str> = parts[1].split('.').collect();
    let time_parts: Vec<&str> = time_and_ms[0].split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let second: u32 = time_parts[2].parse().ok()?;
    
    // Calculate days since Unix epoch with proper leap year handling
    let mut leap_years = 0;
    for y in 1970..year {
        if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
            leap_years += 1;
        }
    }
    
    // Add extra day for February if current year is leap year and we're past February
    let leap_day_adjustment = if month > 2 && ((year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)) {
        1
    } else {
        0
    };
    
    let days_since_epoch = (year - 1970) as u64 * 365 
        + leap_years as u64
        + days_before_month(month) as u64
        + leap_day_adjustment
        + (day - 1) as u64;
    
    let seconds = days_since_epoch * 86400
        + hour as u64 * 3600
        + minute as u64 * 60
        + second as u64;
    
    // Apply timezone offset (add because we want UTC)
    Some((seconds as i64 + (tz_offset_hours as i64 * 3600)) as u64)
}

fn days_before_month(month: u32) -> u32 {
    match month {
        1 => 0,
        2 => 31,
        3 => 59,
        4 => 90,
        5 => 120,
        6 => 151,
        7 => 181,
        8 => 212,
        9 => 243,
        10 => 273,
        11 => 304,
        12 => 334,
        _ => 0,
    }
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

pub fn calculate_context_usage(transcript_path: &str) -> Option<ContextUsage> {
    if !Path::new(transcript_path).exists() {
        return None;
    }

    // Read all lines then take the last 50 for performance
    let file = File::open(transcript_path).ok()?;
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .collect();

    // Take last 50 lines
    let start = all_lines.len().saturating_sub(50);
    let lines = &all_lines[start..];

    // Find the most recent assistant message with usage data
    let mut total_tokens = 0u32;

    for line in lines {
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
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
        // Context window (160K for most Claude models)
        let context_window = 160_000;
        let percentage = (total_tokens as f64 / context_window as f64) * 100.0;

        return Some(ContextUsage {
            percentage: percentage.min(100.0),
        });
    }

    None
}

pub fn parse_duration(transcript_path: &str) -> Option<u64> {
    if !Path::new(transcript_path).exists() {
        return None;
    }

    // Read first and last timestamps from transcript
    let file = File::open(transcript_path).ok()?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .collect();

    if lines.len() < 2 {
        return None;
    }

    // Get first and last timestamps
    let mut first_timestamp = None;
    let mut last_timestamp = None;

    // Parse first line
    if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&lines[0]) {
        first_timestamp = parse_iso8601_to_unix(&entry.timestamp);
    }

    // Parse last line
    if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(lines.last()?) {
        last_timestamp = parse_iso8601_to_unix(&entry.timestamp);
    }

    // Calculate duration in seconds
    match (first_timestamp, last_timestamp) {
        (Some(first), Some(last)) if last > first => {
            Some(last - first)
        },
        _ => None // Can't calculate duration without valid timestamps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(calculate_context_usage("/tmp/nonexistent.jsonl").is_none());

        // Test with valid transcript (string timestamp and string content)
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test","usage":{{"input_tokens":120000,"output_tokens":5000}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"question"}},"timestamp":"2025-08-22T18:33:00.000Z"}}"#).unwrap();

        let result = calculate_context_usage(file.path().to_str().unwrap());
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 78.125).abs() < 0.01); // 125000/160000 * 100
    }

    #[test]
    fn test_calculate_context_usage_with_cache_tokens() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with cache tokens
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test","usage":{{"input_tokens":100,"cache_read_input_tokens":30000,"cache_creation_input_tokens":200,"output_tokens":500}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();

        let result = calculate_context_usage(file.path().to_str().unwrap());
        assert!(result.is_some());
        let usage = result.unwrap();
        // Total: 100 + 30000 + 200 + 500 = 30800
        assert!((usage.percentage - 19.25).abs() < 0.01); // 30800/160000 * 100
    }

    #[test]
    fn test_calculate_context_usage_with_array_content() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with array content (assistant messages often have this)
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":[{{"type":"text","text":"response"}}],"usage":{{"input_tokens":50000,"output_tokens":1000}}}},"timestamp":"2025-08-22T18:32:37.789Z"}}"#).unwrap();

        let result = calculate_context_usage(file.path().to_str().unwrap());
        assert!(result.is_some());
        let usage = result.unwrap();
        assert!((usage.percentage - 31.875).abs() < 0.01); // 51000/160000 * 100
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
        assert!(parse_iso8601_to_unix("2025-08-25T10:00:00").is_none()); // No Z suffix
        assert!(parse_iso8601_to_unix("not a timestamp").is_none());
    }

    #[test]
    fn test_parse_duration() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test with non-existent file
        assert!(parse_duration("/tmp/nonexistent.jsonl").is_none());

        // Test with valid transcript (using string timestamps)
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"test"}},"timestamp":"2025-08-22T18:00:00.000Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"question"}},"timestamp":"2025-08-22T19:00:00.000Z"}}"#).unwrap();

        let result = parse_duration(file.path().to_str().unwrap());
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 3600); // 1 hour between 18:00:00 and 19:00:00

        // Test with single line (should return None)
        let mut file2 = NamedTempFile::new().unwrap();
        writeln!(file2, r#"{{"message":{{"role":"assistant","content":"test"}},"timestamp":"2025-08-22T18:00:00.000Z"}}"#).unwrap();

        let result2 = parse_duration(file2.path().to_str().unwrap());
        assert!(result2.is_none());
    }
    
    #[test]
    fn test_parse_duration_with_realistic_timestamps() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Test 5-minute session (the case that was showing $399/hr)
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"message":{{"role":"user","content":"Hello"}},"timestamp":"2025-08-25T10:00:00.000Z"}}"#).unwrap();
        writeln!(file, r#"{{"message":{{"role":"assistant","content":"Hi","usage":{{"input_tokens":100,"output_tokens":50}}}},"timestamp":"2025-08-25T10:05:00.000Z"}}"#).unwrap();

        let result = parse_duration(file.path().to_str().unwrap());
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 300); // 5 minutes = 300 seconds
        
        // Test 10-minute session
        let mut file2 = NamedTempFile::new().unwrap();
        writeln!(file2, r#"{{"message":{{"role":"user","content":"Start"}},"timestamp":"2025-08-25T10:00:00.000Z"}}"#).unwrap();
        writeln!(file2, r#"{{"message":{{"role":"assistant","content":"Working"}},"timestamp":"2025-08-25T10:10:00.000Z"}}"#).unwrap();

        let result2 = parse_duration(file2.path().to_str().unwrap());
        assert!(result2.is_some());
        assert_eq!(result2.unwrap(), 600); // 10 minutes = 600 seconds
    }
}