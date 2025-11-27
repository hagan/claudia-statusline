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

## Layout Customization

Statusline supports customizable layouts through presets and template-based formatting.

### Built-in Presets

| Preset | Description | Example Output |
|--------|-------------|----------------|
| `default` | Standard layout with all components | `~/project • main +2 • 75% [======>---] • S4.5 • $12.50` |
| `compact` | Minimal space-efficient layout | `project main S4.5 $12` |
| `detailed` | Two-line detailed view | `~/project • main +2`<br>`75% [======>---] • S4.5 • 5m • $12.50` |
| `minimal` | Just directory and model | `~/project S4.5` |
| `power` | Multi-line power user view | (see below) |

### Basic Layout Configuration

```toml
[layout]
# Use a built-in preset
preset = "compact"  # Options: default, compact, detailed, minimal, power

# Or define a custom format (overrides preset)
format = "{directory} • {git_branch} • {model}"

# Custom separator (default: " • ")
separator = " | "
```

### Template Variables

| Variable | Example | Description |
|----------|---------|-------------|
| `{directory}` | `~/projects/app` | Full shortened path |
| `{dir_short}` | `app` | Directory basename only |
| `{git}` | `main +2 ~1` | Full git info |
| `{git_branch}` | `main` | Branch name only |
| `{context}` | `75% [======>---]` | Full context bar |
| `{context_pct}` | `75` | Percentage number |
| `{context_tokens}` | `150k/200k` | Token counts |
| `{model}` | `S4.5` | Abbreviated model |
| `{model_full}` | `Claude Sonnet 4.5` | Full model name |
| `{duration}` | `5m` | Session duration |
| `{cost}` | `$12.50` | Session cost |
| `{cost_short}` | `$12` | Rounded cost |
| `{burn_rate}` | `$3.50/hr` | Cost per hour |
| `{daily_total}` | `$45.00` | Today's total |
| `{lines}` | `+50 -10` | Lines changed |
| `{sep}` | ` • ` | Configured separator |

### Multi-line Layouts

Use `\n` for line breaks:

```toml
[layout]
format = """
{directory} • {git}
{context} • {model} • {cost}
"""
```

### Per-Component Configuration

Fine-tune individual components:

```toml
[layout.components.directory]
format = "short"      # Options: short (default), full, basename
max_length = 30       # Truncate with ellipsis (0 = no limit)
color = "cyan"        # Named color, hex (#FF5733), or ANSI code

[layout.components.git]
format = "full"       # Options: full (default), branch, status
show_when = "always"  # Options: always (default), dirty, never
color = "green"

[layout.components.context]
format = "full"       # Options: full (default), bar, percent, tokens
show_tokens = false   # Show token counts in full format (e.g., "75% [======>---] 150k/200k")
bar_width = 10        # Optional: override progress bar width

[layout.components.model]
format = "abbreviation"  # Options: abbreviation (default), full, version
color = ""               # Empty = use theme default

[layout.components.cost]
format = "full"       # Options: full (default), cost_only, rate_only, with_daily
color = ""
```

#### Context Format Options

| Format | Example Output | Description |
|--------|---------------|-------------|
| `full` | `75% [======>---]` | Percentage + progress bar (default) |
| `full` + `show_tokens` | `75% [======>---] 150k/200k` | With token counts |
| `bar` | `[======>---]` | Progress bar only |
| `percent` | `75%` | Percentage only |
| `tokens` | `150k/200k` | Token counts only |

### Color Override Values

Component colors accept:
- **Named colors**: `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`, `orange`
- **Hex colors**: `#FF5733` or `#F53`
- **256 colors**: `38;5;208` (ANSI format)
- **ANSI codes**: `\x1b[32m` (passthrough)

### Custom User Presets

Create custom presets in `~/.config/claudia-statusline/presets/`:

```bash
mkdir -p ~/.config/claudia-statusline/presets
```

