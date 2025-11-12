# Usage Guide

Complete guide to using Claudia Statusline with examples and command reference.

## Basic Usage

### With Claude Code

Once installed, the statusline automatically appears in Claude Code. No configuration needed!

The statusline updates every 300ms showing:
- Current directory
- Git branch and file changes
- Context usage percentage with progress bar
- Claude model being used
- Session duration
- Cost tracking
- Lines changed in session

**Example output:**
```
~/myproject [main +2 ~1 ?3] • 45% [====------] Sonnet • 1h 23m • +150 -42 • $3.50 ($2.54/h)
```

### Standalone Usage

You can also use the statusline outside of Claude Code by piping JSON to it:

```bash
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | statusline
```

## Command Reference

### Version Information

```bash
# Show version and build info
statusline --version
statusline -v

# Example output:
# statusline 2.15.0
# commit: a1b2c3d
# branch: main
# built: 2025-10-19 12:34:56 UTC
```

### Health Check

```bash
# Human-readable health report
statusline health

# Example output:
# Claudia Statusline Health Report
# ================================
#
# Configuration:
#   Database path: /home/user/.local/share/claudia-statusline/stats.db
#   Database exists: ✅
#   JSON path: /home/user/.local/share/claudia-statusline/stats.json
#   JSON exists: ✅
#   JSON backup enabled: ❌
#
# Statistics:
#   Today's total: $2.50
#   Month total: $45.30
#   All-time total: $128.75
#   Session count: 156
#   Earliest session: 2024-11-01T10:30:00Z

# Machine-readable JSON output
statusline health --json

# Example output:
# {
#   "database_path": "/home/user/.local/share/claudia-statusline/stats.db",
#   "database_exists": true,
#   "json_path": "/home/user/.local/share/claudia-statusline/stats.json",
#   "json_exists": false,
#   "json_backup": false,
#   "today_total": 2.50,
#   "month_total": 45.30,
#   "all_time_total": 128.75,
#   "session_count": 156,
#   "earliest_session": "2024-11-01T10:30:00Z"
# }
```

### Database Maintenance

```bash
# Run standard maintenance
statusline db-maintain

# Operations performed:
# - WAL checkpoint (commit write-ahead log)
# - Optimize (analyze tables, update query planner)
# - Vacuum (reclaim unused space if DB > 10MB)
# - Prune old data (based on retention settings)
# - Integrity check

# Quiet mode (only show errors)
statusline db-maintain --quiet

# Force vacuum even if not needed
statusline db-maintain --force-vacuum

# Skip data pruning
statusline db-maintain --no-prune
```

**Exit codes:**
- `0`: Success
- `1`: Integrity check failed (database corruption)
- `2`: Other error

**Schedule with cron:**
```bash
# Add to crontab (crontab -e)
# Daily maintenance at 3 AM
0 3 * * * /path/to/statusline db-maintain --quiet

# Weekly maintenance on Sunday at 2 AM
0 2 * * 0 /path/to/statusline db-maintain --quiet
```

### Database Migration

```bash
# Check migration status
statusline migrate

# Migrate to SQLite-only mode (archives JSON file)
statusline migrate --finalize

# Migrate and delete JSON file
statusline migrate --finalize --delete-json
```

**What it does:**
1. Verifies data parity between JSON and SQLite
2. Archives or deletes JSON file
3. Updates config to disable JSON backup
4. Enables SQLite-only mode

**Benefits of SQLite-only mode:**
- ~30% faster read performance
- Better concurrent access support
- Smaller memory footprint
- No JSON file I/O overhead

### Context Learning Commands

*(Experimental feature - requires `adaptive_learning = true` in config)*

```bash
# Show all learned context windows
statusline context-learning --status

# Example output:
# Learned Context Windows:
#
# Model: Claude Sonnet 4.5
#   Observed Max: 200000 tokens
#   Confidence: 0.8 (80%)
#   Ceiling Observations: 5
#   Compaction Events: 2
#   Last Updated: 2025-10-19T14:30:00Z
#
# Total models with learned data: 1

# Show detailed learning data for specific model
statusline context-learning --details "Claude Sonnet 4.5"

# Example output:
# Model: Claude Sonnet 4.5
# ======================
#
# Observed Maximum: 200000 tokens
# Confidence Score: 0.8 (80%)
# Ceiling Observations: 5
# Compaction Events: 2
# Last Observed Max: 199847 tokens
# First Seen: 2025-10-15T10:00:00Z
# Last Updated: 2025-10-19T14:30:00Z
#
# Learning Status: ACTIVE (confidence ≥ threshold)
# Currently used for context calculations

# Reset learning data for specific model
statusline context-learning --reset "Claude Sonnet 4.5"

# Example output:
# Reset learning data for model: Claude Sonnet 4.5

# Reset all learning data
statusline context-learning --reset-all

# Example output:
# Reset learning data for all models
```

