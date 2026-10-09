use super::schema::{AppliedUpdate, SessionUpdate};
use super::SqliteDatabase;
use crate::common::{current_date, current_month, current_timestamp};
use crate::retry::{retry_if_retryable, RetryConfig};
use crate::stats::accounting::{self, Baseline, Observation};
use rusqlite::{params, OptionalExtension, Result, Transaction};

/// Session row copied into `session_archive` (auto_reset mode).
struct SessionArchiveData {
    start_time: String,
    last_updated: String,
    cost: f64,
    lines_added: i64,
    lines_removed: i64,
    active_time_seconds: Option<i64>,
    last_activity: Option<String>,
    model_name: Option<String>,
    workspace_dir: Option<String>,
    device_id: Option<String>,
    /// Raw payload baseline (migration 008); falls back to the accounted value when NULL.
    raw_cost: f64,
    raw_lines_added: i64,
    raw_lines_removed: i64,
    process_start_ms: Option<i64>,
}

/// Prior accounting state for one session: cumulative accounted totals plus the
/// raw-counter baseline the classifier compares against.
struct PriorAccounting {
    accounted_cost: f64,
    accounted_lines_added: i64,
    accounted_lines_removed: i64,
    baseline: Baseline,
}

/// Read accounted totals and the raw baseline from columns 0..=6 of a row laid out as
/// `cost, lines_added, lines_removed, COALESCE(raw_cost, cost),
/// COALESCE(raw_lines_added, lines_added), COALESCE(raw_lines_removed, lines_removed),
/// process_start_ms`.
fn prior_from_row(row: &rusqlite::Row<'_>) -> Result<PriorAccounting> {
    Ok(PriorAccounting {
        accounted_cost: row.get(0)?,
        accounted_lines_added: row.get(1)?,
        accounted_lines_removed: row.get(2)?,
        baseline: Baseline {
            raw_cost: row.get(3)?,
            raw_lines_added: row.get(4)?,
            raw_lines_removed: row.get(5)?,
            start_ms: row.get::<_, Option<i64>>(6).ok().flatten(),
        },
    })
}

impl SqliteDatabase {
    /// Update or insert a session with atomic transaction
    ///
    /// Returns `(day_total, session_total)`. See [`Self::update_session_applied`] for the
    /// full set of committed deltas.
    // public library API; the binary now calls update_session_applied
    #[allow(dead_code)]
    pub fn update_session(&self, session_id: &str, update: SessionUpdate) -> Result<(f64, f64)> {
        let applied = self.update_session_applied(session_id, update)?;
        Ok((applied.day_total, applied.session_total))
    }

