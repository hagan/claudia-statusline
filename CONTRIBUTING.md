# Contributing to Claudia Statusline

Thank you for your interest in contributing to Claudia Statusline! This guide will help you get started with development.

## Project Status

**Current Version**: v2.10.0 (Phase 3 Security Hardening Complete)

### Completed Phases
- ✅ Phase 1: SQLite Finalization (v2.8.0)
- ✅ Phase 2: Database Maintenance (v2.9.0)
- ✅ Phase 3: Security Hardening (v2.10.0)

### Upcoming Phases
- 🔜 Phase 4: CLI UX & Diagnostics
- 🔜 Phase 5: Robustness & Performance
- 🔜 Phase 6: Reuse for Other Agents
- 🔜 Phase 7: CI/CD Security

## Development Planning

We follow a phase-based development approach with acceptance criteria for each feature. Please check existing GitHub Issues and pull requests before starting new work.

## Development Setup

### Prerequisites
- Rust 1.70+ (install via [rustup](https://rustup.rs/))
- Git
- Make (optional but recommended)

### Getting Started
```bash
# Clone the repository
git clone https://github.com/hagan/claudia-statusline
cd claudia-statusline

# Build the project
make build  # or: cargo build --release

# Run tests
make test   # or: cargo test

# Check formatting
make fmt    # or: cargo fmt --all -- --check

# Run linter
make lint   # or: cargo clippy --all-targets --all-features -- -D warnings
```

## Development Tips

### Debugging with Logging

When working on stats, SQLite, or other complex operations:

```bash
# Enable debug logging for development
RUST_LOG=debug cargo run

# Debug specific modules
RUST_LOG=statusline::stats=debug cargo run
RUST_LOG=statusline::database=debug cargo run

# Info level for moderate verbosity
RUST_LOG=info cargo run
```

### Testing NO_COLOR Support

```bash
# Test with colors disabled
NO_COLOR=1 cargo run

# Verify output has no ANSI codes
NO_COLOR=1 cargo run | cat -A
```

### Working with SQLite

```bash
# View SQLite database contents
sqlite3 ~/.local/share/claudia-statusline/stats.db

# Common SQLite commands
.tables                    # List all tables
.schema sessions          # Show table schema
SELECT * FROM sessions;   # Query data
.quit                     # Exit
```

### Make Targets

Key make targets for development:

```bash
make build         # Build release binary
make debug         # Build debug binary
make test          # Run all tests
make test-sqlite   # Run SQLite integration tests
make dev          # Build and run with test input
make bench        # Run performance benchmark
make fmt          # Format code
make lint         # Run clippy linter
make clean        # Clean build artifacts
```

### Cutting a Release

Published release artifacts are built and uploaded by the **`release` GitHub Actions
workflow** (`.github/workflows/release.yml`), which triggers on pushing a `v*` tag. That
workflow is the source of truth for what ships; the local scripts/targets below are for
bumping the version and preparing/sanity-checking the tag.

Blessed path:

```bash
# 1. Bump the version (edits VERSION + Cargo.toml and related references)
make bump-patch          # or: make bump-minor / make bump-major
#    (equivalently: ./scripts/bump-version.sh patch)

# 2. Commit the bump through the normal branch -> PR -> main flow, and update CHANGELOG.md.

# 3. From an up-to-date main, create the annotated tag (reads VERSION):
make tag                 # creates vX.Y.Z

# 4. Push the tag — this is what fires the release workflow:
git push origin "v$(cat VERSION)"     # or: git push --tags
```

Notes:
- `scripts/release.sh` is an optional local helper that creates the tag and builds a local
  Linux tarball for spot-checking; the multi-platform artifacts attached to the GitHub
  Release still come from `release.yml`.
- `make release-build` just does a clean local `--release` build + `--version` check; it does
  not tag or publish.
- The version lives in `VERSION` and `Cargo.toml`; `build.rs` reads `VERSION` so they must
  agree. `make bump-*` keeps them in sync — don't edit them by hand.

### Code Organization

The codebase is organized into focused modules (v3.0.0 moved several into
directories — see `docs/architecture/project-map.md` for the full map):

- `main.rs` - Entry point, CLI parsing, orchestration
- `lib.rs` / `render.rs` - Embedding API and the shared stats-update + render flow
- `models.rs` - Data structures and types
- `stats/` - Statistics tracking module (SQLite-only since v3.0.0)
- `database/` - SQLite operations module (schema, CRUD, maintenance, sync)
- `migrations/` - Schema migration framework
- `display.rs` / `theme.rs` / `layout/` - Output formatting, colors, and the layout/template engine
- `git.rs` / `git_utils.rs` / `git_provider.rs` - Git repository operations
- `gsd/` - GSD project-tracking data provider
- `provider/` - `DataProvider` trait and parallel `ProviderOrchestrator`
- `utils.rs` - Utility functions
- `config.rs` - Configuration management
- `error.rs` - Error handling
- `retry.rs` - Retry logic

### Testing Guidelines

1. **Unit Tests**: Add tests in the same module file
2. **Integration Tests**: Add to `tests/integration_tests.rs`
3. **SQLite Tests**: Add to `tests/sqlite_integration_tests.rs`
4. **Property Tests**: Add to `tests/proptest_tests.rs`

#### Test Environment Isolation

**Important**: All integration tests must use environment isolation to prevent test failures caused by host configuration files (e.g., `~/.config/claudia-statusline/config.toml`).

Add this at the start of **every** test function in `tests/` files:

```rust
mod test_support;

#[test]
fn test_your_feature() {
    let _guard = test_support::init();
    // Your test code here - environment is now isolated
}
```

The `test_support::init()` function:
- Redirects `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` to temp directories
- Clears all `STATUSLINE_*` and `CLAUDE_*` environment variables
- Ensures tests use default config values, not your personal settings

If your test needs specific env vars, set them **after** calling `init()`:

```rust
#[test]
fn test_with_custom_env() {
    let _guard = test_support::init();
    std::env::set_var("STATUSLINE_THEME", "light");  // Set after init()
    // Test code...
}
```

Run specific test categories:
```bash
cargo test --lib                    # Unit tests only
cargo test --test integration_tests # Integration tests only
cargo test test_name                # Specific test by name
```

### Performance Considerations

- Keep execution time under 10ms
- Limit file I/O operations
- Use atomic operations for stats updates
- Process only last 50 lines of transcript files
- Validate file sizes (10MB limit for transcripts)

### Security Guidelines

- Always validate user input paths
- Use `validate_path_security()` for path operations
- Limit file sizes to prevent DoS
- Never log sensitive information
- Follow the security checklist in SECURITY.md

## Submitting Changes

### Before Submitting

1. **Format your code**: `cargo fmt --all`
2. **Check linting**: `cargo clippy --all-targets --all-features -- -D warnings`
3. **Run tests**: `cargo test`
4. **Update documentation** if needed
5. **Add tests** for new functionality

### Pull Request Process

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/your-feature`
3. Commit your changes with clear messages
4. Push to your fork: `git push origin feature/your-feature`
5. Open a Pull Request with:
   - Clear description of changes
   - Any breaking changes noted
   - Tests passing
   - Documentation updated

### Commit Message Format

```
type: Brief description

Longer explanation if needed. Wrap at 72 characters.

Fixes #issue_number
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `chore`

## CI/CD Pipeline

All PRs automatically run:
- Format checking (`cargo fmt`)
- Linting (`cargo clippy`)
- Unit and integration tests
- Security audit (`cargo-audit`)
- Multi-platform builds (Linux, macOS, Windows)

## Getting Help

- Check existing issues on GitHub
- Read the documentation in README.md and ARCHITECTURE.md
- Ask questions in GitHub Discussions
- Review the codebase - it's well-documented!

## Code of Conduct

Be respectful, inclusive, and constructive. We're all here to make Claude Code better!

## License

By contributing, you agree that your contributions will be licensed under the MIT License.