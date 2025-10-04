//! Data models for the Claudia Statusline.
//!
//! This module defines the core data structures used throughout the application,
//! including the input format from Claude Code and various status representations.

use serde::Deserialize;

/// Main input structure from Claude Code.
///
/// This structure represents the JSON input received from stdin,
/// containing workspace information, model details, costs, and other metadata.
#[derive(Debug, Default, Deserialize)]
pub struct StatuslineInput {
    /// Workspace information including current directory
    pub workspace: Option<Workspace>,
    /// Model information including display name
    pub model: Option<Model>,
    /// Unique session identifier
    pub session_id: Option<String>,
    /// Path to the transcript file
    #[serde(alias = "transcript_path")]
    pub transcript: Option<String>,
    /// Cost and metrics information
    pub cost: Option<Cost>,
}

/// Workspace information from Claude Code.
///
/// Contains the current working directory path.
#[derive(Debug, Deserialize)]
pub struct Workspace {
    /// Current working directory path
    pub current_dir: Option<String>,
}

/// Model information from Claude Code.
///
/// Contains the display name of the current Claude model being used.
#[derive(Debug, Deserialize)]
pub struct Model {
    /// Display name of the Claude model (e.g., "Claude 3.5 Sonnet")
    pub display_name: Option<String>,
}

/// Cost and metrics information.
///
/// Tracks the total cost in USD and code change metrics for the current session.
#[derive(Debug, Deserialize)]
pub struct Cost {
    /// Total cost in USD for the session
    pub total_cost_usd: Option<f64>,
    /// Total lines of code added
    pub total_lines_added: Option<u64>,
    /// Total lines of code removed
    pub total_lines_removed: Option<u64>,
}

/// Claude model type enumeration
#[derive(Debug, PartialEq)]
pub enum ModelType {
    /// Claude 3 Opus model
    Opus,
    /// Claude 3.5 Sonnet model
    Sonnet35,
    /// Claude 4.5 Sonnet model
    Sonnet45,
    /// Claude 3 Haiku model
    Haiku,
    /// Unknown or unrecognized model
    Unknown,
}

impl ModelType {
    pub fn from_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.contains("opus") {
            ModelType::Opus
        } else if lower.contains("sonnet") {
            // Check for version number to differentiate between Sonnet versions
            if lower.contains("4.5") || lower.contains("4-5") || lower.contains("sonnet-4") {
                ModelType::Sonnet45
            } else {
                // Default to 3.5 for backward compatibility
                ModelType::Sonnet35
            }
        } else if lower.contains("haiku") {
            ModelType::Haiku
        } else {
            ModelType::Unknown
        }
    }

    /// Returns the abbreviated display name for the model
    pub fn abbreviation(&self) -> &str {
        match self {
            ModelType::Opus => "Opus",
            ModelType::Sonnet35 => "S3.5",
            ModelType::Sonnet45 => "S4.5",
            ModelType::Haiku => "Haiku",
            ModelType::Unknown => "Claude",
        }
    }
}

/// Entry in the Claude transcript file (JSONL format)
#[derive(Debug, Deserialize)]
pub struct TranscriptEntry {
    /// The message content and metadata
    pub message: TranscriptMessage,
    /// ISO 8601 formatted timestamp
    pub timestamp: String,
}

/// Message within a transcript entry
#[derive(Debug, Deserialize)]
pub struct TranscriptMessage {
    /// Role of the message sender (user, assistant, etc.)
    pub role: String,
    /// Message content (can be string or array)
    #[serde(default)]
    #[allow(dead_code)]
    pub content: Option<serde_json::Value>,
    /// Token usage information
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Token usage information from Claude
#[derive(Debug, Deserialize)]
pub struct Usage {
    /// Number of input tokens
    pub input_tokens: Option<u32>,
    /// Number of output tokens generated
    pub output_tokens: Option<u32>,
    /// Number of tokens read from cache
    pub cache_read_input_tokens: Option<u32>,
    /// Number of tokens used to create cache
    pub cache_creation_input_tokens: Option<u32>,
}

/// Context window usage information
#[derive(Debug)]
pub struct ContextUsage {
    /// Percentage of context window used (0-100)
    pub percentage: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_json() {
        let json = "{}";
        let result: Result<StatuslineInput, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let input = result.unwrap();
        assert!(input.workspace.is_none());
        assert!(input.model.is_none());
        assert!(input.cost.is_none());
    }

    #[test]
    fn test_parse_complete_json() {
        let json = r#"{
            "workspace": {"current_dir": "/home/user"},
            "model": {"display_name": "Claude Sonnet"},
            "session_id": "abc123",
            "cost": {
                "total_cost_usd": 2.50,
                "total_lines_added": 200,
                "total_lines_removed": 100
            }
        }"#;
        let result: Result<StatuslineInput, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let input = result.unwrap();
        assert_eq!(input.workspace.unwrap().current_dir.unwrap(), "/home/user");
        assert_eq!(input.model.unwrap().display_name.unwrap(), "Claude Sonnet");
        assert_eq!(input.session_id.unwrap(), "abc123");
        assert_eq!(input.cost.unwrap().total_cost_usd.unwrap(), 2.50);
    }

