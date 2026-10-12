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
~/myproject [main +2 ~1 ?3] • 45% [=====>----] Sonnet • 1h 23m • +150 -42 • $3.50 ($2.54/h)
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

### Token Attribution per Claude Code Version

Claude Code's cost figure includes subagents, but every token field it sends
describes the main conversation only. When you opt in, the statusline reads
the agent transcripts (`<project>/<session>/subagents/agent-*.jsonl`) and stores
the result per session, so the `🤖N xx%` segment and the `stats` view can show
how much of a session's input traffic went to agents.

Attribution is off by default. In the legacy layout, set
`display.show_agents = true` (default false). In a custom layout, using any of
`{agents}`, `{agents_count}`, `{agents_pct}`, `{agents_tokens}` or
`{agents_types}` turns it on, and `show_agents` has no effect there. The
built-in presets do not include these variables.

While a transcript is still being caught up, or an agent file is too large to
read, `{agents}` shows the count only (`🤖3`) and `{agents_pct}` /
`{agents_tokens}` are omitted. A render reads at most 8 MiB in total (4 MiB per
file), so a large transcript catches up over several renders; with more than
256 agent files, or when the `subagents` directory cannot be listed completely,
no agent data is shown. Each request is counted once, with its last line's
usage, when its lines are within 32 requests of each other (how Claude Code
writes them). Attribution waits at most 250 ms for a locked database and then
skips saving for that render. It uses the same stats database as the rest of
statusline (the data directory, `XDG_DATA_HOME`, or the test store under
`--test-mode`).

`statusline stats` and `statusline sessions` show attribution only for sessions
that were rendered with attribution on, and count only agent files that a
render has read and saved.

The prompt-cache variables (`{prompt_cache}`, `{cache_hit}`, `{cache_misses}`,
`{cache_miss_cause}`, `{cache_warm}`; segment toggle `display.show_prompt_cache`)
come from Claude Code's `prompt_cache` payload field. `{cache_miss_cause}` lists
up to 3 causes joined by `,`, each at most 40 characters of `[a-z0-9_-]`.

```bash
statusline stats                 # per Claude Code version (default)
statusline stats --sessions      # newest 25 sessions, same ratios
```

Columns: `main_req` (requests of the main conversation), `in_k/req` (input
traffic per main request in thousands: fresh + cache read + cache write),
`out/req`, `agents` (agent transcripts), `agent_req`, `share%` (agent input
traffic / (agent + main)). A step change between two versions is the signal
that an update changed agent or context behaviour.

### Session Browser

```bash
statusline sessions                        # list: newest 25, index + short id
statusline sessions list --all             # every recorded session
statusline sessions list --attributed      # only sessions with attribution data
statusline sessions list --limit 50
statusline sessions show 2                 # by index from the default list
statusline sessions show fe2e              # by session id prefix
statusline sessions pick                   # interactive: list, type a selection, show
statusline sessions list --here            # only this project: workspace == $PWD or below
statusline sessions list --workspace ~/src # only that directory tree
statusline sessions pick --here            # same filter, interactive
```

`--here` / `--workspace PATH` restrict the listing to sessions whose workspace
is that directory *or a directory below it*, so running it in a repo root also
finds sessions opened in its subdirectories. Matching is per path component, so
`~/proj` never matches `~/proj2`. `~` is expanded and the path canonicalized,
because the stored workspace is absolute — a path that does not exist is still
queried verbatim, so a since-deleted workspace remains listable. The two flags
are mutually exclusive.

Under a filter the `#` column keeps the position from the **unfiltered** list
and therefore goes non-contiguous (`1 3 4`). That is deliberate: `show <#>`
always resolves against the full ordering, so a number copied out of a filtered
listing opens the session you actually looked at. A session's workspace is the
last one Claude Code reported for it, so a session that moved between
directories is listed under the most recent one only.