**How it works:**
- Monitors token usage from transcript files
- Detects automatic compaction events (>10% token drop after 150k)
- Filters out manual compactions (ignores `/compact` commands)
- Builds confidence through multiple observations
- Uses learned value when confidence ≥ threshold (default 70%)

**When to use:**
- Enable in config: `adaptive_learning = true`
- Check status to see what's been learned
- Reset if you notice incorrect context limits
- Reset all when upgrading to new model versions

See [CONFIGURATION.md](CONFIGURATION.md#adaptive-context-learning-experimental) for setup details.

### Hook Commands

*(Called automatically by Claude Code when hooks are configured)*

```bash
# PreCompact hook - called when compaction starts
statusline hook precompact --session-id=<SESSION_ID> --trigger=<auto|manual>

# Stop hook - called when compaction completes
statusline hook stop --session-id=<SESSION_ID>
```

**Setup in Claude Code settings.json:**
```json
{
  "hooks": {
    "PreCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "statusline hook precompact"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "statusline hook stop"
          }
        ]
      }
    ]
  }
}
```

**How it works:**
- Claude Code sends hook data as JSON via stdin (no wrapper scripts needed!)
- Hooks create ephemeral state files in `~/.cache/claudia-statusline/`
- State files are session-scoped: `state-{session-id}.json`
- Statusline checks state file on render (<1ms)
- Shows "Compacting..." instead of percentage when active
- Falls back to token-based detection if hooks not configured

**Benefits:**
- **~600x faster**: <1ms detection vs 60s+ token analysis
- **Real-time feedback**: Instant visual confirmation
- **Event-driven**: Zero overhead, no polling
- **Session-safe**: Multi-instance isolation

**Automatic cleanup:**
- State files automatically cleaned up on Stop hook
- Stale states (>2 minutes) automatically cleared
- No manual maintenance required

See README.md for complete hook setup guide.

### Cloud Sync Commands

*(Requires Turso variant binary)*

```bash
# Check sync status and test connection
statusline sync --status

# Push local stats to Turso (preview first)
statusline sync --push --dry-run
statusline sync --push

# Pull remote stats from Turso (preview first)
statusline sync --pull --dry-run
statusline sync --pull
```

See [CLOUD_SYNC.md](CLOUD_SYNC.md) for complete sync setup guide.

### Help

```bash
# Show all available commands
statusline --help
statusline -h
```

## JSON Input Format

The statusline accepts JSON via stdin with this format:

```json
{
  "workspace": {
    "current_dir": "/path/to/directory"
  },
  "model": {
    "display_name": "Claude 3.5 Sonnet"
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

**Fields:**
- `workspace.current_dir` - Working directory path
- `model.display_name` - Claude model name
- `session_id` - Session identifier (optional)
- `transcript_path` - Path to transcript file for context usage (optional)
- `cost.total_cost_usd` - Session cost in USD (optional)
- `cost.total_lines_added` - Lines added count (optional)
- `cost.total_lines_removed` - Lines removed count (optional)

## Understanding the Output

### Format Breakdown

```
~/myproject [main +2 ~1 ?3] • 45% [====------] Sonnet • 1h 23m • +150 -42 • $3.50 ($2.54/h)
```

- `~/myproject` - Current directory (with ~ substitution)
- `[main +2 ~1 ?3]` - Git branch and status
  - `main` - Current branch
  - `+2` - 2 files added (staged)
  - `~1` - 1 file modified
  - `?3` - 3 files untracked
- `45%` - Context usage percentage
- `[====------]` - Visual progress bar (10 chars)
- `Sonnet` - Claude model (abbreviated: Opus/S3.5/S4.5/Haiku)
- `1h 23m` - Session duration
- `+150 -42` - Lines added/removed in session
- `$3.50` - Session cost
- `($2.54/h)` - Burn rate (only shows after 1 minute)

### Color Coding

**Context Usage:**
- Red (≥90%) - Critical, approaching limit
- Orange (≥70%) - Warning
- Yellow (≥50%) - Caution
- White/Gray (<50%) - Normal

**Cost:**
- Green (<$5) - Low cost
- Yellow ($5-$20) - Medium cost
- Red (≥$20) - High cost

**Lines Changed:**
- Green - Lines added (+)
- Red - Lines removed (-)

**Git Info:**
- Green - Branch name and status

## Usage Examples

### Basic Examples

```bash
# Current directory only
echo '{"workspace":{"current_dir":"'$(pwd)'"}}' | statusline

# With model
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Opus"}}' | statusline

# With cost tracking
echo '{"workspace":{"current_dir":"'$(pwd)'"},"cost":{"total_cost_usd":2.50}}' | statusline

# Complete example
echo '{
  "workspace":{"current_dir":"'$(pwd)'"},
  "model":{"display_name":"Claude Sonnet"},
  "cost":{
    "total_cost_usd":3.50,
    "total_lines_added":150,
    "total_lines_removed":42
  }
}' | statusline
```

### Testing Context Usage

Create a test transcript to see progress bar:

```bash
cat > /tmp/test_transcript.jsonl << 'EOF'
{"message":{"role":"assistant","usage":{"input_tokens":40000,"output_tokens":8000}},"timestamp":"2025-10-19T10:00:00Z"}
{"message":{"role":"user"},"timestamp":"2025-10-19T10:30:00Z"}
{"message":{"role":"assistant","usage":{"input_tokens":80000,"output_tokens":12000}},"timestamp":"2025-10-19T11:00:00Z"}
EOF