    #[test]
    fn test_model_type_detection() {
        assert_eq!(ModelType::from_name("Claude Opus"), ModelType::Opus);
        assert_eq!(
            ModelType::from_name("claude-3-opus-20240229"),
            ModelType::Opus
        );
        assert_eq!(ModelType::from_name("Claude 3.5 Sonnet"), ModelType::Sonnet35);
        assert_eq!(ModelType::from_name("Claude Sonnet 4.5"), ModelType::Sonnet45);
        assert_eq!(ModelType::from_name("Claude 4.5 Sonnet"), ModelType::Sonnet45);
        assert_eq!(ModelType::from_name("claude-sonnet-4-5"), ModelType::Sonnet45);
        assert_eq!(ModelType::from_name("Unknown Model"), ModelType::Unknown);
    }

    #[test]
    fn test_model_type_display() {
        assert_eq!(ModelType::Opus.abbreviation(), "Opus");
        assert_eq!(ModelType::Sonnet35.abbreviation(), "S3.5");
        assert_eq!(ModelType::Sonnet45.abbreviation(), "S4.5");
        assert_eq!(ModelType::Haiku.abbreviation(), "Haiku");
        assert_eq!(ModelType::Unknown.abbreviation(), "Claude");
    }

    #[test]
    fn test_transcript_field_alias() {
        // Test that both 'transcript' and 'transcript_path' work
        let json_with_transcript = r#"{
            "workspace": {"current_dir": "/home/user"},
            "transcript": "/path/to/transcript.jsonl"
        }"#;
        let result: Result<StatuslineInput, _> = serde_json::from_str(json_with_transcript);
        assert!(result.is_ok());
        let input = result.unwrap();
        assert_eq!(input.transcript.unwrap(), "/path/to/transcript.jsonl");

        // Test with transcript_path (alias)
        let json_with_transcript_path = r#"{
            "workspace": {"current_dir": "/home/user"},
            "transcript_path": "/path/to/transcript2.jsonl"
        }"#;
        let result2: Result<StatuslineInput, _> = serde_json::from_str(json_with_transcript_path);
        assert!(result2.is_ok());
        let input2 = result2.unwrap();
        assert_eq!(input2.transcript.unwrap(), "/path/to/transcript2.jsonl");
    }

    #[test]
    fn test_transcript_message_content_types() {
        // Test with string content
        let json_string_content = r#"{
            "role": "user",
            "content": "Hello, world!",
            "usage": null
        }"#;
        let result: Result<TranscriptMessage, _> = serde_json::from_str(json_string_content);
        assert!(result.is_ok());
        let msg = result.unwrap();
        assert_eq!(msg.role, "user");
        assert!(msg.content.is_some());

        // Test with array content
        let json_array_content = r#"{
            "role": "assistant",
            "content": [{"type": "text", "text": "Response"}],
            "usage": {"input_tokens": 100, "output_tokens": 50}
        }"#;
        let result2: Result<TranscriptMessage, _> = serde_json::from_str(json_array_content);
        assert!(result2.is_ok());
        let msg2 = result2.unwrap();
        assert_eq!(msg2.role, "assistant");
        assert!(msg2.content.is_some());
        assert!(msg2.usage.is_some());
    }

    #[test]
    fn test_usage_with_cache_tokens() {
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 30000,
            "cache_creation_input_tokens": 200
        }"#;
        let result: Result<Usage, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let usage = result.unwrap();
        assert_eq!(usage.input_tokens.unwrap(), 100);
        assert_eq!(usage.output_tokens.unwrap(), 50);
        assert_eq!(usage.cache_read_input_tokens.unwrap(), 30000);
        assert_eq!(usage.cache_creation_input_tokens.unwrap(), 200);
    }

    #[test]
    fn test_transcript_entry_with_string_timestamp() {
        let json = r#"{
            "message": {
                "role": "assistant",
                "content": "Hello",
                "usage": {"input_tokens": 100, "output_tokens": 50}
            },
            "timestamp": "2025-08-22T18:32:37.789Z"
        }"#;
        let result: Result<TranscriptEntry, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert_eq!(entry.message.role, "assistant");
        assert_eq!(entry.timestamp, "2025-08-22T18:32:37.789Z");
    }

    #[test]
    fn test_statusline_input_with_empty_cost() {
        // Test that empty cost object is handled correctly
        let json = r#"{
            "workspace": {"current_dir": "/test"},
            "session_id": "test-session",
            "cost": {}
        }"#;
        let result: Result<StatuslineInput, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let input = result.unwrap();
        assert_eq!(input.session_id.unwrap(), "test-session");
        assert!(input.cost.is_some());
        let cost = input.cost.unwrap();
        assert!(cost.total_cost_usd.is_none());
        assert!(cost.total_lines_added.is_none());
        assert!(cost.total_lines_removed.is_none());
    }
}
