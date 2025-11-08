//! Adaptive context window learning module
//!
//! This module implements automatic learning of actual context window sizes
//! by observing usage patterns in real-time. It detects:
//!
//! 1. **Compaction Events**: Sudden token drops indicating automatic cleanup
//!    - Example: 195k → 120k tokens (>10% drop, previous >150k)
//!
//! 2. **Ceiling Patterns**: Repeated observations near the same maximum
//!    - Example: Sessions hitting 198k, 199k, 197k repeatedly
//!
//! 3. **Confidence Building**: Multiple observations increase certainty
//!    - Confidence score: 0.0 (none) to 1.0 (certain)
//!
//! The learned values are only used when:
//! - `adaptive_learning = true` in config
//! - Confidence score >= `learning_confidence_threshold`
//! - No user override exists in `model_windows`

use crate::database::SqliteDatabase;
use crate::error::Result;
use chrono::Local;
use log::{debug, info, warn};
use std::fs::File;
use std::io::{BufRead, BufReader};

/// Minimum token count to consider for compaction detection
const MIN_COMPACTION_TOKENS: usize = 150_000;

/// Minimum percentage drop to consider a compaction event
const COMPACTION_DROP_THRESHOLD: f64 = 0.10; // 10%

/// Token variance threshold for ceiling detection (within 2% = same ceiling)
const CEILING_VARIANCE_THRESHOLD: f64 = 0.02; // 2%

/// Proximity threshold for compaction detection (95% of observed max)
/// Used as fallback when transcript is unavailable
const COMPACTION_PROXIMITY_THRESHOLD: f64 = 0.95;

/// Number of recent messages to check for manual compaction commands
const MANUAL_COMPACTION_CHECK_LINES: usize = 5;

/// Context window learning manager
pub struct ContextLearner {
    db: SqliteDatabase,
}

impl ContextLearner {
    /// Create a new context learner with database connection
    pub fn new(db: SqliteDatabase) -> Self {
        Self { db }
    }

    /// Observe token usage for a model and update learned values
    ///
    /// This is called after processing each transcript to:
    /// - Detect compaction events (sudden token drops)
    /// - Track ceiling observations (repeated high values)
    /// - Update confidence scores
    ///
    /// # Arguments
    ///
    /// * `model_name` - The model display name (e.g., "Claude Sonnet 4.5")
    /// * `current_tokens` - Current total token count from this session
    /// * `previous_tokens` - Previous session's token count (if available)
    /// * `transcript_path` - Optional path to transcript for manual compaction detection
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if database operations fail.
    pub fn observe_usage(
        &self,
        model_name: &str,
        current_tokens: usize,
        previous_tokens: Option<usize>,
        transcript_path: Option<&str>,
    ) -> Result<()> {
        debug!(
            "Observing usage for {}: current={}, previous={:?}, transcript={:?}",
            model_name, current_tokens, previous_tokens, transcript_path
        );

        // Get observed max for proximity checking
        let existing = self.db.get_learned_context(model_name)?;
        let observed_max = existing.as_ref().map(|r| r.observed_max_tokens).unwrap_or(0);

        // Detect compaction event
        if let Some(prev) = previous_tokens {
            if self.is_compaction_event(current_tokens, prev, observed_max, transcript_path) {
                info!(
                    "Compaction detected for {}: {} → {} tokens ({:.1}% drop)",
                    model_name,
                    prev,
                    current_tokens,
                    ((prev - current_tokens) as f64 / prev as f64) * 100.0
                );
                self.record_compaction(model_name, prev)?;
            }
        }

        // Update ceiling observation
        if current_tokens > MIN_COMPACTION_TOKENS {
            self.update_ceiling_observation(model_name, current_tokens)?;
        }

        // Recalculate confidence score
        self.update_confidence(model_name)?;

        Ok(())
    }

