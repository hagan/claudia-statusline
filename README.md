# Claude Statusline

A high-performance, customizable statusline for Claude Code written in Rust. Displays workspace information, git status, model usage metrics, session cost tracking, and more in your terminal.

![Claude Statusline Screenshot](statusline.png)

## Features

- **Smart Path Display** - Shows current directory with `~` home substitution
- **Git Integration** - Displays branch name and file status (added/modified/deleted/untracked)
- **Context Usage Tracking** - Real-time percentage of Claude's context window with color warnings
- **Model Detection** - Shows current Claude model (Opus/Sonnet/Haiku)
- **Session Duration** - Tracks conversation length from transcript
- **Cost Tracking** - Displays session cost in USD with color-coded warnings
- **Lines Changed** - Shows added/removed lines count from session
- **Theme-Aware Colors** - Automatically adapts to dark/light terminal themes
- **Dark Mode Optimized** - Enhanced visibility for Claude's dark theme
- **High Performance** - Written in Rust for minimal overhead
- **Source Integrity** - SHA256 hash validation ensures authentic source
- **Patch-Based** - Respects original author's copyright

## Quick Start

### One-Line Install (Claude Code)
```bash
git clone https://github.com/hagan/claude-statusline && cd claude-statusline && ./install-claude-code.sh
```

### System Requirements

**Supported Platforms**: Linux, macOS, Unix-like systems
**Terminal**: Any terminal with ANSI color support

### Prerequisites
- **Rust toolchain** with Cargo (1.70+) - [Install Rust](https://www.rust-lang.org/tools/install)
- **curl** or wget (for downloading original source)
- **patch** (for applying modifications)
- **sha256sum** (for source verification)
- **Git** (optional, for repository status)
- **jq** (required for installer and wrapper script) - [Install jq](https://stedolan.github.io/jq/download/)
- **Make** (optional, but recommended for easy building)

### Installation

#### For Claude Code Users (Recommended)
```bash
# Clone the repository
git clone https://github.com/hagan/claude-statusline
cd claude-statusline

# Run the automated installer
chmod +x install-claude-code.sh
./install-claude-code.sh
```

The installer will:
1. ✅ Detect Claude Code configuration location
2. ✅ Download and validate the original source (SHA256 check)
3. ✅ Apply our patches
4. ✅ Build the optimized binary
5. ✅ Install to `~/.local/bin/statusline`
6. ✅ Create wrapper script at `~/.local/bin/statusline-wrapper.sh`
7. ✅ Configure Claude Code settings automatically
8. ✅ Check your PATH configuration

**Configuration Location:**
- Claude Code always uses: `~/.claude/settings.json`
- **Note**: Claude Code does NOT respect `CLAUDE_HOME` or `CLAUDE_CONFIG_DIR` environment variables
- Configuration is always stored in `~/.claude/` regardless of system settings

#### Manual Build
```bash
# First time: fetch and patch the source (includes SHA256 validation)
make fetch-source

# Verify source integrity
make verify-source

# Build the project
make build

# Install to ~/.local/bin
make install

# Or do everything in one step
make  # Downloads source (with validation) and builds
```

#### Build Without Make
```bash
# Fetch original source
curl -s https://gist.githubusercontent.com/steipete/8396e512171d31e934f0013e5651691e/raw/14f964f0d90e37ad63bc95b1e9edeca0fb008a6f/statusline.rs -o statusline.rs

# Verify SHA256 hash
echo "5f7851061abbd896c2d4956323fa85848df79242448019bbea7799111d3cebda  statusline.rs" | sha256sum -c

# Apply patches
patch statusline.rs < statusline.patch

# Build with Cargo
cargo build --release

# Binary will be at target/release/statusline
```

## Usage

### Standalone
```bash
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | statusline
```

### With Claude Code
The statusline automatically integrates with Claude Code when installed via the installer script.

### Example Output
```
~/myproject [main +2 ~1 ?3] • 45% Sonnet • 1h 23m • +150 -42 • $3.50
```

This shows:
- Working in `~/myproject` directory
- On `main` git branch with 2 added, 1 modified, 3 untracked files
- Using 45% of context window
- Running Claude Sonnet model
- Session has been active for 1 hour 23 minutes
- Added 150 lines and removed 42 lines
- Current session cost is $3.50

## Configuration

### Claude Code Integration

Claude Code stores its configuration in a fixed location:

```bash
# Configuration file (always here, regardless of environment variables)
~/.claude/settings.json
```

#### Manual Configuration

If the installer doesn't configure automatically, add this to your Claude Code config:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/statusline-wrapper.sh",
    "padding": 0
  }
}
```

Add to Claude Code settings:
```bash
jq '. + {"statusLine": {"type": "command", "command": "~/.local/bin/statusline-wrapper.sh", "padding": 0}}' ~/.claude/settings.json > /tmp/settings.json && mv /tmp/settings.json ~/.claude/settings.json
```

### Theme Support

The statusline automatically adapts its colors based on your terminal theme for optimal visibility.

#### Setting Your Theme
- **Dark Mode (default)**: Best for dark terminals and Claude's dark theme
- **Light Mode**: Optimized for light backgrounds

To set your theme, export the `CLAUDE_THEME` environment variable:
```bash
# For dark terminals (default)
export CLAUDE_THEME=dark

