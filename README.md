# Claudia Statusline

*Enhanced statusline for Claude Code - track costs, git status, and context usage in real-time*

[![Mentioned in Awesome Claude Code](https://awesome.re/mentioned-badge.svg)](https://github.com/hesreallyhim/awesome-claude-code)

![Claudia Statusline Screenshot](statusline.png)

A high-performance statusline for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) that shows you workspace info, git status, model usage, session costs, and more.

**Example output:**
```
~/myproject [main +2 ~1 ?3] • 45% [====------] Sonnet • 1h 23m • +150 -42 • $3.50 ($2.54/h)
```

## Quick Install

```bash
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

**That's it!** The installer downloads the right binary, installs it, and configures Claude Code automatically.

**Requirements**: `curl` and `jq` ([install jq](https://stedolan.github.io/jq/download/))

**Need help?** See [Installation Guide](docs/INSTALLATION.md) for all platforms and options.

## What You Get

- 📁 **Current directory** with `~` shorthand
- 🌿 **Git branch and changes** (+2 added, ~1 modified, ?3 untracked)
- 📊 **Context usage** with progress bar (45% [====------])
- 🤖 **Claude model** (Opus/Sonnet/Haiku)
- ⏱️ **Session duration** (1h 23m)
- 💰 **Cost tracking** ($3.50 session, $2.54/hour burn rate)
- 📝 **Lines changed** (+150 added, -42 removed)

**Automatic features:**
- ✅ Persistent cost tracking across sessions
- ✅ Multi-console safe (run multiple Claude instances)
- ✅ Dark/light theme support
- ✅ SQLite database for reliability
- ✅ No configuration needed

## Documentation

- 📦 **[Installation Guide](docs/INSTALLATION.md)** - All platforms, build from source, troubleshooting
- 📖 **[Usage Guide](docs/USAGE.md)** - Commands, examples, JSON format, embedding API
- ⚙️ **[Configuration Guide](docs/CONFIGURATION.md)** - Themes, retention, git timeout, advanced settings
- ☁️ **[Cloud Sync Guide](docs/CLOUD_SYNC.md)** - Turso setup for cross-machine stats (experimental)
- 🗄️ **[Database Migrations](docs/DATABASE_MIGRATIONS.md)** - Schema versioning and migrations

**Project docs:**
- 🏗️ **[ARCHITECTURE.md](ARCHITECTURE.md)** - Technical architecture and module design
- 🤝 **[CONTRIBUTING.md](CONTRIBUTING.md)** - Development guidelines and debugging
- 🔒 **[SECURITY.md](SECURITY.md)** - Security policies and vulnerability reporting
- 📝 **[CHANGELOG.md](CHANGELOG.md)** - Version history and release notes
- 🪟 **[WINDOWS_BUILD.md](WINDOWS_BUILD.md)** - Windows-specific build instructions

## Quick Start

### 1. Install

```bash
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

### 2. Restart Claude Code

The statusline appears automatically - no configuration needed!

### 3. (Optional) Customize

```bash
# Change theme
export CLAUDE_THEME=light  # or dark (default)

# Disable colors
export NO_COLOR=1

# Advanced config
vim ~/.config/claudia-statusline/config.toml
```

See [Configuration Guide](docs/CONFIGURATION.md) for all options.

## Common Questions

<details>
<summary><b>How much does it cost?</b></summary>

Nothing! It's free and open source (MIT license). The cost tracking shows your Claude API usage costs.
</details>

<details>
<summary><b>Will this slow down Claude Code?</b></summary>

No. The binary is designed to refresh quickly while staying out of the way—the hot path completes in a few milliseconds on typical hardware and keeps CPU usage negligible.
</details>

<details>
<summary><b>Where is my data stored?</b></summary>

Locally in `~/.local/share/claudia-statusline/stats.db`. Nothing leaves your machine unless you enable cloud sync.
</details>

<details>
<summary><b>Can I sync stats across machines?</b></summary>

Yes! Download the [Turso variant](https://github.com/hagan/claudia-statusline/releases/latest) and see [Cloud Sync Guide](docs/CLOUD_SYNC.md) for setup.
</details>

<details>
<summary><b>Does this work on Windows?</b></summary>

Yes! Download the [Windows binary](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-windows-amd64.zip) and see [Windows Guide](WINDOWS_BUILD.md).
</details>

<details>
<summary><b>How do I uninstall?</b></summary>

```bash
./scripts/uninstall-statusline.sh
# Or manually: rm ~/.local/bin/statusline
```

See [Installation Guide](docs/INSTALLATION.md#uninstallation) for details.
</details>

## Manual Download

Download for your platform from [latest release](https://github.com/hagan/claudia-statusline/releases/latest):

| Platform | Standard | Turso Sync |
|----------|----------|------------|
| **Linux x86_64** | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-linux-amd64.tar.gz) | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-linux-amd64.tar.gz) |
| **macOS Intel** | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-darwin-amd64.tar.gz) | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-darwin-amd64.tar.gz) |
| **macOS Apple Silicon** | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-darwin-arm64.tar.gz) | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-darwin-arm64.tar.gz) |
| **Windows** | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-windows-amd64.zip) | [Download](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-windows-amd64.zip) |

