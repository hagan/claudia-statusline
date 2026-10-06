# `schema_v6_v3.3.0.sql` and `seed_v6.sql` — provenance

Frozen schema-v6 fixture used by the migration tests in `src/migrations/mod.rs`
(Phase 16: idempotent 007/008, open-time schema repair, registry drift guard).

| Field | Value |
|-------|-------|
| Source | tag `v3.3.0`, `src/database/schema.rs` (the `SCHEMA` constant, text between `r#"` and `"#;`) |
| Schema version | 6 (what a v3.3.0 binary creates for a new database) |
| Extracted | 2026-10-06 |

## Extraction

```bash
{ printf '%s\n' '-- Frozen schema v6 SCHEMA from tag v3.3.0, src/database/schema.rs (text between r#" and "#;).' \
                '-- Do not edit: tests depend on it matching the released binary.'
  git show v3.3.0:src/database/schema.rs | sed -n '/r#"/,/"#;/p' | sed '1d;$d'
} > tests/fixtures/schema_v6_v3.3.0.sql
```

Verify the body is still verbatim:

```bash
diff <(git show v3.3.0:src/database/schema.rs | sed -n '/r#"/,/"#;/p' | sed '1d;$d') \
     <(sed '1,2d' tests/fixtures/schema_v6_v3.3.0.sql)
```

The schema fixture deliberately contains no `schema_migrations` row. A v3.3.0 binary
records version 6 in the same transaction as the schema; the tests reproduce that by
loading `seed_v6.sql`, which ends with that stamp.

## `seed_v6.sql`

Hand-written synthetic data: no row is derived from any real database (the repository is
public). It covers `sessions` (a fully populated row, a row with every optional column
NULL/default, and a row with token counts near `i64::MAX / 2`, a unicode workspace path
and a non-trivial REAL cost), `daily_stats`, `monthly_stats`, `session_archive` and
`learned_context_windows`, using only columns that exist in the v6 schema, and ends with
the v6 `schema_migrations` stamp.
