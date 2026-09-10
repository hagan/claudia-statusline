# Installation Guide

Complete installation instructions for all platforms and scenarios.

## Quick Install (Recommended)

The fastest way to install for most users:

```bash
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

Or with wget:
```bash
wget -qO- https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

The installer will:
1. ✅ Detect your OS and architecture automatically
2. ✅ Download the appropriate pre-built binary
3. ✅ Install to `~/.local/bin/statusline`
4. ✅ Configure Claude Code settings automatically
5. ✅ Check your PATH configuration
6. ✅ Verify the installation works

**Requirements:**
- `curl` or `wget`
- `python3` or `jq` (optional - for auto-configuration; installer will give manual instructions if neither available)

## Platform-Specific Installation

### Linux (x86_64)

**User-local install:**
```bash
curl -L https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-linux-amd64.tar.gz | tar xz
mkdir -p "$HOME/.local/bin"
install -m 755 statusline "$HOME/.local/bin/statusline"

# Ensure ~/.local/bin is on your PATH
case :$PATH: in
  *:"$HOME/.local/bin":*) ;; # already present
  *) echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc";;
esac
```

**System-wide install:**
```bash
curl -L https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-linux-amd64.tar.gz | tar xz
sudo install -m 755 statusline /usr/local/bin/statusline
```

### Linux (ARM64)

```bash
curl -L https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-linux-arm64.tar.gz | tar xz
mkdir -p "$HOME/.local/bin"
install -m 755 statusline "$HOME/.local/bin/statusline"
```

### macOS (Intel)

```bash
curl -L https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-darwin-amd64.tar.gz | tar xz
mkdir -p "$HOME/.local/bin"
install -m 755 statusline "$HOME/.local/bin/statusline"

# Remove quarantine attribute (macOS Gatekeeper)
xattr -d com.apple.quarantine "$HOME/.local/bin/statusline" 2>/dev/null || true
```

**Add to PATH (zsh is default on macOS):**
```bash
# For login shells (Terminal/iTerm)
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zprofile"

# For interactive shells
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"

# Reload
source "$HOME/.zprofile"
source "$HOME/.zshrc"
```

### macOS (Apple Silicon)

```bash
curl -L https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-darwin-arm64.tar.gz | tar xz
mkdir -p "$HOME/.local/bin"
install -m 755 statusline "$HOME/.local/bin/statusline"

# Remove quarantine attribute
xattr -d com.apple.quarantine "$HOME/.local/bin/statusline" 2>/dev/null || true
```

### Windows (x86_64)

**PowerShell installation:**

```powershell
# Create user-local bin directory
New-Item -ItemType Directory -Force "$env:USERPROFILE\bin" | Out-Null

# Download and extract
Invoke-WebRequest -Uri "https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-windows-amd64.zip" -OutFile "$env:TEMP\statusline.zip"
Expand-Archive -Force "$env:TEMP\statusline.zip" "$env:TEMP\statusline"
Copy-Item "$env:TEMP\statusline\statusline.exe" "$env:USERPROFILE\bin\statusline.exe" -Force

# Add to user PATH
[Environment]::SetEnvironmentVariable(
  'Path',
  "$env:USERPROFILE\bin;" + [Environment]::GetEnvironmentVariable('Path', 'User'),
  'User'
)

# Configure Claude Code
$settingsPath = "$env:USERPROFILE\.claude\settings.json"
$settings = @{
    statusLine = @{
        type = "command"
        command = "$env:USERPROFILE\bin\statusline.exe"
        padding = 0
    }
} | ConvertTo-Json -Depth 10
$settings | Out-File -FilePath $settingsPath -Encoding UTF8

# Verify (in new terminal)
statusline --version
```

**Important**: The `"type": "command"` field is required in settings.json for Windows.

See [WINDOWS_BUILD.md](../WINDOWS_BUILD.md) for detailed Windows instructions and troubleshooting.

