# Adaptive Context Learning Guide

**Status**: Experimental (v2.16.0+)
**Default**: Disabled

## Overview

Adaptive Context Learning is an experimental feature that automatically learns the actual context window limits of Claude models by observing real usage patterns. Instead of relying on hardcoded defaults, the statusline detects when Claude performs automatic compaction and learns the true limits over time.

## Why Use Adaptive Learning?

**Traditional approach problems:**
- Hardcoded defaults may be outdated when Anthropic changes limits
- Different model versions may have different limits
- No way to verify assumptions are correct
- Manual configuration required when limits change

**Adaptive learning benefits:**
- Automatically discovers actual context limits
- Adapts when Anthropic updates model capabilities
- Builds confidence through multiple observations
- Falls back to intelligent defaults until confident

## How It Works

### Detection Mechanisms

#### 1. Compaction Event Detection

The statusline monitors for sudden token drops indicating automatic context compaction:

```
Session 1: 195,000 tokens
Session 2: 120,000 tokens (drop > 10% AND prev > 150k)
→ Compaction detected at ~195k limit
```

**Detection criteria:**
- Token drop > 10% from previous session
- Previous session > 150,000 tokens
- Not a manual compaction (see filtering below)

#### 2. Manual Compaction Filtering

To avoid false positives, the system scans transcripts for manual compaction indicators:

**Command detection:**
- `/compact` command in user messages

**Phrase detection** (case-insensitive):
- "compact the context"
- "compress the conversation"
- "summarize our discussion"
- "condense the transcript"
- "remove old messages"
- "clean up the history"
- "reduce the context"
- "trim the conversation"
- "shorten the context"
- "archive old messages"
- "clear the history"
- "reset the context"
- "start fresh"

If any of these are found in the 10 messages before a token drop, the event is marked as manual and not counted toward learning.

#### 3. Ceiling Pattern Observation

The statusline tracks sessions approaching the same maximum repeatedly:

```
Sessions hitting: 198k, 199k, 197k, 199k, 198k
→ Likely ceiling at ~200k
```

Each time a session approaches a previously seen maximum, the ceiling observation count increases.

#### 4. Confidence Building

Confidence increases with observations:

```rust
confidence = (ceiling_observations × 0.1) + (compaction_events × 0.3)
confidence = min(confidence, 1.0)  // Cap at 100%
```

**Examples:**
- 1 ceiling observation = 0.1 confidence (10%)
- 5 ceiling observations = 0.5 confidence (50%)
- 1 compaction event = 0.3 confidence (30%)
- 5 ceiling observations + 2 compaction events = 1.0 confidence (100%)

**Default threshold**: 0.7 (70%) before using learned values

## Configuration

### Enable Adaptive Learning

Edit `~/.config/claudia-statusline/config.toml`:

```toml
[context]
# Global fallback when no model-specific limit is known
window_size = 200000

# Enable adaptive learning (default: false)
adaptive_learning = true

# Minimum confidence before using learned values (0.0-1.0)
# Higher = more observations required before trusting learned limit
# Default: 0.7 (70% confidence)
learning_confidence_threshold = 0.7

# Optional: Manual overrides (highest priority)
[context.model_windows]
# "Claude 3.5 Sonnet" = 200000
# "Claude 3 Opus" = 200000
```

### Priority System

Context window limits are selected in this order:

1. **User config overrides** (highest priority)
   - Defined in `[context.model_windows]`
   - Always used when specified
   - Useful for known limits or special cases

2. **Learned values** (when confident)
   - Used when `adaptive_learning = true`
   - AND confidence ≥ threshold (default 70%)
   - Based on observed real-world usage

3. **Intelligent defaults** (based on model family/version)
   - Built-in heuristics for known models
   - Updated with statusline releases
   - Better than global fallback

4. **Global fallback** (lowest priority)
   - Value from `window_size` setting
   - Used when nothing else available

## CLI Commands

### View Learning Status

```bash
statusline context-learning --status
```

**Example output:**
```
Learned Context Windows:

Model: Claude Sonnet 4.5
  Observed Max: 200000 tokens
  Confidence: 0.8 (80%)
  Ceiling Observations: 5
  Compaction Events: 2
  Last Updated: 2025-10-19T14:30:00Z

Total models with learned data: 1
```

### View Detailed Information

```bash
statusline context-learning --details "Claude Sonnet 4.5"
```

**Example output:**
```
Model: Claude Sonnet 4.5
======================

Observed Maximum: 200000 tokens
Confidence Score: 0.8 (80%)
Ceiling Observations: 5
Compaction Events: 2
Last Observed Max: 199847 tokens
First Seen: 2025-10-15T10:00:00Z
Last Updated: 2025-10-19T14:30:00Z

Learning Status: ACTIVE (confidence ≥ threshold)
Currently used for context calculations
```

### Reset Learning Data

Reset a specific model:
```bash
statusline context-learning --reset "Claude Sonnet 4.5"
```

Reset all models:
```bash
statusline context-learning --reset-all
```

**When to reset:**
- Anthropic announces context limit changes
- You notice incorrect context percentage calculations
- After upgrading to a new major model version
- Testing or debugging adaptive learning

