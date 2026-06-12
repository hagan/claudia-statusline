# Risky Areas — Claudia Statusline

> **Source:** Derived from `.understand-anything/knowledge-graph.json` (575 nodes, 767 edges)
> at commit `ac29509`, cross-checked against the source tree. Connectivity = inbound + outbound
> dependency-type edges (`imports`, `depends_on`, `calls`, `implements`, `configures`,
> `tested_by`), rolled up to the containing file.
>
> This document flags areas that warrant care. It is descriptive only — it records risk signals
> and does **not** prescribe code changes. Companion docs:
> `docs/architecture/project-map.md`, `docs/architecture/key-flows.md`.

---

## 1. Highly connected files (high blast radius)

Ranked by total degree. High fan-**in** = many callers depend on it (changes ripple outward);
high fan-**out** = it orchestrates many modules (easy to break by changing a dependency).

| File | Degree | Fan-in | Fan-out | LOC | Why it's risky |
|---|---:|---:|---:|---:|---|
| `src/main.rs` | 22 | 0 | 22 | 1,824 | Binary orchestrator; wires together every subsystem. Large + branch-heavy. |
| `src/lib.rs` | 22 | 0 | 22 | 312 | Public library API (`render_statusline`); mirrors `main` and re-exports modules. Drift between the two is a recurring hazard. |
| `src/gsd/mod.rs` | 17 | 3 | 14 | 458 | `GsdProvider` reaches across all GSD submodules; high fan-out into roadmap/state/todos/update. |
| `src/database/mod.rs` | 16 | 6 | 10 | 215 | SQLite façade (`SqliteDatabase`); central to all persistence and triggers migrations on open. |
| `src/common.rs` | 15 | **14** | 1 | 264 | Foundational helpers (data-dir, device id, dates). 14 modules depend on it — a signature change ripples broadly. |
| `src/error.rs` | 14 | **14** | 0 | 104 | Unified `StatuslineError` type. Touched by nearly everything; changing a variant breaks many call sites. |
| `src/provider/mod.rs` | 13 | 11 | 2 | 769 | `DataProvider` trait + `ProviderOrchestrator` — the extension seam every provider implements. |
| `src/stats/mod.rs` | 12 | 4 | 8 | 84 | Thin re-export hub for the stats submodules; small but widely referenced. |
| `src/git.rs` | 9 | 5 | 4 | 700 | Git status parsing (two porcelain paths). External-process output parsing = fragile inputs. |
| `src/utils.rs` | 9 | 6 | 3 | 1,348 | Grab-bag utilities incl. transcript parsing + terminal sanitization. Large and broadly used. |
| `src/config.rs` | 9 | 7 | 2 | 1,638 | Wide config surface (18 config structs); also governs serialized TOML compatibility. |

**Top two hubs to treat as load-bearing:** `src/error.rs` and `src/common.rs` (fan-in 14 each).
Any change to their public surface should be assumed to affect most of the codebase.

---

## 2. Modules with unclear / split ownership

These are places where a single concept is spread across multiple files, so "where does this
logic belong?" is genuinely ambiguous and easy to get wrong.

- **Session state is owned in three places.**
  `src/stats/session.rs` (`SessionStats`, in-memory `update_session`),
  `src/database/session.rs` (SQL `update_session` / `update_session_tx`), and
  `src/database/schema.rs` (`SessionUpdate` struct). The flow constructs a `SessionUpdate` in
  `lib.rs`/`main.rs`, but `active_time_seconds` / `last_activity` are deliberately computed
  *inside* `SqliteDatabase::update_session` and left `None` at the construction site (noted in
  the source as a v3.0.0 cleanup). The ownership boundary is subtle and undocumented at the type level.