# For light terminals
export CLAUDE_THEME=light

# Add to your shell profile (~/.bashrc or ~/.zshrc) to make it permanent
echo 'export CLAUDE_THEME=dark' >> ~/.bashrc
```

#### Color Coding

**Dark Theme Colors:**
- **Directory**: Cyan
- **Git Info**: Green
- **Context Usage**:
  - Red (≥90%) - Critical
  - Orange (≥70%) - Warning
  - Yellow (≥50%) - Caution
  - White (<50%) - Normal (high contrast for dark backgrounds)
- **Model Name**: Cyan
- **Session Duration**: Light gray
- **Lines Changed**: Green (+added) / Red (-removed)
- **Cost**:
  - Green (<$5) - Low cost
  - Yellow ($5-$20) - Medium cost
  - Red (≥$20) - High cost

**Light Theme Colors:**
- **Directory**: Cyan
- **Git Info**: Green
- **Context Usage**:
  - Red (≥90%) - Critical
  - Orange (≥70%) - Warning
  - Yellow (≥50%) - Caution
  - Gray (<50%) - Normal (appropriate for light backgrounds)
- **Model Name**: Cyan
- **Session Duration**: Light gray
- **Lines Changed**: Green (+added) / Red (-removed)
- **Cost**:
  - Green (<$5) - Low cost
  - Yellow ($5-$20) - Medium cost
  - Red (≥$20) - High cost

### JSON Input Format
```json
{
  "workspace": {
    "current_dir": "/path/to/directory"
  },
  "model": {
    "display_name": "Claude Sonnet 3.5"
  },
  "session_id": "optional-session-id",
  "transcript_path": "/path/to/transcript.jsonl",
  "cost": {
    "total_cost_usd": 3.50,
    "total_lines_added": 150,
    "total_lines_removed": 42
  }
}
```

## Development

### Makefile Targets

The project includes a comprehensive Makefile with these targets:

| Target | Description |
|--------|-------------|
| `make` or `make all` | Default: fetch source and build |
| `make fetch-source` | Download and patch original source |
| `make verify-source` | Verify SHA256 hash of source |
| `make build` | Build release binary |
| `make debug` | Build debug binary |
| `make release-optimized` | Build with maximum optimizations |
| `make install` | Install to ~/.local/bin |
| `make test` | Run test suite |
| `make bench` | Run performance benchmarks |
| `make dev` | Clean, build, and test |
| `make size` | Compare binary sizes |
| `make clean` | Remove all artifacts and source |
| `make clean-whitespace` | Remove trailing whitespace from all project files |
| `make update-patch` | Generate new patch from current source |
| `make help` | Show all available targets |

### Project Structure
```
claude-statusline/
├── statusline.patch         # Our modifications to original code
├── SOURCE_VERSION.md        # Documents exact version and SHA256 hash
├── LICENSE                  # MIT License (our contributions only)
├── NOTICE                   # Attribution to original author
├── Cargo.toml              # Rust dependencies
├── Makefile                # Build automation with SHA256 validation
├── install-claude-code.sh  # Automated installer
├── statusline-wrapper.sh   # JSON format adapter (camelCase → snake_case)
├── claude-settings-example.json # Example Claude Code config
├── README.md               # This file
└── .gitignore              # Excludes generated files
```

Note: `statusline.rs` is generated from the original gist with patches applied.

### Building from Source
```bash
# Debug build
make debug

