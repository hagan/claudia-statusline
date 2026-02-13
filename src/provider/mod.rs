use std::collections::HashMap;
use std::time::Duration;

/// Errors that can occur during provider execution.
#[derive(Debug)]
pub enum ProviderError {
    /// Provider is not available
    Unavailable(String),
    /// Provider timed out
    Timeout { provider: String, limit: Duration },
    /// Provider collection error
    CollectionError(String),
}

/// Result type for provider operations.
pub type ProviderResult = Result<HashMap<String, String>, ProviderError>;

/// A data provider that collects variables for the statusline layout.
pub trait DataProvider: Send + Sync {
    /// Human-readable name for logging and diagnostics.
    fn name(&self) -> &str;

    /// Priority for variable conflict resolution (higher wins).
    fn priority(&self) -> u32;

    /// Maximum time this provider is allowed to run.
    fn timeout(&self) -> Duration;

    /// Quick check whether this provider can run in the current context.
    fn is_available(&self) -> bool;

    /// Collect variables. Called in a scoped thread.
    fn collect(&self) -> ProviderResult;
}

/// Orchestrates parallel execution of data providers.
pub struct ProviderOrchestrator {
    providers: Vec<Box<dyn DataProvider>>,
}

impl ProviderOrchestrator {
    /// Create a new empty orchestrator.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a provider with the orchestrator.
    pub fn register(&mut self, provider: Box<dyn DataProvider>) {
        self.providers.push(provider);
    }

    /// Execute all available providers and merge results.
    ///
    /// STUB: Returns empty HashMap. Implementation pending (GREEN phase).
    pub fn collect_all(&self) -> HashMap<String, String> {
        HashMap::new()
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