- **Persistence is split between `src/stats/` and `src/database/`.**
  `src/stats/persistence.rs` (`update_stats_data`, `load_from_sqlite`, `migrate_to_sqlite`,
  `perform_sqlite_dual_write`) sits on top of `src/database/*`. Two surfaces both legitimately
  claim "persist stats," and the load path has legacy-JSON fallback logic that is easy to
  regress (see §4).

- **Provider logic is spread across three locations.**
  `src/provider/mod.rs` (trait + orchestrator), `src/stats/provider.rs` (`StatsProvider`),
  `src/gsd/mod.rs` (`GsdProvider`), plus `src/provider/test_provider.rs`. Adding or reordering
  providers touches multiple files with no single registry-of-record beyond
  `ProviderOrchestrator::register`.

- **`src/utils.rs` (1,348 LOC) is a catch-all.** It mixes transcript token parsing, terminal
  sanitization, and misc helpers with fan-in 6. Unclear ownership makes it a magnet for
  unrelated additions.

---

## 3. Duplicated-looking responsibilities (name collisions / repeated patterns)

These are not necessarily bugs — several are intentional layering — but the **collisions make it
easy to edit the wrong file** or assume the wrong behavior.

| Symbol / file | Locations | Note |
|---|---|---|
| `update_session` | `src/stats/session.rs` **and** `src/database/session.rs` | Same name, different layers; the stats one delegates to the database one. Easy to confuse which to change. |
| `sync.rs` | `src/sync.rs` (Turso cloud sync, `SyncManager`) **and** `src/database/sync.rs` (DB-level sync) | Two unrelated "sync" files. |
| `config.rs` | `src/config.rs` (app config) **and** `src/gsd/config.rs` (GSD config) | Two distinct config systems sharing a filename. |
| `state.rs` | `src/state.rs` **and** `src/gsd/state.rs` | Two distinct "state" concepts. |
| `fill_vars` | `src/gsd/{roadmap,state,todos,update}.rs` (4×) | Repeated provider-fill pattern duplicated per submodule rather than abstracted. |
| `read_with_cache` | `src/gsd/{roadmap,state,update}.rs` (3×) | Same caching pattern reimplemented in three GSD submodules. |
| `new` / `collect` | `database/mod.rs` & `stats/provider.rs` / `provider/test_provider.rs` & `stats/provider.rs` | Provider/constructor conventions repeated across the abstraction. |

> The install/uninstall shell scripts also duplicate helpers (`detect_config_file`,
> `validate_path`, `install_binary`, `configure_claude`, `usage`, `print_summary`) across
> `scripts/install-statusline.sh`, `scripts/quick-install.sh`, and `scripts/uninstall-statusline.sh`.
> Lower stakes (not shipped in the binary), but the same logic exists in multiple copies.

---

## 4. Places where changes need extra tests

> **Coverage signal caveat:** the graph contains **no surviving `tested_by` edges for `src/`
> production files** — coverage is via inline `*/tests.rs` modules and `tests/` integration
> tests rather than explicit links. So the items below are flagged by data sensitivity and
> blast radius, and the named test files are where regressions would surface.

- **Migrations & schema** — `src/migrations/mod.rs` (`MigrationRunner`, `Migration::migrate`,
  `run_migrations_on_db`) and `src/database/schema.rs`. Versioned DDL applied to *existing*
  user databases; a bad change can corrupt or block live SQLite stores. Exercise
  `tests/sqlite_integration_tests.rs` and `tests/db_maintenance_tests.rs`.
- **Stats load/dual-write path** — `src/stats/persistence.rs::update_stats_data` and
  `perform_sqlite_dual_write`. The legacy `stats.json` → SQLite migration fallback (using
  `load()` instead of `default()`) exists specifically to avoid wiping v2.x history; regressions
  here are silent data loss. Covered by `tests/sqlite_integration_tests.rs`,
  `tests/integration_tests.rs`, `src/stats/tests.rs`, `src/database/tests.rs`.
