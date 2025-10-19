# Configuration Guide

Complete guide to configuring Claudia Statusline with all available options.

## Configuration Files

### Locations

- **Claude Code Settings**: `~/.claude/settings.json` or `~/.claude/settings.local.json`
- **Statusline Config**: `~/.config/claudia-statusline/config.toml`
- **Database**: `~/.local/share/claudia-statusline/stats.db`
- **Debug Logs** (if enabled): `~/.cache/statusline-debug.log`

### Settings Priority

1. `~/.claude/settings.local.json` (highest priority)
2. `~/.claude/settings.json`

If `settings.local.json` exists, it completely overrides `settings.json`.

## Claude Code Integration

### Basic Configuration

The installer configures this automatically. If you need to set it manually:

**File**: `~/.claude/settings.json` (or `settings.local.json`)

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/statusline",
    "padding": 0
  }
}
```

**Fields:**
- `type`: Must be `"command"` (required for Windows)
- `command`: Path to statusline binary (absolute or in PATH)
- `padding`: Vertical padding (0 = no padding)

### Using jq to Configure

```bash
# Add statusline to settings.json
jq '. + {"statusLine": {"type": "command", "command": "~/.local/bin/statusline", "padding": 0}}' \
  ~/.claude/settings.json > /tmp/settings.json && \
  mv /tmp/settings.json ~/.claude/settings.json

# Add to settings.local.json instead
jq '. + {"statusLine": {"type": "command", "command": "~/.local/bin/statusline", "padding": 0}}' \
  ~/.claude/settings.local.json > /tmp/settings.json && \
  mv /tmp/settings.json ~/.claude/settings.local.json
```

## Statusline Configuration

### Config File Location

Create `~/.config/claudia-statusline/config.toml` with your preferences.

### Complete Example

```toml
# Database Configuration
[database]
# Enable JSON backup alongside SQLite (default: true)
# Set to false for SQLite-only mode (30% faster reads)
json_backup = true

# Data retention policies (in days, 0 = keep forever)
retention_days_sessions = 90    # Keep session data for 90 days
retention_days_daily = 365      # Keep daily stats for 1 year
retention_days_monthly = 0      # Keep monthly stats forever

# Git Configuration
[git]
# Git operation timeout in milliseconds (default: 200)
# Prevents hangs on large repositories or slow filesystems
timeout_ms = 200

# Theme Configuration
# Can also be set via CLAUDE_THEME or STATUSLINE_THEME environment variables
theme = "dark"  # Options: "dark" or "light"

# Cloud Sync Configuration (requires Turso variant)
[sync]
enabled = false                  # Enable cloud sync
provider = "turso"               # Only "turso" supported currently
sync_interval_seconds = 60       # Auto-sync interval (Phase 3, not yet implemented)
soft_quota_fraction = 0.75       # Warn at 75% of Turso quota

[sync.turso]
# Turso database connection
database_url = "libsql://your-database.turso.io"
auth_token = "${TURSO_AUTH_TOKEN}"  # Environment variable or literal token
```

### Minimal Configuration

Most users don't need a config file - defaults work great! But if you want to customize:

```toml
# Minimal config for SQLite-only mode (faster)
[database]
json_backup = false
```

## Environment Variables

### Theme

```bash
# Dark theme (default)
export CLAUDE_THEME=dark

# Light theme
export CLAUDE_THEME=light

# Alternative variable name
export STATUSLINE_THEME=dark
```

### Colors

```bash
# Disable all ANSI colors
export NO_COLOR=1
```

### Git Timeout

```bash
# Override git timeout (milliseconds)
export STATUSLINE_GIT_TIMEOUT_MS=500
```

### Logging

```bash
# Set log level (default: warn)
export RUST_LOG=info        # Show info logs
export RUST_LOG=debug       # Show debug logs
export RUST_LOG=trace       # Show all logs

# Module-specific logging
export RUST_LOG=statusline::stats=debug  # Debug stats module only
```

### Turso Sync (Turso variant only)

```bash
# Store Turso auth token in environment
export TURSO_AUTH_TOKEN="your-token-here"

