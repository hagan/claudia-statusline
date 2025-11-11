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

# Display Configuration
[display]
# Control which components are shown in the statusline
# All components are visible by default
show_directory = true       # Current working directory
show_git = true            # Git branch and file changes
show_context = true        # Context usage progress bar
show_model = true          # Claude model name (e.g., "S4.5")
show_duration = true       # Session duration
show_lines_changed = true  # Code additions/deletions (+123/-45)
show_cost = true           # Session and daily totals

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

## Display Component Customization

You can selectively show or hide individual components of the statusline.

### Available Components

The statusline can display up to 7 components:

1. **Directory** - Current working directory path
2. **Git** - Branch name and file changes
3. **Context** - Context usage progress bar
4. **Model** - Claude model name (e.g., "S4.5")
5. **Duration** - Session duration
6. **Lines Changed** - Code additions/deletions (+123/-45)
7. **Cost** - Session and daily totals

### Default Configuration

All components are visible by default:

```toml
[display]
show_directory = true
show_git = true
show_context = true
show_model = true
show_duration = true
show_lines_changed = true
show_cost = true
```

### Example Configurations

#### Minimal Display (Directory + Cost Only)

Perfect for focusing on costs while keeping orientation:

```toml
[display]
show_directory = true
show_git = false
show_context = false
show_model = false
show_duration = false
show_lines_changed = false
show_cost = true
```

**Output:** `~/projects/myapp • $0.25 ($3.45 today)`

#### Developer Focus (Git + Context + Lines)

Best for active development work:

```toml
[display]
show_directory = true
show_git = true
show_context = true
show_model = false
show_duration = false
show_lines_changed = true
show_cost = false
```

**Output:** `~/projects/myapp • main +2 ~1 • [====------] 42% • +123/-45`

#### Cost Tracking (Model + Duration + Cost)

For monitoring API usage and costs:

```toml
[display]
show_directory = true
show_git = false
show_context = false
show_model = true
show_duration = true
show_lines_changed = false
show_cost = true
```

**Output:** `~/projects/myapp • S4.5 • 5m • $0.25 ($3.45 today) $3.00/h`

#### Clean Minimal (Directory Only)

Maximum simplicity:

```toml
[display]
show_directory = true
show_git = false
show_context = false
show_model = false
show_duration = false
show_lines_changed = false
show_cost = false
```

**Output:** `~/projects/myapp`

### Partial Configuration

You can specify only the components you want to change. Unspecified components default to `true`:

```toml
[display]
# Only hide git info, everything else shows
show_git = false
```

### Using with Themes

Display toggles work seamlessly with theme settings:

```toml
theme = "light"

[display]
show_directory = true
show_cost = true
show_context = true
# Hide everything else
show_git = false
show_model = false
show_duration = false
show_lines_changed = false
```

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

### Context Window Configuration

**Default**: 200,000 tokens (modern Claude models: Sonnet 3.5+, Opus 3.5+, Sonnet 4.5+)

The statusline intelligently detects context window size based on model family and version:
- **Sonnet 3.5+, 4.5+**: 200k tokens
- **Opus 3.5+**: 200k tokens
- **Older models** (Sonnet 3.0, etc.): 160k tokens
- **Unknown models**: Uses default from config

#### Override Context Window Size

To override the default or set model-specific sizes, edit `~/.config/claudia-statusline/config.toml`:

```toml
[context]
# Default context window size for unknown models
window_size = 200000

# Optional: Override for specific models
[context.model_windows]
"Claude 3.5 Sonnet" = 200000
"Claude Sonnet 4.5" = 200000
"Claude 3 Haiku" = 100000
```

**Note**: The statusline automatically detects the correct window size for most models. Manual overrides are only needed for:
- Unreleased models
- Custom model configurations
- Testing purposes

#### Adaptive Context Learning (Experimental)

