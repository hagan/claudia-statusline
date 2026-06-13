# Key Flows — Claudia Statusline

> **Source:** Derived from `.understand-anything/knowledge-graph.json` and
> `docs/architecture/project-map.md`, with call sequences verified against the source at
> commit `ac29509` (crate version 3.0.0).
>
> Function and type names below are real symbols. Paths are repository-relative. This document
> is descriptive only; it does not propose changes.

---

## 1. App startup flow

Entry: `fn main() -> Result<()>` in `src/main.rs`.

1. **Parse CLI** — `Cli::parse()` (clap derive). Flags include `--log-level`, `--no-color`,
   `--theme`, `--config`, `--test-mode`, `--version-full`, `--list-vars`.
2. **Apply environment precedence (CLI > env > config)** — `main` writes resolved values into
   process env vars so downstream loaders pick them up:
   - `--log-level` → `RUST_LOG`, then `env_logger::Builder::from_env(...).default_filter_or("warn").init()`.
   - `--no-color` → `NO_COLOR=1`.
   - `--theme` → `CLAUDE_THEME` and `STATUSLINE_THEME`.
   - `--config` → `STATUSLINE_CONFIG_PATH`.
   - `--test-mode` → `STATUSLINE_TEST_MODE=1` and a redirected `XDG_DATA_HOME`
     (`~/.local/share-test`) so the run uses an isolated database.
3. **Early-return modes** (no stdin read):
   - `--version-full` → prints `version_string()` and returns.
   - `--list-vars` → `handle_list_vars(&cli)` (runs all providers, prints variables by source).
4. **Subcommand dispatch** — `match cli.command`:
   - `Commands::GenerateConfig` → writes `config::Config::example_toml()` to
     `Config::default_config_path()` (dir `0o700`, file `0o600` on Unix).
   - `Commands::Migrate { .. }` → `dump_database_schema()` / `run_schema_migrations()` /
     `finalize_migration(delete_json)` / `show_migration_roadmap()`.
   - `Commands::DbMaintain { .. }` → `perform_database_maintenance(...)`.
   - `Commands::Health { json }` → `show_health_report(json)`.
   - `Commands::Sync { .. }` → `handle_sync_command(...)` (only compiled with the
     `turso-sync` feature).
   - `Commands::ContextLearning { .. }` → `handle_context_learning_command(...)`.
   - `Commands::Hook { action }` → `handle_hook_command(action)` (see flow 2b).
5. **Default path (no subcommand)** — falls through to the request/render flow below.

---

## 2. Request / render flow

This is the primary path: stdin JSON → rendered status line on stdout.

### 2a. Default binary path (`src/main.rs`, tail of `main`)

1. **Read stdin** — `io::stdin().read_to_string(&mut buffer)`.
2. **Parse** — `serde_json::from_str::<StatuslineInput>(&buffer)`; on parse error it logs a
   warning and uses `StatuslineInput::default()` (never fails the line).
   `StatuslineInput` and related models live in `src/models.rs` (e.g. `ModelType`,
   `TokenBreakdown`, with `from_name` / `extract_version` / `abbreviation`).
3. **Resolve current directory** — from `input.workspace.current_dir`, else `env::current_dir()`,
   else `"~"`. **Early exit:** if empty or `"~"`, print just the directory segment and return.
4. **Update stats (if `session_id` + `cost.total_cost_usd` present)** — builds a
   `database::SessionUpdate` (cost, lines added/removed, model name, workspace dir, device id
   from `common::get_device_id()`, token breakdown from
   `utils::get_token_breakdown_from_transcript(path)`) and calls
   `update_stats_data(|data| data.update_session(session_id, SessionUpdate { .. }))`
   (see flow 3).
5. **Track context tokens** — `utils::get_token_count_from_transcript(...)` →
   `stats::update_stats_data(|data| data.update_max_tokens(session, current_tokens))`; this
   feeds compaction detection and (if enabled) adaptive learning (flow 2c).
6. **Collect provider variables** — the `ProviderOrchestrator` in `src/provider/mod.rs`
   (`collect_all`, `register`) gathers data from registered `DataProvider` implementors,
   including `GsdProvider` (`src/gsd/mod.rs`) and the stats provider.
7. **Gather git status** — `src/git.rs`: `get_git_status()` →
   `parse_git_status()` / `parse_git_status_v2()` → `apply_status_codes()` →
   `format_git_info()` (returns a `GitStatus`).