# Then reference in config.toml:
# auth_token = "${TURSO_AUTH_TOKEN}"
```

## CLI Flags

Command-line flags override environment variables and config file settings.

### Theme Override

```bash
# Use light theme
statusline --theme light

# Use dark theme
statusline --theme dark
```

### Disable Colors

```bash
# Disable colors (overrides NO_COLOR env)
statusline --no-color
```

### Custom Config File

```bash
# Use alternate config file
statusline --config /path/to/config.toml
```

### Log Level Override

```bash
# Override RUST_LOG environment variable
statusline --log-level debug
statusline --log-level info
statusline --log-level warn
statusline --log-level error
statusline --log-level trace
```

## Configuration Precedence

Order of precedence (highest to lowest):

1. **CLI flags** (`--theme`, `--no-color`, `--config`, `--log-level`)
2. **Environment variables** (`CLAUDE_THEME`, `NO_COLOR`, `RUST_LOG`, etc.)
3. **Config file** (`~/.config/claudia-statusline/config.toml`)
4. **Built-in defaults**

Example:
```bash
# This will use light theme, even if config.toml says dark
statusline --theme light < input.json
```

## Theme Customization

### Dark Theme (Default)

Optimized for dark terminals and Claude's dark theme:

- **Directory**: Cyan
- **Git branch**: Green
- **Context usage**:
  - Red (≥90%) - Critical
  - Orange (≥70%) - Warning
  - Yellow (≥50%) - Caution
  - White (<50%) - Normal
- **Model name**: Cyan
- **Session duration**: Light gray
- **Lines changed**: Green (+) / Red (-)
- **Cost**:
  - Green (<$5)
  - Yellow ($5-$20)
  - Red (≥$20)

### Light Theme

Optimized for light backgrounds:

- Same as dark theme except context <50% uses gray instead of white

### Customizing Colors

Colors are hardcoded in `src/display.rs`. To change:

1. Clone the repository
2. Edit `src/display.rs`:
   ```rust
   impl Colors {
       const CYAN: &'static str = "\x1b[36m";      // Directory, model
       const GREEN: &'static str = "\x1b[32m";     // Git, +lines, low cost
       const RED: &'static str = "\x1b[31m";       // Critical, -lines, high cost
       const ORANGE: &'static str = "\x1b[38;5;208m";  // Warning
       const YELLOW: &'static str = "\x1b[33m";    // Caution, medium cost
       const WHITE: &'static str = "\x1b[37m";     // Normal (dark theme)
       const GRAY: &'static str = "\x1b[90m";      // Normal (light theme)
       const LIGHT_GRAY: &'static str = "\x1b[38;5;245m";  // Duration
       const RESET: &'static str = "\x1b[0m";      // Reset
   }
   ```
3. Rebuild: `cargo build --release`

## Data Retention

Configure how long to keep historical data in SQLite database.

### Default Retention

```toml
[database]
retention_days_sessions = 90    # Individual sessions: 90 days
retention_days_daily = 365      # Daily aggregates: 1 year
retention_days_monthly = 0      # Monthly aggregates: forever
```

### Custom Retention

```toml
[database]
# Aggressive pruning (minimal storage)
retention_days_sessions = 30    # Keep only 1 month
retention_days_daily = 90       # Keep 3 months
retention_days_monthly = 365    # Keep 1 year

# OR keep everything forever
retention_days_sessions = 0
retention_days_daily = 0
retention_days_monthly = 0
```

### Maintenance Schedule

Prune old data automatically with cron:

```bash
# Add to crontab (crontab -e)
# Daily maintenance at 3 AM
0 3 * * * /path/to/statusline db-maintain --quiet
```

## Database Configuration

### SQLite-Only Mode (Recommended)

For best performance, disable JSON backup:

```toml
[database]
json_backup = false
```

**Benefits:**
- ~30% faster reads
- Lower memory usage
- No JSON file I/O overhead
- Better concurrent access

**Migration:**
```bash
# Migrate to SQLite-only mode
statusline migrate --finalize
```

### Dual-Write Mode (Default)

Keep both SQLite and JSON:

```toml
[database]
json_backup = true  # Default
```

**When to use:**
- Transitioning from old versions
- Want backup in human-readable format
- Debugging or development

## Git Configuration

### Timeout Adjustment

```toml
[git]
# Increase timeout for slow filesystems or large repos
timeout_ms = 500

