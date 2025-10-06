// Sync module for cloud synchronization
// Only compiled when turso-sync feature is enabled

use crate::common::get_device_id;
use crate::config::SyncConfig;
use crate::database::SqliteDatabase;
use crate::error::{Result, StatuslineError};
use crate::stats::StatsData;
use chrono::Local;
use log::{debug, info, warn};
use std::env;

/// Sync status information
#[derive(Debug, Clone)]
#[allow(dead_code)] // Will be used in Phase 2+
pub struct SyncStatus {
    pub enabled: bool,
    pub provider: String,
    pub connected: bool,
    pub last_sync: Option<i64>,
    pub error_message: Option<String>,
}

impl Default for SyncStatus {
    fn default() -> Self {
        SyncStatus {
            enabled: false,
            provider: "none".to_string(),
            connected: false,
            last_sync: None,
            error_message: None,
        }
    }
}

/// Sync manager handles cloud synchronization
pub struct SyncManager {
    config: SyncConfig,
    status: SyncStatus,
}

impl SyncManager {
    /// Create a new sync manager from configuration
    pub fn new(config: SyncConfig) -> Self {
        let status = SyncStatus {
            enabled: config.enabled,
            provider: config.provider.clone(),
            connected: false,
            last_sync: None,
            error_message: None,
        };

        // If sync is disabled, set status accordingly
        if !config.enabled {
            debug!("Sync is disabled in configuration");
        }

        SyncManager { config, status }
    }

    /// Check if sync is enabled and configured
    #[allow(dead_code)] // Will be used in Phase 2+
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && !self.config.turso.database_url.is_empty()
    }

    /// Get current sync status
    pub fn status(&self) -> &SyncStatus {
        &self.status
    }

    /// Test connection to remote sync service
    pub fn test_connection(&mut self) -> Result<bool> {
        if !self.config.enabled {
            return Ok(false);
        }

        match self.config.provider.as_str() {
            "turso" => self.test_turso_connection(),
            _ => Err(StatuslineError::Sync(format!(
                "Unknown sync provider: {}",
                self.config.provider
            ))),
        }
    }

    /// Test Turso connection
    fn test_turso_connection(&mut self) -> Result<bool> {
        let turso_config = &self.config.turso;

        // Validate configuration
        if turso_config.database_url.is_empty() {
            self.status.error_message = Some("Turso database URL is empty".to_string());
            return Ok(false);
        }

        // Resolve auth token (may be env var reference)
        let auth_token = self.resolve_auth_token(&turso_config.auth_token)?;
        if auth_token.is_empty() {
            self.status.error_message = Some("Turso auth token is empty".to_string());
            return Ok(false);
        }

        info!("Testing Turso connection to {}", turso_config.database_url);

        // TODO: Actual libSQL connection test will go here in Phase 1
        // For now, just validate configuration
        warn!("Turso connection test not yet implemented (Phase 1 placeholder)");

        // Mock successful connection for now
        self.status.connected = false;
        self.status.error_message = Some("Connection test not yet implemented".to_string());

        Ok(false)
    }

    /// Resolve auth token, handling environment variable references
    /// Supports both ${VAR} and $VAR syntax
    fn resolve_auth_token(&self, token_config: &str) -> Result<String> {
        if token_config.is_empty() {
            return Ok(String::new());
        }

        // Check for environment variable reference
        if token_config.starts_with("${") && token_config.ends_with('}') {
            // Extract variable name: ${VAR_NAME} -> VAR_NAME
            let var_name = &token_config[2..token_config.len() - 1];
            env::var(var_name).map_err(|_| {
                StatuslineError::Sync(format!("Environment variable {} not found", var_name))
            })
        } else if let Some(var_name) = token_config.strip_prefix('$') {
            // Extract variable name: $VAR_NAME -> VAR_NAME
            env::var(var_name).map_err(|_| {
                StatuslineError::Sync(format!("Environment variable {} not found", var_name))
            })
        } else {
            // Use token directly
            Ok(token_config.to_string())
        }
    }

    /// Push local stats to remote (Turso)
    pub fn push(&mut self, dry_run: bool) -> Result<PushResult> {
        if !self.is_enabled() {
            return Err(StatuslineError::Sync(
                "Sync is not enabled or not configured".to_string(),
            ));
        }

        info!("Starting sync push (dry_run={})", dry_run);

        // Get device ID
        let device_id = get_device_id();
        debug!("Device ID: {}", device_id);

        // Load local database
        let db_path = StatsData::get_sqlite_path()?;
        let db = SqliteDatabase::new(&db_path)?;

        // Get current timestamp for sync tracking
        let _sync_timestamp = Local::now().timestamp();

        // Count records to sync
        let sessions_count = db.count_sessions()?;
        let daily_count = db.count_daily_stats()?;
        let monthly_count = db.count_monthly_stats()?;

        info!(
            "Found {} sessions, {} daily, {} monthly stats to push",
            sessions_count, daily_count, monthly_count
        );

        if dry_run {
            info!("Dry run mode - no data will be pushed");
            return Ok(PushResult {
                sessions_pushed: sessions_count as u32,
                daily_stats_pushed: daily_count as u32,
                monthly_stats_pushed: monthly_count as u32,
                dry_run: true,
            });
        }

        // TODO: Actual Turso push implementation
        // For Phase 2, we'll implement the real push logic here
        warn!("Push implementation not yet complete - this is a Phase 2 prototype");

        Ok(PushResult {
            sessions_pushed: 0,
            daily_stats_pushed: 0,
            monthly_stats_pushed: 0,
            dry_run: false,
        })
    }

    /// Pull remote stats to local database
    pub fn pull(&mut self, dry_run: bool) -> Result<PullResult> {
        if !self.is_enabled() {
            return Err(StatuslineError::Sync(
                "Sync is not enabled or not configured".to_string(),
            ));
        }

        info!("Starting sync pull (dry_run={})", dry_run);

        // Get device ID
        let device_id = get_device_id();
        debug!("Device ID: {}", device_id);

        if dry_run {
            info!("Dry run mode - no data will be pulled");
            return Ok(PullResult {
                sessions_pulled: 0,
                daily_stats_pulled: 0,
                monthly_stats_pulled: 0,
                conflicts_resolved: 0,
                dry_run: true,
            });
        }

        // TODO: Actual Turso pull implementation
        // For Phase 2, we'll implement the real pull logic here
        warn!("Pull implementation not yet complete - this is a Phase 2 prototype");

        Ok(PullResult {
            sessions_pulled: 0,
            daily_stats_pulled: 0,
            monthly_stats_pulled: 0,
            conflicts_resolved: 0,
            dry_run: false,
        })
    }
}