    /// Check if this represents a compaction event
    ///
    /// A compaction event is detected when:
    /// - Previous tokens >= MIN_COMPACTION_TOKENS (150k)
    /// - Current tokens < previous tokens
    /// - Drop percentage >= COMPACTION_DROP_THRESHOLD (10%)
    /// - NOT a manual compaction (user explicitly requested summary)
    /// - Close to observed ceiling (if known) OR first observation at high level
    fn is_compaction_event(
        &self,
        current_tokens: usize,
        previous_tokens: usize,
        observed_max: usize,
        transcript_path: Option<&str>,
    ) -> bool {
        // Basic checks
        if previous_tokens < MIN_COMPACTION_TOKENS {
            return false;
        }

        if current_tokens >= previous_tokens {
            return false;
        }

        let drop_percent = (previous_tokens - current_tokens) as f64 / previous_tokens as f64;
        if drop_percent < COMPACTION_DROP_THRESHOLD {
            return false;
        }

        // Check if user manually requested compaction
        if let Some(path) = transcript_path {
            if Self::is_manual_compaction(path) {
                debug!(
                    "Skipping compaction event - user manually requested summary ({}→{})",
                    previous_tokens, current_tokens
                );
                return false;
            }
        }

        // Proximity check: Only record if near the observed ceiling
        // This filters out manual compactions that weren't detected by pattern matching
        if observed_max > 0 {
            let proximity = previous_tokens as f64 / observed_max as f64;
            if proximity < COMPACTION_PROXIMITY_THRESHOLD {
                debug!(
                    "Skipping compaction at {} (only {:.1}% of observed max {})",
                    previous_tokens,
                    proximity * 100.0,
                    observed_max
                );
                return false;
            }
        } else {
            // First observation - must be at high level (190k+) to be automatic
            if previous_tokens < 190_000 {
                debug!(
                    "Skipping first compaction at {} (below 190k threshold)",
                    previous_tokens
                );
                return false;
            }
        }

        true
    }

