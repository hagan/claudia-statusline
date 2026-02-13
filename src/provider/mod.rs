use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

/// Errors that can occur during provider execution.
#[derive(Error, Debug)]
pub enum ProviderError {
    /// Provider is not available (e.g., not in a git repo).
    #[error("Provider '{0}' is not available")]
    Unavailable(String),

    /// Provider execution exceeded its timeout limit.
    #[error("Provider '{provider}' timed out after {limit:?}")]
    Timeout {
        /// Name of the provider that timed out.
        provider: String,
        /// The timeout duration that was exceeded.
        limit: Duration,
    },

    /// Provider encountered an error during variable collection.
    #[error("Provider collection error: {0}")]
    CollectionError(String),
}

/// Result type for provider operations.
pub type ProviderResult = Result<HashMap<String, String>, ProviderError>;

/// A data provider that collects variables for the statusline layout.
///
/// Providers run in parallel via [`ProviderOrchestrator`]. Each provider
/// returns a `HashMap<String, String>` where keys are layout variable
/// names (e.g., "directory", "git", "cost") and values are the
/// pre-formatted strings ready for template substitution.
///
/// # Implementing a Provider
///
/// All providers must be `Send + Sync` because they are shared across
/// scoped threads during parallel execution.
///
/// The [`is_available`](DataProvider::is_available) method is called before
/// spawning a thread and should be fast (no I/O). Pass context at
/// construction time and check internal state in `is_available`.
///
/// The [`collect`](DataProvider::collect) method runs in a scoped thread
/// with a per-provider timeout enforced by the orchestrator.
pub trait DataProvider: Send + Sync {
    /// Human-readable name for logging and diagnostics.
    fn name(&self) -> &str;

    /// Priority for variable conflict resolution (higher wins).
    ///
    /// When two providers set the same variable key, the provider with
    /// the higher priority value wins. Recommended scale: 0-100, with
    /// 50 as the default for standard providers.
    fn priority(&self) -> u32;

    /// Maximum time this provider is allowed to run.
    ///
    /// If the provider's [`collect`](DataProvider::collect) method does
    /// not complete within this duration, the orchestrator discards its
    /// results and uses an empty variable map instead.
    fn timeout(&self) -> Duration;

    /// Quick check whether this provider can run in the current context.
    ///
    /// Called before spawning a thread. Should be fast (no I/O).
    /// Examples: checking if the current directory is a git repo,
    /// checking if a config feature is enabled.
    fn is_available(&self) -> bool;

    /// Collect variables for the statusline layout.
    ///
    /// Called in a scoped thread by the orchestrator. Returns a
    /// `HashMap<String, String>` mapping variable names to their
    /// formatted values, or a [`ProviderError`] on failure.
    fn collect(&self) -> ProviderResult;
}

/// Orchestrates parallel execution of data providers with timeout enforcement.
///
/// The orchestrator holds a collection of [`DataProvider`] implementations,
/// runs them in parallel using [`std::thread::scope`], enforces per-provider
/// timeouts, and merges results by priority (higher priority wins conflicts).
pub struct ProviderOrchestrator {
    providers: Vec<Box<dyn DataProvider>>,
}

impl Default for ProviderOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderOrchestrator {
    /// Create a new empty orchestrator with no registered providers.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a data provider with the orchestrator.
    ///
    /// Providers are executed in the order they are registered, but
    /// results are merged by priority (lower priority first, so higher
    /// priority values overwrite lower ones for the same key).
    pub fn register(&mut self, provider: Box<dyn DataProvider>) {
        self.providers.push(provider);
    }

    /// Execute all available providers in parallel and merge their results.
    ///
    /// This method:
    /// 1. Filters providers to those where [`is_available`](DataProvider::is_available) returns `true`
    /// 2. Spawns one scoped thread per available provider
    /// 3. Polls each thread for completion, enforcing per-provider timeouts
    /// 4. Collects successful results as `(priority, variables)` pairs
    /// 5. Sorts by priority ascending and merges with `HashMap::extend`,
    ///    so higher-priority providers overwrite lower-priority ones
    ///
    /// Providers that timeout, fail, or panic produce no variables rather
    /// than blocking the orchestrator.
    pub fn collect_all(&self) -> HashMap<String, String> {
        // Filter to available providers only
        let available: Vec<&dyn DataProvider> = self
            .providers
            .iter()
            .filter(|p| p.is_available())
            .map(|p| p.as_ref())
            .collect();

        if available.is_empty() {
            return HashMap::new();
        }

        // Collect results with timeouts using scoped threads
        let mut results: Vec<(u32, HashMap<String, String>)> = Vec::new();

        thread::scope(|s| {
            // Spawn a thread per provider, collecting handles with metadata
            let handles: Vec<_> = available
                .iter()
                .map(|provider| {
                    let timeout = provider.timeout();
                    let name = provider.name().to_string();
                    let priority = provider.priority();

                    let handle = s.spawn(move || provider.collect());

                    (handle, timeout, name, priority)
                })
                .collect();

            // Collect results, enforcing per-provider timeouts
            for (handle, timeout, name, priority) in handles {
                let start = Instant::now();

                loop {
                    if handle.is_finished() {
                        match handle.join() {
                            Ok(Ok(vars)) => {
                                results.push((priority, vars));
                            }
                            Ok(Err(e)) => {
                                log::debug!("Provider '{}' failed: {:?}", name, e);
                            }
                            Err(_) => {
                                log::warn!("Provider '{}' panicked", name);
                            }
                        }
                        break;
                    }

                    if start.elapsed() > timeout {
                        log::warn!("Provider '{}' timed out after {:?}", name, timeout);
                        // Thread will be joined when scope exits,
                        // but we skip collecting its result
                        break;
                    }

                    thread::sleep(Duration::from_millis(5));
                }
            }
        });

        // Merge results by priority: sort ascending, then extend
        // so higher priority values overwrite lower priority ones
        results.sort_by_key(|(priority, _)| *priority);
        let mut merged = HashMap::new();
        for (_, vars) in results {
            merged.extend(vars);
        }

        merged
    }
}