# Release build with optimizations
make build        # or
make release      # Standard release
make release-optimized  # Maximum optimizations

# Development workflow (clean, build, test)
make dev

# Run tests
make test

# Clean build artifacts AND source file
make clean

# Compare binary sizes
make size
```

### Testing
```bash
# Run basic tests
make test

# Benchmark performance
make bench

# Test the patch system
make clean && make fetch-source

# Manual testing with sample inputs
echo '{}' | ./target/release/statusline
echo '{"workspace":{"current_dir":"/tmp"}}' | ./target/release/statusline
echo '{"model":{"display_name":"Claude Sonnet"}}' | ./target/release/statusline

# Test with different themes
export CLAUDE_THEME=dark
echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Opus"}}' | ./target/release/statusline

export CLAUDE_THEME=light
echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Opus"}}' | ./target/release/statusline

# Test with Claude Code format (snake_case - what Claude actually sends)
echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Opus"}}' | ./statusline-wrapper.sh

# Test with cost tracking
echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet"},"cost":{"total_cost_usd":3.50,"total_lines_added":150,"total_lines_removed":42}}' | ./target/release/statusline

# Test with different cost levels
# Low cost (green)
echo '{"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":2.50}}' | ./target/release/statusline

# Medium cost (yellow)
echo '{"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":12.00}}' | ./target/release/statusline

# High cost (red)
echo '{"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":25.00}}' | ./target/release/statusline

