-- Synthetic seed data for the frozen schema v6 fixture (schema_v6_v3.3.0.sql).
-- Hand-written; nothing here is derived from a real database (the repo is public).
-- Uses only columns that exist in the v3.3.0 (v6) SCHEMA.

INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed,
    max_tokens_observed, device_id, sync_timestamp, model_name, workspace_dir,
    total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens,
    active_time_seconds, last_activity)
VALUES ('synthetic-session-a', '2026-01-01T09:00:00+00:00', '2026-01-01T10:30:00+00:00', 1.5, 120, 30,
    85000, 'synthetic-device-1', 1767261000, 'Claude Opus 4', '/tmp/synthetic/alpha',
    1000, 2000, 30000, 4000, 3600, '2026-01-01T10:30:00+00:00');

INSERT INTO sessions (session_id, start_time, last_updated)
VALUES ('synthetic-session-b', '2026-01-02T08:00:00+00:00', '2026-01-02T08:05:00+00:00');

INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed,
    max_tokens_observed, device_id, model_name, workspace_dir,
    total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens,
    active_time_seconds, last_activity)
VALUES ('synthetic-session-c', '2026-02-15T12:00:00+00:00', '2026-02-15T18:45:00+00:00', 12.345678, 98765, 4321,
    199999, 'synthetic-device-2', 'Claude Sonnet 4.5', '/tmp/synthetic/проект',
    4611686018427387903, 4611686018427387902, 4611686018427387901, 4611686018427387900,
    24300, '2026-02-15T18:45:00+00:00');

INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count,
    device_id, total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
VALUES ('2026-01-01', 1.5, 120, 30, 1, 'synthetic-device-1', 1000, 2000, 30000, 4000);

INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
VALUES ('2026-02-15', 12.345678, 98765, 4321, 1);

INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count,
    device_id, total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
VALUES ('2026-01', 1.5, 120, 30, 2, 'synthetic-device-1', 1000, 2000, 30000, 4000);

INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count)
VALUES ('2026-02', 12.345678, 98765, 4321, 1);

INSERT INTO session_archive (session_id, start_time, end_time, archived_at, cost, lines_added, lines_removed,
    active_time_seconds, last_activity, model_name, workspace_dir, device_id)
VALUES ('synthetic-session-a', '2025-12-31T09:00:00+00:00', '2025-12-31T11:00:00+00:00', '2025-12-31T16:00:00+00:00',
    0.75, 40, 10, 1800, '2025-12-31T11:00:00+00:00', 'Claude Opus 4', '/tmp/synthetic/alpha', 'synthetic-device-1');

INSERT INTO session_archive (session_id, start_time, end_time, archived_at, cost, lines_added, lines_removed)
VALUES ('synthetic-session-c', '2026-02-14T12:00:00+00:00', '2026-02-14T13:00:00+00:00', '2026-02-14T18:00:00+00:00',
    3.25, 7, 3);

INSERT INTO learned_context_windows (model_name, observed_max_tokens, ceiling_observations, compaction_count,
    last_observed_max, last_updated, confidence_score, first_seen, workspace_dir, device_id)
VALUES ('Claude Opus 4', 195000, 3, 2, 194500, '2026-01-01T10:30:00+00:00', 0.82,
    '2025-12-01T00:00:00+00:00', '/tmp/synthetic/alpha', 'synthetic-device-1');

INSERT INTO learned_context_windows (model_name, observed_max_tokens, last_observed_max, last_updated, first_seen)
VALUES ('Claude Sonnet 4.5', 150000, 150000, '2026-02-15T18:45:00+00:00', '2026-02-15T12:00:00+00:00');

INSERT OR IGNORE INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms) VALUES (6, '2026-01-01T00:00:00+00:00', '', 'New database with complete schema (v6)', 0);
