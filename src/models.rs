//! Data models for the Claudia Statusline.
//!
//! This module defines the core data structures used throughout the application,
//! including the input format from Claude Code and various status representations.

use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

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

/// Token usage breakdown from transcript.
///
/// Contains detailed token counts for cost analysis and cache efficiency tracking.
#[derive(Debug, Clone, Default)]
pub struct TokenBreakdown {
    /// Input tokens (excluding cache)
    pub input_tokens: u32,
    /// Output tokens generated
    pub output_tokens: u32,
    /// Cache read tokens (cache hits - saves money)
    pub cache_read_tokens: u32,
    /// Cache creation tokens (initial cache write cost)
    pub cache_creation_tokens: u32,
}

impl TokenBreakdown {
    /// Returns the total token count (sum of all token types)
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_creation_tokens
    }
}

/// Claude model type enumeration
#[derive(Debug, PartialEq)]
pub enum ModelType {
    /// Recognized Claude model with extracted name and version
    Model { family: String, version: String },
    /// Unknown or unrecognized model
    Unknown,
}

impl ModelType {
    pub fn from_name(name: &str) -> Self {
        let lower = name.to_lowercase();

        // Extract model family
        let family = if lower.contains("opus") {
            "Opus"
        } else if lower.contains("sonnet") {
            "Sonnet"
        } else if lower.contains("haiku") {
            "Haiku"
        } else {
            return ModelType::Unknown;
        };

        // Extract version number dynamically
        let version = Self::extract_version(&lower).unwrap_or_default();

        ModelType::Model {
            family: family.to_string(),
            version,
        }
    }

    /// Extracts version number from model name
    /// Handles patterns like "3.5", "4.5", "3-5", "4-5", "4", etc.
    fn extract_version(name: &str) -> Option<String> {
        static VERSION_REGEX: OnceLock<Regex> = OnceLock::new();

        let regex = VERSION_REGEX.get_or_init(|| Regex::new(r"\d+(?:[.\-]\d+)?").unwrap());

        regex.find_iter(name).find_map(|mat| {
            let candidate = mat.as_str();

            // Skip build identifiers or dates (e.g., 20240229)
            if candidate.len() > 5 {
                return None;
            }

            let normalized = candidate.replace('-', ".");
            let parts: Vec<&str> = normalized.split('.').collect();

            if parts.iter().all(|part| !part.is_empty() && part.len() <= 2) {
                Some(normalized)
            } else {
                None
            }
        })
    }

    /// Returns the abbreviated display name for the model
    /// Examples: "Opus" → "Opus", "Sonnet 3.5" → "S3.5", "Haiku 4.5" → "H4.5"
    pub fn abbreviation(&self) -> String {
        match self {
            ModelType::Model { family, version } => {
                match family.as_str() {
                    "Opus" => {
                        // Opus is already short, just show "Opus"
                        "Opus".to_string()
                    }
                    "Sonnet" => {
                        if version.is_empty() {
                            // Default to 3.5 for backward compatibility
                            "S3.5".to_string()
                        } else {
                            format!("S{}", version)
                        }
                    }
                    "Haiku" => {
                        // Haiku is already short, just show "Haiku"
                        "Haiku".to_string()
                    }
                    _ => family.clone(),
                }
            }
            ModelType::Unknown => "Claude".to_string(),
        }
    }

