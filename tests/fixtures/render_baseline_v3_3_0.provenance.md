# `render_baseline_v3_3_0_*.txt` — provenance

Golden render lines used by `tests/agents_render_tests.rs` (Phase 17, AGENT-01 / SC1:
"with default config the render is byte-identical to v3.3.0"). They were captured from the
**`v3.3.0` tag binary**, never from the working tree, so they are an external reference
(Phase 10 lesson: verify against an external reference, never an in-repo constant).

| Field | Value |
|-------|-------|
| Source | tag `v3.3.0` (`git rev-list -n1 v3.3.0` = `69d8efb756daca24b7601b94e14e554eaebbacd8`) |
| Build | `cargo build --release` of `git archive v3.3.0`, rustc 1.96.0 (ac68faa20 2026-05-25) |
| Captured | 2026-10-08 |
| Color | `NO_COLOR=1` (no ANSI escapes in any fixture) |
| Trailing newline | none (v3.3.0 prints the line without one); fixtures are byte-exact stdout |

## Inputs (all synthetic, no real transcript data)

- `agents_tree/0a1b2c3d-0000-4000-8000-000000000001.jsonl` — main transcript: a user line plus
  assistant lines with `requestId` and `message.usage`; request `req_synth_main_0001` is
  written twice and its `output_tokens` is revised on the later line (multiblock shape).
- `agents_tree/0a1b2c3d-0000-4000-8000-000000000001/subagents/agent-a0000000000000001.jsonl` —
  three assistant requests from one subagent.
- `.../agent-a0000000000000001.meta.json` — `{"agentType":"Explore"}`.
- `render_baseline_v3_3_0_payload.json` — session id, `transcript_path` placeholder
  `__TRANSCRIPT_PATH__`, `workspace.current_dir = "/tmp"`, model, cost, version and the full
  documented `prompt_cache` object (code.claude.com/docs/en/statusline, 2026-10-08).
- `render_baseline_v3_3_0_bad_prompt_cache_payload.json` — identical except a wrong-typed
  `prompt_cache`: `{"misses":"2","hit_ratio":"high","warm":"yes","last_miss_cause":{"causes":"tools_changed"}}`.

All timestamps are in the past and nothing in the payload produces a wall-clock-dependent
segment.

| Fixture | Payload | Config |
|---------|---------|--------|
| `render_baseline_v3_3_0_agents.txt` | `render_baseline_v3_3_0_payload.json` | none (defaults) |
| `render_baseline_v3_3_0_bad_prompt_cache.txt` | `render_baseline_v3_3_0_bad_prompt_cache_payload.json` | none (defaults) |
| `render_baseline_v3_3_0_detailed.txt` | `render_baseline_v3_3_0_payload.json` | `[layout]` / `preset = "detailed"` |
| `render_baseline_v3_3_0_power.txt` | `render_baseline_v3_3_0_payload.json` | `[layout]` / `preset = "power"` |

The two config files are exactly:

```toml
[layout]
preset = "detailed"
```

```toml
[layout]
preset = "power"
```

v3.3.0 honors `STATUSLINE_CONFIG` (`src/config.rs` line 1868 at the tag, after
`STATUSLINE_CONFIG_PATH`), so the config is passed that way.

## Build the reference binary (no worktree, no install)

```bash
SCRATCH=$(mktemp -d)
mkdir -p "$SCRATCH/v330"
git archive v3.3.0 | tar -x -C "$SCRATCH/v330"
env CARGO_TARGET_DIR="$SCRATCH/v330-target" cargo build --release \
    --manifest-path "$SCRATCH/v330/Cargo.toml"
BIN="$SCRATCH/v330-target/release/statusline"
```

Never `make install` this binary.

## Capture script (`capture.sh`)

```bash
#!/bin/bash
# usage: capture.sh <binary> <repo> <name> <payload-template> <preset-or-empty> <out>
set -eu
BIN=$1; REPO=$2; NAME=$3; TPL=$4; PRESET=$5; OUT=$6
SID=0a1b2c3d-0000-4000-8000-000000000001
X=$(mktemp -d)
/bin/cp -R "$REPO/tests/fixtures/agents_tree" "$X/tree"
sed "s|__TRANSCRIPT_PATH__|$X/tree/$SID.jsonl|" "$REPO/tests/fixtures/$TPL" > "$X/payload.json"
EXTRA=()
if [ -n "$PRESET" ]; then
  printf '[layout]\npreset = "%s"\n' "$PRESET" > "$X/config.toml"
  EXTRA=(STATUSLINE_CONFIG="$X/config.toml")
fi
cd /tmp
env -i PATH=/usr/bin:/bin HOME="$X" XDG_CONFIG_HOME="$X/c" XDG_DATA_HOME="$X/d" XDG_CACHE_HOME="$X/cache" NO_COLOR=1 "${EXTRA[@]+"${EXTRA[@]}"}" \
  "$BIN" < "$X/payload.json" > "$OUT"
if grep -qF "$X" "$OUT"; then echo "TEMPDIR LEAK in $NAME" >&2; exit 2; fi
```

