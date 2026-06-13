# Project Map — Claudia Statusline

> **Source:** Generated from `.understand-anything/knowledge-graph.json`
> (575 nodes, 767 edges, 10 layers), cross-checked against the repository.
> **Analyzed commit:** `ac29509` · **Crate version:** 3.0.0 · **Edition:** 2021
>
> This document is descriptive only. It records what exists; it does not propose changes.

## High-level overview

Claudia Statusline is a Rust **binary + library** crate (`name = "statusline"`) that renders
a feature-rich status line for Claude Code. The binary reads a session JSON payload on stdin
and prints a single formatted line showing workspace/directory, git status, model, context
usage, session duration, cost, and burn rate.

Key characteristics:

- **Persistent stats** in a bundled SQLite database (via `rusqlite`), with versioned migrations.
- **Rendering** is configurable: 11 embedded themes, 5 layout presets, and custom templates.
- **Extension surface** via a `DataProvider` trait and an orchestrator, including GSD
  (`.planning/`) integration.
- **Optional cloud sync** to Turso, gated behind the `turso-sync` Cargo feature.
- **Experimental** features: hook-based compaction detection and adaptive context learning.

Languages present: Rust (primary), TOML, Markdown, shell, SQL, YAML, JSON.
Notable dependencies: `serde`/`serde_json`, `rusqlite` (bundled SQLite), `clap`, `chrono`,
`toml`, `thiserror`, `regex`, `dirs`, `fs2`, `log`/`env_logger`, `sha2`, `hostname`; optional
`libsql` + `tokio` for the `turso-sync` feature.

## Architectural layers

The knowledge graph organizes the repository into 10 layers:

| Layer | Node count | What it contains |
|---|---:|---|
| Application & Entry | 4 | `main.rs`, `lib.rs`, `hook_handler.rs`, `build.rs` |
| Rendering & Display | 20 | `display.rs`, `theme.rs`, `layout/*`, `themes/*.toml`, `templates/default.tmpl` |
| Domain & Stats | 11 | `stats/*`, `models.rs`, `context_learning.rs`, `state.rs`, `common.rs` |
| Persistence & Sync | 21 | `database/*`, `migrations/*`, `sync.rs`, `retry.rs`, Turso schema SQL + table nodes |
| Git Integration | 3 | `git.rs`, `git_provider.rs`, `git_utils.rs` |
| Provider & GSD Integrations | 9 | `provider/*`, `gsd/*` |
| Configuration & Foundations | 9 | `config.rs`, `error.rs`, `version.rs`, `utils.rs`, `Cargo.toml`, `deny.toml`, `VERSION` |
| CI/CD & Build | 53 | 6 GitHub Actions workflows + jobs, `Makefile` + targets, install/release shell scripts |
| Tests & Examples | 34 | `tests/*` integration/property/regression tests, `examples/*` Turso programs |
| Documentation | 17 | root + `docs/*.md`, `NOTICE` |

## Major modules

| Module | Path | Responsibility |
|---|---|---|
| **Entry / orchestration** | `src/main.rs`, `src/lib.rs`, `src/hook_handler.rs` | CLI binary, public library API, Claude Code hook handling |
| **Rendering & display** | `src/display.rs`, `src/theme.rs`, `src/layout/` (`format`, `presets`, `template`, `variables`, `mod`) | Assembles the status-line string; themes, presets, custom templates |
| **Domain & stats** | `src/stats/` (`aggregation`, `cache`, `persistence`, `session`, `provider`, `mod`), `src/models.rs`, `src/context_learning.rs`, `src/state.rs` | Cost / burn-rate / context math, session state, adaptive learning |
| **Persistence & sync** | `src/database/` (`schema`, `session`, `daily`, `monthly`, `analytics`, `maintenance`, `sync`, `mod`), `src/migrations/`, `src/sync.rs`, `src/retry.rs` | SQLite store, versioned migrations, optional Turso cloud sync, retry logic |
| **Git integration** | `src/git.rs`, `src/git_provider.rs`, `src/git_utils.rs` | Branch and working-tree status |
| **Provider & GSD** | `src/provider/` (`mod`, `test_provider`), `src/gsd/` (`roadmap`, `todos`, `state`, `config`, `update`, `mod`) | `DataProvider` trait + orchestrator; reads `.planning/` GSD project state |
| **Configuration & foundations** | `src/config.rs`, `src/common.rs`, `src/error.rs`, `src/utils.rs`, `src/version.rs` | TOML config, shared helpers, unified error type, version info |

