# Credits and Acknowledgments

## Original Inspiration
- **Peter Steinberger** (@steipete) - Created the original statusline.rs gist that inspired this project
  - Source: https://gist.github.com/steipete/8396e512171d31e934f0013e5651691e
  - The original concept of a Claude Code statusline in Rust

## Current Implementation
Claudia Statusline is now a complete, independent implementation with extensive enhancements.

## Main Contributors
- **Claudia Statusline Team** - Complete rewrite and feature implementation
  - Persistent statistics tracking with dual storage backend
  - Security hardening and input validation (v2.2.1)
  - Multi-console support with process-safe locking
  - SQLite integration with migration framework (v2.2.0)
  - Progress bars and burn rate calculations
  - Cross-platform CI/CD automation
  - Comprehensive test suite (79+ tests)

## Technical Assistance
- **Claude Code Assistant** - Code review, security analysis, and implementation guidance
  - Identified critical security vulnerabilities (fixed in v2.2.1)
  - Helped design the SQLite migration framework
  - Assisted with test coverage improvements

## Dependencies and Libraries
We gratefully acknowledge the authors of these excellent Rust crates:
- **serde** and **serde_json** - JSON serialization
- **rusqlite** - SQLite database integration
- **fs2** - Cross-platform file locking
- **chrono** - Date and time handling
- **tempfile** - Temporary file management (testing)

## Community
- All GitHub contributors who have submitted issues, PRs, or feedback
- Claude Code users who have tested and provided feedback

## Special Thanks
- The Rust community for excellent documentation and tooling
- The SQLite team for the embedded database engine
- GitHub Actions team for CI/CD infrastructure

---

This project stands on the shoulders of giants. While we've built something new and comprehensive, we honor the original inspiration and all who have contributed to making Claudia Statusline what it is today.