`show` prints the full session id, Claude Code version, model, workspace,
start and last-update times, active time, the cost counter of the last CLI
process (resets on `--resume`; daily money lives in `daily_stats`), lines
added/removed, the largest context seen, the payload token totals, main vs
agent attribution, one row per agent transcript (type, requests, input,
cache reads, output, size), the main transcript path and a ready
`claude --resume <session_id>` line.

An index always refers to the default ordering (all sessions, newest first),
independent of `--limit`. An ambiguous prefix is refused with the matching ids;
`pick` refuses to run without a terminal on stdin so it never hangs a script.

The raw payload mirror is off by default. Set
`STATUSLINE_DEBUG_INPUT=/absolute/path.json` to write the raw payload of each
render there (mode 0600, replaced atomically via a temp file and rename), then
inspect it with `jq .` after a Claude Code update to spot new or renamed
fields. Relative paths and values like `off` are ignored. An existing symlink,
directory, FIFO or other non-regular file at that path is refused and nothing
is written; parent directories are not created. If the process is killed
between creating the temp file and renaming it, a `.slmirror.<pid>.<nanos>.tmp`
file (mode 0600, holding the payload) can be left next to the target; delete it
by hand. `last-input.json` files left in the data directory by earlier
development builds can be deleted by hand.

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

# PostCompact hook - called after compaction completes
statusline hook postcompact --session-id=<SESSION_ID>

# Stop hook - called after each agent response (not for compaction cleanup!)
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
    "PostCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "statusline hook postcompact"
          }
        ]
      }
    ]
  }
}
```

> **Note**: Claude Code now has a dedicated **`PostCompact`** hook event (fires
> after compaction completes), so wire `postcompact` to it directly. The hook
> command reads `session_id` from the JSON Claude Code sends on stdin, so the
> native event supplies a proper session id (avoiding the empty-id edge case of
> the old `SessionStart[compact]` workaround).
>
> **Older Claude Code without `PostCompact`?** Fall back to the previous wiring —
> a `SessionStart` hook with `"matcher": "compact"` calling the same
> `statusline hook postcompact` command.

**How it works:**
- Claude Code sends hook data as JSON via stdin (no wrapper scripts needed!)
- Hooks create ephemeral state files in `~/.cache/claudia-statusline/`
- State files are session-scoped: `state-{session-id}.json`
- Statusline checks state file on render (<1ms)
- Shows "Compacting..." instead of percentage when active
- Falls back to token-based detection if hooks not configured

**Hook lifecycle:**
1. **PreCompact** fires → Creates state file → Statusline shows "Compacting..."
2. Compaction runs...
3. **PostCompact** fires → Clears state file → Statusline returns to normal
   *(on older Claude Code, `SessionStart[compact]` plays this role)*

**Benefits:**
- **~600x faster**: <1ms detection vs 60s+ token analysis
- **Real-time feedback**: Instant visual confirmation
- **Event-driven**: Zero overhead, no polling
- **Session-safe**: Multi-instance isolation

**Automatic cleanup:**
- State files automatically cleaned up by PostCompact hook
- Stale states (>2 minutes) automatically cleared as fallback
- No manual maintenance required

### Keeping the statusline fresh while idle (`refreshInterval`)

Claude Code re-runs the statusline on events (each assistant message, after
`/compact`, permission-mode and vim-mode changes). Time-based segments — the
**rate-limit reset countdown** (`{rate_limit_5h_reset}` / `rate_limit_reset_countdown`)
and git status during long idle/subagent waits — can otherwise go stale between
events. Add a timer in `~/.claude/settings.json` so the line also re-runs on a
fixed cadence:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/statusline",
    "refreshInterval": 10
  }
}
```

`refreshInterval` is in seconds (minimum `1`). Use it when you display the reset
countdown or want git/rate-limit state to keep updating while the session is idle.

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

### Development & Testing Mode

Use `--test-mode` to prevent test data from polluting your production database:

```bash
# Run with isolated test database
echo '{"workspace":{"current_dir":"/tmp"}}' | statusline --test-mode

# Output shows [TEST] indicator
# Example: [TEST] • /tmp • main • S3.5 • 15s • day: $0.05
```

