# Migration Guide – JSON to SQLite

## Why This Exists
Claudia Statusline originally persisted statistics in a single JSON file. Since v2.2.0 the project has maintained a dual-write path: SQLite for reliability and concurrency, JSON for backward compatibility. The current releases (2.15.x) prefer SQLite for reads and only keep JSON if you opt to retain the backup. This guide explains how to complete the migration and operate in SQLite-only mode.

## Current Behaviour
- `stats.db` is created on demand in the XDG data directory.
- The CLI reads from SQLite first; if the database is missing it falls back to JSON and immediately attempts to migrate.
- JSON writing is controlled by the `database.json_backup` flag (default `true` for smooth upgrades).
- `statusline migrate --finalize` verifies parity, archives or deletes the JSON file, and flips `json_backup = false` in `config.toml`.

## Recommended Migration Path
1. **Verify the CLI can see your data**
   ```bash
   statusline health
   ```
   Confirm that both the JSON and SQLite files exist and the totals look sensible.

2. **Trigger any remaining import work**
   Run the statusline inside Claude (or pipe sample JSON) to ensure the dual-write path has copied everything into SQLite.

3. **Finalize the migration**
   ```bash
   statusline migrate --finalize            # Archive stats.json
   statusline migrate --finalize --delete-json  # Delete stats.json instead
   ```
   The command:
   - Loads both stores and compares session counts and total cost (1¢ tolerance).
   - Aborts if a mismatch is detected, leaving files untouched.
   - Archives `stats.json` with a timestamp (or deletes it) when parity is confirmed.
   - Writes `~/.config/claudia-statusline/config.toml` (creating it if needed) with `json_backup = false`.

4. **Optionally remove the archive**
   If you chose the default archival path you can keep the `.migrated.*` file for safekeeping or remove it after verifying the database.

## Benefits of SQLite-only Mode
- Fewer disk writes and smaller I/O footprint.
- No more locking contention when multiple Claude windows are open.
- Maintenance tasks (`statusline db-maintain`) run faster because there is only one canonical store.

## Rolling Back
You can re-enable JSON at any time:
```toml
# ~/.config/claudia-statusline/config.toml
[database]
json_backup = true
```
On the next run the CLI will recreate `stats.json` from the SQLite data.

## Troubleshooting
- **Migration parity warning** – Run the CLI once more to trigger a fresh dual-write cycle, then re-run `statusline migrate --finalize`.
- **Missing SQLite file** – Delete `stats.db` and run the CLI; it will recreate the database from JSON before you finalize.
- **Read-only environments** – Use `statusline health --json` to check paths, then copy files to a writable location before finalizing.

For schema-level changes (adding tables/columns) see `docs/DATABASE_MIGRATIONS.md`.
