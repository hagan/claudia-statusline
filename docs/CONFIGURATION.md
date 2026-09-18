# Configuration Guide

Complete guide to configuring Claudia Statusline with all available options.

## Configuration Files

### Locations

- **Claude Code Settings**: `~/.claude/settings.json` or `~/.claude/settings.local.json`
- **Statusline Config**: platform-dependent — `~/Library/Application Support/claudia-statusline/config.toml`
  on macOS, `~/.config/claudia-statusline/config.toml` on Linux. Run `statusline config path`
  for the authoritative answer on your machine, and see
  [Where your config actually lives](#config-file-location).
- **Database**: `~/.local/share/claudia-statusline/stats.db`
- **Debug Logs** (if enabled): `~/.cache/statusline-debug.log`

> **Shorthand used in this document.** Later sections write the config directory as
> `~/.config/claudia-statusline/` for brevity. Read that as *your* config directory:
> `~/Library/Application Support/claudia-statusline/` on macOS without `XDG_CONFIG_HOME`,
> `$XDG_CONFIG_HOME/claudia-statusline/` when that variable is set, and
> `~/.config/claudia-statusline/` on Linux. `statusline config path` prints the real one.
> This applies to `presets/` and custom theme files too, which live in the same directory.

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

**Where your config actually lives depends on your platform.** The single most common
setup problem is creating `~/.config/claudia-statusline/config.toml` on macOS, where
statusline never reads it.

Statusline searches these locations in order and uses the FIRST one that exists:

1. `$STATUSLINE_CONFIG_PATH` — an explicit path, if set
2. `$STATUSLINE_CONFIG` — an explicit path, if set
3. The platform config directory:
   - if `$XDG_CONFIG_HOME` is set: `$XDG_CONFIG_HOME/claudia-statusline/config.toml`
   - otherwise on **macOS**: `~/Library/Application Support/claudia-statusline/config.toml`
   - otherwise on **Linux**: `~/.config/claudia-statusline/config.toml`
4. `~/.claudia-statusline.toml`

The top-level `--config <PATH>` flag overrides all of these for a single run.

So on macOS, unless you have exported `XDG_CONFIG_HOME`, create your config at:

```bash
mkdir -p ~/Library/Application\ Support/claudia-statusline
$EDITOR ~/Library/Application\ Support/claudia-statusline/config.toml
```

and on Linux at:

```bash
mkdir -p ~/.config/claudia-statusline
$EDITOR ~/.config/claudia-statusline/config.toml
```

Rather than guessing, ask the binary:

```bash
statusline config path
```

It prints the active file and every candidate in order, marking each as found or not
found. It also flags **misplaced** config files. `statusline config validate` issues a
`misplaced_config` warning for the same situation, for example:

```text
⚠️  warning <misplaced-config>: /Users/you/.config/claudia-statusline/config.toml —
    ~/.config is only searched when XDG_CONFIG_HOME points at it — on macOS the config
    dir is ~/Library/Application Support; statusline never reads it — move it to
    /Users/you/Library/Application Support/claudia-statusline/config.toml
```

See [The `config` Command](#the-config-command) for the full reference.

### Complete Example

```toml
# Database Configuration
[database]
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
# Minimal config - tune database retention
[database]
retention_days_sessions = 90
```

## The `config` Command

`statusline config` groups three configuration-file utilities:

| Command | Purpose |
|---------|---------|
| `statusline config validate [PATH] [--json] [--strict]` | Check a config file for unknown keys, type errors, invalid values and unreachable caches |
| `statusline config generate` | Write an example config file to the default config path |
| `statusline config path [--json]` | Show the active config file and the full search order |

### `config validate`

```bash
# Validate the config that would actually be loaded
statusline config validate

# Validate a specific file
statusline config validate ~/some/other-config.toml

# Machine-readable output
statusline config validate --json

# Fail CI on warnings too
statusline config validate --strict
```

`PATH` is a **positional** argument, not a flag. The top-level `--config <PATH>`
option selects a config file for *rendering* a status line; it is not accepted by
`config validate`. Pass the path positionally instead.

With no `PATH`, `config validate` checks the config that would actually be loaded and
reports which file that was. If no config file is found anywhere it validates the
built-in defaults, emits a `no_config_found` notice and exits 0.

#### What is an error and what is a warning

**Errors** — these make the exit code non-zero:

- **unknown keys** — a setting statusline does not recognize (usually a typo)
- **type mismatches** — a string where a number or array is expected, and similar
- **invalid values** — an unknown enum variant, an out-of-range number, an unparseable
  duration
- **config IO failures** — no file exists at the path, the file is unreadable, the path
  is a directory, the path is not a regular file, or the file is larger than the
  1 MiB (1048576 byte) config size cap

**Warnings** — these do not affect the exit code unless `--strict` is passed:

- a cache that is **stale**, **unreadable** or **unparseable**
- informational cross-field advice (for example `[ant] enabled = false` while
  `[ant.accounts.*]` tables are configured)
- a config file found at a location statusline **never consults** — see
  [Where your config actually lives](#config-file-location)

#### Absent caches produce no finding

**An absent cache produces no finding at all — not an error, and not even a warning.**
A missing cache is the normal state for anyone who has never run a sync, so warning
about it would make the default experience noisy for no reason. `config validate --json`
still *reports* absence as `caches.<name>.state == "absent"`, so a script that wants to
act on a missing cache can do so without every default user seeing a warning.

#### Exit codes

| Situation | Without `--strict` | With `--strict` |
|-----------|--------------------|-----------------|
| Clean config | `0` | `0` |
| Warnings only | `0` | `1` |
| Any error | `1` | `1` |

`--strict` exists so CI can enforce a typo-free config with fresh caches without that
becoming everyone's default.

In `--json` output these three facts are **separate fields**, so a consumer never has to
infer one from another:

- `valid` means `counts.errors == 0`. It is **independent of `--strict`**: a
  warnings-only run reports `"valid": true` even when `--strict` made it exit 1.
- `strict` reports whether the flag was passed.
- `exit_code` reports the process exit code that was actually used.

#### `--json` schema

Top-level keys:

`schema_version`, `valid`, `strict`, `exit_code`, `target_path`, `target_source`,
`active_path`, `active_source`, `counts`, `notices`, `findings`, `caches`.

Each entry in `findings` carries `severity` (`error` or `warning`), `key`, `kind`,
`message` and `hint`. The `kind` vocabulary is **closed** — a scripted consumer can
filter on it:

| `kind` | Meaning |
|--------|---------|
| `syntax_error` | The file is not valid TOML |
| `unknown_key` | Not a recognized setting |
| `type_error` | A section failed to deserialize |
| `invalid_value` | Right type, unacceptable value; also cross-field advice |
| `stale_cache` | Cache is at or beyond its staleness threshold |
| `unreadable_cache` | Cache is unreadable **or** unparseable — both states map to this one kind |
| `misplaced_config` | A config file exists at a location statusline never reads |
| `config_io` | The target file could not be read (missing, unreadable, a directory, not a regular file, over the size cap) |

`findings` is sorted into a total, process-independent order by
`(severity, key, kind, message)`, so diffing two runs in CI is meaningful.

`caches` carries one entry per classified cache, each with `state`
(`fresh` / `stale` / `absent` / `unreadable` / `unparseable`), `age`, `path` and
`detail`. `caches.models` and `caches.prices` are always present. **`caches.usage` is
present only when `STATUSLINE_ANT_ACCOUNT` names an active account** — the usage cache is
per-account, so with no active account there is nothing to classify and the key is
omitted entirely rather than reported as absent.

Example, a warnings-only run under `--strict`:

```json
{
  "schema_version": 1,
  "valid": true,
  "strict": true,
  "exit_code": 1,
  "counts": { "errors": 0, "warnings": 1 },
  "findings": [
    {
      "severity": "warning",
      "key": "ant.enabled",
      "kind": "invalid_value",
      "message": "`enabled` is false while 1 `[ant.accounts.*]` table(s) are configured, ...",
      "hint": null
    }
  ],
  "caches": {
    "models": { "state": "absent", "age": null, "path": "...", "detail": null },
    "prices": { "state": "absent", "age": null, "path": "...", "detail": null }
  }
}
```

### `config generate`

Writes an example config file to the default config path (see
[Where your config actually lives](#config-file-location)).

```bash
statusline config generate
```

`statusline generate-config` still works but is **deprecated** in favour of
`statusline config generate`; it prints a deprecation note on stderr.

### `config path`

Shows which config file is active and the full search order, marking each candidate as
found or not found. This is the authoritative answer for your machine.

```bash
statusline config path
statusline config path --json
```

It also lists **misplaced** config files — files that exist at a location statusline
never reads on this platform.

### See also

For machine-readable project-state facts (`.planning/` milestone, phase and progress),
see the [GSD project state](USAGE.md#gsd-project-state) section of USAGE.md; its schema
is documented there and is not duplicated here.

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
| `{token_rate}` | `12.5 tok/s • 150K` | Token rate (combined, respects `rate_display`) |
| `{token_rate_only}` | `12.5 tok/s` | Total token rate only |
| `{token_input_rate}` | `5.2K tok/s` | Input + cache read rate |
| `{token_output_rate}` | `8.7K tok/s` | Output token rate |
| `{token_cache_rate}` | `41.7K tok/s` | Cache read rate |
| `{token_cache_hit}` | `85%` | Cache hit ratio |
| `{token_cache_roi}` | `12.3x` | Cache ROI multiplier |
| `{token_session_total}` | `150K` | Session token total |
| `{token_daily_total}` | `day: 2.5M` | Daily token total |
| `{sep}` | ` • ` | Configured separator |

> **Note:** Token rate variables require `[token_rate] enabled = true` in config.
> The `{token_rate}` variable respects both `display_mode` and `rate_display` settings.

### GSD Project-Tracking Variables

When the working directory is inside a project with a `.planning/` directory
(one containing both `STATE.md` and `config.json`), the GSD provider produces a
set of `gsd_*` variables. `statusline --list-vars` prints the full set with
their current values; the two below are new.

| Variable | Example | Description |
|----------|---------|-------------|
| `{gsd_milestone}` | `v3.3.0` | Current milestone identifier, read from STATE.md's YAML frontmatter |
| `{gsd_milestone_name}` | `Cost Accuracy & Honesty` | Human-readable milestone name, same source |

```console
$ echo '{"workspace":{"current_dir":"'"$PWD"'"}}' | statusline --list-vars
...
  gsd_milestone = "v3.3.0"
  gsd_milestone_name = "Cost Accuracy & Honesty"
  gsd_phase = "P12: config-validation-machine-readable-state"
```

Both are empty strings when unavailable, so `{if gsd_milestone}...{endif}`
behaves. `{gsd_milestone_name}` is truncated by the existing
`[gsd] phase_max_width` setting — it deliberately does not add a config key of
its own. Both are blanked by `[gsd] show_phase = false`, alongside the phase,
progress and plan variables.

> **Where these variables are actually consumed.** The `gsd_*` variables are
> reported by `statusline --list-vars` and are available to library consumers
> that run the provider themselves (`GsdProvider` / `ProviderOrchestrator`).
> They are **not** wired into the statusline the binary prints: the render path
> builds its variable map without the GSD provider, so putting
> `{gsd_milestone}` in a `[layout] format` renders an empty string today. This
> is a pre-existing gap, not a property of the milestone variables — every
> `gsd_*` variable behaves the same way. It is tracked separately; wiring the
> provider into the render path is a design change, not a documentation fix.

#### The STATE.md frontmatter contract

GSD writes a YAML frontmatter block at the top of `.planning/STATE.md`.
Statusline reads **exactly four keys** from it with a small hand-rolled scanner
(no YAML library is used, and none is a dependency of this binary):

| Key | Used for |
|-----|----------|
| `gsd_state_version` | The version gate (see below) |
| `milestone` | `{gsd_milestone}` |
| `milestone_name` | `{gsd_milestone_name}` |
| `last_activity` | The staleness date behind `{gsd_stale}` (internal; not a template variable) |

Everything else in the block — `status`, `stopped_at`, `paused_at`,
`last_updated`, and the whole nested `progress:` map — is **not read at all**.

**Version gate.** Only a `gsd_state_version` whose MAJOR component is `1`
(`1.0`, `1.7`, …) is parsed. A `2.x` version, a missing version, or a
non-numeric one makes statusline ignore the block entirely and fall back to the
prose patterns. A future GSD release that reshapes the schema therefore cannot
be silently misread as wrong values.

**Graceful degradation.** Absent, unfenced, unclosed or future-versioned
frontmatter is never an error: the milestone variables stay empty, everything
else is derived exactly as it was before, and the status line still renders.
Nothing in this path can fail a render.

Three things a reader would otherwise get wrong:

1. **The frontmatter `progress:` block is deliberately NOT used.** The
   `{gsd_progress_*}` variables are computed by counting phase checkboxes in
   `.planning/ROADMAP.md`. The recorded block is a snapshot and goes stale, so
   the two sources are known to disagree — on this repository at the time of
   writing the frontmatter records `percent: 33` while the computed value is
   `66`. There is one progress source, and it is the computed one.

2. **The phase variables come from STATE.md PROSE, not from frontmatter.** GSD
   emits its `current_phase*` keys only when the STATE.md body carries a
   `Current Phase:` field, which its own body template does not write, so in
   practice those keys are never present. The prose patterns are therefore the
   phase source, and as of this release they accept more shapes than before:
   `Phase: N` without ` of M`, decimal and zero-padded phase tokens (`999.1`,
   `05.1`), and `—` / `–` / `--` as well as ` - ` in the `**Current focus:**`
   line. **This is an intended output change:** a repository whose STATE.md did
   not parse under the older, narrower patterns now reports a populated
   `{gsd_phase}` where it previously reported an empty one, and any template
   gated on `{if gsd_phase}` — including the bundled default template — gains
   its GSD segment as a result. That is the fix, not a regression. (Per the
   note above, that segment is visible through `--list-vars` and to library
   consumers rather than in the line the binary currently prints.)

3. **Do not parse STATE.md yourself.** An external tool that wants all three
   facts — milestone, phase and progress — without parsing prose should consume
   statusline's structured GSD state command, a versioned JSON document
   documented in [`docs/USAGE.md`](USAGE.md), rather than reading `.planning/`
   files directly. Statusline does the parsing once and emits a stable
   contract; the file formats above are GSD's, and can change.

### API-Equivalent Cost Variables

These variables price the session's token counts against a Claude price table
compiled into the binary. **They are opt-in** — none appears unless you reference
it in your layout format, and the default statusline is unchanged.

| Variable | Example | Description |
|----------|---------|-------------|
| `{api_equiv_cost}` | `$0.97` | Notional API-equivalent session cost |
| `{api_equiv_cost_labeled}` | `~$0.97 API-equiv` | Same figure, explicitly labeled |
| `{api_equiv_cost_input}` | `$0.50` | Uncached input tokens |
| `{api_equiv_cost_output}` | `$0.25` | Output tokens |
| `{api_equiv_cost_cache_write}` | `$0.12` | Cache-creation tokens |
| `{api_equiv_cost_cache_read}` | `$0.10` | Cache-read tokens (own ~0.1x rate) |
| `{api_equiv_cost_by_model}` | `claude-opus-4-8:$12.40 claude-haiku-4-5:$0.30` | Per-model costs from the `[ant]` usage cache |

> **This is NOT what you are billed.** It is what the same token usage *would*
> cost at public API list prices. On a Claude subscription (Pro/Max) you pay your
> plan price regardless — the figure is for comparison only, never a statement of
> charges. Use `{api_equiv_cost_labeled}` where the distinction could matter to a
> reader.
>
> The separate `{cost}` variable is unaffected by these settings, but note that it
> is **not necessarily billed money either**: it reports the `cost.total_cost_usd`
> figure Claude Code puts in the payload, which is itself a usage-based estimate.
> On a subscription that is also notional. Only API-key usage bills against a
> dollar figure like these.

**Reading the output:**

- **`unknown`** — the model has no exact entry in the price table (see
  `[pricing.aliases]` below), or the entry it has cannot price one of the token
  types present. A few upstream rows publish no separate 1-hour cache-write rate;
  rather than bill those tokens at the cheaper 5-minute rate, that model reports
  `unknown` when 1-hour cache-writes are present. Unknown is never `$0.00`. (When
  it is a *synced* row that lacks that rate, the same id's bundled 1-hour rate is
  spliced in where doing so is safe — see the per-id union under `[pricing]`
  below.)
- **A trailing `+`** (e.g. `$0.25+`) — the payload reported only some of the four
  cost dimensions, so the figure is a *lower bound*: the real API-equivalent cost
  is at least that much. An absent dimension cannot be distinguished from genuine
  zero usage, so it is disclosed rather than silently treated as zero.
- **A missing variable** — the payload carried no token counts at all. The
  variables are omitted entirely rather than rendering `$0.00`.
- `{api_equiv_cost_by_model}` reflects the **organization-wide month-to-date**
  usage cache, not this session, so it can legitimately differ in scale from the
  session headline beside it.

**Known limitation — long-context sessions are understated.** Some models charge
a higher rate once a request exceeds 200k tokens (for Sonnet 4/4.5, roughly 2x
input and 1.5x output). The bundled price table records only standard-tier rates,
so a session past that threshold is priced low — around 39% low in the worst case
— and this is **not** currently flagged with `unknown` or a trailing `+`. Treat
the figure as a floor for long-context sessions on those models. Likewise, the
session headline prices cache-creation tokens at the 5-minute rate because the
payload does not break them down by TTL; a session using 1-hour caching is
understated on that component.

### `[pricing]` Configuration

```toml
[pricing]
# Where prices come from. Default: "auto".
#   "auto"    - use the cache written by `statusline ant sync-pricing` while it
#               is fresher than max_age; otherwise the bundled table
#   "bundled" - only the price table compiled into the binary; the cache is
#               never even read
#   "synced"  - the synced cache regardless of its age (you are opting out of
#               staleness demotion)
source = "auto"

# How long a synced cache stays fresh under source = "auto".
# Single-unit duration: s / m / h / d. Default: "30d".
# Ignored under "bundled" and "synced". An unparseable value behaves like the
# default rather than disabling demotion.
max_age = "30d"

# Map a model id your setup reports to an EXACT id in the price table.
# Useful behind a proxy or gateway that rewrites model names.
[pricing.aliases]
"my-gateway/opus" = "claude-opus-4-8"
```

Lookup is **exact-match only** — no fuzzy matching, no prefix matching, no
normalization, and aliases do not chain. An alias whose target is not itself a
table entry resolves to `unknown`. This is deliberate: a wrong price is worse than
no price. This holds identically for both sources.

**How the source is chosen.** Under `source = "auto"` a synced cache younger
than `max_age` is selected and anything older is ignored in favor of the bundled
table; under `"synced"` the cache is selected regardless of its age; under
`"bundled"` the cache is never read at all. Selection is **per id**, though: even
when the synced cache is selected, an individual model may still be priced from
the bundled row, because the two tables are consulted as a union that gap-fills
(see below). Selecting a source therefore decides which table gets *first refusal*
per model id — not that every model in the line came from it.

**A synced cache can only ever add or update prices.** Lookups run against the
per-id union of the two sources: a synced row wins for the ids it covers, and the
bundled table fills every gap it leaves — including when a synced row is present
but unusable. An unusable synced row also falls **through** to `[pricing.aliases]`
rather than suppressing it, so an aliased model that used to price cannot be
turned into `unknown` by a refresh.

The union resolves the one **optional** rate per dimension, not merely per row.
When a winning synced row omits the optional 1-hour cache-write rate
(`cache_creation_1h`), that single dimension is backfilled from the **same model
id's** bundled row — never from another id, and never from the cache itself. The
backfill is admitted only when the bundled 1-hour rate is finite, is inside the
plausibility band (`1e-9`–`1e-2`), and is not cheaper than the winning synced
row's own 5-minute `cache_creation` rate.

A refresh therefore cannot make a model that used to price render `unknown` — with
one deliberate exception. If a refresh both drops the 1-hour rate **and** raises
that row's 5-minute rate above the bundled 1-hour rate, the bundled rate is
refused rather than substituted, and that model renders `unknown` for sessions
that used 1-hour cache-creation tokens. Substituting a rate known to understate
the charge would replace an honest `unknown` with a confident wrong number, which
is the one thing the pricing path never does.

A missing, corrupt or wrong-schema cache is silently ignored and the bundled table
is used, so rendering never fails and never blanks because of the cache.

**What makes a synced row unusable.** A row is used only if all four of its rates
(`input`, `output`, `cache_creation`, `cache_read`) are finite and inside the
plausibility band `1e-9`–`1e-2` USD per token, **and** its `cache_read` rate is
strictly below its `input` rate. Anything else is refused and the bundled row
prices that id. The band exists so an upstream typo — a `3e-6` rate published as
`3e6` — cannot win the union and render a wildly wrong cost; the bundled table's
own rates span roughly `3e-8` to `7.5e-5`, so the band leaves ample headroom on
both sides. The same gate is applied identically to bundled and synced rows, so a
hand-edited or foreign-producer cache gets no special trust.

**Where the cache lives.**

| Platform | Path |
|----------|------|
| Linux | `${XDG_CACHE_HOME:-~/.cache}/claudia-statusline/ant/prices.json` |
| macOS | `~/Library/Caches/claudia-statusline/ant/prices.json` |

Deleting that file restores bundled-only behavior immediately — nothing else
needs changing. To inspect the cache, read the file: it records `fetched_at`
(RFC3339 UTC), `source` (the upstream URL that was fetched), `version` (a content
hash of the raw upstream payload) and its `prices` table. The
`statusline ant sync-pricing` command prints the same `Cache:` / `Source:` /
`Snapshot:` values in its success summary unless you pass `--quiet`.

**Known limitation: a stale synced row can outrank a *corrected* bundled row.**
Under `source = "auto"`, freshness is measured against the clock only — the
cache's `fetched_at` against `max_age` — and never against the bundled table's own
`vendored_at`. So after an upgrade that ships a **corrected** bundled rate, a
synced snapshot captured before that correction keeps winning the union for that
id for up to `max_age` (30 days by default). This is not hypothetical for this
project: the `claude-opus-4-8` row was once overstated by 3x and corrected in the
bundled table. Two remedies work today: delete `prices.json` (the next
`ant sync-pricing` re-fetches it), or set `source = "bundled"` until you re-sync.
This limitation is **not** fixed today; it is tracked for Phase 12, alongside the
`config --validate` work over the same `[pricing]` surface.

**The render side is bounded, and degrades rather than fails.** A `prices.json`
larger than 1 MiB is rejected **before it is parsed**: the reader reads at most one
byte past the cap and refuses the whole file rather than parsing a prefix, so the
parse work is bounded to 1 MiB. A cache that clears the byte cap but then turns out
to carry more than 4096 rows is discarded **after** parsing, before any lookup runs
against it — the row cap bounds the rows that are retained and the lookup work
done over them, not the parse itself. A file with an unexpected `schema_version`,
or one that is not valid JSON, is discarded the same way. In every one of these
cases the render silently falls back to the bundled table.

Refresh the cache with `statusline ant sync-pricing` — an explicit, out-of-band,
keyless command that reads no Anthropic credential. **Rendering itself never
touches the network** regardless of `source`: it only reads the cache file that
command wrote. Nothing refreshes the cache automatically — see
[INSTALLATION.md](INSTALLATION.md#ant-enrichment-refresh-optional) for the
SessionStart hook, cron and launchd recipes. Until you schedule one of them,
`source = "auto"` has no cache to prefer and every render uses the bundled table.

The bundled table is vendored from
[LiteLLM](https://github.com/BerriAI/litellm) (MIT) by
`scripts/vendor-pricing.sh` and covers every Claude model LiteLLM carries.
Regenerate it with `make vendor-pricing`, or verify it reproduces byte-for-byte
with `make vendor-pricing-check`. Rendering never touches the network: the table
is compiled in.

If a bad value in `[pricing]` fails to parse, only this section falls back to
defaults — the rest of your configuration is preserved, and a warning is logged.

### Layout Mode vs Legacy Mode

The statusline supports two display modes:

**Layout Mode** (template-based):
- Activated when `[layout] format` is set or `preset` is not "default"
- Uses template variables like `{token_rate}`, `{token_rate_only}`, etc.
- Token rate format controlled by `[layout.components.token_rate] format` options:
  - `rate_only`: Just the rate (e.g., "12.5 tok/s")
  - `with_session`: Rate + session total (e.g., "12.5 tok/s • 150K")
  - `with_daily`: Rate + daily total (e.g., "12.5 tok/s (day: 2.5M)")
  - `full`: All three components

**Legacy Mode** (non-template):
- Used when no custom layout is configured
- Token rate format controlled by `[token_rate] display_mode`:
  - `summary`: Simple rate only (default)
  - `detailed`: Breakdown by input/output tokens with cache metrics
  - `cache_only`: Focus on cache metrics and ROI

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
format = "abbreviation"  # Options: abbreviation (default), full, name, version
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

#### Model Format Options

| Format | Example Output | Description |
|--------|---------------|-------------|
| `abbreviation` | `O4.5`, `S4.5`, `H4.5` | Short form with version (default) |
| `full` | `Claude Opus 4.5` | Full display name from Claude |
| `name` | `Opus`, `Sonnet`, `Haiku` | Model family only, no version |
| `version` | `4.5` | Version number only |

**Template variables** (always available regardless of format):
- `{model}` - Uses configured format
- `{model_full}` - Always full name
- `{model_name}` - Always family name only

### Color Override Values

Component colors accept:
- **Named colors**: `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`, `orange`
- **Hex colors**: `#FF5733` or `#F53`
- **256 colors**: `38;5;208` (ANSI format)
- **ANSI codes**: `\x1b[32m` (passthrough)

### Custom User Presets

Create custom presets in your config directory's `presets/` subdirectory — on macOS
that is `~/Library/Application Support/claudia-statusline/presets/`, on Linux
`~/.config/claudia-statusline/presets/`. Run `statusline config path` if unsure:

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

Create `mytheme.toml` in your config directory (`statusline config path` names it;
`~/Library/Application Support/claudia-statusline/` on macOS,
`~/.config/claudia-statusline/` on Linux):

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

## Token Rate Configuration

Real-time token consumption tracking with configurable display options.

> **Note**: Token rate features always work in v3.0.0+ (SQLite-only).

### Basic Configuration

```toml
[token_rate]
enabled = true                # Enable token rate tracking
display_mode = "detailed"     # "summary", "detailed", or "cache_only"
```

### Rate Display Options

Control which rates to display:

```toml
[token_rate]
rate_display = "both"         # "both", "output_only", "input_only"
```

| Value | Output Example | Description |
|-------|----------------|-------------|
| `both` | `In:5.2K Out:8.7K tok/s` | Show both input and output rates |
| `output_only` | `Out:8.7K tok/s` | Show only generation rate |
| `input_only` | `In:5.2K tok/s` | Show only context rate |

### Rolling Window (Responsive Rates)

Enable responsive output rate that reacts quickly to changes:

```toml
[token_rate]
rate_window_seconds = 60      # Calculate output rate from last 60 seconds
```

**Hybrid approach:**
- **Input rate**: Always uses session average (stable, context-based)
- **Output rate**: Uses rolling window when configured (responsive, generation-based)

This provides the best of both worlds—stable context tracking with responsive generation speed.

| Value | Behavior |
|-------|----------|
| `0` | Session average for both rates (default) |
| `30` | 30-second window for output rate |
| `60` | 60-second window (balanced) |
| `300` | 5-minute window (smoother) |

### Display Modes

| Mode | Output Example | Description |
|------|----------------|-------------|
| `summary` | `12.5 tok/s • 150K` | Single combined rate |
| `detailed` | `In:5.2K Out:8.7K tok/s` | Separate input/output rates |
| `cache_only` | `Cache:85%` | Focus on cache efficiency |

### Time Units

```toml
[layout.components.token_rate]
time_unit = "second"          # "second", "minute", or "hour"
```

## Claude API Pricing Reference

> **Note**: This is a reference table only. The statusline receives pre-calculated costs
> from Claude Code—it does not calculate costs from tokens. Pricing may change;
> see [Anthropic's official pricing](https://docs.anthropic.com/en/docs/about-claude/pricing) for current rates.

### Model Pricing (November 2025)

| Model | Input | Output | Cache Write (5-min) | Cache Read |
|-------|-------|--------|---------------------|------------|
| **Opus 4** | $15.00/M | $75.00/M | $18.75/M (1.25×) | $1.50/M (0.1×) |
| **Opus 4.5** | $5.00/M | $25.00/M | $6.25/M (1.25×) | $0.50/M (0.1×) |
| **Sonnet 4** | $3.00/M | $15.00/M | $3.75/M (1.25×) | $0.30/M (0.1×) |
| **Sonnet 4.5** | $3.00/M | $15.00/M | $3.75/M (1.25×) | $0.30/M (0.1×) |
| **Haiku 3.5** | $0.80/M | $4.00/M | $1.00/M (1.25×) | $0.08/M (0.1×) |

*M = million tokens. Cache write multiplier: 1.25× input price. Cache read: 0.1× input price.*

### Understanding Your Burn Rate

The burn rate shown (e.g., `$64.70/hr`) is calculated from:

```
burn_rate = (session_cost × 3600) / session_duration_seconds
```

**Common cost drivers:**

| Token Type | Relative Cost | Notes |
|------------|---------------|-------|
| **Cache creation** | 1.25× input | Initial cache builds are expensive |
| **Output** | 5× input | Generation costs more than input |
| **Cache read** | 0.1× input | Cached context is 90% cheaper |
| **Input** | 1× (base) | Standard prompt/context cost |

### Example Cost Breakdown

For a 30-minute Opus 4 session with heavy cache building:

| Token Type | Count | Rate | Cost |
|------------|-------|------|------|
| Cache creation | 1,000,000 | $18.75/M | $18.75 |
| Output | 20,000 | $75.00/M | $1.50 |
| Cache read | 150,000 | $1.50/M | $0.23 |
| Input | 1,000 | $15.00/M | $0.02 |
| **Total** | | | **$20.50** |

Burn rate: $20.50 × 2 = **$41.00/hr**

### Cost Optimization Tips

1. **Leverage cache reads**: Once cached, reads are 90% cheaper than re-sending context
2. **Use Opus 4.5**: 67% cheaper than Opus 4 with similar capabilities
3. **Monitor cache creation**: High `cache_creation_tokens` = high initial cost
4. **Batch operations**: 50% discount on batch API calls

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

SQLite is the canonical store. All advanced features work out of the box:

**Advanced features (always available in v3.0.0+):**
- **Token rates**: Real-time token consumption tracking (`[token_rate] enabled = true`)
- **Rolling window rates**: Responsive rate updates (`rate_window_seconds > 0`)
- **Adaptive context learning**: Automatic context window detection
- **Cloud sync**: Multi-device synchronization (when enabled)

**Cleaning up a leftover JSON file:**
```bash
statusline migrate --finalize
```

### json_backup (removed in v3.0.0)

The `database.json_backup` field is no longer documented as part of the
active config surface. v2.x configs that still contain it parse successfully
but the value is functionally meaningless: the binary emits a one-line stderr
deprecation note when set to `true` and continues rendering from SQLite.
Remove the field from your config to silence the note. A leftover stats.json
file is still read once on startup for recovery **when SQLite is missing or
unusable** (see MIGRATION_GUIDE.md).

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

**Token tracking note (auto_reset mode):**

> ⚠️ **Token totals may spike after auto-reset events.** Unlike cost and lines (which use
> archived baselines), token tracking treats post-reset values as fresh counts. This means
> daily/monthly token totals will include the full session token count as a delta after each
> reset, potentially inflating totals.
>
> **Example:** Session has 50K tokens, auto-resets, then accumulates 10K more.
> Daily total becomes: 50K (full) + 10K (delta) = 60K instead of just 60K cumulative.
>
> For precise token continuity, consider using `wall_clock` mode instead.

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
# Show the active config file and the full search order (authoritative)
statusline config path

# Same, machine-readable
statusline config path --json | jq '.active'

# Validate the config that would actually be loaded
statusline config validate

# Machine-readable, and fail on warnings too (useful in CI)
statusline config validate --json --strict
```

`config validate` reports unknown keys, type errors, invalid values and unreachable
caches. It exits 0 on a clean or warnings-only config and non-zero on any error; see
[Exit codes](#exit-codes).

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
- Run `statusline config path` — it names the active file and every candidate that was
  searched, and flags a config sitting at a path this platform never reads. On macOS the
  config dir is `~/Library/Application Support/claudia-statusline/`, NOT `~/.config/`
  (see [Config File Location](#config-file-location))
- Run `statusline config validate` — it reports TOML syntax errors, unknown keys and
  type errors directly
- Check permissions: `chmod 644 "$(statusline config path --json | jq -r '.active')"`

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