**File**: `~/.config/claudia-statusline/presets/mypreset.toml`
```toml
format = "{dir_short} [{git_branch}] {model} ${cost_short}"
```

Use with:
```toml
[layout]
preset = "mypreset"
```

### Example Configurations

#### Compact Git-Focused
```toml
[layout]
preset = "compact"

[layout.components.git]
show_when = "dirty"  # Only show when there are changes
```

#### Cost-Focused Power User
```toml
[layout]
format = "{directory} • {model}\n{cost} ({burn_rate}) | Day: {daily_total}"

[layout.components.cost]
format = "full"
color = "#FFD700"  # Gold
```

#### Minimal for Narrow Terminals
```toml
[layout]
preset = "minimal"

[layout.components.directory]
format = "basename"
max_length = 15
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

Statusline includes **11 embedded themes** and supports custom TOML-based themes.

### Embedded Themes

#### 1. Dark (Default)
Optimized for dark terminals:
- Directory: Cyan
- Git branch: Green
- Context: White (normal) → Yellow (50%) → Orange (70%) → Red (90%+)
- Cost: Green (<$5) → Yellow ($5-$20) → Red (≥$20)

#### 2. Light
Optimized for light backgrounds:
- Same as dark but uses gray instead of white for better visibility

#### 3. Monokai
Vibrant Sublime Text-inspired colors:
- Directory: #66D9EF (cyan)
- Git branch: #A6E22E (green)
- Model: #F92672 (magenta)
- Bold, saturated palette for maximum visual impact

#### 4. Solarized
Precision colors by Ethan Schoonover:
- Directory: #268BD2 (blue)
- Git branch: #859900 (green)
- Model: #2AA198 (cyan)
- Scientifically designed for reduced eye strain

#### 5. High-Contrast
WCAG AAA accessibility (7:1+ contrast):
- Directory: #00FFFF (bright cyan)
- Git branch: #00FF00 (bright green)
- Cost high: #FF0000 (pure red)
- Maximum readability for visual impairments

#### 6. Gruvbox
Retro groove color scheme with warm, earthy tones:
- Directory: #83A598 (blue)
- Git branch: #B8BB26 (green)
- Model: #FB4934 (red)
- Warm, nostalgic palette inspired by vintage terminals

#### 7. Nord
Arctic, north-bluish color palette:
- Directory: #88C0D0 (frost blue)
- Git branch: #A3BE8C (green)
- Model: #B48EAD (purple)
- Cool, muted tones for reduced visual fatigue

#### 8. Dracula
Dark theme with vibrant purple and pink tones:
- Directory: #8BE9FD (cyan)
- Git branch: #50FA7B (green)
- Model: #FF79C6 (pink)
- Popular theme with bold, saturated colors

#### 9. One Dark
Atom editor's iconic balanced dark theme:
- Directory: #61AFEF (blue)
- Git branch: #98C379 (green)
- Model: #C678DD (purple)
- Professional, well-balanced color scheme

#### 10. Tokyo Night
Deep blue theme inspired by Tokyo's night skyline:
- Directory: #7AA2F7 (blue)
- Git branch: #9ECE6A (green)
- Model: #BB9AF7 (purple)
- Neon-inspired colors with deep blue background

#### 11. Catppuccin
Soothing pastel theme (Mocha variant):
- Directory: #89B4FA (blue)
- Git branch: #A6E3A1 (green)
- Model: #F5C2E7 (pink)
- Soft, warm pastel colors for comfortable viewing

### Using Themes

**Via environment variable:**
```bash
export STATUSLINE_THEME=monokai
export STATUSLINE_THEME=solarized
export STATUSLINE_THEME=gruvbox
export STATUSLINE_THEME=dracula
export STATUSLINE_THEME=catppuccin
```

**Via config file:**
```toml
[theme]
name = "nord"  # or any of the 11 embedded themes
```

**Via CLI flag:**
```bash
statusline --theme solarized
```

### Creating Custom Themes

Create `~/.config/claudia-statusline/mytheme.toml`:

```toml
name = "mytheme"
description = "My custom theme"

