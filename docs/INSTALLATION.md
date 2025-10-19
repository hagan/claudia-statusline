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
- `jq` for configuration ([install jq](https://stedolan.github.io/jq/download/))

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
- **jq** for installer script ([install jq](https://stedolan.github.io/jq/download/))
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