# Test with transcript
echo '{
  "workspace":{"current_dir":"'$(pwd)'"},
  "model":{"display_name":"Claude Sonnet"},
  "transcript_path":"/tmp/test_transcript.jsonl"
}' | statusline
```

### Testing Burn Rate

```bash
# Burn rate only shows after 1 minute of session time
echo '{
  "workspace":{"current_dir":"'$(pwd)'"},
  "model":{"display_name":"Claude Sonnet"},
  "transcript_path":"/tmp/test_transcript.jsonl",
  "cost":{"total_cost_usd":15.50}
}' | statusline
```

### Testing Different Themes

```bash
# Dark theme (default)
export CLAUDE_THEME=dark
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Opus"}}' | statusline

# Light theme
export CLAUDE_THEME=light
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Opus"}}' | statusline

# No colors
export NO_COLOR=1
echo '{"workspace":{"current_dir":"'$(pwd)'"},"model":{"display_name":"Claude Opus"}}' | statusline
```

## Embedding in Other Tools

Statusline can be used as a library in other Rust applications:

```rust
use statusline::{render_from_json, render_statusline, StatuslineInput};
use statusline::models::{Workspace, Model, Cost};

// From JSON
let json = r#"{
  "workspace": {"current_dir": "/path/to/project"},
  "model": {"display_name": "Claude 3.5 Sonnet"}
}"#;

// Preview mode: does not update persistent stats
let line = render_from_json(json, false).expect("render");
println!("{}", line);

// Structured input
let input = StatuslineInput {
    workspace: Some(Workspace { current_dir: Some("/path/to/project".into()) }),
    model: Some(Model { display_name: Some("Claude 3 Opus".into()) }),
    cost: Some(Cost {
        total_cost_usd: Some(3.25),
        total_lines_added: Some(10),
        total_lines_removed: Some(2)
    }),
    session_id: Some("my-session".into()),
    transcript: None,
};

// When update_stats=true, persistent stats are updated
let line = render_statusline(&input, true).expect("render");
```

See `examples/embedding_example.rs` for complete example.

## Performance

- **Execution Time**: ~5ms average
- **Memory Usage**: ~2MB resident
- **CPU Usage**: <0.1%
- **Update Frequency**: Every 300ms in Claude Code
- **Transcript Processing**: Only reads last 50 lines
- **Git Operations**: 200ms timeout to prevent hangs

## Troubleshooting

### Statusline shows only "~"

**Cause**: Claude Code sending JSON but statusline not receiving it correctly

**Fix**:
```bash
# Re-run installer to update configuration
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash

# Or manually test
echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet"}}' | ~/.local/bin/statusline
```

### Git status not showing

**Cause**: Not in a git repository or git not installed

**Verify**:
```bash
git rev-parse --is-inside-work-tree
which git
```

### Context usage shows 0%

**Cause**: Transcript file path incorrect or file doesn't contain usage data

**Verify**:
```bash
# Check transcript exists and is readable
ls -la /path/to/transcript.jsonl

# Check for usage data
grep -c "usage" /path/to/transcript.jsonl
```

### Cost tracking not showing

**Cause**: Claude Code not sending cost data, or using old binary

**Fix**:
```bash
# Check version
statusline --version

# Ensure version is 2.1.0+
# If not, upgrade:
curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
```

## Next Steps

- See [CONFIGURATION.md](CONFIGURATION.md) for customization options
- See [CLOUD_SYNC.md](CLOUD_SYNC.md) for cloud sync setup
- See [INSTALLATION.md](INSTALLATION.md) for installation details