**Standard**: Local-only, recommended for most users
**Turso Sync**: Includes cloud sync features (experimental, requires setup)

Then extract and install:
```bash
tar xzf statusline-*.tar.gz
mv statusline ~/.local/bin/
```

See [Installation Guide](docs/INSTALLATION.md) for detailed instructions.

## Troubleshooting

**"statusline not found"** after install?
```bash
export PATH="$HOME/.local/bin:$PATH"
# Add to ~/.bashrc or ~/.zshrc to persist
```

**macOS says "cannot be opened"?**
```bash
xattr -d com.apple.quarantine ~/.local/bin/statusline
```

**Statusline shows only "~"?**
```bash
# Re-run installer to fix configuration
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

**More help?** See [Installation Guide](docs/INSTALLATION.md#troubleshooting) and [Usage Guide](docs/USAGE.md#troubleshooting)

## Advanced Features

<details>
<summary><b>📊 Database Maintenance</b></summary>

Keep your stats database optimized:

```bash
statusline db-maintain
```

Schedule with cron:
```bash
# Daily at 3 AM
0 3 * * * /path/to/statusline db-maintain --quiet
```

See [Usage Guide](docs/USAGE.md#database-maintenance) for details.
</details>

<details>
<summary><b>☁️ Cloud Sync</b></summary>

Track costs across multiple machines:

1. Download [Turso variant](https://github.com/hagan/claudia-statusline/releases/latest)
2. Create free [Turso](https://turso.tech/) account
3. Configure sync in `~/.config/claudia-statusline/config.toml`
4. Push/pull stats: `statusline sync --push` / `statusline sync --pull`

See [Cloud Sync Guide](docs/CLOUD_SYNC.md) for complete setup.
</details>

<details>
<summary><b>🔧 Building from Source</b></summary>

For developers or latest features:

```bash
git clone https://github.com/hagan/claudia-statusline
cd claudia-statusline
./scripts/install-statusline.sh

# Or manual build
cargo build --release

# Build with Turso sync
cargo build --release --features turso-sync
```

**Requirements**: Rust 1.70+ ([install](https://rustup.rs/))

See [Installation Guide](docs/INSTALLATION.md#building-from-source) for details.
</details>

<details>
<summary><b>🎨 Themes & Colors</b></summary>

```bash
# Dark theme (default)
export CLAUDE_THEME=dark

# Light theme
export CLAUDE_THEME=light

# Disable colors
export NO_COLOR=1
```

See [Configuration Guide](docs/CONFIGURATION.md#theme-customization) for customization.
</details>

## Contributing

We welcome contributions! Please see:

- 🐛 **[Issues](https://github.com/hagan/claudia-statusline/issues)** - Bug reports and feature requests
- 💬 **[Discussions](https://github.com/hagan/claudia-statusline/discussions)** - Questions and ideas
- 🤝 **[Contributing Guide](CONTRIBUTING.md)** - Development guidelines
- 🔒 **[Security Policy](SECURITY.md)** - Reporting vulnerabilities

## Credits

**Original Inspiration**: [Peter Steinberger's statusline.rs](https://gist.github.com/steipete/8396e512171d31e934f0013e5651691e)

**Current Implementation**: Complete Rust rewrite with persistent stats, cloud sync, and enhanced features.

**License**: MIT - See [LICENSE](LICENSE)

---

**Made with ❤️ for the Claude Code community**

*"Claudia" - because every AI assistant deserves a companion!*