#[cfg(test)]
mod test_provider;

#[cfg(test)]
pub use test_provider::TestProvider;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    /// Helper to create a HashMap from key-value pairs.
    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_trait_has_required_methods() {
        let provider = TestProvider::new("test", 50)
            .with_variables(vars(&[("key", "value")]))
            .with_timeout(Duration::from_millis(500));

        assert_eq!(provider.name(), "test");
        assert_eq!(provider.priority(), 50);
        assert_eq!(provider.timeout(), Duration::from_millis(500));
        assert!(provider.is_available());

        let result = provider.collect().expect("collect should succeed");
        assert_eq!(result.get("key").map(|s| s.as_str()), Some("value"));
    }

    #[test]
    fn test_orchestrator_merges_two_providers() {
        let mut orch = ProviderOrchestrator::new();

        let provider_a = TestProvider::new("provider_a", 50)
            .with_variables(vars(&[("key_a", "val_a")]));
        let provider_b = TestProvider::new("provider_b", 50)
            .with_variables(vars(&[("key_b", "val_b")]));

        orch.register(Box::new(provider_a));
        orch.register(Box::new(provider_b));

        let result = orch.collect_all();
        assert_eq!(result.get("key_a").map(|s| s.as_str()), Some("val_a"));
        assert_eq!(result.get("key_b").map(|s| s.as_str()), Some("val_b"));
    }

    #[test]
    fn test_orchestrator_priority_merge() {
        let mut orch = ProviderOrchestrator::new();

        let low_priority = TestProvider::new("low", 10)
            .with_variables(vars(&[("shared_key", "low")]));
        let high_priority = TestProvider::new("high", 90)
            .with_variables(vars(&[("shared_key", "high")]));

        orch.register(Box::new(low_priority));
        orch.register(Box::new(high_priority));

        let result = orch.collect_all();
        assert_eq!(
            result.get("shared_key").map(|s| s.as_str()),
            Some("high"),
            "Higher priority provider's value should win"
        );
    }

    #[test]
    fn test_orchestrator_skips_unavailable() {
        let mut orch = ProviderOrchestrator::new();

        let available = TestProvider::new("available", 50)
            .with_variables(vars(&[("available", "yes")]));
        let unavailable = TestProvider::new("unavailable", 50)
            .with_variables(vars(&[("unavailable", "no")]))
            .unavailable();

        orch.register(Box::new(available));
        orch.register(Box::new(unavailable));

        let result = orch.collect_all();
        assert_eq!(result.get("available").map(|s| s.as_str()), Some("yes"));
        assert!(
            result.get("unavailable").is_none(),
            "Unavailable provider's variables should not appear"
        );
    }

    #[test]
    fn test_orchestrator_timeout_returns_empty() {
        let mut orch = ProviderOrchestrator::new();

        // Slow provider: 500ms sleep with 50ms timeout (10x margin per Research pitfall 5)
        let slow = TestProvider::new("slow", 50)
            .with_variables(vars(&[("slow_key", "slow_value")]))
            .with_timeout(Duration::from_millis(50))
            .with_delay(Duration::from_millis(500));

        // Fast provider: no delay, 1s timeout
        let fast = TestProvider::new("fast", 50)
            .with_variables(vars(&[("fast", "yes")]));

        orch.register(Box::new(slow));
        orch.register(Box::new(fast));

        let result = orch.collect_all();
        assert_eq!(result.get("fast").map(|s| s.as_str()), Some("yes"));
        assert!(
            result.get("slow_key").is_none(),
            "Timed-out provider's variables should not appear"
        );
    }

    #[test]
    fn test_orchestrator_empty_providers() {
        let orch = ProviderOrchestrator::new();
        let result = orch.collect_all();
        assert!(result.is_empty(), "Empty orchestrator should return empty HashMap");
    }
}