[colors]
# Component colors
directory = "#00AAFF"           # Hex color
git_branch = "green"            # Named color
model = "cyan"
duration = "light_gray"
separator = "light_gray"

# State-based colors
lines_added = "green"
lines_removed = "red"

# Cost threshold colors
cost_low = "green"              # < $5
cost_medium = "yellow"          # $5-$20
cost_high = "red"               # ≥ $20

# Context usage threshold colors
context_normal = "white"        # < 50%
context_caution = "yellow"      # 50-70%
context_warning = "orange"      # 70-90%
context_critical = "red"        # ≥ 90%

# Optional: Custom palette with hex colors
[palette.custom]
my_blue = "#0088FF"
my_purple = "#AA00FF"
```

**Supported color formats:**
- **Named colors**: `red`, `green`, `blue`, `cyan`, `magenta`, `yellow`, `white`, `gray`, `light_gray`, `orange`
- **Hex colors**: `#RRGGBB` (e.g., `#FF0000`)
- **ANSI escape codes**: `\x1b[31m` (advanced)

**Load custom theme:**
```bash
export STATUSLINE_THEME=mytheme
```

### Theme Priority

1. CLI flag: `--theme <name>`
2. Environment: `$STATUSLINE_THEME` or `$CLAUDE_THEME`
3. Config file: `theme.name`
4. Default: `dark`

### Examples

See `themes/` directory for complete theme examples:
- `themes/dark.toml`
- `themes/light.toml`
- `themes/monokai.toml`
- `themes/solarized.toml`
- `themes/high-contrast.toml`

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

### Burn Rate Configuration

**Added in v2.21.0**: Choose how session duration is calculated for burn rate (cost per hour).

#### The Problem

Long-running Claude sessions (multi-day projects) include idle time (nights, weekends, breaks), resulting in artificially low burn rates:
- **Example**: $8.99 over 22 days (535 hours) = $0.02/hr ❌
- **Reality**: $8.99 over 5 hours of actual usage = $1.80/hr ✅

#### Configuration

Edit `~/.config/claudia-statusline/config.toml`:

```toml
[burn_rate]
# How to calculate session duration
# Options: "wall_clock" (default), "active_time", "auto_reset"
mode = "wall_clock"

# Inactivity threshold in minutes (default: 60)
# Messages separated by this duration are considered idle
inactivity_threshold_minutes = 60
```

#### Mode Comparison

| Mode | Duration Calculation | Best For | Example |
|------|---------------------|----------|---------|
| **"wall_clock"** (default) | Total time from session start to now | Quick sessions, accurate historical view | 22 days = $0.02/hr |
| **"active_time"** | Sum of time between messages (excludes idle gaps) | Multi-day projects, accurate current rate | 5 hours = $1.80/hr ✅ |
| **"auto_reset"** | Automatically start new session after inactivity | Long breaks, separate work sessions | Each day = separate session |

#### Mode Details

##### 1. Wall-Clock Mode (Default)

**When to use:**
- Short sessions (< 1 day)
- Backward compatibility with existing behavior
- Tracking total time including thinking/breaks

**How it works:**
- Duration = `now - session_start_time`
- Includes all idle time
- Simple, predictable calculation

**Example:**
```
Session: 10:00 AM → 5:00 PM (7 hours wall-clock)
Cost: $3.50
Burn rate: $3.50 / 7h = $0.50/hr
```

##### 2. Active Time Mode

**When to use:**
- Multi-day projects with long idle periods
- Want accurate cost per active hour
- Tracking actual productivity time

**How it works:**
- Tracks time between consecutive messages
- Excludes gaps ≥ inactivity threshold (default: 60 min)
- Accumulates only active time in database
- Updates automatically on every message

**Example:**
```
Session over 3 days:
- Day 1: 2 hours active (10 AM - 12 PM)
- Day 2: 3 hours active (2 PM - 5 PM)
- Day 3: 1 hour active (9 AM - 10 AM)

Total active time: 6 hours
Cost: $12.00
Burn rate: $12.00 / 6h = $2.00/hr ✅
```