    /// Returns the canonical model name for database storage
    /// This normalizes different display name variations to a consistent format
    /// Examples:
    /// - "Claude 3.5 Sonnet" → "Sonnet 3.5"
    /// - "Sonnet 4.5" → "Sonnet 4.5"
    /// - "claude-sonnet-4-5-20250929" → "Sonnet 4.5"
    pub fn canonical_name(&self) -> String {
        match self {
            ModelType::Model { family, version } => {
                if version.is_empty() {
                    family.clone()
                } else {
                    format!("{} {}", family, version)
                }
            }
            ModelType::Unknown => "Unknown".to_string(),
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
        // Test Opus detection
        let opus = ModelType::from_name("Claude Opus");
        assert!(matches!(opus, ModelType::Model { family, .. } if family == "Opus"));

        let opus3 = ModelType::from_name("claude-3-opus-20240229");
        assert!(
            matches!(opus3, ModelType::Model { family, version } if family == "Opus" && version == "3")
        );

        // Test Sonnet detection
        let sonnet35 = ModelType::from_name("Claude 3.5 Sonnet");
        assert!(
            matches!(sonnet35, ModelType::Model { family, version } if family == "Sonnet" && version == "3.5")
        );

        let sonnet45 = ModelType::from_name("Claude Sonnet 4.5");
        assert!(
            matches!(sonnet45, ModelType::Model { family, version } if family == "Sonnet" && version == "4.5")
        );

        let sonnet45_alt = ModelType::from_name("Claude 4.5 Sonnet");
        assert!(
            matches!(sonnet45_alt, ModelType::Model { family, version } if family == "Sonnet" && version == "4.5")
        );

        let sonnet45_dash = ModelType::from_name("claude-sonnet-4-5");
        assert!(
            matches!(sonnet45_dash, ModelType::Model { family, version } if family == "Sonnet" && version == "4.5")
        );

        // Test Haiku detection
        let haiku = ModelType::from_name("Claude Haiku");
        assert!(matches!(haiku, ModelType::Model { family, .. } if family == "Haiku"));

        let haiku45 = ModelType::from_name("Claude Haiku 4.5");
        assert!(
            matches!(haiku45, ModelType::Model { family, version } if family == "Haiku" && version == "4.5")
        );

        // Test unknown
        assert_eq!(ModelType::from_name("Unknown Model"), ModelType::Unknown);
    }

    #[test]
    fn test_canonical_name_normalization() {
        // Test that different display names normalize to the same canonical name
        assert_eq!(
            ModelType::from_name("Claude Sonnet 4.5").canonical_name(),
            "Sonnet 4.5"
        );

        assert_eq!(
            ModelType::from_name("Sonnet 4.5").canonical_name(),
            "Sonnet 4.5"
        );

        assert_eq!(
            ModelType::from_name("claude-sonnet-4-5-20250929").canonical_name(),
            "Sonnet 4.5"
        );

        assert_eq!(
            ModelType::from_name("Claude 4.5 Sonnet").canonical_name(),
            "Sonnet 4.5"
        );

        // Test Sonnet 3.5 variations
        assert_eq!(
            ModelType::from_name("Claude 3.5 Sonnet").canonical_name(),
            "Sonnet 3.5"
        );

        assert_eq!(
            ModelType::from_name("claude-3-5-sonnet-20240620").canonical_name(),
            "Sonnet 3.5"
        );

        // Test Opus
        assert_eq!(
            ModelType::from_name("Claude Opus").canonical_name(),
            "Opus"
        );

        assert_eq!(
            ModelType::from_name("Claude 3.5 Opus").canonical_name(),
            "Opus 3.5"
        );

        // Test Haiku
        assert_eq!(
            ModelType::from_name("Claude Haiku").canonical_name(),
            "Haiku"
        );

        assert_eq!(
            ModelType::from_name("Claude 4.5 Haiku").canonical_name(),
            "Haiku 4.5"
        );

        // Test Unknown
        assert_eq!(
            ModelType::from_name("Unknown Model").canonical_name(),
            "Unknown"
        );
    }

    #[test]
    fn test_model_type_display() {
        assert_eq!(ModelType::from_name("Claude Opus").abbreviation(), "Opus");
        assert_eq!(
            ModelType::from_name("Claude 3.5 Sonnet").abbreviation(),
            "S3.5"
        );
        assert_eq!(
            ModelType::from_name("Claude 4.5 Sonnet").abbreviation(),
            "S4.5"
        );
        assert_eq!(ModelType::from_name("Claude Sonnet").abbreviation(), "S3.5"); // Default to 3.5
        assert_eq!(ModelType::from_name("Claude Haiku").abbreviation(), "Haiku");
        assert_eq!(ModelType::Unknown.abbreviation(), "Claude");
    }

    #[test]
    fn test_version_extraction() {
        // Test various version number formats
        let test_cases = vec![
            ("Claude 3.5 Sonnet", "3.5"),
            ("Claude Sonnet 4.5", "4.5"),
            ("claude-sonnet-4-5", "4.5"),
            ("Claude 4.5 Sonnet", "4.5"),
            ("claude-3-opus-20240229", "3"),
            ("Claude Haiku 3", "3"),
            ("Claude Opus 4.1", "4.1"),
            ("Claude Sonnet 5.0", "5.0"),
            ("Claude Haiku 6-2", "6.2"),
        ];

        for (input, expected_version) in test_cases {
            let model = ModelType::from_name(input);
            if let ModelType::Model { version, .. } = model {
                assert_eq!(
                    version, expected_version,
                    "Failed for input '{}': expected '{}', got '{}'",
                    input, expected_version, version
                );
            } else {
                panic!("Expected Model variant for input '{}'", input);
            }
        }
    }

    #[test]
    fn test_future_model_versions() {
        // Test that future versions work without code changes
        assert_eq!(
            ModelType::from_name("Claude Sonnet 5.0").abbreviation(),
            "S5.0"
        );
        assert_eq!(
            ModelType::from_name("Claude Sonnet 6.5").abbreviation(),
            "S6.5"
        );
        assert_eq!(
            ModelType::from_name("Claude Haiku 4.5").abbreviation(),
            "Haiku"
        );
        assert_eq!(
            ModelType::from_name("Claude Opus 4.0").abbreviation(),
            "Opus"
        );

        // Test edge cases
        assert_eq!(
            ModelType::from_name("Claude Sonnet 10.5").abbreviation(),
            "S10.5"
        );
        assert_eq!(
            ModelType::from_name("Claude Sonnet 3-7").abbreviation(),
            "S3.7"
        );
    }

    #[test]
    fn test_model_detection_edge_cases() {
        // No version number - should handle gracefully
        let no_version = ModelType::from_name("Claude Sonnet");
        assert!(matches!(&no_version, ModelType::Model { family, .. } if family == "Sonnet"));
        assert_eq!(no_version.abbreviation(), "S3.5"); // Falls back to 3.5

        // Unknown model
        assert_eq!(ModelType::from_name("GPT-4"), ModelType::Unknown);
        assert_eq!(ModelType::from_name("Unknown Model"), ModelType::Unknown);

        // Case insensitive
        let lowercase = ModelType::from_name("claude sonnet 4.5");
        assert!(matches!(&lowercase, ModelType::Model { family, version }
            if family == "Sonnet" && version == "4.5"));

        // Multiple digits
        let multi = ModelType::from_name("Claude Sonnet 12.34");
        assert!(matches!(&multi, ModelType::Model { version, .. } if version == "12.34"));

        // Ignore build identifiers when extracting versions
        let slug = ModelType::from_name("claude-sonnet-20240229");
        assert!(matches!(&slug, ModelType::Model { version, .. } if version.is_empty()));
        assert_eq!(slug.abbreviation(), "S3.5");
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
