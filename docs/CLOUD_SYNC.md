# Cloud Sync Guide

Complete guide to setting up and using Turso cloud sync for cross-machine cost tracking.

## Overview

Cloud sync allows you to track Claude Code costs across multiple machines and see consolidated statistics.

**Status**: Experimental (Phase 2 complete - manual push/pull)

**Use cases:**
- Track total costs across work laptop, home desktop, and cloud VMs
- See which machine is using the most tokens
- Consolidated cost reports across all devices
- Backup of session history

## Prerequisites

### System Requirements

- **Turso variant binary** (includes `--features turso-sync`)
- **Turso account** (free tier available)
- **Internet connection** for sync operations

### Install Turso Variant

Download from [latest release](https://github.com/hagan/claudia-statusline/releases/latest):

| Platform | Download Link |
|----------|---------------|
| **Linux x86_64** | [statusline-turso-linux-amd64.tar.gz](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-linux-amd64.tar.gz) |
| **Linux ARM64** | [statusline-turso-linux-arm64.tar.gz](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-linux-arm64.tar.gz) |
| **macOS Intel** | [statusline-turso-darwin-amd64.tar.gz](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-darwin-amd64.tar.gz) |
| **macOS Apple Silicon** | [statusline-turso-darwin-arm64.tar.gz](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-darwin-arm64.tar.gz) |
| **Windows** | [statusline-turso-windows-amd64.zip](https://github.com/hagan/claudia-statusline/releases/latest/download/statusline-turso-windows-amd64.zip) |

Or build from source:
```bash
cargo build --release --features turso-sync
```

## Quick Start

### 1. Create Turso Account

Visit [turso.tech](https://turso.tech/) and sign up for free tier.

Free tier includes:
- 500 databases
- 9 GB total storage
- 1 billion row reads/month
- Local and remote replicas

### 2. Install Turso CLI

```bash
curl -sSfL https://get.tur.so/install.sh | bash
```

Verify installation:
```bash
turso --version
```

### 3. Authenticate Turso CLI

```bash
turso auth login
```

This opens your browser to authenticate.

### 4. Create Database

```bash
# Create database
turso db create claude-statusline

# Verify it was created
turso db list
```

### 5. Get Credentials

```bash
# Get database URL
turso db show claude-statusline --url

# Create authentication token
turso db tokens create claude-statusline

# Store token in environment (recommended)
export TURSO_AUTH_TOKEN="your-token-here"

# Add to shell config to persist
echo 'export TURSO_AUTH_TOKEN="your-token-here"' >> ~/.bashrc
```

### 6. Configure Statusline

Create or edit `~/.config/claudia-statusline/config.toml`:

```toml
[sync]
enabled = true
provider = "turso"
sync_interval_seconds = 60      # For future auto-sync (Phase 3)
soft_quota_fraction = 0.75      # Warn at 75% of quota

[sync.turso]
database_url = "libsql://your-database-name.turso.io"
auth_token = "${TURSO_AUTH_TOKEN}"  # References environment variable
```

**Alternative**: Store token directly (less secure):
```toml
[sync.turso]
database_url = "libsql://your-database-name.turso.io"
auth_token = "your-token-here"  # Hardcoded (not recommended)
```

### 7. Initialize Database Schema

```bash
# One-time setup to create tables in Turso
cargo run --example setup_schema --features turso-sync --release
```

This creates the core tables and indexes used by the sync commands:
- `sessions`
- `daily_stats`
- `monthly_stats`

> Tip: follow up with `cargo run --example migrate_turso --features turso-sync --release` to ensure the remote `schema_migrations` table exists and records the initial version.

### 8. Test Connection

```bash
statusline sync --status
```

Expected output:
```
Sync Status
===========
Provider: Turso
Enabled: Yes
Database URL: libsql://your-database.turso.io
Connection: OK ✓
```

### 9. Push Initial Data

```bash
# Preview what will be pushed (recommended first)
statusline sync --push --dry-run

# Actually push to cloud
statusline sync --push
```

Expected output:
```
Pushing to Turso...
  Pushed 42 sessions
  Pushed 30 daily stats
  Pushed 3 monthly stats
Sync complete! ✓
```

## Usage

### Check Sync Status

```bash
statusline sync --status
```

Shows:
- Sync configuration
- Connection status
- Last push/pull timestamps
- Device ID

### Push Local Stats

Upload your local stats to Turso:

```bash
# Preview first (safe, no changes)
statusline sync --push --dry-run

# Push for real
statusline sync --push
```

**What happens:**
- Sessions, daily stats, and monthly stats uploaded
- Local data remains unchanged
- Existing remote data updated (last-write-wins)
- New local data inserted

### Pull Remote Stats

Download stats from other machines:

```bash
# Preview first (safe, no changes)
statusline sync --pull --dry-run

# Pull for real
statusline sync --pull
```

**What happens:**
- Remote data downloaded
- Local data merged with remote
- Conflicts resolved (last-write-wins)
- Device-specific data preserved

### Workflow Example

**On Machine A (work laptop):**
```bash
# Use Claude Code, accumulate $10 in costs
statusline sync --push
```

**On Machine B (home desktop):**
```bash
# Pull stats from Machine A
statusline sync --pull

# Use Claude Code, accumulate $5 more
statusline sync --push
```

**Back on Machine A:**
```bash
# Pull stats from Machine B
statusline sync --pull

# See consolidated costs ($15 total)
statusline health
```

## Configuration

### Full Configuration Example

```toml
[sync]
# Enable cloud sync
enabled = true

# Provider (only "turso" supported)
provider = "turso"

# Auto-sync interval in seconds (Phase 3, not yet implemented)
sync_interval_seconds = 60

# Warn when approaching quota (0.0-1.0)
soft_quota_fraction = 0.75

[sync.turso]
# Database connection URL from: turso db show <db-name> --url
database_url = "libsql://claude-statusline-abc123.turso.io"

# Auth token from: turso db tokens create <db-name>
# Use environment variable (recommended)
auth_token = "${TURSO_AUTH_TOKEN}"

# Or hardcode (not recommended for security)
# auth_token = "your-actual-token-here"
```

### Environment Variables

```bash
# Recommended: Store token in environment
export TURSO_AUTH_TOKEN="your-token-here"

# Add to shell config for persistence
echo 'export TURSO_AUTH_TOKEN="your-token-here"' >> ~/.bashrc  # bash
echo 'export TURSO_AUTH_TOKEN="your-token-here"' >> ~/.zshrc   # zsh
```

## Privacy & Security

### What IS Synced

- ✅ Device ID (anonymous 16-char hash of hostname+username)
- ✅ Session costs (USD)
- ✅ Line counts (added/removed)
- ✅ Timestamps (when sessions occurred)
- ✅ Daily/monthly aggregates

### What is NOT Synced

- ❌ File paths or directory names
- ❌ Git branches or repository names
- ❌ Code content or transcript data
- ❌ Your actual username or hostname (only one-way hash)
- ❌ Model names or context usage details

### Data Privacy

**Device IDs are anonymous:**
```rust
// Pseudocode of how device ID is generated
device_id = sha256(hostname + username)[0..16]
// Example: "a1b2c3d4e5f6g7h8"
```

Nobody can reverse-engineer your hostname/username from the device ID.

### Verify What's Stored

```bash
# Inspect actual data in Turso
cargo run --example inspect_turso_data --features turso-sync --release
```

This shows exactly what data is stored remotely.

## Advanced Usage

### Multiple Devices

**Recommended workflow:**
1. Set up Turso on first machine
2. Push initial data
3. Install Turso variant on other machines
4. Use same `database_url` and `auth_token` in config
5. Pull before first use on new machine
6. Push periodically from each machine

**Sync schedule (manual, Phase 2):**
- Push after long sessions
- Pull before starting work on different machine
- Weekly sync to consolidate all devices

### Database Maintenance

```bash
# Check schema version
cargo run --example check_turso_version --features turso-sync --release

# Run migrations if schema changes
cargo run --example migrate_turso --features turso-sync --release
```

### Conflict Resolution

Currently uses **last-write-wins** strategy:
- Sessions: Most recent `last_updated` timestamp wins
- Daily/monthly stats: Aggregates are summed (no conflicts)

**Example conflict:**
1. Machine A: Session cost = $5 at 10:00 AM
2. Machine B: Same session cost = $6 at 10:30 AM
3. Result: $6 wins (more recent)

### Quota Monitoring

Free tier limits:
- 9 GB total storage
- 1 billion row reads/month

With default retention (90/365/0 days):
- ~50 sessions/day = ~2 MB/year/device
- Well within free tier for multiple devices

Check usage:
```bash
turso db inspect claude-statusline
```

## Troubleshooting

### "Connection failed"

**Cause**: Network issue or invalid credentials

**Fix**:
```bash
# Verify database exists
turso db list

# Test connection with curl
curl -H "Authorization: Bearer $TURSO_AUTH_TOKEN" \
  https://your-database.turso.io

# Regenerate token if needed
turso db tokens create claude-statusline
```

### "Authentication failed"

**Cause**: Invalid or expired auth token

**Fix**:
```bash
# Create new token
turso db tokens create claude-statusline

# Update config.toml or environment variable
export TURSO_AUTH_TOKEN="new-token-here"
```

### "Schema version mismatch"

**Cause**: Turso database schema outdated

**Fix**:
```bash
# Run migrations
cargo run --example migrate_turso --features turso-sync --release

# Verify version
cargo run --example check_turso_version --features turso-sync --release
```

### "Quota exceeded"

**Cause**: Exceeded free tier limits

**Fix**:
```bash
# Check usage
turso db inspect claude-statusline

# Options:
# 1. Increase retention to delete old data
# 2. Upgrade to paid tier
# 3. Create new database, migrate recent data only
```

### "Device ID conflicts"

**Cause**: Same hostname+username on multiple machines

**Fix**: This shouldn't happen in practice, but if it does, device IDs include random component to prevent collisions.

### "Sync taking too long"

**Cause**: Large dataset or slow connection

**Fix**:
```bash
# Use dry-run to see how much data will sync
statusline sync --push --dry-run

# Consider pruning old local data first
statusline db-maintain

# Or increase retention periods to reduce data
```

## Future Roadmap

### Phase 3: Automatic Sync (Planned)

Background worker that syncs automatically:
```bash
# Future: Auto-sync every 60 seconds
# (set via sync_interval_seconds in config)
```

Features:
- Automatic push after each session
- Automatic pull on startup
- Retry logic with exponential backoff
- Offline queueing

### Phase 4: Analytics Dashboard (Planned)

Cross-machine analytics:
```bash
# Future: View stats across all devices
statusline sync --stats

# Example output:
# Total Costs: $125.50
#   - work-laptop: $75.20 (60%)
#   - home-desktop: $40.15 (32%)
#   - cloud-vm: $10.15 (8%)
```

Features:
- Cost breakdown by device
- Usage patterns over time
- Most expensive sessions
- Context usage trends

## Next Steps

- See [CONFIGURATION.md](CONFIGURATION.md) for all config options
- See [USAGE.md](USAGE.md) for command reference
- See [DATABASE_MIGRATIONS.md](DATABASE_MIGRATIONS.md) for schema details
- See [INSTALLATION.md](INSTALLATION.md) for Turso variant installation
