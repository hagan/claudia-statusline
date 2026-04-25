use std::collections::HashMap;
use std::time::Duration;

use super::{DataProvider, ProviderError, ProviderResult};

/// Controls what `collect()` does when called.
#[derive(Clone)]
pub enum ProviderBehavior {
    /// Return variables normally (default).
    Normal,
    /// Panic with the given message.
    Panic(String),
    /// Return a `ProviderError::CollectionError` with the given message.
    Error(String),
}

/// A configurable test provider for unit testing the provider system.
///
/// Allows tests to control every aspect of provider behavior: name, priority,
/// timeout, availability, returned variables, simulated delay, and failure mode.
pub struct TestProvider {
    pub name: String,
    pub priority: u32,
    pub timeout: Duration,
    pub available: bool,
    pub variables: HashMap<String, String>,
    pub delay: Option<Duration>,
    pub behavior: ProviderBehavior,
}

impl TestProvider {
    /// Create a new TestProvider with sensible defaults.
    ///
    /// Defaults: timeout 1s, available true, no delay, empty variables.
    pub fn new(name: &str, priority: u32) -> Self {
        Self {
            name: name.to_string(),
            priority,
            timeout: Duration::from_secs(1),
            available: true,
            variables: HashMap::new(),
            delay: None,
            behavior: ProviderBehavior::Normal,
        }
    }

    /// Set the variables this provider will return.
    pub fn with_variables(mut self, vars: HashMap<String, String>) -> Self {
        self.variables = vars;
        self
    }

    /// Set a simulated delay before returning results.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Set the timeout for this provider.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Mark this provider as unavailable.
    pub fn unavailable(mut self) -> Self {
        self.available = false;
        self
    }

    /// Set the behavior for `collect()` (normal, panic, or error).
    pub fn with_behavior(mut self, behavior: ProviderBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

impl DataProvider for TestProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn timeout(&self) -> Duration {
        self.timeout
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn collect(&self) -> ProviderResult {
        if let Some(delay) = self.delay {
            std::thread::sleep(delay);
        }
        match &self.behavior {
            ProviderBehavior::Normal => Ok(self.variables.clone()),
            ProviderBehavior::Panic(msg) => panic!("{}", msg),
            ProviderBehavior::Error(msg) => Err(ProviderError::CollectionError(msg.clone())),
        }
    }
}