# Test with lines changed only
echo '{"workspace":{"current_dir":"/tmp"},"cost":{"total_lines_added":500,"total_lines_removed":100}}' | ./target/release/statusline
```

## Customization

### Modifying Colors
Edit the `Colors` struct in `statusline.rs`:
```rust
impl Colors {
    const CYAN: &'static str = "\x1b[36m";
    const GREEN: &'static str = "\x1b[32m";
    // ... adjust as needed
}
```

### Changing Context Limits
Update token limit in `calculate_context_usage()`:
```rust
// Default is 160000 tokens
latest_usage = Some((total * 100.0 / 160000.0).min(100.0));
```

## Documentation

- [README.md](README.md) - This file, main documentation
- [LICENSE](LICENSE) - MIT License for our contributions with important clarifications
- [NOTICE](NOTICE) - Attribution and copyright notices
- [SOURCE_VERSION.md](SOURCE_VERSION.md) - Source version and hash documentation

## Changelog

### v1.2.1 (2025-08-24)
- **Fixed bullet separator visibility** - Now uses light gray in dark theme for better visibility
- All separator bullets are now theme-aware for optimal contrast

### v1.2.0 (2025-08-24)
- **CRITICAL FIX**: Fixed wrapper script JSON format handling (Claude sends snake_case, not camelCase)
- Added automatic debug wrapper creation during installation
- Enhanced troubleshooting documentation and installer output
- Improved error messages emphasizing Claude Code restart requirement
- Updated all documentation to reflect correct JSON format

### v1.1.0 (2025-08-23)
- Added cost tracking feature - displays session cost in USD with color-coded thresholds
- Added lines changed tracking - shows added/removed line counts
- Enhanced display logic for multiple optional components
- Updated wrapper scripts to handle cost object conversion
- Improved component separation with conditional bullet points
- Binary size increased slightly to ~529KB

### v1.0.0 (2025-08-22)
- Initial release with core features
- Git integration with detailed file status
- Context usage tracking with color warnings
- Model detection (Opus/Sonnet/Haiku)
- Session duration tracking
- Theme support for dark/light terminals
- SHA256 source validation
- Patch-based build system

## Contributing

Contributions are welcome! Please:
1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests with `make test`
5. Submit a pull request

## Performance

- **CPU Usage**: <0.1% (minimal overhead)
- **Execution Time**: ~5ms average
- **Memory Usage**: ~2MB resident
- **Binary Size**: ~529KB (release build with optimizations)
- **Update Frequency**: Every 300ms in Claude Code
- **Transcript Processing**: Only reads last 50 lines for efficiency

## Troubleshooting

### Common Issues

**Statusline only shows "~" (most common issue)**
- This means Claude Code is sending JSON but the wrapper isn't parsing it correctly
- **Quick Fix**: Re-run the installer to get the updated wrapper:
  ```bash
  git pull && ./install-claude-code.sh
  ```
- **Debug Mode** (to see what Claude is sending):
  ```bash
  # Switch to debug wrapper
  jq '.statusLine.command = "~/.local/bin/statusline-wrapper-debug.sh"' ~/.claude/settings.json > /tmp/config.tmp && mv /tmp/config.tmp ~/.claude/settings.json

  # Restart Claude Code, then check the log
  cat ~/.cache/statusline-debug.log
  ```
- **Manual Test**:
  ```bash
  echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet"}}' | ~/.local/bin/statusline
  ```

**Build fails with "Hash mismatch!"**
- The original gist may have been updated
- Check SOURCE_VERSION.md for the expected version
- Report an issue if the gist has changed

**Statusline not displaying at all**
- Ensure binary is in PATH: `export PATH="$HOME/.local/bin:$PATH"`
- Check executable permissions: `chmod 755 ~/.local/bin/statusline ~/.local/bin/statusline-wrapper.sh`
- **MUST RESTART CLAUDE CODE** after installation

**Git status not showing**
- Verify you're in a git repository: `git rev-parse --is-inside-work-tree`
- Check git is installed and in PATH: `which git`

**Context usage shows 0%**
- Verify transcript_path points to valid JSONL file
- Check file contains assistant messages with usage data
- Ensure transcript file is readable: `ls -la /path/to/transcript.jsonl`

**Claude Code integration not working**
- Check your config: `cat ~/.claude/settings.json | jq '.statusLine'`
- Verify wrapper script exists: `ls -la ~/.local/bin/statusline-wrapper.sh`
- Test wrapper manually: `echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet"}}' | ~/.local/bin/statusline-wrapper.sh`
- Test binary directly: `echo '{"workspace":{"current_dir":"/tmp"}}' | ~/.local/bin/statusline`
- Ensure jq is installed: `which jq`
- **IMPORTANT**: Restart Claude Code after installation or configuration changes

**Cost tracking not showing**
- Ensure you're using the latest wrapper scripts (updated 2025-08-23)
- Check if cost data is being sent: Use debug wrapper to see JSON input
- Update wrapper scripts: `./install-claude-code.sh`
- Cost only appears if Claude Code sends cost data in the JSON

## How It Works

This project respects the original author's work by using a patch-based build system:

