use std::collections::HashMap;
use std::time::Duration;

use super::{DataProvider, ProviderResult};

/// A configurable test provider for unit testing the provider system.
///
/// Allows tests to control every aspect of provider behavior: name, priority,
/// timeout, availability, returned variables, and simulated delay.
pub struct TestProvider {
    pub name: String,
    pub priority: u32,
    pub timeout: Duration,
    pub available: bool,
    pub variables: HashMap<String, String>,
    pub delay: Option<Duration>,
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
        Ok(self.variables.clone())
    }
}