    /// Check if a manual compaction was requested in recent transcript messages
    ///
    /// Looks for common compaction commands and phrases in user messages:
    /// - /compact, /summarize
    /// - "summarize our conversation"
    /// - "compress the context"
    /// - etc.
    fn is_manual_compaction(transcript_path: &str) -> bool {
        // Read last few lines from transcript
        let file = match File::open(transcript_path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let reader = BufReader::new(file);
        let lines: Vec<String> = reader
            .lines()
            .filter_map(|line| line.ok())
            .collect::<Vec<_>>()
            .into_iter()
            .rev() // Most recent first
            .take(MANUAL_COMPACTION_CHECK_LINES)
            .collect();

        // Check each line for manual compaction indicators
        for line in lines {
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
                // Check if this is a user message
                if let Some(role) = entry.pointer("/message/role").and_then(|v| v.as_str()) {
                    if role == "user" {
                        // Get message content
                        if let Some(content) =
                            entry.pointer("/message/content").and_then(|v| v.as_str())
                        {
                            let content_lower = content.to_lowercase();

                            // Check for explicit compaction commands
                            let compaction_patterns = [
                                "/compact",
                                "/summarize",
                                "summarize our conversation",
                                "summarize the conversation",
                                "summarize this conversation",
                                "compress the context",
                                "reduce the context",
                                "create a summary",
                                "make a summary",
                                "condense our conversation",
                                "condense the conversation",
                                "shorten the conversation",
                                "compact the context",
                            ];

                            for pattern in &compaction_patterns {
                                if content_lower.contains(pattern) {
                                    debug!(
                                        "Manual compaction detected: user message contains '{}'",
                                        pattern
                                    );
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }

        false
    }

    /// Record a compaction event in the database
    ///
    /// Updates the observed maximum and increments compaction count.
    fn record_compaction(&self, model_name: &str, observed_max: usize) -> Result<()> {
        let now = Local::now().to_rfc3339();

        // Get existing record or create new one
        let existing = self.db.get_learned_context(model_name)?;

        if let Some(mut record) = existing {
            // Update existing record
            record.compaction_count += 1;
            if observed_max > record.observed_max_tokens {
                record.observed_max_tokens = observed_max;
            }
            record.last_observed_max = observed_max;
            record.last_updated = now;

            self.db.update_learned_context(&record)?;
        } else {
            // Create new record
            let record = LearnedContextWindow {
                model_name: model_name.to_string(),
                observed_max_tokens: observed_max,
                ceiling_observations: 0,
                compaction_count: 1,
                last_observed_max: observed_max,
                last_updated: now.clone(),
                confidence_score: 0.0,
                first_seen: now,
            };

            self.db.insert_learned_context(&record)?;
        }

        Ok(())
    }

    /// Update ceiling observation for a model
    ///
    /// If the current tokens are within CEILING_VARIANCE_THRESHOLD of the
    /// observed maximum, increment ceiling_observations.
    fn update_ceiling_observation(&self, model_name: &str, current_tokens: usize) -> Result<()> {
        let now = Local::now().to_rfc3339();

        let existing = self.db.get_learned_context(model_name)?;

        if let Some(mut record) = existing {
            // Check if this is near the ceiling
            let variance = if record.observed_max_tokens > 0 {
                (current_tokens as f64 - record.observed_max_tokens as f64).abs()
                    / record.observed_max_tokens as f64
            } else {
                1.0 // First observation
            };

            if variance <= CEILING_VARIANCE_THRESHOLD {
                // Within 2% of observed max = ceiling hit
                record.ceiling_observations += 1;
                debug!(
                    "Ceiling observation for {}: {} tokens (variance: {:.2}%)",
                    model_name,
                    current_tokens,
                    variance * 100.0
                );
            }

            // Update max if higher
            if current_tokens > record.observed_max_tokens {
                record.observed_max_tokens = current_tokens;
            }

            record.last_observed_max = current_tokens;
            record.last_updated = now;

            self.db.update_learned_context(&record)?;
        } else {
            // Create new record with first ceiling observation
            let record = LearnedContextWindow {
                model_name: model_name.to_string(),
                observed_max_tokens: current_tokens,
                ceiling_observations: 1,
                compaction_count: 0,
                last_observed_max: current_tokens,
                last_updated: now.clone(),
                confidence_score: 0.0,
                first_seen: now,
            };

            self.db.insert_learned_context(&record)?;
        }

        Ok(())
    }

    /// Calculate and update confidence score for a model
    ///
    /// Confidence is based on:
    /// - Ceiling observations: Each adds 0.1 (max 0.5)
    /// - Compaction events: Each adds 0.3 (max 0.5)
    /// - Total confidence capped at 1.0
    fn update_confidence(&self, model_name: &str) -> Result<()> {
        if let Some(mut record) = self.db.get_learned_context(model_name)? {
            let confidence = self.calculate_confidence(
                record.ceiling_observations,
                record.compaction_count,
            );

            if confidence != record.confidence_score {
                debug!(
                    "Updated confidence for {}: {:.2} → {:.2} (ceiling={}, compaction={})",
                    model_name, record.confidence_score, confidence, record.ceiling_observations, record.compaction_count
                );

                record.confidence_score = confidence;
                record.last_updated = Local::now().to_rfc3339();
                self.db.update_learned_context(&record)?;
            }
        }

        Ok(())
    }

    /// Calculate confidence score based on observations
    ///
    /// Formula:
    /// - Ceiling score: min(ceiling_observations * 0.1, 0.5)
    /// - Compaction score: min(compaction_count * 0.3, 0.5)
    /// - Total: min(ceiling_score + compaction_score, 1.0)
    pub fn calculate_confidence(
        &self,
        ceiling_observations: i32,
        compaction_count: i32,
    ) -> f64 {
        let ceiling_score = (ceiling_observations as f64 * 0.1).min(0.5);
        let compaction_score = (compaction_count as f64 * 0.3).min(0.5);
        (ceiling_score + compaction_score).min(1.0)
    }

    /// Get learned context window for a model if confidence is high enough
    ///
    /// Returns the learned window size only if:
    /// - A record exists for this model
    /// - Confidence score >= threshold
    ///
    /// # Arguments
    ///
    /// * `model_name` - The model display name
    /// * `confidence_threshold` - Minimum confidence required (0.0-1.0)
    pub fn get_learned_window(
        &self,
        model_name: &str,
        confidence_threshold: f64,
    ) -> Result<Option<usize>> {
        if let Some(record) = self.db.get_learned_context(model_name)? {
            if record.confidence_score >= confidence_threshold {
                debug!(
                    "Using learned window for {}: {} tokens (confidence: {:.2})",
                    model_name, record.observed_max_tokens, record.confidence_score
                );
                return Ok(Some(record.observed_max_tokens));
            } else {
                debug!(
                    "Learned window for {} below confidence threshold: {:.2} < {:.2}",
                    model_name, record.confidence_score, confidence_threshold
                );
            }
        }

        Ok(None)
    }

    /// Get detailed learning information for a specific model
    pub fn get_learned_window_details(&self, model_name: &str) -> Result<Option<LearnedContextWindow>> {
        Ok(self.db.get_learned_context(model_name)?)
    }

    /// Get all learned context windows with their details
    pub fn get_all_learned_windows(&self) -> Result<Vec<LearnedContextWindow>> {
        Ok(self.db.get_all_learned_contexts()?)
    }

    /// Get all learned context windows with their details (alias for compatibility)
    pub fn get_all_learned(&self) -> Result<Vec<LearnedContextWindow>> {
        self.get_all_learned_windows()
    }

    /// Reset learned data for a specific model
    pub fn reset_model(&self, model_name: &str) -> Result<()> {
        warn!("Resetting learned context data for: {}", model_name);
        Ok(self.db.delete_learned_context(model_name)?)
    }

    /// Reset all learned context data
    pub fn reset_all(&self) -> Result<()> {
        warn!("Resetting ALL learned context data");
        Ok(self.db.delete_all_learned_contexts()?)
    }
}

/// Learned context window record from database
#[derive(Debug, Clone)]
pub struct LearnedContextWindow {
    pub model_name: String,
    pub observed_max_tokens: usize,
    pub ceiling_observations: i32,
    pub compaction_count: i32,
    pub last_observed_max: usize,
    pub last_updated: String,
    pub confidence_score: f64,
    pub first_seen: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_learner() -> (ContextLearner, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database and run migrations
        let db = SqliteDatabase::new(&db_path).unwrap();
        let mut runner = crate::migrations::MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        (ContextLearner::new(db), temp_dir)
    }

    #[test]
    fn test_is_compaction_event() {
        let (learner, _temp) = create_test_learner();

        // Should detect: 195k → 120k (38% drop, prev > 150k, first observation >190k)
        assert!(learner.is_compaction_event(120_000, 195_000, 0, None));

        // Should NOT detect: small drop
        assert!(!learner.is_compaction_event(190_000, 195_000, 0, None));

        // Should NOT detect: low token count
        assert!(!learner.is_compaction_event(100_000, 140_000, 0, None));

        // Should NOT detect: increase
        assert!(!learner.is_compaction_event(200_000, 195_000, 0, None));

        // Should NOT detect: first observation below 190k (likely manual)
        assert!(!learner.is_compaction_event(100_000, 180_000, 0, None));

        // Should detect: near observed max (195k is 97.5% of 200k)
        assert!(learner.is_compaction_event(120_000, 195_000, 200_000, None));

        // Should NOT detect: far from observed max (180k is 90% of 200k)
        assert!(!learner.is_compaction_event(100_000, 180_000, 200_000, None));
    }

    #[test]
    fn test_calculate_confidence() {
        let (learner, _temp) = create_test_learner();

        // No observations
        assert_eq!(learner.calculate_confidence(0, 0), 0.0);

        // 1 ceiling observation
        assert_eq!(learner.calculate_confidence(1, 0), 0.1);

        // 5 ceiling observations (max 0.5)
        assert_eq!(learner.calculate_confidence(5, 0), 0.5);

        // 10 ceiling observations (capped at 0.5)
        assert_eq!(learner.calculate_confidence(10, 0), 0.5);

        // 1 compaction
        assert_eq!(learner.calculate_confidence(0, 1), 0.3);

        // 2 compactions (capped at 0.5)
        assert_eq!(learner.calculate_confidence(0, 2), 0.5);

        // 5 ceiling + 1 compaction = 0.8
        assert_eq!(learner.calculate_confidence(5, 1), 0.8);

        // 5 ceiling + 2 compactions = 1.0 (capped)
        assert_eq!(learner.calculate_confidence(5, 2), 1.0);
    }

    #[test]
    fn test_record_compaction() {
        let (learner, _temp) = create_test_learner();

        // Record first compaction
        learner
            .record_compaction("Test Model", 195_000)
            .unwrap();

        // Verify it was recorded
        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();

        assert_eq!(record.model_name, "Test Model");
        assert_eq!(record.observed_max_tokens, 195_000);
        assert_eq!(record.compaction_count, 1);
        assert_eq!(record.ceiling_observations, 0);

        // Record second compaction with higher max
        learner
            .record_compaction("Test Model", 199_000)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();

        assert_eq!(record.observed_max_tokens, 199_000); // Updated to higher
        assert_eq!(record.compaction_count, 2);
    }

    #[test]
    fn test_update_ceiling_observation() {
        let (learner, _temp) = create_test_learner();

        // Record first ceiling observation
        learner
            .update_ceiling_observation("Test Model", 198_000)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();

        assert_eq!(record.ceiling_observations, 1);
        assert_eq!(record.observed_max_tokens, 198_000);

        // Record another near the ceiling (within 2%)
        learner
            .update_ceiling_observation("Test Model", 199_000)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();

        assert_eq!(record.ceiling_observations, 2);
        assert_eq!(record.observed_max_tokens, 199_000); // Updated to higher
    }

    #[test]
    fn test_observe_usage_flow() {
        let (learner, _temp) = create_test_learner();

        // Simulate approaching ceiling
        learner
            .observe_usage("Test Model", 198_000, None, None)
            .unwrap();
        learner
            .observe_usage("Test Model", 199_000, Some(198_000), None)
            .unwrap();
        learner
            .observe_usage("Test Model", 197_000, Some(199_000), None)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();

        // Should have ceiling observations but no compaction
        assert!(record.ceiling_observations >= 2);
        assert_eq!(record.compaction_count, 0);

        // Now simulate compaction near the ceiling
        learner
            .observe_usage("Test Model", 120_000, Some(197_000), None)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();

        // Should have recorded the compaction
        assert_eq!(record.compaction_count, 1);
        assert_eq!(record.observed_max_tokens, 199_000); // Max from ceiling observations
    }

    #[test]
    fn test_get_learned_window_with_threshold() {
        let (learner, _temp) = create_test_learner();

        // Record enough observations to reach threshold
        learner
            .observe_usage("High Confidence", 198_000, None, None)
            .unwrap();
        for _ in 0..4 {
            learner
                .observe_usage("High Confidence", 199_000, Some(198_000), None)
                .unwrap();
        }
        // Simulate compaction near ceiling
        learner
            .observe_usage("High Confidence", 120_000, Some(199_000), None)
            .unwrap();

        // Should be above 0.7 threshold
        let learned = learner
            .get_learned_window("High Confidence", 0.7)
            .unwrap();
        assert!(learned.is_some());
        assert_eq!(learned.unwrap(), 199_000);

        // Low confidence model
        learner
            .observe_usage("Low Confidence", 195_000, None, None)
            .unwrap();

        let learned = learner
            .get_learned_window("Low Confidence", 0.7)
            .unwrap();
        assert!(learned.is_none()); // Below threshold
    }

    #[test]
    fn test_reset_operations() {
        let (learner, _temp) = create_test_learner();

        // Add some data
        learner
            .observe_usage("Model A", 198_000, None, None)
            .unwrap();
        learner
            .observe_usage("Model B", 195_000, None, None)
            .unwrap();

        // Reset one model
        learner.reset_model("Model A").unwrap();

        assert!(learner
            .db
            .get_learned_context("Model A")
            .unwrap()
            .is_none());
        assert!(learner
            .db
            .get_learned_context("Model B")
            .unwrap()
            .is_some());

        // Reset all
        learner.reset_all().unwrap();

        assert!(learner
            .db
            .get_learned_context("Model B")
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_manual_compaction_detection() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a transcript with manual compaction request
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"message":{{"role":"user","content":"Let's start a new project"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"message":{{"role":"assistant","content":"Great! What project?"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"message":{{"role":"user","content":"Please summarize our conversation to save context"}}}}"#
        )
        .unwrap();
        file.flush().unwrap();

        // Should detect manual compaction
        assert!(ContextLearner::is_manual_compaction(
            file.path().to_str().unwrap()
        ));

        // Create a transcript without compaction request
        let mut file2 = NamedTempFile::new().unwrap();
        writeln!(
            file2,
            r#"{{"message":{{"role":"user","content":"Hello"}}}}"#
        )
        .unwrap();
        writeln!(
            file2,
            r#"{{"message":{{"role":"assistant","content":"Hi there!"}}}}"#
        )
        .unwrap();
        file2.flush().unwrap();

        // Should NOT detect manual compaction
        assert!(!ContextLearner::is_manual_compaction(
            file2.path().to_str().unwrap()
        ));
    }

    #[test]
    fn test_proximity_filtering() {
        let (learner, _temp) = create_test_learner();

        // Establish an observed max of 200k
        learner
            .observe_usage("Test Model", 198_000, None, None)
            .unwrap();
        learner
            .observe_usage("Test Model", 200_000, Some(198_000), None)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();
        assert_eq!(record.observed_max_tokens, 200_000);

        // Compaction at 180k (90% of 200k) should be filtered out
        learner
            .observe_usage("Test Model", 100_000, Some(180_000), None)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();
        // Compaction count should NOT have increased
        assert_eq!(record.compaction_count, 0);

        // Compaction at 195k (97.5% of 200k) should be recorded
        learner
            .observe_usage("Test Model", 100_000, Some(195_000), None)
            .unwrap();

        let record = learner
            .db
            .get_learned_context("Test Model")
            .unwrap()
            .unwrap();
        // Now compaction count should have increased
        assert_eq!(record.compaction_count, 1);
    }
}