## Key entry points

- **`src/main.rs`** (≈1,824 lines) — the `statusline` binary. Parses CLI args (clap),
  reads session JSON from stdin, orchestrates stats/git/rendering, prints the line.
  Declared in `Cargo.toml` as `[[bin]] name = "statusline"`.
- **`src/lib.rs`** — library root (`[lib] name = "statusline"`). Public embedding API,
  exercised by the programs under `examples/`.
- **`src/hook_handler.rs`** — Claude Code hook entry path (experimental real-time
  compaction detection).
- **`build.rs`** — Cargo build script; reads `VERSION` (a `configures` edge `VERSION → build.rs`
  is recorded in the graph).

## Important dependency relationships

Most depended-on `src/` files, measured by inbound `imports` / `depends_on` / `calls` edges in
the graph (rolled up to the containing file):

| File | Inbound edges | Notes |
|---|---:|---|
| `src/common.rs` | 14 | Shared helpers (e.g. data-dir resolution) — foundational hub |
| `src/error.rs` | 14 | Unified `thiserror`-based error type — foundational hub |
| `src/provider/mod.rs` | 8 | `DataProvider` trait + orchestrator; the extension seam |
| `src/config.rs` | 7 | TOML configuration consumed across modules |
| `src/database/mod.rs` | 6 | SQLite database façade |
| `src/utils.rs` | 6 | General utilities (incl. terminal-output sanitization) |
| `src/retry.rs` | 5 | Retry helper used by git/sync paths |
| `src/models.rs` | 5 | Session data models (serde) |
| `src/git.rs` | 5 | Git status collection |
| `src/gsd/roadmap.rs` | 5 | GSD roadmap parsing |

Structural facts recorded in the graph:

- The `DataProvider` trait in `src/provider/mod.rs` is implemented by `GsdProvider`
  (`src/gsd/mod.rs`), `StatsProvider` (`src/stats/provider.rs`), and `TestProvider`
  (`src/provider/test_provider.rs`); `ProviderOrchestrator` `depends_on` `DataProvider`.
- `src/database/mod.rs::new` calls `migrations::run_migrations_on_db`; `stats` session
  updates call into `database/session`; maintenance calls `common::get_data_dir`.
- `scripts/setup-turso-schema.sql` is applied by `scripts/setup-turso.sh` (`migrates` edge).
- `src/lib.rs` has no internal inbound import edges — expected for a crate root.

### Request / data flow

```
stdin JSON ──► src/main.rs (clap args + parse)
   ├─► src/models.rs (deserialize session payload)
   ├─► src/stats/ (cost, burn rate, context %) ◄──► src/database/ (SQLite read/write, migrations)
   ├─► src/git.rs / src/git_utils.rs (branch + changes)
   ├─► src/provider/ orchestrator ──► src/gsd/ (.planning state), stats provider
   └─► src/display.rs (assemble segments)
          └─► src/theme.rs + src/layout/ (preset/template + variables) ──► themes/*.toml
   ──► stdout: rendered status line
```

Optional/secondary paths: `src/sync.rs` pushes stats to Turso when the `turso-sync` feature is
enabled; `src/hook_handler.rs` provides a separate fast path for compaction hook events.

## Test locations

- **`tests/`** — integration tests, including burn-rate scenarios (accumulation, auto-reset,
  long sessions, multi-day wall clock), `sqlite_integration_tests.rs`, `theme_integration.rs`,
  `layout_integration_tests.rs`, `display_config_integration.rs`, `hook_integration_tests.rs`,
  `lib_api_tests.rs`, `context_tokens_display_tests.rs`, `db_maintenance_tests.rs`,
  `regression_tests.rs`, and property tests in `proptest_tests.rs`
  (seeds in `proptest_tests.proptest-regressions`).
  - **`tests/test_support.rs`** is the shared test harness, imported by 21 of the integration
    test files — the central test-support node in the graph.