## Building from Source

### Prerequisites

- **Rust toolchain** 1.70+ ([install Rust](https://rustup.rs/))
- **Make** (optional but recommended)
- **Git** (optional, for repository status features)

### Clone and Install

```bash
git clone https://github.com/hagan/claudia-statusline
cd claudia-statusline

# Automated installation
./scripts/install-statusline.sh

# Or with options
./scripts/install-statusline.sh --help           # Show all options
./scripts/install-statusline.sh --dry-run        # Preview what will be done
./scripts/install-statusline.sh --verbose        # Detailed output
./scripts/install-statusline.sh --prefix /usr/local/bin  # Custom install location
```

### Manual Build

```bash
# Build with Make
make build          # Release build
make install        # Build and install to ~/.local/bin

# Build with Cargo directly
cargo build --release

# Binary will be at:
./target/release/statusline
```

### Build Turso Sync Variant

```bash
# Build with cloud sync features
cargo build --release --features turso-sync

# Install
cargo install --path . --features turso-sync
```

## Upgrading

### Upgrade Pre-built Binary

```bash
# Linux (x86_64 example)
tmpdir=$(mktemp -d) && cd "$tmpdir"
curl -L https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-linux-amd64.tar.gz | tar xz
install -m 755 statusline "$HOME/.local/bin/statusline" 2>/dev/null || sudo install -m 755 statusline /usr/local/bin/statusline

# Verify new version
statusline --version
```

### Upgrade from Source

```bash
cd claudia-statusline
git pull
./scripts/install-statusline.sh --skip-config
```

## Uninstallation

### Automated Uninstaller

```bash
./scripts/uninstall-statusline.sh

# Options:
./scripts/uninstall-statusline.sh --help         # Show options
./scripts/uninstall-statusline.sh --dry-run      # Preview removal
./scripts/uninstall-statusline.sh --force        # Skip prompts
./scripts/uninstall-statusline.sh --keep-logs    # Keep debug logs
./scripts/uninstall-statusline.sh --skip-config  # Keep Claude settings
```

The uninstaller will:
- Show current statusLine configuration
- Offer to remove or keep settings.json changes
- Create timestamped backup before any modifications
- Preserve all other Claude settings
- Remove binary and optional debug logs

### Manual Uninstallation

```bash
# Remove binary
rm ~/.local/bin/statusline

# Remove data (optional)
rm -rf ~/.local/share/claudia-statusline/
rm -rf ~/.config/claudia-statusline/

# Edit ~/.claude/settings.json and remove "statusLine" section
# Or use jq:
jq 'del(.statusLine)' ~/.claude/settings.json > /tmp/settings.tmp && mv /tmp/settings.tmp ~/.claude/settings.json
```

## Troubleshooting

### "statusline not found"

**Cause**: `~/.local/bin` not in PATH

**Fix**:
```bash
# For bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
source "$HOME/.bashrc"

# For zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"
source "$HOME/.zshrc"

# Verify
which statusline
```

### macOS Gatekeeper Error

**Error**: "statusline cannot be opened because it is from an unidentified developer"

**Fix**:
```bash
# Remove quarantine attribute
xattr -d com.apple.quarantine ~/.local/bin/statusline

# OR allow in System Settings
# System Settings → Privacy & Security → Allow anyway
```

### macOS PATH Issues

If `statusline` command not found after installation:

```bash
# Add to both login and interactive shells
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zprofile"
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"

# Reload
source "$HOME/.zprofile"
source "$HOME/.zshrc"

# Verify
which statusline && statusline --version
```

### Windows PATH Not Updated

**Fix**: Restart PowerShell/Terminal after installation to pick up PATH changes.

### Installer Fails to Configure Claude

**Fix**: Manually add to `~/.claude/settings.json`:
```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/statusline",
    "padding": 0
  }
}
```

Or use jq:
```bash
jq '. + {"statusLine": {"type": "command", "command": "~/.local/bin/statusline", "padding": 0}}' ~/.claude/settings.json > /tmp/settings.json && mv /tmp/settings.json ~/.claude/settings.json
```

## ant Enrichment Refresh (Optional)

The optional `ant` enrichment (model-metadata + per-account usage/cost, off by default)
reads from a local cache that you keep fresh out-of-band. **Install is docs-only:** the
installer touches **no** refresh wiring, and there is **no shipped wrapper script** — the
recipes below are raw commands you add yourself.

### The credential-reality split

Three caches are refreshed by different mechanisms, because they need different credentials:

- **Usage** (`ant sync-usage`) is refreshed by a **SessionStart hook**, because the
  per-account **Admin key** lives in your interactive, env-swapped shell — exactly where the
  hook runs.
- **Models** (`ant sync-models`) is refreshed by **cron / launchd**, because it uses the
  **standard key**, which is global and headless-safe. Usage is deliberately **NOT** wired
  into cron/launchd (the Admin key is not available in a headless environment).
- **Prices** (`ant sync-pricing`) is **keyless**. Its upstream is a public LiteLLM snapshot on
  `raw.githubusercontent.com`, so the command reads no Anthropic credential of any kind — it is
  the only `ant sync-*` command with no credential constraint, and therefore safe in **all
  three** mechanisms: SessionStart hook, cron and launchd. It is also independent of
  `[ant] enabled`; pricing is a separate feature that merely shares the cache directory.
  **If you never schedule it, `[pricing] source = "auto"` has no cache to prefer and every
  render falls back to the price table compiled into the binary.**

**The refresh commands are independent — never chain them.** Write each one as its own,
individually detached invocation: `&`-separated commands in a raw shell hook, one object per
command in the native `async` array, a separate crontab line per command, a separate launchd job
per command. Never join them with `&&`. A missing Admin key failing `sync-usage`, or a failing
`sync-models`, must not be able to prevent the keyless price refresh from running.

`--max-age` makes each command self-throttling: it skips the network fetch if the cache is
younger than the given window. A manual `ant sync-models` / `ant sync-usage` / `ant sync-pricing`
**without** `--max-age` always fetches.

### SessionStart hook (refreshes usage and prices; runs at every session start)

Add to `~/.claude/settings.json`. **Always** pass `--quiet` AND redirect stdio
(`>/dev/null 2>&1`) AND detach (`&`): SessionStart **stdout becomes Claude's context**, so an
un-redirected sync summary would be injected into the conversation. Detaching keeps session
start instant.

```jsonc
// ~/.claude/settings.json — SessionStart hook (raw shell, detached + redirected).
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "statusline ant sync-usage --quiet --max-age 10m >/dev/null 2>&1 & statusline ant sync-models --quiet --max-age 24h >/dev/null 2>&1 & statusline ant sync-pricing --quiet --max-age 7d >/dev/null 2>&1 &"
          }
        ]
      }
    ]
  }
}
```

Cleaner alternative — Claude Code supports a native `async: true` field for background hooks
(no shell detach needed):

```jsonc
// ~/.claude/settings.json — native async background hooks.
{
  "hooks": {
    "SessionStart": [
      { "hooks": [
        { "type": "command", "async": true, "command": "statusline ant sync-usage --quiet --max-age 10m" },
        { "type": "command", "async": true, "command": "statusline ant sync-models --quiet --max-age 24h" },
        { "type": "command", "async": true, "command": "statusline ant sync-pricing --quiet --max-age 7d" }
      ]}
    ]
  }
}
```

Usage is refreshed on a tight 10m window (cost figures move during a session); models on 24h
(metadata changes rarely); prices on 7d.

**Why `7d` for prices.** The render side treats a synced price cache as fresh for
`[pricing] max_age`, which defaults to `30d`, so a 7d refresh window keeps the cache comfortably
inside that freshness window even if a run or two is missed. `--max-age` also makes repeated hook
firings free: when the cache on disk is younger than 7d, `ant sync-pricing` skips the fetch
entirely, touches no network, and exits 0.

### launchd plist (macOS — refreshes models daily)

Use the **absolute** binary path: launchd jobs get a minimal `PATH` and will not find
`statusline` otherwise. Save as `~/Library/LaunchAgents/com.claudia.statusline.sync-models.plist`
and load with `launchctl load ~/Library/LaunchAgents/com.claudia.statusline.sync-models.plist`.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.claudia.statusline.sync-models</string>
  <key>ProgramArguments</key>
  <array>
    <!-- ABSOLUTE path: launchd has a minimal PATH. -->
    <string>/Users/USERNAME/.local/bin/statusline</string>
    <string>ant</string><string>sync-models</string>
    <string>--quiet</string><string>--max-age</string><string>24h</string>
  </array>
  <!-- Daily at 09:00 local. -->
  <key>StartCalendarInterval</key><dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
  <key>RunAtLoad</key><false/>
  <!-- If the standard key is env-based, supply it here (the models key is headless-safe): -->
  <!-- <key>EnvironmentVariables</key><dict><key>ANTHROPIC_API_KEY</key><string>...</string></dict> -->
  <key>StandardOutPath</key><string>/tmp/statusline-sync-models.log</string>
  <key>StandardErrorPath</key><string>/tmp/statusline-sync-models.err</string>
</dict>
</plist>
```

### launchd plist (macOS — refreshes prices weekly)

A **separate** job, not extra arguments on the models job: the price refresh is keyless and must
keep running even if the models job is failing. Save as
`~/Library/LaunchAgents/com.claudia.statusline.sync-pricing.plist` and load with
`launchctl load ~/Library/LaunchAgents/com.claudia.statusline.sync-pricing.plist`.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.claudia.statusline.sync-pricing</string>
  <key>ProgramArguments</key>
  <array>
    <!-- ABSOLUTE path: launchd has a minimal PATH. -->
    <string>/Users/USERNAME/.local/bin/statusline</string>
    <string>ant</string><string>sync-pricing</string>
    <string>--quiet</string><string>--max-age</string><string>7d</string>
  </array>
  <!-- Mondays at 09:05 local (Weekday 1). -->
  <key>StartCalendarInterval</key><dict><key>Weekday</key><integer>1</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>5</integer></dict>
  <key>RunAtLoad</key><false/>
  <!-- No EnvironmentVariables block: sync-pricing reads no credential. -->
  <key>StandardOutPath</key><string>/tmp/statusline-sync-pricing.log</string>
  <key>StandardErrorPath</key><string>/tmp/statusline-sync-pricing.err</string>
</dict>
</plist>
```

### cron lines (Linux — refreshes models daily, prices weekly)

Absolute path again (cron has a minimal `PATH`). Two **separate** lines, so a failing models
refresh cannot stop the keyless price refresh:

```cron
# crontab -e — daily models refresh at 09:00.
0 9 * * * /home/USERNAME/.local/bin/statusline ant sync-models --quiet --max-age 24h >/dev/null 2>&1

# crontab -e — weekly price refresh, Mondays at 09:05. No API key needed.
5 9 * * 1 /home/USERNAME/.local/bin/statusline ant sync-pricing --quiet --max-age 7d >/dev/null 2>&1
```

## Verification

After installation, verify everything works:

```bash
# Check version
statusline --version

# Check health
statusline health

# Test with sample input
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | statusline
```

Expected output should show formatted statusline with current directory.

## Next Steps

- See [USAGE.md](USAGE.md) for usage examples and commands
- See [CONFIGURATION.md](CONFIGURATION.md) for customization options
- See [CLOUD_SYNC.md](CLOUD_SYNC.md) for cloud sync setup (optional)
