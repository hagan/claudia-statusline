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

        // Create async runtime for libSQL operations
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| StatuslineError::Sync(format!("Failed to create async runtime: {}", e)))?;

        // Test connection in async context
        let result = runtime.block_on(async {
            self.test_turso_connection_async(&turso_config.database_url, &auth_token)
                .await
        });

        match result {
            Ok(_) => {
                self.status.connected = true;
                self.status.error_message = None;
                info!("Successfully connected to Turso");
                Ok(true)
            }
            Err(e) => {
                self.status.connected = false;
                self.status.error_message = Some(e.to_string());
                warn!("Failed to connect to Turso: {}", e);
                Ok(false)
            }
        }
    }

    /// Async helper to test Turso connection
    async fn test_turso_connection_async(
        &self,
        database_url: &str,
        auth_token: &str,
    ) -> Result<()> {
        use libsql::Builder;

        // Build database connection
        let db = Builder::new_remote(database_url.to_string(), auth_token.to_string())
            .build()
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to build database: {}", e)))?;

        // Get a connection
        let conn = db
            .connect()
            .map_err(|e| StatuslineError::Sync(format!("Failed to connect: {}", e)))?;

        // Test query - just check if we can execute a simple query
        conn.execute("SELECT 1", ())
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to execute test query: {}", e)))?;

        debug!("Turso connection test successful");
        Ok(())
    }

    /// Async helper to push data to Turso
    /// Returns (sessions_pushed, daily_pushed, monthly_pushed)
    async fn push_to_turso_async(
        &self,
        database_url: &str,
        auth_token: &str,
        device_id: &str,
        sessions: std::collections::HashMap<String, crate::stats::SessionStats>,
        daily_stats: std::collections::HashMap<String, crate::stats::DailyStats>,
        monthly_stats: std::collections::HashMap<String, crate::stats::MonthlyStats>,
    ) -> Result<(u32, u32, u32)> {
        use libsql::Builder;

        // Build database connection
        let db = Builder::new_remote(database_url.to_string(), auth_token.to_string())
            .build()
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to build database: {}", e)))?;

        let conn = db
            .connect()
            .map_err(|e| StatuslineError::Sync(format!("Failed to connect: {}", e)))?;

        // Push sessions
        let mut sessions_pushed = 0u32;
        for (session_id, stats) in sessions.iter() {
            let query = "INSERT OR REPLACE INTO sessions
                         (device_id, session_id, start_time, last_updated, cost, lines_added, lines_removed)
                         VALUES (?, ?, ?, ?, ?, ?, ?)";

            conn.execute(
                query,
                libsql::params![
                    device_id,
                    session_id.as_str(),
                    stats.start_time.as_deref().unwrap_or(""),
                    stats.last_updated.as_str(),
                    stats.cost,
                    stats.lines_added as i64,
                    stats.lines_removed as i64,
                ],
            )
            .await
            .map_err(|e| {
                StatuslineError::Sync(format!("Failed to push session {}: {}", session_id, e))
            })?;

            sessions_pushed += 1;
        }

        // Push daily stats
        let mut daily_pushed = 0u32;
        for (date, stats) in daily_stats.iter() {
            let query = "INSERT OR REPLACE INTO daily_stats
                         (device_id, date, total_cost, total_lines_added, total_lines_removed)
                         VALUES (?, ?, ?, ?, ?)";

            conn.execute(
                query,
                libsql::params![
                    device_id,
                    date.as_str(),
                    stats.total_cost,
                    stats.lines_added as i64,
                    stats.lines_removed as i64,
                ],
            )
            .await
            .map_err(|e| {
                StatuslineError::Sync(format!("Failed to push daily stats for {}: {}", date, e))
            })?;

            daily_pushed += 1;
        }

        // Push monthly stats
        let mut monthly_pushed = 0u32;
        for (month, stats) in monthly_stats.iter() {
            let query = "INSERT OR REPLACE INTO monthly_stats
                         (device_id, month, total_cost, total_lines_added, total_lines_removed, session_count)
                         VALUES (?, ?, ?, ?, ?, ?)";

            conn.execute(
                query,
                libsql::params![
                    device_id,
                    month.as_str(),
                    stats.total_cost,
                    stats.lines_added as i64,
                    stats.lines_removed as i64,
                    stats.sessions as i64,
                ],
            )
            .await
            .map_err(|e| {
                StatuslineError::Sync(format!("Failed to push monthly stats for {}: {}", month, e))
            })?;

            monthly_pushed += 1;
        }

        debug!(
            "Pushed {} sessions, {} daily, {} monthly stats to Turso",
            sessions_pushed, daily_pushed, monthly_pushed
        );

        Ok((sessions_pushed, daily_pushed, monthly_pushed))
    }

    /// Async helper to pull data from Turso
    /// Returns (sessions, daily_stats, monthly_stats)
    async fn pull_from_turso_async(
        &self,
        database_url: &str,
        auth_token: &str,
        device_id: &str,
    ) -> Result<(
        std::collections::HashMap<String, crate::stats::SessionStats>,
        std::collections::HashMap<String, crate::stats::DailyStats>,
        std::collections::HashMap<String, crate::stats::MonthlyStats>,
    )> {
        use libsql::Builder;
        use std::collections::HashMap;

        // Build database connection
        let db = Builder::new_remote(database_url.to_string(), auth_token.to_string())
            .build()
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to build database: {}", e)))?;

        let conn = db
            .connect()
            .map_err(|e| StatuslineError::Sync(format!("Failed to connect: {}", e)))?;

        // Pull sessions for this device
        let query = "SELECT session_id, start_time, last_updated, cost, lines_added, lines_removed
                     FROM sessions WHERE device_id = ?";

        let mut rows = conn
            .query(query, libsql::params![device_id])
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to query sessions: {}", e)))?;

        let mut sessions = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to read session row: {}", e)))?
        {
            let session_id: String = row
                .get(0)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get session_id: {}", e)))?;
            let start_time: Option<String> = row.get(1).ok();
            let last_updated: String = row
                .get(2)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get last_updated: {}", e)))?;
            let cost: f64 = row
                .get(3)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get cost: {}", e)))?;
            let lines_added: i64 = row
                .get(4)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get lines_added: {}", e)))?;
            let lines_removed: i64 = row.get(5).map_err(|e| {
                StatuslineError::Sync(format!("Failed to get lines_removed: {}", e))
            })?;

            sessions.insert(
                session_id,
                crate::stats::SessionStats {
                    cost,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                    last_updated,
                    start_time,
                },
            );
        }

        // Pull daily stats for this device
        let query = "SELECT date, total_cost, total_lines_added, total_lines_removed
                     FROM daily_stats WHERE device_id = ?";

        let mut rows = conn
            .query(query, libsql::params![device_id])
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to query daily stats: {}", e)))?;

        let mut daily_stats = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to read daily row: {}", e)))?
        {
            let date: String = row
                .get(0)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get date: {}", e)))?;
            let total_cost: f64 = row
                .get(1)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get total_cost: {}", e)))?;
            let lines_added: i64 = row
                .get(2)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get lines_added: {}", e)))?;
            let lines_removed: i64 = row.get(3).map_err(|e| {
                StatuslineError::Sync(format!("Failed to get lines_removed: {}", e))
            })?;

            daily_stats.insert(
                date,
                crate::stats::DailyStats {
                    total_cost,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                    sessions: Vec::new(),
                },
            );
        }

        // Pull monthly stats for this device
        let query =
            "SELECT month, total_cost, total_lines_added, total_lines_removed, session_count
                     FROM monthly_stats WHERE device_id = ?";

        let mut rows = conn
            .query(query, libsql::params![device_id])
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to query monthly stats: {}", e)))?;

        let mut monthly_stats = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StatuslineError::Sync(format!("Failed to read monthly row: {}", e)))?
        {
            let month: String = row
                .get(0)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get month: {}", e)))?;
            let total_cost: f64 = row
                .get(1)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get total_cost: {}", e)))?;
            let lines_added: i64 = row
                .get(2)
                .map_err(|e| StatuslineError::Sync(format!("Failed to get lines_added: {}", e)))?;
            let lines_removed: i64 = row.get(3).map_err(|e| {
                StatuslineError::Sync(format!("Failed to get lines_removed: {}", e))
            })?;
            let session_count: i64 = row.get(4).map_err(|e| {
                StatuslineError::Sync(format!("Failed to get session_count: {}", e))
            })?;

            monthly_stats.insert(
                month,
                crate::stats::MonthlyStats {
                    total_cost,
                    sessions: session_count as usize,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                },
            );
        }

        debug!(
            "Pulled {} sessions, {} daily, {} monthly stats from Turso",
            sessions.len(),
            daily_stats.len(),
            monthly_stats.len()
        );

        Ok((sessions, daily_stats, monthly_stats))
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

        // Get all data from local database
        let sessions = db.get_all_sessions()?;
        let daily_stats = db.get_all_daily_stats()?;
        let monthly_stats = db.get_all_monthly_stats()?;

        info!(
            "Pushing {} sessions, {} daily, {} monthly stats to Turso",
            sessions.len(),
            daily_stats.len(),
            monthly_stats.len()
        );

        // Resolve auth token
        let auth_token = self.resolve_auth_token(&self.config.turso.auth_token)?;

        // Create async runtime for Turso operations
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| StatuslineError::Sync(format!("Failed to create async runtime: {}", e)))?;

        // Push to Turso in async context
        let result = runtime.block_on(async {
            self.push_to_turso_async(
                &self.config.turso.database_url,
                &auth_token,
                &device_id,
                sessions,
                daily_stats,
                monthly_stats,
            )
            .await
        });

        match result {
            Ok(counts) => {
                self.status.last_sync = Some(Local::now().timestamp());
                info!(
                    "Successfully pushed {} sessions, {} daily, {} monthly stats",
                    counts.0, counts.1, counts.2
                );
                Ok(PushResult {
                    sessions_pushed: counts.0,
                    daily_stats_pushed: counts.1,
                    monthly_stats_pushed: counts.2,
                    dry_run: false,
                })
            }
            Err(e) => {
                self.status.error_message = Some(e.to_string());
                warn!("Failed to push to Turso: {}", e);
                Err(e)
            }
        }
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

        // Load local database
        let db_path = StatsData::get_sqlite_path()?;
        let db = SqliteDatabase::new(&db_path)?;

        // Resolve auth token
        let auth_token = self.resolve_auth_token(&self.config.turso.auth_token)?;

        // Create async runtime for Turso operations
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| StatuslineError::Sync(format!("Failed to create async runtime: {}", e)))?;

        // Pull from Turso in async context
        let result = runtime.block_on(async {
            self.pull_from_turso_async(&self.config.turso.database_url, &auth_token, &device_id)
                .await
        });

        match result {
            Ok((remote_sessions, remote_daily, remote_monthly)) => {
                info!(
                    "Pulled {} sessions, {} daily, {} monthly stats from Turso",
                    remote_sessions.len(),
                    remote_daily.len(),
                    remote_monthly.len()
                );

                // Merge remote data into local database with conflict resolution
                let mut sessions_pulled = 0u32;
                let mut daily_pulled = 0u32;
                let mut monthly_pulled = 0u32;
                let mut conflicts_resolved = 0u32;

                // Get local data for conflict resolution
                let local_sessions = db.get_all_sessions()?;

                // Merge sessions (last-write-wins based on last_updated timestamp)
                for (session_id, remote_stats) in remote_sessions.iter() {
                    let should_update = if let Some(local_stats) = local_sessions.get(session_id) {
                        // Conflict: check timestamps
                        if remote_stats.last_updated > local_stats.last_updated {
                            conflicts_resolved += 1;
                            true
                        } else {
                            false
                        }
                    } else {
                        // No conflict: new session
                        true
                    };

                    if should_update {
                        db.upsert_session_direct(
                            session_id,
                            remote_stats.start_time.as_deref(),
                            &remote_stats.last_updated,
                            remote_stats.cost,
                            remote_stats.lines_added,
                            remote_stats.lines_removed,
                        )?;
                        sessions_pulled += 1;
                    }
                }

                // For daily and monthly stats, we use simple replacement (no timestamps)
                // This is acceptable because they're aggregates
                for (date, remote_stats) in remote_daily.iter() {
                    db.upsert_daily_stats_direct(
                        date,
                        remote_stats.total_cost,
                        remote_stats.lines_added,
                        remote_stats.lines_removed,
                    )?;
                    daily_pulled += 1;
                }

                for (month, remote_stats) in remote_monthly.iter() {
                    db.upsert_monthly_stats_direct(
                        month,
                        remote_stats.total_cost,
                        remote_stats.lines_added,
                        remote_stats.lines_removed,
                        remote_stats.sessions,
                    )?;
                    monthly_pulled += 1;
                }

                self.status.last_sync = Some(Local::now().timestamp());
                info!(
                    "Successfully merged {} sessions ({} conflicts), {} daily, {} monthly stats",
                    sessions_pulled, conflicts_resolved, daily_pulled, monthly_pulled
                );

                Ok(PullResult {
                    sessions_pulled,
                    daily_stats_pulled: daily_pulled,
                    monthly_stats_pulled: monthly_pulled,
                    conflicts_resolved,
                    dry_run: false,
                })
            }
            Err(e) => {
                self.status.error_message = Some(e.to_string());
                warn!("Failed to pull from Turso: {}", e);
                Err(e)
            }
        }
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