## Database Storage

Learned data is stored in SQLite at `~/.local/share/claudia-statusline/stats.db`:

```sql
CREATE TABLE learned_context_windows (
    model_name TEXT PRIMARY KEY,
    observed_max_tokens INTEGER NOT NULL,
    ceiling_observations INTEGER DEFAULT 0,
    compaction_count INTEGER DEFAULT 0,
    last_observed_max INTEGER NOT NULL,
    last_updated TEXT NOT NULL,
    confidence_score REAL DEFAULT 0.0,
    first_seen TEXT NOT NULL
);
```

## Example Learning Session

Let's walk through how the system learns a context limit:

**Day 1:**
```
Session 1: 150,000 tokens → First ceiling observation (confidence: 0.1)
Session 2: 180,000 tokens → Second ceiling observation (confidence: 0.2)
Session 3: 195,000 tokens → Third ceiling observation (confidence: 0.3)
```

**Day 2:**
```
Session 4: 198,000 tokens → Fourth ceiling observation (confidence: 0.4)
Session 5: 199,500 tokens → Fifth ceiling observation (confidence: 0.5)
```

**Day 3:**
```
Session 6: 199,800 tokens
Session 7: 120,000 tokens → COMPACTION DETECTED! (confidence: 0.5 + 0.3 = 0.8)
```

**Result:** Confidence = 0.8 (80%) ≥ threshold (70%)
**Action:** Statusline now uses 199,800 tokens as the learned limit for this model

## Troubleshooting

### Confidence Not Increasing

**Problem**: Learning data shows observations but confidence stays low

**Check:**
```bash
statusline context-learning --details "Your Model Name"
```

**Common causes:**
- Ceiling observations increase slowly (0.1 per observation)
- Need 7+ ceiling observations to reach 70% without compactions
- Or need 2+ compaction events

**Solution:**
- Continue using Claude normally, confidence builds over time
- Lower threshold in config if you want to trust earlier
- Or manually set limit in `[context.model_windows]`

### False Compaction Detection

**Problem**: System detects compaction when you manually used `/compact`

**Verification:**
```bash
statusline context-learning --details "Your Model Name"
# Check if compaction_count seems too high
```

**Solution:**
- The system should filter `/compact` commands automatically
- If false positives occur, reset and report a bug
- Use manual override in config as workaround

### Learned Limit Seems Wrong

**Problem**: Learned limit doesn't match expected context window

**Investigation:**
```bash
statusline context-learning --details "Your Model Name"
# Check observed_max_tokens and last_observed_max
```

**Solutions:**
1. Wait for more observations (confidence may be premature)
2. Reset learning data: `statusline context-learning --reset "Model Name"`
3. Add manual override in config:
   ```toml
   [context.model_windows]
   "Your Model Name" = 200000
   ```

### Learning Not Working

**Problem**: No learning data appears even after many sessions

**Checklist:**
```bash
# 1. Verify adaptive learning is enabled
grep "adaptive_learning" ~/.config/claudia-statusline/config.toml

# 2. Check database exists
ls -la ~/.local/share/claudia-statusline/stats.db

# 3. Verify transcript is being read
statusline health

# 4. Check for errors
RUST_LOG=debug echo '{"workspace":{"current_dir":"'$(pwd)'"}}' | statusline
```

**Common causes:**
- `adaptive_learning = false` in config (default)
- Transcript path not provided by Claude Code
- Sessions not reaching high enough token counts (need >150k)
- Database migration needed

## Performance Impact

Adaptive learning adds minimal overhead:

- **Database queries**: +1 SELECT per statusline render
- **Learning logic**: Only runs when session updates stats
- **Transcript scanning**: Only when token drop detected
- **Total overhead**: <5ms average

**Recommendation**: Leave enabled once configured, negligible performance cost.

## Privacy & Security

**Local-only learning:**
- All learning data stored locally in SQLite
- No network requests or telemetry
- Transcript content never stored (only token counts)

**What's stored:**
- Model name (e.g., "Claude Sonnet 4.5")
- Token counts (numbers only)
- Observation timestamps
- Confidence scores

**What's NOT stored:**
- Transcript content
- User messages
- Assistant responses
- File paths or project details

## Future Enhancements

Potential improvements for future versions:

- **Cross-device sync**: Share learned data via Turso cloud sync
- **Confidence visualization**: Progress bars showing learning status
- **Manual confirmation**: Prompt before applying learned values
- **API integration**: Fetch official limits if Anthropic provides endpoint
- **Model version detection**: Learn separate limits for minor versions

## See Also

- [CONFIGURATION.md](CONFIGURATION.md) - Complete configuration guide
- [USAGE.md](USAGE.md) - CLI command reference
- [ARCHITECTURE.md](../ARCHITECTURE.md) - Technical implementation details
- [Planning Document](../.claude/planning/08_adaptive_context.md) - Feature roadmap

## Feedback

This is an experimental feature. Please report issues or suggestions:
- GitHub Issues: https://github.com/hagan/claudia-statusline/issues
- Include output from `statusline context-learning --details "Your Model"`
- Mention your model version and typical usage patterns