- **Burn-rate & active-time math** — `src/stats/session.rs`, `src/database/session.rs`
  (`get_session_active_time`). Already backed by a large dedicated suite
  (`tests/burn_rate_*` — accumulation, auto-reset, long gaps, multi-day wall clock, high
  volume); any change to the accounting must keep those green.
- **Rendering surface** — `src/display.rs` (1,412 LOC), `src/layout/variables.rs` (848 LOC),
  `src/theme.rs`, and the `src/layout/` template engine. 11 themes × 5 presets × custom
  templates is a large output-combination space. Covered by `tests/theme_integration.rs`,
  `tests/layout_integration_tests.rs`, `tests/display_config_integration.rs`, and the
  2,066-line `src/layout/tests.rs`.
- **Git status parsing** — `src/git.rs` (`parse_git_status`, `parse_git_status_v2`,
  `apply_status_codes`). Parses external `git` output (two porcelain formats, one behind the
  `git_porcelain_v2` feature); fragile to format/locale variation.
- **Context / compaction detection** — `src/context_learning.rs` (`ContextLearner`) and
  `src/hook_handler.rs`. Experimental, stateful, timing-sensitive. Covered by
  `tests/context_learning_sanitization_tests.rs`, `tests/context_tokens_display_tests.rs`,
  `tests/hook_integration_tests.rs`.
- **Config surface** — `src/config.rs`. Adding/renaming fields affects deserialization of
  user-authored TOML; check `tests/baseline_config_test.rs` and `tests/display_config_integration.rs`.
- **Cloud sync** — `src/sync.rs` (`SyncManager`, `resolve_auth_token`). Feature-gated
  (`turso-sync`), networked, hardest to test deterministically; auth-token resolution handles
  `${VAR}`/`$VAR`/literal forms.

---

## 5. Areas GSD should inspect before modifying

A pre-edit checklist for `/gsd:plan`-style work, ordered by blast radius:

1. **`src/error.rs` and `src/common.rs`** — fan-in 14 each. Inspect all callers before changing
   any public signature; assume codebase-wide impact.
2. **`src/main.rs` ↔ `src/lib.rs`** — the binary and library duplicate the stats-update +
   render logic. A change in one almost always needs the mirror change in the other; verify both
   plus `tests/lib_api_tests.rs`.
3. **Persistence stack** (`src/stats/persistence.rs` → `src/database/*` → `src/migrations/`) —
   any schema, migration, or load-path change is a data-integrity event on live SQLite stores.
   Confirm the legacy-JSON fallback still migrates, and add/extend migration tests.
4. **Session ownership** (`stats/session.rs`, `database/session.rs`, `database/schema.rs`) —
   before touching session fields, confirm which layer owns the field (notably `active_time_seconds`
   / `last_activity`, computed in the DB layer).
5. **Provider seam** (`src/provider/mod.rs` + `src/stats/provider.rs` + `src/gsd/mod.rs`) — adding
   data sources or variables touches the trait, the orchestrator registration, and possibly the
   GSD `fill_vars` duplication. Inspect all four GSD submodules that implement `fill_vars`.
6. **Name-collision files** (`sync.rs`, `config.rs`, `state.rs`, `session.rs` — each appears in
   two locations) — verify you are editing the intended file/layer; grep by full path, not base name.
7. **Rendering** (`display.rs`, `layout/`, `theme.rs`) — re-run the theme/layout/display
   integration suites; output regressions are easy and not always obvious.
8. **External-input parsers** (`src/git.rs`, transcript parsing in `src/utils.rs`) — fragile to
   upstream format changes; treat inputs as untrusted.
9. **`.planning/` is a symlink** to an external secure mount (`~/mnt/claudia-docs-secure/.planning`),
   shared GSD state — not a normal in-repo directory. Don't assume it's local or writable in CI.

---

*Connectivity and duplication figures above are reproducible from
`.understand-anything/knowledge-graph.json`. See `project-map.md` for the full module map and
`key-flows.md` for the call sequences referenced here.*