- **Inline module tests** — `src/database/tests.rs`, `src/stats/tests.rs`, `src/gsd/tests.rs`,
  `src/layout/tests.rs`, and the mock in `src/provider/test_provider.rs`.
- **`examples/`** — `setup_schema`, `inspect_turso_data`, `check_turso_version`,
  `migrate_turso`, `embedding_example`. The Turso examples are gated by the `turso-sync`
  feature (per `Cargo.toml`).

## Build / run commands

From the `Makefile` (targets present: `all build debug release install uninstall clean
test test-sqlite test-install test-all test-manual clean-test show-db-path check check-code
dev bench version bump-major bump-minor bump-patch tag size lint fmt release-build`):

| Command | Effect |
|---|---|
| `make` / `make build` | Build the release binary |
| `make debug` | Build debug binary with symbols |
| `make release` | Build optimized release binary |
| `make install` | Build and install to `~/.local/bin` |
| `make test` | Run unit and integration tests |
| `make test-sqlite` | Run SQLite integration tests |
| `make test-all` | Run all tests |
| `make test-manual` | Run against an isolated test database |
| `make check-code` | Run `rustfmt` and `clippy` |
| `make dev` | Build and run with test input |
| `make bench` | Run a performance benchmark |
| `make show-db-path` | Show production vs test database paths |

Direct Cargo equivalents (standard for this crate layout):

- `cargo build --release` — build the `statusline` binary.
- `cargo test` — run the test suite.
- `cargo build --release --features turso-sync` — build with optional Turso cloud sync.

Quick install (from `README.md`):

```bash
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

Install/release tooling lives in `scripts/` (`install-statusline.sh`, `quick-install.sh`,
`uninstall-statusline.sh`, `release.sh`, `bump-version.sh`, `maintenance.sh`,
`setup-turso.sh`, `setup-turso-schema.sql`, `toggle-debug.sh`, `test-installation.sh`).
CI is defined under `.github/workflows/` (`build`, `test`, `security`, `release`,
`test-binaries`).

> Note: the `Makefile`'s `SOURCES` variable lists a flat file set
> (`src/stats.rs`, etc.) that predates the current module-directory layout
> (`src/stats/`, `src/database/`, `src/layout/`, `src/gsd/`). The build targets themselves
> use Cargo and are unaffected; the variable is recorded here only for accuracy.

## Risky or central files

These stand out by fan-in (blast radius) and/or size. Listed as observations, not
recommendations.

- **`src/error.rs`** and **`src/common.rs`** — highest fan-in (14 inbound edges each).
  Changes to the error type or shared helpers ripple across most of the codebase.
- **`src/main.rs`** (≈1,824 lines) — large, central orchestration with significant branching.
- **`src/config.rs`** (≈1,638 lines, fan-in 7) — wide configuration surface; relates to both
  in-memory consumers and serialized TOML compatibility.
- **`src/database/` + `src/migrations/`** — persistent state. `src/migrations/mod.rs`
  (≈830 lines) and `src/database/schema.rs` carry versioned DDL applied to existing SQLite
  stores; this is the data-integrity hotspot.
- **`src/sync.rs`** (≈772 lines) + Turso — networked, feature-gated, experimental; the hardest
  paths to exercise deterministically.
- **`src/display.rs`** (≈1,412 lines), **`src/layout/variables.rs`** (≈848 lines),
  **`src/theme.rs`** (≈865 lines) — the rendering surface, where 11 themes × 5 presets ×
  templates create a large output-combination space (covered by `src/layout/tests.rs`,
  ≈2,066 lines, and theme integration tests).
- **`src/provider/mod.rs`** (≈769 lines, fan-in 8) — the trait/orchestrator seam that the
  GSD/stats/test providers depend on.

---

*Largest files by line count (from the scan inventory):* `src/layout/tests.rs` (2,066),
`src/main.rs` (1,824), `src/gsd/tests.rs` (1,800), `src/config.rs` (1,638),
`src/display.rs` (1,412), `src/utils.rs` (1,348), `src/database/tests.rs` (1,119),
`src/context_learning.rs` (1,004), `src/stats/tests.rs` (917), `src/theme.rs` (865).