    /// Update or insert a session with atomic transaction, returning exactly what the
    /// transaction committed (totals after the write, applied deltas and the new baseline).
    pub fn update_session_applied(
        &self,
        session_id: &str,
        update: SessionUpdate,
    ) -> Result<AppliedUpdate> {
        let retry_config = RetryConfig::for_db_ops();

        // Wrap the entire transaction in retry logic.
        //
        // IMMEDIATE (not the default DEFERRED): update_session_tx reads then writes, so a
        // DEFERRED transaction starts as a reader and upgrades to a writer mid-transaction.
        // Under concurrent writers (multiple processes/connections) two such read->write
        // upgrades hit an immediate SQLITE_BUSY that busy_timeout cannot wait out, dropping a
        // write (issue #57). IMMEDIATE takes the write lock at BEGIN so concurrent writers
        // serialize there and retry_if_retryable absorbs the contention cleanly.
        retry_if_retryable(&retry_config, || {
            let mut conn = self.get_connection()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

            let result = self.update_session_tx(&tx, session_id, update.clone())?;

            tx.commit()?;
            Ok(result)
        })
        .map_err(|e| match e {
            crate::error::StatuslineError::Database(db_err) => db_err,
            _ => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(e.to_string()),
            ),
        })
    }

    /// Update only max_tokens_observed for a session (for adaptive learning)
    /// Only updates if new value is greater than current value
    pub fn update_max_tokens_observed(&self, session_id: &str, current_tokens: u32) -> Result<()> {
        let retry_config = RetryConfig::for_db_ops();

        retry_if_retryable(&retry_config, || {
            let conn = self.get_connection()?;
            conn.execute(
                "UPDATE sessions
                 SET max_tokens_observed = ?2
                 WHERE session_id = ?1
                   AND (max_tokens_observed IS NULL OR max_tokens_observed < ?2)",
                params![session_id, current_tokens as i64],
            )?;
            Ok(())
        })
        .map_err(|e| match e {
            crate::error::StatuslineError::Database(db_err) => db_err,
            _ => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(e.to_string()),
            ),
        })
    }

    /// Archive a session to session_archive table (for auto_reset mode)
    /// This preserves the work period history before resetting the session counters
    fn archive_session(tx: &Transaction, session_id: &str) -> Result<()> {
        // Query current session data
        let session_data: Option<SessionArchiveData> = tx
            .query_row(
                "SELECT start_time, last_updated, cost, lines_added, lines_removed,
                        active_time_seconds, last_activity, model_name, workspace_dir, device_id,
                        COALESCE(raw_cost, cost), COALESCE(raw_lines_added, lines_added),
                        COALESCE(raw_lines_removed, lines_removed), process_start_ms
                 FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok(SessionArchiveData {
                        start_time: row.get(0)?,
                        last_updated: row.get(1)?,
                        cost: row.get(2)?,
                        lines_added: row.get(3)?,
                        lines_removed: row.get(4)?,
                        active_time_seconds: row.get(5).ok(),
                        last_activity: row.get(6).ok(),
                        model_name: row.get(7).ok(),
                        workspace_dir: row.get(8).ok(),
                        device_id: row.get(9).ok(),
                        raw_cost: row.get(10)?,
                        raw_lines_added: row.get(11)?,
                        raw_lines_removed: row.get(12)?,
                        process_start_ms: row.get::<_, Option<i64>>(13).ok().flatten(),
                    })
                },
            )
            .optional()?;

        if let Some(d) = session_data {
            let archived_at = current_timestamp();

            // Insert into session_archive, carrying the raw baseline and inferred start so
            // the next update classifies against them (D-09).
            tx.execute(
                "INSERT INTO session_archive (
                    session_id, start_time, end_time, archived_at,
                    cost, lines_added, lines_removed,
                    active_time_seconds, last_activity,
                    model_name, workspace_dir, device_id,
                    raw_cost, raw_lines_added, raw_lines_removed, process_start_ms
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    session_id,
                    d.start_time,
                    d.last_updated, // end_time = last_updated
                    archived_at,
                    d.cost,
                    d.lines_added,
                    d.lines_removed,
                    d.active_time_seconds,
                    d.last_activity,
                    d.model_name,
                    d.workspace_dir,
                    d.device_id,
                    d.raw_cost,
                    d.raw_lines_added,
                    d.raw_lines_removed,
                    d.process_start_ms,
                ],
            )?;

            log::info!(
                "Archived session {} ({}–{}, ${:.2}, +{}-{} lines)",
                session_id,
                d.start_time,
                d.last_updated,
                d.cost,
                d.lines_added,
                d.lines_removed
            );
        }

        Ok(())
    }

    fn update_session_tx(
        &self,
        tx: &Transaction,
        session_id: &str,
        update: SessionUpdate,
    ) -> Result<AppliedUpdate> {
        let now = current_timestamp();
        let today = current_date();
        let month = current_month();

        // Extract values from update struct
        let cost = update.cost;
        let lines_added = update.lines_added;
        let lines_removed = update.lines_removed;
        let model_name = update.model_name.as_deref();
        let workspace_dir = update.workspace_dir.as_deref();
        let device_id = update.device_id.as_deref();

        // Extract token breakdown values (0 if not provided)
        let (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) = update
            .token_breakdown
            .as_ref()
            .map(|tb| {
                (
                    tb.input_tokens as i64,
                    tb.output_tokens as i64,
                    tb.cache_read_tokens as i64,
                    tb.cache_creation_tokens as i64,
                )
            })
            .unwrap_or((0, 0, 0, 0));

        // Calculate active_time_seconds and last_activity based on burn_rate mode
        let config = crate::config::get_config();

        // AUTO_RESET MODE: Check for inactivity and archive/reset session if threshold exceeded
        // IMPORTANT: This must happen BEFORE delta calculation so that archived sessions
        // are treated as new sessions (delta = full value, not negative difference)
        if config.burn_rate.mode == "auto_reset" {
            // Query last_activity for this session
            let last_activity: Option<String> = tx
                .query_row(
                    "SELECT last_activity FROM sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .ok();

            if let Some(last_activity_str) = last_activity {
                // Calculate time since last activity
                if let Some(last_activity_unix) =
                    crate::utils::parse_iso8601_to_unix(&last_activity_str)
                {
                    if let Ok(now_duration) =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    {
                        let now_unix = now_duration.as_secs();
                        let time_since_last = now_unix.saturating_sub(last_activity_unix);
                        let threshold_seconds =
                            (config.burn_rate.inactivity_threshold_minutes as u64) * 60;

                        if time_since_last >= threshold_seconds {
                            // INACTIVITY THRESHOLD EXCEEDED - ARCHIVE AND RESET SESSION
                            log::info!(
                                "Auto-reset triggered for session {} ({} seconds idle, threshold {} seconds)",
                                session_id,
                                time_since_last,
                                threshold_seconds
                            );

                            // Archive the session (preserves work period history)
                            Self::archive_session(tx, session_id)?;

                            // Delete from sessions table (UPSERT below will recreate as new session)
                            tx.execute(
                                "DELETE FROM sessions WHERE session_id = ?1",
                                params![session_id],
                            )?;

                            log::info!("Session {} archived and reset", session_id);
                        }
                    }
                }
            }
        }

        // Calculate deltas AFTER the auto_reset check (an archived session has no live row).
        // Token columns keep their own handling (D-16); cost/lines go through the classifier.
        let old_row: Option<(PriorAccounting, i64, i64, i64, i64)> = tx
            .query_row(
                "SELECT cost, lines_added, lines_removed,
                        COALESCE(raw_cost, cost), COALESCE(raw_lines_added, lines_added),
                        COALESCE(raw_lines_removed, lines_removed), process_start_ms,
                        COALESCE(total_input_tokens, 0), COALESCE(total_output_tokens, 0),
                        COALESCE(total_cache_read_tokens, 0), COALESCE(total_cache_creation_tokens, 0)
                 FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        prior_from_row(row)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ))
                },
            )
            .optional()?;

        // Prior accounting state plus token deltas.
        let (
            prior,
            input_tokens_delta,
            output_tokens_delta,
            cache_read_tokens_delta,
            cache_creation_tokens_delta,
        ) = if let Some((prior, old_input, old_output, old_cache_read, old_cache_creation)) =
            old_row
        {
            // Live session.
            // IMPORTANT: Token deltas must be non-negative because:
            // - Transcript parser only reads last N lines (buffer_lines)
            // - For long sessions, older messages scroll out of buffer
            // - This can cause transcript sum < DB stored value (false "decrease")
            // - Negative deltas would incorrectly subtract from daily/monthly totals
            // Solution: clamp token deltas to 0 minimum
            (
                Some(prior),
                (input_tokens - old_input).max(0),
                (output_tokens - old_output).max(0),
                (cache_read_tokens - old_cache_read).max(0),
                (cache_creation_tokens - old_cache_creation).max(0),
            )
        } else if config.burn_rate.mode == "auto_reset" {
            // Session was just archived and deleted (or never existed): classify against
            // the latest archived raw baseline and carry its accounted totals (D-09, D-18).
            // "Latest" is the AUTOINCREMENT id (insertion order), not `archived_at`: that is
            // a local-offset RFC 3339 string, which does not sort chronologically across a
            // DST fall-back or on equal-second ties (WR-03).
            //
            // TOKEN BEHAVIOR IN AUTO_RESET MODE:
            // session_archive doesn't track tokens, so token deltas use full values after reset.
            // This means:
            // - Daily/monthly token totals will JUMP after auto-reset because the full
            //   session token count is added as if it were a delta (no baseline to subtract)
            // - Example: Session has 50K tokens, auto-resets, then accumulates 10K more.
            //   Daily total gets +50K (full) + +10K (delta) = 60K instead of just 60K cumulative
            // - For most use cases (tracking daily consumption), this is acceptable since
            //   tokens typically accumulate within a single work period before reset
            // - Cost and lines use archived baselines so they don't have this issue
            // - If precise token continuity is needed, consider using wall_clock mode instead
            let archived: Option<PriorAccounting> = tx
                .query_row(
                    "SELECT cost, lines_added, lines_removed,
                                COALESCE(raw_cost, cost), COALESCE(raw_lines_added, lines_added),
                                COALESCE(raw_lines_removed, lines_removed), process_start_ms
                         FROM session_archive
                         WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
                    params![session_id],
                    prior_from_row,
                )
                .optional()?;
            (
                archived,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            )
        } else {
            // New session: no baseline, full values.
            (
                None,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            )
        };

        // COST/LINES: one classification per update via the shared policy in
        // `stats::accounting` (restart by inferred start, high-water fallback, stale updates
        // contribute nothing; D-01, D-02, D-03, D-17). The stored session totals are the
        // cumulative accounted values: prior accounted + delta (D-05, D-06, D-18).
        let (base_cost, base_lines_added, base_lines_removed) = prior
            .as_ref()
            .map(|p| {
                (
                    p.accounted_cost,
                    p.accounted_lines_added,
                    p.accounted_lines_removed,
                )
            })
            .unwrap_or((0.0, 0, 0));
        let obs = Observation {
            cost,
            lines_added: i64::try_from(lines_added).unwrap_or(i64::MAX),
            lines_removed: i64::try_from(lines_removed).unwrap_or(i64::MAX),
            start_ms: update.inferred_start_ms,
        };
        let outcome = accounting::classify(
            prior.map(|p| p.baseline),
            obs,
            accounting::RESTART_TOLERANCE_MS,
        );
        log::trace!(
            "accounting: session {} classified as {:?} (cost delta {:.6})",
            session_id,
            outcome.kind,
            outcome.cost_delta
        );
        let cost_delta = outcome.cost_delta;
        let lines_added_delta = outcome.lines_added_delta;
        let lines_removed_delta = outcome.lines_removed_delta;
        let new_cost = base_cost + cost_delta;
        let new_lines_added = base_lines_added.saturating_add(lines_added_delta);
        let new_lines_removed = base_lines_removed.saturating_add(lines_removed_delta);

        let (active_time_to_save, last_activity_to_save) = if config.burn_rate.mode == "active_time"
        {
            // Query existing active_time_seconds and last_activity for this session
            let (old_active_time, old_last_activity): (Option<i64>, Option<String>) = tx
                .query_row(
                    "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| Ok((row.get(0).ok(), row.get(1).ok())),
                )
                .unwrap_or((None, None));

            let current_active_time = old_active_time.unwrap_or(0) as u64;

            // Calculate time since last activity
            if let Some(last_activity_str) = old_last_activity {
                if let Some(last_activity_unix) =
                    crate::utils::parse_iso8601_to_unix(&last_activity_str)
                {
                    if let Ok(now_duration) =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    {
                        let now_unix = now_duration.as_secs();
                        let time_since_last = now_unix.saturating_sub(last_activity_unix);

                        // Only add to active time if less than inactivity threshold
                        let threshold_seconds =
                            (config.burn_rate.inactivity_threshold_minutes as u64) * 60;
                        let new_active_time = if time_since_last < threshold_seconds {
                            // Active conversation - add the delta
                            current_active_time + time_since_last
                        } else {
                            // Idle period - don't add to active time
                            current_active_time
                        };

                        (Some(new_active_time as i64), now.clone())
                    } else {
                        // Can't get current time - keep existing
                        (Some(current_active_time as i64), now.clone())
                    }
                } else {
                    // Can't parse last_activity - keep existing
                    (Some(current_active_time as i64), now.clone())
                }
            } else {
                // No previous activity - this is the first update
                (Some(0), now.clone())
            }
        } else if config.burn_rate.mode == "auto_reset" {
            // For auto_reset mode: Track last_activity for inactivity detection
            // After reset, session starts fresh (active_time from update struct or None)
            (update.active_time_seconds.map(|t| t as i64), now.clone())
        } else {
            // For wall_clock mode: Use values from update struct or defaults
            (
                update.active_time_seconds.map(|t| t as i64),
                update.last_activity.clone().unwrap_or_else(|| now.clone()),
            )
        };

        // Convert max_tokens_observed to i64 for SQLite
        let max_tokens = update.max_tokens_observed.map(|t| t as i64);

        // Convert active_time_seconds to i64 for SQLite
        let active_time = active_time_to_save;

        // Use calculated last_activity
        let last_activity = &last_activity_to_save;

        // UPSERT session (atomic operation)
        // Token handling: Use MAX to preserve cumulative totals when older messages scroll
        // out of the transcript buffer. Transcript parser only reads last N lines, so token
        // counts can appear to "decrease". Using MAX ensures we never lose previously counted tokens.
        tx.execute(
            "INSERT INTO sessions (
                session_id, start_time, last_updated, cost, lines_added, lines_removed,
                model_name, workspace_dir, device_id,
                total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens,
                max_tokens_observed, active_time_seconds, last_activity,
                raw_cost, raw_lines_added, raw_lines_removed, process_start_ms
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20)
             ON CONFLICT(session_id) DO UPDATE SET
                last_updated = ?3,
                cost = cost + ?21,
                lines_added = lines_added + ?22,
                lines_removed = lines_removed + ?23,
                raw_cost = ?17,
                raw_lines_added = ?18,
                raw_lines_removed = ?19,
                process_start_ms = COALESCE(?20, process_start_ms),
                model_name = ?7,
                workspace_dir = ?8,
                device_id = ?9,
                total_input_tokens = MAX(COALESCE(total_input_tokens, 0), ?10),
                total_output_tokens = MAX(COALESCE(total_output_tokens, 0), ?11),
                total_cache_read_tokens = MAX(COALESCE(total_cache_read_tokens, 0), ?12),
                total_cache_creation_tokens = MAX(COALESCE(total_cache_creation_tokens, 0), ?13),
                max_tokens_observed = CASE
                    WHEN ?14 IS NOT NULL AND ?14 > COALESCE(max_tokens_observed, 0)
                    THEN ?14
                    ELSE max_tokens_observed
                END,
                active_time_seconds = COALESCE(?15, active_time_seconds),
                last_activity = COALESCE(?16, last_activity)",
            params![
                session_id, &now, &now, new_cost, new_lines_added, new_lines_removed,
                model_name, workspace_dir, device_id,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                max_tokens, active_time, last_activity,
                outcome.next.raw_cost, outcome.next.raw_lines_added, outcome.next.raw_lines_removed,
                outcome.next.start_ms,
                cost_delta, lines_added_delta, lines_removed_delta
            ],
        )?;

        // Proper session counting: We need to track which sessions we've seen for each period
        // Since we don't have a junction table, we'll use the session_count field itself
        // as a counter that gets SET (not incremented) based on actual distinct sessions

        // For daily: count distinct sessions that have been updated today
        // We determine "updated today" by checking if last_updated matches today's date
        // Use 'localtime' modifier to ensure timezone consistency with current_date()
        let daily_session_count: i64 = tx
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM sessions
                 WHERE date(last_updated, 'localtime') = ?1",
                params![&today],
                |row| row.get(0),
            )
            .unwrap_or(1); // Default to 1 (this session) if query fails

        // For monthly: count distinct sessions updated this month
        // Use 'localtime' modifier to ensure timezone consistency with current_month()
        let monthly_session_count: i64 = tx
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM sessions
                 WHERE strftime('%Y-%m', last_updated, 'localtime') = ?1",
                params![&month],
                |row| row.get(0),
            )
            .unwrap_or(1);

        // Update daily stats atomically with delta values
        // Note: session_count is SET (not incremented) to the actual count of distinct sessions
        tx.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count,
                                      total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(date) DO UPDATE SET
                total_cost = total_cost + ?2,
                total_lines_added = total_lines_added + ?3,
                total_lines_removed = total_lines_removed + ?4,
                session_count = ?5,
                total_input_tokens = COALESCE(total_input_tokens, 0) + ?6,
                total_output_tokens = COALESCE(total_output_tokens, 0) + ?7,
                total_cache_read_tokens = COALESCE(total_cache_read_tokens, 0) + ?8,
                total_cache_creation_tokens = COALESCE(total_cache_creation_tokens, 0) + ?9",
            params![&today, cost_delta, lines_added_delta, lines_removed_delta, daily_session_count,
                    input_tokens_delta, output_tokens_delta, cache_read_tokens_delta, cache_creation_tokens_delta],
        )?;

        // Update monthly stats atomically with delta values
        // Note: session_count is SET (not incremented) to the actual count of distinct sessions
        tx.execute(
            "INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count,
                                        total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(month) DO UPDATE SET
                total_cost = total_cost + ?2,
                total_lines_added = total_lines_added + ?3,
                total_lines_removed = total_lines_removed + ?4,
                session_count = ?5,
                total_input_tokens = COALESCE(total_input_tokens, 0) + ?6,
                total_output_tokens = COALESCE(total_output_tokens, 0) + ?7,
                total_cache_read_tokens = COALESCE(total_cache_read_tokens, 0) + ?8,
                total_cache_creation_tokens = COALESCE(total_cache_creation_tokens, 0) + ?9",
            params![&month, cost_delta, lines_added_delta, lines_removed_delta, monthly_session_count,
                    input_tokens_delta, output_tokens_delta, cache_read_tokens_delta, cache_creation_tokens_delta],
        )?;

        // Get totals for return
        let day_total: f64 = tx
            .query_row(
                "SELECT total_cost FROM daily_stats WHERE date = ?1",
                params![&today],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        let month_total: f64 = tx
            .query_row(
                "SELECT total_cost FROM monthly_stats WHERE month = ?1",
                params![&month],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        let (session_total, session_lines_added, session_lines_removed): (f64, i64, i64) = tx
            .query_row(
                "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((0.0, 0, 0));

        Ok(AppliedUpdate {
            day_total,
            month_total,
            session_total,
            session_lines_added,
            session_lines_removed,
            cost_delta,
            lines_added_delta,
            lines_removed_delta,
            kind: outcome.kind,
            baseline: outcome.next,
        })
    }

    /// Get session duration in seconds
    #[allow(dead_code)]
    pub fn get_session_duration(&self, session_id: &str) -> Option<u64> {
        let conn = self.get_connection().ok()?;

        let start_time: String = conn
            .query_row(
                "SELECT start_time FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .ok()?;

        // Parse ISO 8601 timestamp
        if let Ok(start) = chrono::DateTime::parse_from_rfc3339(&start_time) {
            let now = chrono::Local::now();
            let duration = now.signed_duration_since(start);
            Some(duration.num_seconds() as u64)
        } else {
            None
        }
    }

    /// Get max tokens observed for a specific session (for compaction detection)
    pub fn get_session_max_tokens(&self, session_id: &str) -> Option<usize> {
        let conn = self.get_connection().ok()?;
        let max_tokens: i64 = conn
            .query_row(
                "SELECT max_tokens_observed FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .ok()?;
        Some(max_tokens as usize)
    }

    /// Reset max_tokens_observed for a session after compaction completes
    ///
    /// This allows Phase 2 heuristic compaction detection to start fresh
    /// after a compaction event, preventing false positive "Compacting..." states.
    ///
    /// Called by the PostCompact hook handler (via SessionStart[compact]).
    pub fn reset_session_max_tokens(&self, session_id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE sessions SET max_tokens_observed = 0 WHERE session_id = ?1",
            params![session_id],
        )?;
        log::debug!("Reset max_tokens_observed to 0 for session {}", session_id);
        Ok(())
    }

    /// Reset max_tokens_observed for ALL sessions
    ///
    /// Workaround for Claude Code bug #9567 where hooks receive empty session_id.
    /// Since only one session compacts at a time, resetting all is safe.
    /// The tracking will rebuild on the next statusline call.
    ///
    /// Returns the number of sessions affected.
    pub fn reset_all_sessions_max_tokens(&self) -> Result<usize> {
        let conn = self.get_connection()?;
        let rows_affected = conn.execute("UPDATE sessions SET max_tokens_observed = 0", [])?;
        log::info!(
            "Reset max_tokens_observed to 0 for all {} sessions (empty session_id workaround)",
            rows_affected
        );
        Ok(rows_affected)
    }

    /// Get the start_time for a session (ISO 8601 string)
    ///
    /// Used by auto-reset desync detection to compare in-memory vs database start times.
    pub fn get_session_start_time(&self, session_id: &str) -> Option<String> {
        let conn = self.get_connection().ok()?;
        conn.query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// Get active_time_seconds and last_activity for a session
    ///
    /// Returns (active_time_seconds, last_activity) for JSON backup persistence
    /// and active_time burn rate mode.
    pub fn get_session_active_time(
        &self,
        session_id: &str,
    ) -> Option<(Option<u64>, Option<String>)> {
        let conn = self.get_connection().ok()?;
        conn.query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| {
                let active_time: Option<i64> = row.get(0).ok();
                let last_activity: Option<String> = row.get(1).ok();
                Ok((active_time.map(|t| t as u64), last_activity))
            },
        )
        .ok()
    }

    /// Get token breakdown for a session
    ///
    /// Returns (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens)
    /// Returns None if any token count is negative (DB corruption/migration edge case)
    /// NULL values are treated as 0 via COALESCE.
    pub fn get_session_token_breakdown(&self, session_id: &str) -> Option<(u32, u32, u32, u32)> {
        let conn = self.get_connection().ok()?;
        conn.query_row(
            "SELECT COALESCE(total_input_tokens, 0), COALESCE(total_output_tokens, 0), COALESCE(total_cache_read_tokens, 0), COALESCE(total_cache_creation_tokens, 0)
             FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| {
                let input: i64 = row.get(0)?;
                let output: i64 = row.get(1)?;
                let cache_read: i64 = row.get(2)?;
                let cache_creation: i64 = row.get(3)?;

                // Guard against negative values (DB corruption/migration edge cases)
                if input < 0 || output < 0 || cache_read < 0 || cache_creation < 0 {
                    log::warn!(
                        "Negative token values detected for session {}: input={}, output={}, cache_read={}, cache_creation={}",
                        session_id, input, output, cache_read, cache_creation
                    );
                    return Err(rusqlite::Error::InvalidQuery);
                }

                Ok((input as u32, output as u32, cache_read as u32, cache_creation as u32))
            },
        )
        .ok()
    }

    /// Import sessions from JSON stats data (for migration)
    pub fn import_sessions(
        &self,
        sessions: &std::collections::HashMap<String, crate::stats::SessionStats>,
    ) -> Result<()> {
        let mut conn = self.get_connection()?;
        let tx = conn.transaction()?;

        for (session_id, session) in sessions.iter() {
            // Insert session (don't use UPSERT, just INSERT as this is initial import)
            tx.execute(
                "INSERT OR IGNORE INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed, active_time_seconds, last_activity)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session_id,
                    session.start_time.as_deref().unwrap_or(""),
                    &session.last_updated,
                    session.cost,
                    session.lines_added as i64,
                    session.lines_removed as i64,
                    session.active_time_seconds.map(|t| t as i64),
                    session.last_activity.as_deref(),
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }
}
