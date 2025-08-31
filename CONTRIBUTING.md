# Contributing to Claudia Statusline

Thank you for your interest in contributing to Claudia Statusline! This guide will help you get started with development.

## Development Planning

For structured development work, please refer to the **[Planning Pack](.claude/planning/README.md)** which provides:
- Acceptance-driven phase documents
- Execution protocol for implementations
- Clear success criteria for each phase

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

### Code Organization

The codebase is organized into focused modules:

- `main.rs` - Entry point, CLI parsing, orchestration
- `models.rs` - Data structures and types
- `stats.rs` - Statistics tracking (SQLite-first with JSON backup)
- `database.rs` - SQLite operations
- `display.rs` - Output formatting and colors
- `git.rs` - Git repository operations
- `utils.rs` - Utility functions
- `config.rs` - Configuration management
- `error.rs` - Error handling
- `retry.rs` - Retry logic

### Testing Guidelines

1. **Unit Tests**: Add tests in the same module file
2. **Integration Tests**: Add to `tests/integration_tests.rs`
3. **SQLite Tests**: Add to `tests/sqlite_integration_tests.rs`
4. **Property Tests**: Add to `tests/proptest_tests.rs`

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