/// Result of a push operation
#[derive(Debug, Clone)]
pub struct PushResult {
    pub sessions_pushed: u32,
    pub daily_stats_pushed: u32,
    pub monthly_stats_pushed: u32,
    pub dry_run: bool,
}

/// Result of a pull operation
#[derive(Debug, Clone)]
pub struct PullResult {
    pub sessions_pulled: u32,
    pub daily_stats_pulled: u32,
    pub monthly_stats_pulled: u32,
    pub conflicts_resolved: u32,
    pub dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TursoConfig;

    #[test]
    fn test_sync_manager_disabled() {
        let config = SyncConfig {
            enabled: false,
            ..Default::default()
        };
        let manager = SyncManager::new(config);
        assert!(!manager.is_enabled());
        assert!(!manager.status().enabled);
    }

    #[test]
    fn test_sync_manager_enabled_no_url() {
        let config = SyncConfig {
            enabled: true,
            turso: TursoConfig {
                database_url: String::new(),
                auth_token: String::new(),
            },
            ..Default::default()
        };
        let manager = SyncManager::new(config);
        assert!(!manager.is_enabled()); // Not enabled because URL is empty
    }

    #[test]
    fn test_resolve_auth_token_direct() {
        let config = SyncConfig::default();
        let manager = SyncManager::new(config);

        let token = manager.resolve_auth_token("my-direct-token").unwrap();
        assert_eq!(token, "my-direct-token");
    }

    #[test]
    fn test_resolve_auth_token_env_var() {
        env::set_var("TEST_TURSO_TOKEN", "test-token-value");

        let config = SyncConfig::default();
        let manager = SyncManager::new(config);

        let token = manager.resolve_auth_token("${TEST_TURSO_TOKEN}").unwrap();
        assert_eq!(token, "test-token-value");

        let token2 = manager.resolve_auth_token("$TEST_TURSO_TOKEN").unwrap();
        assert_eq!(token2, "test-token-value");

        env::remove_var("TEST_TURSO_TOKEN");
    }

    #[test]
    fn test_resolve_auth_token_missing_env() {
        let config = SyncConfig::default();
        let manager = SyncManager::new(config);

        let result = manager.resolve_auth_token("${NONEXISTENT_VAR}");
        assert!(result.is_err());
    }
}
