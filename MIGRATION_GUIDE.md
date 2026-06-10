# Migration Guide – JSON to SQLite

## Why This Exists
Claudia Statusline originally persisted statistics in a single JSON file. From v2.2.0 the project maintained a dual-write path: SQLite for reliability and concurrency, JSON for backward compatibility. **JSON writes were REMOVED in v3.0.0.** The `database.json_backup` field is now ignored legacy — setting it to `true` produces a one-line stderr deprecation note and the binary continues rendering from SQLite. This guide explains how to operate in the SQLite-only world and how to recover legacy data.

## Current Behaviour
- `stats.db` is created on demand in the XDG data directory and is the canonical store.
- The CLI reads from SQLite first; if no usable SQLite database is present it falls back to a legacy `stats.json` (one-shot recovery) and imports it into SQLite.
- JSON writing was removed in v3.0.0. The `database.json_backup` field is ignored legacy; when set to `true` the binary prints a one-line stderr deprecation note and continues.
- Use `statusline migrate --finalize` to archive or delete a leftover `stats.json` file. This is an explicit user-driven cleanup step, not a required migration step.

## Recommended Migration Path
1. **Verify the CLI can see your data**
   ```bash
   statusline health
   ```
   Confirm that the SQLite file exists and the totals look sensible. If a legacy `stats.json` is still present it will be imported automatically the first time SQLite is missing or unusable.

2. **Archive or delete the leftover JSON file (optional cleanup)**
   ```bash
   statusline migrate --finalize            # Archive stats.json
   statusline migrate --finalize --delete-json  # Delete stats.json instead
   ```
   The command:
   - Loads both stores and compares session counts and total cost (1¢ tolerance).
   - Aborts if a mismatch is detected, leaving files untouched.
   - Archives `stats.json` with a timestamp (or deletes it) when parity is confirmed.
   - Normalizes `config.toml` (the `json_backup` field is reset; this is a no-op in v3.0.0+).

3. **Optionally remove the archive**
   If you chose the default archival path you can keep the `.migrated.*` file for safekeeping or remove it after verifying the database.

## Benefits of SQLite-only Mode
- Fewer disk writes and smaller I/O footprint.
- No more locking contention when multiple Claude windows are open.
- Maintenance tasks (`statusline db-maintain`) run faster because there is only one canonical store.

## Rolling Back

v3.0.0 removed JSON write paths entirely; enabling the legacy `json_backup`
field is a no-op (the binary will print a one-line stderr deprecation note and
continue rendering from SQLite). To restore JSON dual-write, install v2.22.x:

```bash
cargo install --version 2.22.1 claudia-statusline
```

If you have a legacy `stats.json` file you want to recover from, it is read
once on startup as a one-shot fallback **when SQLite is missing or unusable**
(the stats.db file is absent or cannot be opened) — the data is imported into
SQLite on that run. After import, archive or delete the leftover file with
`statusline migrate --finalize` or `statusline migrate --finalize --delete-json`.

## Troubleshooting
- **Missing SQLite file** – Delete `stats.db` and run the CLI; it will recreate the database from a legacy `stats.json` if one is present.
- **Read-only environments** – Use `statusline health --json` to check paths, then copy files to a writable location.

For schema-level changes (adding tables/columns) see `docs/DATABASE_MIGRATIONS.md`.