8. **Render** — `src/display.rs`: `format_statusline_string()` /
   `format_statusline_with_layout()` / `format_output_with_config()` assemble the segments,
   using `format_context_bar()`, `format_token_rates()`, `context_color()`, `cost_color()`,
   and `Colors`. Theme/layout resolution comes from `src/theme.rs`
   (`ThemeManager`, `resolve_color`, `load_embedded`, `list_themes`) and `src/layout/`
   (presets/template/variables, default template `src/templates/default.tmpl`).
9. **Output** — the assembled string is printed to stdout.

### 2b. Library API path (`src/lib.rs`)

`pub fn render_statusline(input: &StatuslineInput, update_stats: bool) -> Result<String>`
performs the same logic as 2a steps 3–8 but returns the `String` instead of printing. It is the
public embedding entry point exercised by `examples/embedding_example.rs`. When
`update_stats` is true and a `session_id` is present, it calls
`stats::update_stats_data(|data| data.update_session(..))`; otherwise it loads existing totals
via `stats::get_or_load_stats_data()` + `stats::get_daily_total(..)`.

### 2c. Hook path (`src/hook_handler.rs`)

Triggered by `Commands::Hook`. Functions: `handle_precompact`, `handle_postcompact`,
`clear_state_file_directly`. This is the experimental, low-latency compaction-detection path
that updates state directly rather than going through the full render. Adaptive learning is
driven from `src/context_learning.rs` (`ContextLearner`: `observe_usage`,
`is_compaction_event`, `is_manual_compaction`, `record_compaction`,
`update_ceiling_observation`, `get_learned_window`, `rebuild_from_sessions`).

---

## 3. Data persistence flow

Backed by bundled SQLite via `rusqlite`. JSON is no longer written as of v3.0.0; legacy
`stats.json` is read once and migrated in.

### Write path

`stats::update_stats_data<F>(updater)` in `src/stats/persistence.rs`:

1. **Load** — `StatsData::load_from_sqlite()`; on failure, falls back to `StatsData::load()`,
   which reads any legacy `stats.json` and migrates it into SQLite (rather than starting from
   `default()`, which would drop a v2.x user's history).
2. **Apply** — runs the caller's `updater(&mut stats_data)` closure (e.g.
   `StatsData::update_session` / `update_max_tokens` from `src/stats/session.rs` and
   `src/stats/mod.rs`).
3. **Persist** — `perform_sqlite_dual_write(&stats_data)` writes back to SQLite (primary
   storage). Returns the `(daily_total, monthly_total)` tuple from the closure.

### Database layer (`src/database/`)

- `SqliteDatabase::new()` (`src/database/mod.rs`) opens/creates the database and obtains a
  connection (`get_connection`).
- On open, migrations run via `migrations::run_migrations_on_db` (`src/migrations/mod.rs`),
  driven by `MigrationRunner` / `Migration::migrate`. Schema DDL lives in
  `src/database/schema.rs` (the embedded `SCHEMA` const, `SessionUpdate`).
- Session writes: `src/database/session.rs` — `update_session`, `update_session_tx`
  (transactional), `update_max_tokens_observed`, `archive_session`,
  `get_session_active_time`, `get_session_token_breakdown`, `import_sessions`.
  `active_time_seconds` / `last_activity` are owned and computed inside
  `SqliteDatabase::update_session`, not at the `SessionUpdate` construction site.
- Aggregates and maintenance: `src/database/daily.rs`, `src/database/monthly.rs`,
  `src/database/analytics.rs`, `src/database/maintenance.rs`, plus `src/stats/aggregation.rs`
  and `src/stats/cache.rs`.

### Cloud sync (optional, `turso-sync` feature)

`src/sync.rs` — `SyncManager` with `push` / `pull` (and async `push_to_turso_async` /
`pull_from_turso_async`), reporting via `SyncStatus`. Network/transient failures use the
retry helper in `src/retry.rs`. The remote Turso schema is in
`scripts/setup-turso-schema.sql`, applied by `scripts/setup-turso.sh`.

---

## 4. Config and auth flow

### Configuration (`src/config.rs`)