# Decrease for very fast local repos
timeout_ms = 100
```

```bash
# Or via environment variable
export STATUSLINE_GIT_TIMEOUT_MS=500
```

**What happens on timeout:**
- Git operations are killed after timeout
- Statusline continues without git info
- No hanging or slowdowns

## Debug Configuration

### Enable Debug Logging

```bash
# Via installer
./scripts/install-statusline.sh --with-debug-logging

# Or manually add wrapper script to ~/.claude/settings.json:
{
  "statusLine": {
    "type": "command",
    "command": "/path/to/debug-wrapper.sh",
    "padding": 0
  }
}
```

**Debug wrapper example:**
```bash
#!/bin/bash
LOG_FILE="$HOME/.cache/statusline-debug.log"
echo "[$(date)] Input:" >> "$LOG_FILE"
cat | tee -a "$LOG_FILE" | /path/to/statusline 2>> "$LOG_FILE"
```

### View Debug Logs

```bash
# Tail logs in real-time
tail -f ~/.cache/statusline-debug.log

# Clear logs
> ~/.cache/statusline-debug.log
```

## Advanced Configuration

### Context Window Limit

Default is 160,000 tokens. To change, edit `src/utils.rs` and rebuild:

```rust
// In calculate_context_usage() function
latest_usage = Some((total * 100.0 / 160000.0).min(100.0));
//                                    ^^^^^^ Change this value
```

### Progress Bar Width

Default is 10 characters. To change, edit `src/display.rs` and rebuild:

```rust
// In create_progress_bar() function
fn create_progress_bar(percentage: f64, width: usize) -> String {
    // Default width is 10, change when calling:
    let bar = create_progress_bar(percentage, 15);  // 15 chars instead
}
```

### Burn Rate Display

Burn rate only shows after 1 minute. To change threshold, edit `src/display.rs`:

```rust
fn format_burn_rate(cost: f64, hours: f64) -> String {
    if hours < 0.0167 { // Less than 1 minute (0.0167 hours)
        return String::new();
    }
    // ...
}
```

## XDG Base Directory Specification

Statusline follows XDG standards. You can override locations:

```bash
# Override config directory
export XDG_CONFIG_HOME=~/my-config
# Config will be at: ~/my-config/claudia-statusline/config.toml

# Override data directory
export XDG_DATA_HOME=~/my-data
# Database will be at: ~/my-data/claudia-statusline/stats.db

# Override cache directory
export XDG_CACHE_HOME=~/my-cache
# Logs will be at: ~/my-cache/statusline-debug.log
```

## Troubleshooting Configuration

### Check Current Configuration

```bash
# Show where config would be loaded from
statusline health --json | jq '.config_path'

# Check if config file exists
ls -la ~/.config/claudia-statusline/config.toml

# Validate config syntax
# (no built-in validator yet, check for TOML syntax errors manually)
```

### Test Configuration

```bash
# Test with specific theme
statusline --theme light <<< '{"workspace":{"current_dir":"'$(pwd)'"}}'

# Test with no colors
statusline --no-color <<< '{"workspace":{"current_dir":"'$(pwd)'"}}'

# Test with custom config
statusline --config /path/to/test-config.toml <<< '{"workspace":{"current_dir":"'$(pwd)'"}}'
```

### Common Issues

**Config not being loaded:**
- Check file path: `~/.config/claudia-statusline/config.toml`
- Check TOML syntax (no syntax validator built-in)
- Check permissions: `chmod 644 ~/.config/claudia-statusline/config.toml`

**Settings.json changes not applied:**
- Restart Claude Code after any settings changes
- Check for typos in JSON syntax
- Verify path to statusline binary is correct

**Environment variables not working:**
- Check variable is exported: `export CLAUDE_THEME=light`
- Restart shell/terminal after setting
- Verify with: `echo $CLAUDE_THEME`

## Next Steps

- See [USAGE.md](USAGE.md) for command usage and examples
- See [CLOUD_SYNC.md](CLOUD_SYNC.md) for cloud sync configuration
- See [INSTALLATION.md](INSTALLATION.md) for installation options