Each capture uses a fresh `mktemp -d` home/XDG tree, a non-git CWD (`/tmp`) and a clean
environment (`env -i`). The binary logs a harmless "SQLite database not found" warning on
stderr for the fresh data dir; only stdout is captured.

## Capture + determinism proof

```bash
R=$(git rev-parse --show-toplevel)
for run in 1 2; do mkdir -p "$SCRATCH/cap$run"
  ./capture.sh "$BIN" "$R" agents           render_baseline_v3_3_0_payload.json              ""       "$SCRATCH/cap$run/agents.txt"
  ./capture.sh "$BIN" "$R" bad_prompt_cache render_baseline_v3_3_0_bad_prompt_cache_payload.json ""   "$SCRATCH/cap$run/bad_prompt_cache.txt"
  ./capture.sh "$BIN" "$R" detailed         render_baseline_v3_3_0_payload.json              detailed "$SCRATCH/cap$run/detailed.txt"
  ./capture.sh "$BIN" "$R" power            render_baseline_v3_3_0_payload.json              power    "$SCRATCH/cap$run/power.txt"
  [ $run = 1 ] && sleep 2
done
for f in agents bad_prompt_cache detailed power; do
  cmp "$SCRATCH/cap1/$f.txt" "$SCRATCH/cap2/$f.txt"; echo "$f cmp exit=$?"
done
```

Result on 2026-10-08 (two runs, fresh tempdir each, 2 s apart):

```
agents cmp exit=0
bad_prompt_cache cmp exit=0
detailed cmp exit=0
power cmp exit=0
```

No output contained its tempdir path (the script's leak check passed for all eight runs).
The run-1 outputs were copied verbatim to `tests/fixtures/render_baseline_v3_3_0_<name>.txt`.

## Verify

Re-run the capture above with a freshly built v3.3.0 binary, then:

```bash
for f in agents bad_prompt_cache detailed power; do
  cmp "$SCRATCH/cap1/$f.txt" "tests/fixtures/render_baseline_v3_3_0_$f.txt" && echo "$f OK"
done
```

Do not edit or re-capture these fixtures to make a failing test pass. A mismatch means the
working tree diverged from v3.3.0 and must be investigated.

## Lib-path goldens (`*_lib.txt`)

`render_baseline_v3_3_0_agents_lib.txt` and `render_baseline_v3_3_0_bad_prompt_cache_lib.txt`
are the output of **v3.3.0's own `statusline::render_from_json(payload, false)`**, captured
from the tag source, because v3.3.0's two render paths already differ on this payload:

```
binary: /tmp • 6% [=>--------] • O4.5 • +42 -7 • $1.23
lib:    /tmp • 6% [=>--------] • O4.5 • +42 -7 • $1.23 ($7.41/hr)
```

Cause (v3.3.0 `src/display.rs` ~line 578): the burn-rate duration is taken from the
session's DB row first, then the transcript, then `cost.total_duration_ms`. The spawned
binary runs with `update_stats = true`, so it has just created the session row and its
duration is under `burn_rate.min_duration_seconds` (no burn rate). The lib call uses
`update_stats = false`, finds no row and falls through to the payload's 600 s duration,
giving `$1.2345 * 3600 / 600 = $7.41/hr`. This is pre-existing v3.3.0 behavior, not a
Phase 17 regression, so each path is pinned to what v3.3.0 itself produces on that path.

Capture: `tests/agents_render_tests.rs` (the Phase 17 harness, `lib_render`) was copied into
the extracted `v3.3.0` tree together with the fixtures, plus this capture-only test:

```rust
#[test]
#[serial_test::serial]
fn capture_v3_3_0_lib_goldens() {
    let Some(dir) = std::env::var_os("CAPTURE_DIR") else { return };
    let dir = PathBuf::from(dir);
    std::fs::write(dir.join("agents_lib.txt"), lib_render(PAYLOAD)).unwrap();
    std::fs::write(dir.join("bad_prompt_cache_lib.txt"), lib_render(BAD_PROMPT_CACHE_PAYLOAD)).unwrap();
}
```

```bash
cd "$SCRATCH/v330"
for run in 1 2; do mkdir -p "$SCRATCH/libcap$run"; T=$(mktemp -d)
  env CAPTURE_DIR="$SCRATCH/libcap$run" CARGO_TARGET_DIR="$SCRATCH/v330-target" \
      XDG_CONFIG_HOME=$T/c XDG_DATA_HOME=$T/d STATUSLINE_TEST_MODE=1 \
      cargo test --test agents_render_tests capture_v3_3_0_lib_goldens
  [ $run = 1 ] && sleep 2
done
for f in agents_lib bad_prompt_cache_lib; do
  cmp "$SCRATCH/libcap1/$f.txt" "$SCRATCH/libcap2/$f.txt"; echo "$f cmp exit=$?"
done
```

Result on 2026-10-08: `agents_lib cmp exit=0`, `bad_prompt_cache_lib cmp exit=0`; neither
output contains a tempdir path. Run-1 outputs were copied verbatim to
`tests/fixtures/render_baseline_v3_3_0_<name>.txt`. Running the unmodified golden tests
against the v3.3.0 source also showed the four binary goldens pass there.
