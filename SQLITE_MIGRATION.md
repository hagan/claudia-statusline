# SQLite Migration Guide

## Overview

Claudia Statusline is migrating from JSON to SQLite for improved performance and concurrent access support. This is happening in three phases to ensure zero downtime and no data loss.

## Migration Phases

### Phase 1: Dual-Write (v2.2.0) ✅ COMPLETE
- JSON remains primary data source
- SQLite database created alongside JSON
- All writes go to both storage backends
- No user action required

### Phase 2: SQLite-First (v2.7.0) ✅ CURRENT
- **SQLite-first reads with JSON backup writes**
- Automatic migration from JSON on first run
- JSON maintained as backup/compatibility layer
- Zero configuration needed - fully automatic

### Phase 3: SQLite Only (v3.0.0) 🔜 PLANNED
- JSON writing will be removed
- SQLite becomes the only storage backend
- Smaller binary, faster performance
- Breaking change for tools reading JSON directly

## What Happens in v2.7.0

When you upgrade to v2.7.0, the following happens automatically:

1. **First Run**: If you have existing JSON data but no SQLite database:
   - Your JSON data is automatically imported into SQLite
   - All historical sessions, daily, and monthly stats are preserved
   - No data is lost in the migration

2. **Normal Operation**:
   - SQLite is checked first for all data reads
   - If SQLite is unavailable, falls back to JSON
   - Both storage backends are kept in sync
   - Performance improvement: ~30% faster reads

3. **Data Location**:
   - SQLite: `~/.local/share/claudia-statusline/stats.db`
   - JSON: `~/.local/share/claudia-statusline/stats.json` (backup)

## Benefits of SQLite

- **Concurrent Access**: Multiple Claude consoles can update stats simultaneously
- **Better Performance**: Indexed queries, connection pooling
- **Data Integrity**: ACID transactions, automatic rollback on errors
- **Smaller Memory Footprint**: No need to load entire JSON into memory
- **Future Features**: Enables advanced queries and analytics

## Troubleshooting

### Verify Migration Success

Check if your data migrated correctly:

```bash
# Check SQLite database exists
ls -la ~/.local/share/claudia-statusline/stats.db

# View sessions in SQLite
sqlite3 ~/.local/share/claudia-statusline/stats.db \
  "SELECT COUNT(*) as sessions FROM sessions;"

# View total cost
sqlite3 ~/.local/share/claudia-statusline/stats.db \
  "SELECT printf('Total: $%.2f', SUM(cost)) FROM sessions;"
```

### Manual Migration

If automatic migration didn't occur, you can trigger it manually:

```bash
# Remove SQLite database (backup first if needed)
mv ~/.local/share/claudia-statusline/stats.db \
   ~/.local/share/claudia-statusline/stats.db.backup

# Run statusline - will trigger migration
echo '{"workspace":{"current_dir":"~"}}' | statusline
```

### Reset Everything

If you need to start fresh:

```bash
# Backup current data
cp -r ~/.local/share/claudia-statusline \
      ~/.local/share/claudia-statusline.backup

# Remove all data
rm -rf ~/.local/share/claudia-statusline

# Next run will create fresh databases
```

## FAQ

**Q: Will I lose my statistics during migration?**
A: No, all data is preserved. The migration is designed to be lossless.

**Q: Can I still use the JSON file?**
A: Yes, in v2.7.0 JSON is maintained as a backup. This will change in v3.0.0.

**Q: What if SQLite fails?**
A: The system automatically falls back to JSON if SQLite is unavailable.

**Q: How much disk space does this use?**
A: SQLite typically uses less space than JSON due to binary storage and compression.

**Q: Can I disable SQLite?**
A: Not in v2.7.0. If you need JSON-only, stay on v2.3.0.

## Technical Details

### Database Schema

```sql
-- Sessions table
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    start_time TEXT NOT NULL,
    last_updated TEXT NOT NULL,
    cost REAL DEFAULT 0.0,
    lines_added INTEGER DEFAULT 0,
    lines_removed INTEGER DEFAULT 0
);

-- Daily aggregates
CREATE TABLE daily_stats (
    date TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0
);

-- Monthly aggregates
CREATE TABLE monthly_stats (
    month TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0
);
```

### Migration Code Path

1. `StatsData::load()` tries SQLite first via `load_from_sqlite()`
2. If SQLite has no sessions but JSON exists, `migrate_to_sqlite()` runs
3. Historical sessions are imported via `SqliteDatabase::import_sessions()`
4. Current session continues to update both backends

## Support

If you encounter issues with the migration:

1. Check the [GitHub Issues](https://github.com/hagan/claudia-statusline/issues)
2. Include your SQLite and JSON file sizes in any bug reports
3. Run with `RUST_LOG=debug` for detailed migration logs