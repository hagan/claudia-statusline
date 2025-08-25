# Migration Guide - SQLite Dual Storage (v2.2.0)

## Overview

Starting with version 2.2.0, Claudia Statusline now uses a dual-storage backend with both JSON and SQLite for improved concurrent access support. This guide explains the migration process and what to expect.

## What's Changing?

### Before (v2.1.x)
- Stats stored only in `~/.local/share/claudia-statusline/stats.json`
- File locking with fs2 for concurrent access
- Potential race conditions with multiple Claude consoles

### After (v2.2.0+)
- Stats stored in **both** formats:
  - `~/.local/share/claudia-statusline/stats.json` (primary, backward compatible)
  - `~/.local/share/claudia-statusline/stats.db` (secondary, better concurrency)
- SQLite database with WAL mode for robust concurrent access
- Automatic migration of existing data

## Migration Phases

### Phase 1: Dual-Write (Current - v2.2.0)
- **Status**: ✅ Implemented
- JSON remains the primary data source
- SQLite is written to simultaneously for future transition
- Full backward compatibility maintained
- No user action required

### Phase 2: SQLite Primary (Planned - v2.3.0)
- **Status**: 🔄 Planned
- SQLite becomes primary data source
- JSON maintained for backward compatibility
- Automatic migration on first run
- Faster performance for concurrent access

### Phase 3: SQLite Only (Future - v3.0.0)
- **Status**: 📋 Future
- JSON support deprecated
- Migration tool provided for legacy data
- Significant performance improvements
- Full ACID compliance

## What You Need to Know

### Automatic Migration
When you upgrade to v2.2.0, the following happens automatically:

1. **First Run**: Your existing `stats.json` is read normally
2. **Database Creation**: A new `stats.db` file is created
3. **Data Migration**: Existing data is copied to SQLite
4. **Dual Updates**: All future updates write to both files

### File Locations
```
~/.local/share/claudia-statusline/
├── stats.json    # Your existing stats (still primary)
└── stats.db      # New SQLite database (secondary)
```

### Performance Impact
- **Minimal overhead**: <5ms per update
- **Binary size increase**: ~2MB (includes bundled SQLite)
- **Better concurrency**: Multiple Claude consoles work seamlessly
- **No data loss**: Dual-write ensures redundancy

## Troubleshooting

### Q: Can I delete the SQLite database?
**A:** Yes, in v2.2.0 the JSON file is still primary. The SQLite database will be recreated on next run.

### Q: What if the migration fails?
**A:** The statusline continues working with JSON only. Check stderr for error messages.

### Q: Can I disable SQLite?
**A:** Currently no, but it's non-intrusive and provides benefits even if not fully utilized yet.

### Q: Will my stats be preserved?
**A:** Yes, absolutely. The dual-write system ensures no data loss during migration.

### Q: What about disk space?
**A:** The SQLite database is typically smaller than the JSON file due to efficient storage.

## Monitoring the Migration

You can verify the dual-write is working by checking for debug output:
```bash
# Run statusline and look for messages like:
# SQLite dual-write successful: day=$12.34, session=$5.67
```

Check file existence:
```bash
ls -la ~/.local/share/claudia-statusline/
# Should show both stats.json and stats.db
```

## Benefits of SQLite

### Why We're Migrating
1. **Concurrent Access**: Multiple Claude consoles can safely update stats
2. **ACID Compliance**: Atomic, Consistent, Isolated, Durable transactions
3. **Better Performance**: Indexed queries and efficient storage
4. **Data Integrity**: Built-in corruption detection and recovery
5. **Future Features**: Enables advanced queries and analytics

### Technical Details
- **WAL Mode**: Write-Ahead Logging for concurrent reads/writes
- **Busy Timeout**: 10-second timeout prevents lock errors
- **Transactions**: All updates are atomic
- **Schema Versioning**: Built-in migration framework

## For Developers

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

-- Daily statistics
CREATE TABLE daily_stats (
    date TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0
);

-- Monthly statistics
CREATE TABLE monthly_stats (
    month TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0
);
```

### Testing the Migration
```bash
# Run the integration tests
cargo test sqlite_integration

# Specific migration test
cargo test test_json_to_sqlite_migration
```

## Rollback Instructions

If you need to rollback to v2.1.x:

1. **Keep your stats.json**: It's still being updated and is fully compatible
2. **Downgrade the binary**: `cargo install --version 2.1.3 claudia-statusline`
3. **Remove SQLite file** (optional): `rm ~/.local/share/claudia-statusline/stats.db`

Your stats will be preserved in the JSON file throughout.

## Future Roadmap

### v2.3.0 (Q4 2025)
- SQLite becomes primary storage
- Advanced analytics queries
- Stats export functionality

### v2.4.0 (Q1 2026)
- Historical stats visualization
- Performance profiling data
- Custom aggregation periods

### v3.0.0 (Q2 2026)
- JSON deprecated (with migration tool)
- GraphQL API for stats queries
- Multi-machine stats sync

## Getting Help

If you experience any issues with the migration:

1. Check stderr output for error messages
2. File an issue: https://github.com/hagan/claudia-statusline/issues
3. Include your `stats.json` file (sanitized if needed)
4. Mention your OS and claudia-statusline version

## Summary

The migration to SQLite is designed to be seamless and transparent. Your existing data is preserved, and the dual-write system ensures a smooth transition. No action is required on your part - just update to v2.2.0 and enjoy better concurrent access support!

---

*Last Updated: August 25, 2025*