1. **Downloads** the original `statusline.rs` from [Peter Steinberger's gist](https://gist.github.com/steipete/8396e512171d31e934f0013e5651691e)
2. **Validates** SHA256 hash to ensure correct version:
   ```
   5f7851061abbd896c2d4956323fa85848df79242448019bbea7799111d3cebda
   ```
3. **Applies** a patch file (`statusline.patch`) with our modifications
4. **Builds** the modified version with Cargo

This approach ensures:
- ✅ We don't redistribute the original copyrighted code
- ✅ Only our modifications (patch file) are in the repository
- ✅ The patch is applied to the exact version it was created for
- ✅ Build failures occur if the original source changes unexpectedly
- ✅ Source integrity is cryptographically verified

## License

This project's modifications and build system are licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

**Important**: The MIT License applies ONLY to our modifications (patches, build scripts, documentation). The original statusline.rs code remains the property of Peter Steinberger and is not covered by this license.

## Credits & Acknowledgments

- **Original Author**: [Peter Steinberger (@steipete)](https://github.com/steipete)
- **Original Source**: [statusline.rs gist](https://gist.github.com/steipete/8396e512171d31e934f0013e5651691e)
- **Modifications**: Patch-based enhancements for [Claude Code](https://claude.ai/code) integration
- **Build System**: Custom Makefile with SHA256 validation
- **License**: Our modifications are MIT licensed; original code retains author's rights

## FAQ

**Q: Why does the build download code from a gist?**
A: We respect the original author's copyright. By fetching the source directly and applying patches, we only distribute our modifications, not the original code.

**Q: What if the gist changes?**
A: The build includes SHA256 hash validation. If the source changes, the build will fail with a hash mismatch error, preventing unexpected behavior.

**Q: Can I use this outside of Claude Code?**
A: Yes! The statusline binary works standalone. Just pipe JSON to it: `echo '{...}' | statusline`

**Q: How do I customize the colors?**
A: After fetching the source with `make fetch-source`, edit the colors in `statusline.rs`, then recreate the patch file.

**Q: Does this work on Windows?**
A: Not natively, but it works in WSL (Windows Subsystem for Linux) or Git Bash.

**Q: Where does Claude Code store its configuration?**
A: Claude Code always stores configuration in `~/.claude/settings.json`, regardless of environment variables or system configuration.

**Q: The installer configured the wrong location. How do I fix it?**
A: Manually add the statusLine configuration to the correct file using:
```bash
# Add statusline to Claude Code settings
jq '. + {"statusLine": {"type": "command", "command": "~/.local/bin/statusline-wrapper.sh", "padding": 0}}' ~/.claude/settings.json > /tmp/settings.json && mv /tmp/settings.json ~/.claude/settings.json
```

**Q: How much does it impact Claude Code performance?**
A: Minimal impact - uses <0.1% CPU and updates only every 300ms.

## Uninstallation

### Automated (Recommended)
```bash
./uninstall-statusline.sh
```

The uninstaller will:
- Show you the current statusLine configuration before removing it
- Offer options to automatically remove or skip settings.json modification
- Create a timestamped backup of your settings before any changes
- Preserve all other Claude Code settings
- Remove the statusline binary and wrapper scripts
- Optionally clean up debug logs

### Manual
```bash
# Remove binary and wrapper scripts
rm ~/.local/bin/statusline
rm ~/.local/bin/statusline-wrapper.sh
rm ~/.local/bin/statusline-wrapper-debug.sh

# Edit ~/.claude/settings.json and remove the "statusLine" section
# Or use jq to remove it:
jq 'del(.statusLine)' ~/.claude/settings.json > /tmp/settings.tmp && mv /tmp/settings.tmp ~/.claude/settings.json

# Clean build artifacts
make clean
```

## Support

- Report issues: [GitHub Issues](https://github.com/hagan/claude-statusline/issues)
- Claude Code docs: [Official Documentation](https://docs.anthropic.com/en/docs/claude-code)
- Original gist: [Peter Steinberger's statusline](https://gist.github.com/steipete/8396e512171d31e934f0013e5651691e)