**What test mode does:**
- Uses separate database: `~/.local/share-test/claudia-statusline/stats.db`
- Adds yellow `[TEST]` indicator to output
- Prevents modifications to production database at `~/.local/share/claudia-statusline/stats.db`

**Combine with other flags:**
```bash
# Test mode with custom config and logging
statusline --test-mode --config ./test-config.toml --log-level debug < test.json

# Test mode with specific theme
statusline --test-mode --theme dark < test.json
```

**Clean up test data:**
```bash
# Remove test database
rm -rf ~/.local/share-test/claudia-statusline

# Or use Makefile (if in project directory)
make clean-test
```

See [README.md Development & Testing](../README.md#development--testing) for more testing strategies.

## GSD project state

`statusline gsd state [--json] [--dir PROJECT_DIR]` reports the current
**milestone**, **phase** and **progress** of a GSD project (`.planning/`).

With `--json` it emits **exactly one** JSON document on stdout. Statusline does
the Markdown parsing once and hands you a structured result, so an external tool
reads all three facts **without parsing prose itself**.

```bash
# Human-readable report for the project in the current directory
statusline gsd state

# One machine-readable document
statusline gsd state --json

# A project elsewhere -- PROJECT_DIR is the directory CONTAINING `.planning/`
statusline gsd state --json --dir ~/projects/my-app
```

Statusline **never writes** `STATE.md` or any other planning file. This command
is read-only.

### Schema (`schema_version` 1)

Every field except `schema_version` and `warnings` is **nullable**.

| Key path | Type | Meaning |
|----------|------|---------|
| `schema_version` | integer | Always present. The stability gate — check it first. |
| `source.planning_dir` | string \| null | The `.planning/` directory the facts were read from. |
| `source.state_md` | string \| null | Path to `STATE.md`, when that file exists. |
| `source.roadmap_md` | string \| null | Path to `ROADMAP.md`, when that file exists. |
| `source.state_frontmatter_version` | string \| null | The RAW `gsd_state_version` of `STATE.md`, reported even when it is a version this build does not support. |
| `milestone.id` | string \| null | Milestone identifier, e.g. `"v3.3.0"`. Never truncated. |
| `milestone.name` | string \| null | Milestone name, e.g. `"Cost Accuracy & Honesty"`. |
| `phase.id` | string \| null | Phase identifier. Equal to `phase.number` today; reserved for a future GSD that distinguishes a directory slug from a number. |
| `phase.number` | **string** \| null | The phase token exactly as `STATE.md` writes it. |
| `phase.name` | string \| null | Phase name, when `STATE.md` names one. |
| `phase.display` | string \| null | `"P12: Name"`, or `"P12"` when there is no name. |
| `progress.phases_completed` | integer \| null | Completed phase checkboxes in `ROADMAP.md`. |
| `progress.phases_total` | integer \| null | Total phase checkboxes in `ROADMAP.md`. |
| `progress.phases_percent` | integer \| null | Integer percentage of phases complete. |
| `progress.plans_completed` | integer \| null | Completed plan checkboxes in the current phase's section. |
| `progress.plans_total` | integer \| null | Total plan checkboxes in the current phase's section. |
| `warnings` | array of strings | Always present, possibly empty. Why a fact is null, and which emitted numbers are known to be unreliable. |

**`schema_version` is the stability contract.** Version `1` guarantees the key
paths above with the types above. Adding a key is a **minor, non-breaking**
change and does *not* bump the integer; removing a key, renaming one, or
changing its type **does**. Check `schema_version` before trusting any other
field, and treat an unrecognised value as "do not parse".

### `phase.number` is a STRING

It is the literal token written in `STATE.md`, never normalised — so it may be
zero-padded (`"04"`) or decimal (`"999.1"`). Do **not** coerce it to an integer;
compare it as a string, or normalise it yourself if you need to.

### Exit codes

| Code | When |
|------|------|
| `0` | A well-formed document was produced — **including** the case where every field is `null` and `warnings` explains why. "There is no GSD state here" is a valid answer, so do not use the exit code as the presence signal; check the fields. |
| `1` | `--dir` names a path that is not a directory. This is a usage error: the message goes to **stderr** and **stdout is empty**, so a half-written document can never break the single-document contract. |

An absent, unreadable, unparseable or future-versioned `STATE.md` all exit `0`
with a well-formed document.

### Progress is COMPUTED, not recorded

`progress.*` is computed by counting the checkboxes in `ROADMAP.md`. The
`progress:` block that GSD records inside `STATE.md`'s YAML frontmatter is
**deliberately not used** — it drifts. In this repository at the time of
writing, the recorded block says `percent: 33` while the computed value is
`100`. One source per fact; the computed one wins.

> **Known limitation.** The phase and plan counters make two layout
> assumptions, and they can be wrong:
>
> - Any checkbox line containing the literal `**Phase ` is counted as a phase
>   entry, so a *plan* line whose description quotes that text inflates
>   `phases_total`.
> - The plan counters assume a phase's checkboxes directly follow its
>   `- [ ] **Phase N:` summary entry. In a roadmap that lists all phases first
>   and then expands them under `### Phase N:` headings, the plans counted after
>   a summary entry may belong to a different phase.
>
> When either applies, the command still emits the number — and adds a
> `warnings` entry saying the number may be inaccurate. A number known to be
> unreliable is never presented as plain fact. Check `warnings` before acting on
> `progress.*`.

### Reading all three facts with `jq`

```bash
statusline gsd state --json | jq -r '
  if .schema_version == 1
  then "milestone=\(.milestone.id // "?")  phase=\(.phase.number // "?")  progress=\(.progress.phases_completed // "?")/\(.progress.phases_total // "?") (\(.progress.phases_percent // "?")%)"
  else "unsupported schema_version \(.schema_version)" end'
```

Run against this repository, as a SNAPSHOT — these are measured values, not a
contract, and they move every time a `ROADMAP.md` checkbox is ticked. Read the
block for its SHAPE; re-run the pipeline for the numbers:

```text
milestone=v3.3.0  phase=12  progress=4/4 (100%)
```

That `100%` is one of the caveated numbers described just above: this
repository's `ROADMAP.md` triggers BOTH layout assumptions, so the same document
carries two `warnings` entries saying the phase and plan counts may be
inaccurate. It is a live illustration of why `warnings` must be checked before
`progress.*` is acted on — `4/4` here does not mean the milestone is finished.

Note that no part of that pipeline looks at `STATE.md` or `ROADMAP.md` — which
is the whole point.

The `STATE.md` frontmatter keys statusline consumes are documented in
[CONFIGURATION.md](CONFIGURATION.md) under the STATE.md frontmatter contract.

## How spend and token totals are counted

Claude Code reports cumulative counters per CLI process: `cost.total_cost_usd`,
`cost.total_lines_added` and `cost.total_lines_removed` only ever describe the
process that is running now. The statusline turns them into daily and monthly
totals by adding only the *increase* since the last update it saw for that
session.

A session id can outlive a process (a restart or `--resume`), and a counter can
also go down within one process (a correction, or an update that arrives out of
order). To tell these apart the statusline infers when the process started:
*now minus `cost.total_duration_ms`*. Two starts that differ by 60 seconds or
less are the same process.

| What the statusline sees | How it is counted |
|---|---|
| A counter grows | The increase counts. |
| A counter drops, same process (start within 60 seconds) | A correction or an out-of-order update: it counts 0, and later growth counts only above the previous peak. Example: 10.00, then 9.80, then 10.30 counts 10.30 in total, not 10.50. |
| Cost drops and the process started more than 60 seconds later | A restart: the new values count in full. Earlier spend, including yesterday's total, is never reduced. |
| Only lines added or removed drop and the process started more than 60 seconds later | A correction, not a restart (see the peak rule above). Only a drop in cost can mark a restart. |
| The update's process started more than 60 seconds *earlier* than the stored one, and a counter dropped | A stale update from an older process: it is ignored. |
| The update's process started more than 60 seconds *earlier* than the stored one, and nothing dropped | Ordinary growth. The earlier start is kept, because the stored one came from a late reading (for example the machine slept before the status line ran). |
| No `total_duration_ms`, or a value of 0 | The start is unknown, so the update is treated as the same process (peak rule above). |

Cost, lines added and lines removed are classified together: one update is
either a restart for all three counters or for none of them. Only a drop in
cost decides that it is a restart. A later start without a cost drop is
ordinary growth (or a correction, if a line counter dipped), because current
Claude Code versions carry the session's totals across a resume.

### Session totals are cumulative

The session total stored in the database (`statusline sessions`, database
views, cloud sync) is what that session contributed to your daily and monthly
totals across all of its processes. Example: $10.00 in a first process, then a
restart and $0.50 in a second one, is stored as $10.50. The status line itself
still shows Claude Code's current `total_cost_usd`, so after that restart it
reads `$0.50`.

With `burn_rate.mode = "auto_reset"`, a new work period continues the
session's cumulative total instead of starting again from zero.

### Known limits

- **Restart without `total_duration_ms`.** If a restart is reported without a
  duration, it cannot be told apart from a correction. The new process's spend
  is undercounted until it passes the previous peak. Totals are never
  overcounted in this case.
- **Resume restored slightly low.** If Claude Code ever restores a resumed
  session's cost slightly below the last value the statusline saw, that resume
  looks like a restart and its cost is counted again. Run with
  `RUST_LOG=debug` and look for `accounting: restart detected` to spot it.
- **Out-of-range payload values.** For the daily, monthly and session
  totals, one process's cost is capped at $10,000,000 and each line counter at
  4,294,967,296 (2^32). Larger values in a payload are treated as these
  limits, so a corrupt payload cannot overflow the database. The status line
  still shows the payload's own cost.

### Token totals

Claude Code writes one transcript line per content block of a response, and
each line repeats the same `usage`. Each API request is now counted once: the
last line with usage for a `requestId` wins. Lines without a `requestId`
(older transcripts) still count individually.

Output and cache-creation token totals recorded before v3.4.0 (session,
daily/monthly, token rates and cache ROI) may be inflated, typically about 2x.
Cost and context % were never affected. Existing history is not recomputed:
the transcript is only read in part and daily/monthly sums cannot be split back
into sessions. A session that was running during the upgrade keeps its larger
stored token totals, and its daily token growth pauses until the new
de-duplicated sum passes the old stored value.

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
- `cost.total_duration_ms` - Running time reported by Claude Code, in ms (optional; used to tell restarts from corrections, see [How spend and token totals are counted](#how-spend-and-token-totals-are-counted))

## Understanding the Output

### Format Breakdown

```
~/myproject [main +2 ~1 ?3] • 45% [=====>----] Sonnet • 1h 23m • +150 -42 • $3.50 ($2.54/h)
```

- `~/myproject` - Current directory (with ~ substitution)
- `[main +2 ~1 ?3]` - Git branch and status
  - `main` - Current branch
  - `+2` - 2 files added (staged)
  - `~1` - 1 file modified
  - `?3` - 3 files untracked
- `45%` - Context usage percentage
- `[=====>----]` - Visual progress bar (10 chars)
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

// Preview mode: does not update persistent stats. The stats database is
// opened read-only and is never created, migrated or repaired.
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

Previews with `update_stats = false` open the stats database read-only and
never create, migrate or repair it. Attribution then advances in memory only,
so a very large transcript that no normal render has read yet can stay
count-only across previews.

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

**Cause**: Claude Code not sending cost data, or using old binary. If the
daily total looks lower or higher than the session cost on the status line, see
[How spend and token totals are counted](#how-spend-and-token-totals-are-counted).

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