- **Cached load** — `config::get_config() -> &'static Config` initializes a process-global
  `OnceCell` (`CONFIG.get_or_init`). On first call it runs `Config::load()` (falling back to
  `Config::default()` with a warning on error), then layers **environment overrides** on top,
  e.g. `CLAUDE_THEME` (then `STATUSLINE_THEME`) → `config.display.theme`,
  `STATUSLINE_SHOW_CONTEXT_TOKENS`, `STATUSLINE_BURN_RATE_MODE`,
  `STATUSLINE_BURN_RATE_THRESHOLD`, `STATUSLINE_BURN_RATE_MIN_DURATION`,
  `STATUSLINE_TOKEN_RATE_ENABLED`, and more. Net precedence is **CLI flag → env var →
  config file → default** (the CLI layer is applied in `main` per flow 1).
- **File discovery / IO** — `find_config_file()` locates the TOML config; `load_from_file()`
  parses it; `save()` writes it; `example_toml()` renders the documented example;
  `default_config_path()` resolves the canonical location. `get_effective_threshold()` resolves
  the active context threshold.
- **Config types** — `Config` plus the nested `DisplayConfig`, `ContextConfig`, `CostConfig`,
  `DatabaseConfig`, `RetryConfig` / `RetrySettings`, `BurnRateConfig`, `LayoutConfig`,
  `ComponentsConfig` (and the per-component `*ComponentConfig` types), `TokenRateConfig`,
  and `SyncConfig`.

### Auth (sync token resolution)

There is no end-user authentication; the only credential handling is the Turso sync token,
resolved by `SyncManager::resolve_auth_token(&self, token_config)` in `src/sync.rs`:

- `${VAR_NAME}` → reads env var `VAR_NAME` (errors `StatuslineError::Sync` if unset).
- `$VAR_NAME` → same, prefix form.
- otherwise → the literal string is used as the token directly.
- empty → empty token (sync effectively disabled).

This indirection keeps secrets out of the committed config file.

---

## 5. Test / build / deploy flow

### Test

- **Integration tests** — `tests/` (burn-rate scenarios, `sqlite_integration_tests.rs`,
  `theme_integration.rs`, `layout_integration_tests.rs`, `display_config_integration.rs`,
  `hook_integration_tests.rs`, `lib_api_tests.rs`, `context_tokens_display_tests.rs`,
  `db_maintenance_tests.rs`, `regression_tests.rs`, property tests `proptest_tests.rs`).
  Shared harness: `tests/test_support.rs` (imported by 21 integration test files).
- **Inline unit tests** — `src/database/tests.rs`, `src/stats/tests.rs`, `src/gsd/tests.rs`,
  `src/layout/tests.rs`, and the mock `src/provider/test_provider.rs`.
- **Run:** `make test` (unit + integration), `make test-sqlite`, `make test-all`,
  `make test-manual` (isolated test DB), or `cargo test`. `--test-mode` / `STATUSLINE_TEST_MODE`
  redirects `XDG_DATA_HOME` so tests never touch the production database
  (`make show-db-path` shows prod vs test paths).

### Build

- **Build script** — `build.rs` runs at compile time and reads `VERSION` (graph edge
  `VERSION → build.rs`, type `configures`).
- **Commands** — `make build` / `make release` / `make debug` / `make install`
  (installs to `~/.local/bin`), or `cargo build --release`. Optional cloud sync:
  `cargo build --release --features turso-sync`.
- **Release profile** (`Cargo.toml`): `opt-level = "z"`, `lto = true`, `codegen-units = 1`,
  `strip = true`, `panic = "abort"`. Quality gates: `make check-code` (`rustfmt` + `clippy`),
  `make lint`, `make fmt`; dependency policy in `deny.toml`.

### Deploy / release

- **CI** — GitHub Actions in `.github/workflows/`: `build`, `test`, `security`, `release`,
  `test-binaries`, `test-binaries-fixed`. Per the graph, `build`/`release` jobs run
  `cargo build --release` (modeled as `deploys → src/main.rs`); `test-binaries*` are
  `workflow_run`-triggered after `build` completes (`depends_on → build`).
- **Release tooling** — `scripts/release.sh` (runs/bundles `scripts/install-statusline.sh`),
  `scripts/bump-version.sh`, and Make targets `bump-major` / `bump-minor` / `bump-patch`,
  `tag`, `release-build`.
- **End-user install** — `scripts/quick-install.sh` (the `curl | bash` path in `README.md`)
  downloads a prebuilt binary; `scripts/install-statusline.sh` installs from a local build;
  `scripts/uninstall-statusline.sh` removes it.

---

*See `docs/architecture/project-map.md` for the module/layer map, dependency centrality, and
the list of central/risky files referenced above.*
