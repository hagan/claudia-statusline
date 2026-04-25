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

/// How `TestProvider`'s simulated `delay` interacts with `timeout`.
///
/// - `Cooperative`: poll-sleeps the delay in 5ms increments and bails out
///   when `elapsed >= timeout`, matching the contract documented on
///   `DataProvider::timeout()`. On bail, `collect()` returns
///   `ProviderError::Timeout` so the orchestrator drops the (partial)
///   variable set, mirroring how a real provider that respects its
///   deadline would surface a deadline miss.
/// - `Uncooperative`: unconditional `thread::sleep(delay)`, used only by
///   tests that intentionally simulate a misbehaving provider (e.g. the B3
///   regression test for the spawn-time clock fix in `provider/mod.rs`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DelayMode {
    Cooperative,
    Uncooperative,
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
    pub delay_mode: DelayMode,
    pub behavior: ProviderBehavior,
}

impl TestProvider {
    /// Create a new TestProvider with sensible defaults.
    ///
    /// Defaults: timeout 1s, available true, no delay, cooperative delay mode,
    /// empty variables.
    pub fn new(name: &str, priority: u32) -> Self {
        Self {
            name: name.to_string(),
            priority,
            timeout: Duration::from_secs(1),
            available: true,
            variables: HashMap::new(),
            delay: None,
            delay_mode: DelayMode::Cooperative,
            behavior: ProviderBehavior::Normal,
        }
    }

    /// Set the variables this provider will return.
    pub fn with_variables(mut self, vars: HashMap<String, String>) -> Self {
        self.variables = vars;
        self
    }

    /// Set a simulated delay before returning results.
    ///
    /// The delay is applied **cooperatively**: `collect()` poll-sleeps in 5ms
    /// increments and bails out as soon as `elapsed >= self.timeout`,
    /// honoring the `DataProvider::timeout()` contract. On bail-out the
    /// provider returns `ProviderError::Timeout` rather than producing
    /// partial variables, which matches how a well-behaved real provider
    /// should report a missed deadline.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self.delay_mode = DelayMode::Cooperative;
        self
    }

    /// Set a simulated delay that is **NOT** cancellable at the timeout.
    ///
    /// Equivalent to `std::thread::sleep(delay)` inside `collect()`. Used by
    /// regression tests that need to simulate a misbehaving provider that
    /// ignores its timeout contract — for example the B3 spawn-time-clock
    /// regression test in `src/provider/mod.rs::tests`. Production code
    /// should use the cooperative `with_delay` instead.
    pub fn with_uncooperative_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self.delay_mode = DelayMode::Uncooperative;
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
            match self.delay_mode {
                DelayMode::Uncooperative => {
                    // Intentionally non-cooperative: simulates a provider that
                    // ignores its deadline. Used only by B3 regression tests.
                    std::thread::sleep(delay);
                }
                DelayMode::Cooperative => {
                    // Cooperative deadline (CONTEXT.md D-02 / PR #29 B1):
                    // poll-sleep in 5ms increments and bail at `self.timeout`
                    // so the orchestrator's wall-clock guarantee holds even
                    // when `delay > timeout`. Matches the cadence of the
                    // orchestrator's polling loop.
                    let start = std::time::Instant::now();
                    let mut deadline_hit = false;
                    while start.elapsed() < delay {
                        if start.elapsed() >= self.timeout {
                            deadline_hit = true;
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    if deadline_hit {
                        return Err(ProviderError::Timeout {
                            provider: self.name.clone(),
                            limit: self.timeout,
                        });
                    }
                }
            }
        }
        match &self.behavior {
            ProviderBehavior::Normal => Ok(self.variables.clone()),
            ProviderBehavior::Panic(msg) => panic!("{}", msg),
            ProviderBehavior::Error(msg) => Err(ProviderError::CollectionError(msg.clone())),
        }
    }
}