The statusline can **learn actual context limits** by observing your real usage patterns. When enabled, it automatically detects when Claude compacts the conversation and builds confidence in the true limit over time.

**Enable adaptive learning** in `~/.config/claudia-statusline/config.toml`:

```toml
[context]
window_size = 200000

# Adaptive Learning (Experimental)
# Learns actual context limits by observing compaction events
# Default: false (disabled)
adaptive_learning = true

# Minimum confidence score to use learned values (0.0-1.0)
# Higher = more observations required before using learned limit
# Default: 0.7 (70% confidence)
learning_confidence_threshold = 0.7
```

**How it works:**
1. Monitors token usage from Claude's transcript files
2. Detects **automatic compaction** (sudden >10% token drop after >150k tokens)
3. Filters out **manual compactions** (when you use `/compact` commands)
4. Builds **confidence** through multiple observations
5. Uses learned value when confidence ≥ threshold (default 70%)

**Priority system:**
1. **User config overrides** (`[context.model_windows]`) - highest priority
2. **Learned values** (when confident) - used if no override
3. **Intelligent defaults** (based on model family/version)
4. **Global fallback** (`window_size`) - lowest priority

**View learned data:**
```bash
statusline context-learning --status
statusline context-learning --details "Claude Sonnet 4.5"
```

**Reset learning data:**
```bash
statusline context-learning --reset "Claude Sonnet 4.5"
statusline context-learning --reset-all
```

**Rebuild learned data (recovery):**
```bash
# Rebuild from session history
statusline context-learning --rebuild

# Clean rebuild (reset first, then rebuild)
statusline context-learning --reset-all --rebuild
```

For detailed information, see [Adaptive Learning Guide](ADAPTIVE_LEARNING.md).

#### Context Percentage Display Mode

**Updated in v2.16.5**: Choose how context percentage is calculated and displayed.

The statusline can show percentage of either the **total context window** ("full" mode) or the **working window** ("working" mode). The calculations automatically adapt based on your `adaptive_learning` setting.

**Configure display mode** in `~/.config/claudia-statusline/config.toml`:

```toml
[context]
# Context percentage display mode
# Options: "full" (default) or "working"
# Default: "full"
percentage_mode = "full"

# Buffer reserved for Claude's responses (default: 40000)
buffer_size = 40000

# Auto-compact warning threshold (default: 75.0)
# Mode-aware: adjusts automatically based on percentage_mode
auto_compact_threshold = 75.0

# Enable adaptive learning to automatically detect actual context limits
# Default: false
adaptive_learning = false
```

**Mode comparison** (example with 150K tokens):

**With Adaptive Learning DISABLED** (uses Anthropic's advertised values):
| Mode | Calculation | Display | Description |
|------|-------------|---------|-------------|
| **"full"** (default) | 150K / 200K = **75%** | Uses advertised total (200K) | Matches Anthropic's specs ✅ |
| **"working"** | 150K / 160K = **94%** | Uses advertised working (160K) | Shows usable conversation space |

**With Adaptive Learning ENABLED** (refines based on 557 observations showing compaction at ~156K):
| Mode | Calculation | Display | Description |
|------|-------------|---------|-------------|
| **"full"** | 150K / 196K = **77%** | Uses learned total (156K + 40K buffer) | Refined estimate of actual total |
| **"working"** | 150K / 156K = **96%** | Uses learned compaction point (156K) | Precise proximity to compaction ⚠ |

**Key difference**: Adaptive learning refines BOTH modes by learning the actual compaction point from observations, then calculating the total window as `compaction_point + buffer`.

**When to use "working" mode:**
- You want to track proximity to auto-compaction
- You have adaptive learning enabled and need precise compaction warnings
- You're optimizing for maximum context usage

**When to use "full" mode (recommended):**
- You want intuitive percentages (100% = full context)
- You prefer consistency with Anthropic's advertised specifications
- You're using adaptive learning and want to see refined total window estimate

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