**Configuration:**
```toml
[burn_rate]
mode = "active_time"
inactivity_threshold_minutes = 60  # Default: 1 hour

# Adjust threshold based on workflow:
# inactivity_threshold_minutes = 30   # Shorter breaks
# inactivity_threshold_minutes = 120  # Longer thinking time
```

##### 3. Auto-Reset Mode

**When to use:**
- Work in distinct daily sessions
- Want separate stats per work period
- Long breaks between coding sessions
- Track each work period independently

**How it works:**
- Automatically archives current session after inactivity threshold
- Resets counters (cost, lines, duration) to zero
- Creates fresh session on next message
- **History preserved** in `session_archive` table
- Daily/monthly stats continue to accumulate across resets
- Burn rate shows current work period only

**Example:**
```
Monday 9 AM - 12 PM: Work Period 1 ($3.00, +120 lines)
  → Idle for 2+ hours
Monday 2 PM - 5 PM:  Work Period 2 ($4.50, +180 lines)
  → Idle overnight
Tuesday 9 AM - now:  Work Period 3 ($2.00, +50 lines, 2h = $1.00/hr)

Statusline shows: $2.00 (current period), Daily total: $9.50
Archive table has: 2 previous work periods preserved
```

**Key Features:**
- ✅ **Automatic session management**: No manual resets needed
- ✅ **History preserved**: All work periods archived to `session_archive` table
- ✅ **Daily stats accurate**: Costs and lines accumulate across resets
- ✅ **Clean burn rate**: Shows current work period, not multi-day average
- ✅ **Configurable threshold**: Adjust inactivity detection to your workflow

**Configuration:**
```toml
[burn_rate]
mode = "auto_reset"
inactivity_threshold_minutes = 60  # Reset after 1 hour idle
```

#### Technical Details

**Database tracking:**
- New columns (v2.21.0): `active_time_seconds`, `last_activity`
- New table (v2.21.0): `session_archive` (for auto_reset mode)
- Migration v5 automatically adds columns and table to existing databases
- Active time accumulates incrementally on each message
- Auto-reset archives old sessions before creating new ones

**Calculation logic:**
```rust
// Active time mode
if time_since_last_message < threshold {
    active_time += time_since_last_message  // Add delta
} else {
    active_time += 0  // Idle - don't add
}

// Auto-reset mode
if time_since_last_activity >= threshold {
    archive_session(session_id)     // Save to session_archive
    delete_session(session_id)      // Remove from sessions
    create_new_session(session_id)  // Fresh counters
}

// Display
burn_rate = total_cost / (session_duration / 3600.0)
```

**Backward compatibility:**
- Default mode is "wall_clock" (preserves existing behavior)
- Existing sessions work without changes
- Migration runs automatically on first use

#### Choosing the Right Mode

**Use "wall_clock" if:**
- ✅ Sessions are < 1 day
- ✅ You want simple, predictable calculations
- ✅ You include thinking/break time in productivity

**Use "active_time" if:**
- ✅ Multi-day projects with nights/weekends
- ✅ You want accurate $/hour for actual work
- ✅ Sessions span multiple days

**Use "auto_reset" if:**
- ✅ You work in distinct daily sessions
- ✅ You want separate tracking per work period
- ✅ Long breaks (lunch, overnight) should end sessions

#### Example Configurations

**Power user (multi-day projects, short breaks):**
```toml
[burn_rate]
mode = "active_time"
inactivity_threshold_minutes = 30  # 30-min break = still active
```

**Consultant (separate client sessions):**
```toml
[burn_rate]
mode = "auto_reset"
inactivity_threshold_minutes = 120  # 2-hour break = new session
```

**Default (simple tracking):**
```toml
[burn_rate]
mode = "wall_clock"
# threshold not used in wall_clock mode
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
