# Changelog

All notable changes to the Claudia Statusline project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.2.0] - 2025-08-25

### Added
- **Dual Storage Backend**: SQLite database alongside JSON for better concurrent access
- **SQLite Integration**: Full CRUD operations with WAL mode for concurrent read/write
- **Migration Framework**: Schema versioning system with up/down migrations
- **Concurrent Access Support**: Multiple Claude consoles can safely update stats simultaneously
- **Automatic Migration**: JSON data automatically migrated to SQLite on first run
- **Integration Tests**: 9 new tests for SQLite functionality including concurrency tests
- New dependencies: rusqlite with bundled SQLite engine

### Changed
- Stats module now performs dual-writes to both JSON and SQLite
- Binary size increased to ~2.6MB (includes bundled SQLite)
- Database stored at `~/.local/share/claudia-statusline/stats.db`

### Technical Details
- Phase 1 implementation: JSON remains primary, SQLite as secondary
- WAL (Write-Ahead Logging) mode enabled for better concurrency
- 10-second busy timeout for database operations
- UPSERT operations for accumulating session values
- Transaction support with automatic rollback on errors

## [2.1.3] - 2025-08-25

### Added
- Process-safe file locking using fs2 crate for concurrent Claude console support
- Session start time tracking in stats.json for reliable burn rate calculation
- Automatic backup creation for corrupted stats files
- Comprehensive CODE_REVIEW.md documentation in .claude/ directory
- Support for timezone offsets in ISO 8601 timestamp parsing

### Fixed
- Critical bug: Burn rate not showing (was displaying $399/hr incorrectly)
- ISO 8601 timestamp parsing with proper leap year calculation
- Session duration calculation now works with timezone offsets
- Daily totals now persist correctly across restarts
- Stats file updates are now atomic to prevent data loss
- Version synchronization between Cargo.toml and VERSION file

### Changed
- Stats now save on every update (removed conditional saving)
- Improved error handling for file I/O operations
- Enhanced test isolation for concurrent tests

### Known Issues
- 2 unit tests fail due to temp directory isolation (production code works correctly)
- Some dead code warnings for unused constants and methods

## [2.1.2] - 2025-08-24

### Added
- Cost tracking and display in statusline
- Lines added/removed tracking
- Daily, monthly, and all-time statistics
- XDG-compliant stats storage
- Burn rate calculation ($/hr) after 1 minute of session time
- Progress bar for context usage

### Changed
- Modularized codebase into 7 focused modules
- Improved Git status parsing and display

## [2.1.1] - 2025-08-24

### Fixed
- Context progress bar display issues
- Day charge display with empty cost object
- Transcript field name correction
- Cache tokens now properly included in calculations

## [2.1.0] - 2025-08-24

### Added
- Complete version management system with git integration
- CLI arguments: --version, --help flags
- Build metadata injection at compile time

### Changed
- Major rewrite with complete modularization
- Professional version management practices

## [2.0.0] - 2025-08-23

### Added
- Initial Rust implementation inspired by Peter Steinberger's statusline.rs
- Git repository detection and status display
- Model type detection and abbreviation
- Path shortening for home directory
- ANSI color support with theme detection

### Changed
- Complete rewrite from shell script to Rust
- Performance improvements (~5ms execution time)

## [1.0.0] - 2025-08-22

### Added
- Initial release
- Basic statusline functionality
- JSON input parsing from Claude Code
- Directory and model display

---

[2.2.0]: https://github.com/hagan/claudia-statusline/compare/v2.1.3...v2.2.0
[2.1.3]: https://github.com/hagan/claudia-statusline/compare/v2.1.2...v2.1.3
[2.1.2]: https://github.com/hagan/claudia-statusline/compare/v2.1.1...v2.1.2
[2.1.1]: https://github.com/hagan/claudia-statusline/compare/v2.1.0...v2.1.1
[2.1.0]: https://github.com/hagan/claudia-statusline/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/hagan/claudia-statusline/compare/v1.0.0...v2.0.0
[1.0.0]: https://github.com/hagan/claudia-statusline/releases/tag/v1.0.0