#!/bin/bash
#
# check-hook-isolation.sh — Canary proof that the hook tests never touch a real
# data/cache directory (INTEG-06 / Phase 14 SC4).
#
# The production stats.db is written continuously by the live statusline, so its
# mtime can never serve as evidence of isolation. Instead this script seeds a
# throwaway canary HOME + XDG tree, runs the hook tests with that tree as the
# "real" environment, and proves the canary is byte- and mtime-identical after.
#
# WHAT THIS DOES:
#   1. Builds the binary and the hook test targets under the real environment
#      (so the later passes only run tests, never compile).
#   2. Seeds two canary stats.db files — the XDG_DATA_HOME location and the
#      dirs::data_dir() fallback location under the canary HOME — by piping a
#      session payload into target/debug/statusline, then stamps every session
#      with max_tokens_observed = 12345. Also seeds a canary hook state file in
#      the cache dir.
#   3. Snapshots every canary file (shasum + mtime), excluding any cargo/rustup
#      dirs under the canary HOME.
#   4. Pass A: runs hook_integration_tests, lib hook_handler tests and bin
#      hook_handler tests with HOME and XDG_* pointed at the canary.
#      Pass B: the same with XDG_* unset (fallback resolution under HOME).
#   5. After each pass re-snapshots and diffs against the baseline, and checks
#      the 12345 stamp survived in both DBs. Any difference = leak.
#
# USAGE:
#   scripts/check-hook-isolation.sh
#
# EXIT CODES:
#   0  hook tests isolated (canary untouched, XDG set and unset)
#   1  leak detected, or a test failed
#
set -euo pipefail

cd "$(dirname "$0")/.."

REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

CANARY="$(mktemp -d)"
SNAPDIR="$(mktemp -d)"
trap '/bin/rm -rf "$CANARY" "$SNAPDIR"' EXIT

OS="$(uname)"
if [ "$OS" = "Darwin" ]; then
    FALLBACK_DATA="$CANARY/home/Library/Application Support"
    CACHE_STATE_DIR="$CANARY/home/Library/Caches/claudia-statusline"
else
    FALLBACK_DATA="$CANARY/home/.local/share"
    CACHE_STATE_DIR="$CANARY/cache/claudia-statusline"
fi
XDG_DB="$CANARY/data/claudia-statusline/stats.db"
FALLBACK_DB="$FALLBACK_DATA/claudia-statusline/stats.db"

SEED='{"session_id":"canary-1","workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet 4.5"},"cost":{"total_cost_usd":0.5}}'

echo "==> Building binary and hook test targets (real environment)"
cargo build --quiet
cargo test --quiet --no-run --test hook_integration_tests --lib --bin statusline

mkdir -p "$CANARY/home" "$CANARY/data" "$CANARY/config" "$CANARY/cache"

echo "==> Seeding canary data dirs"
# XDG-set location
printf '%s' "$SEED" | env -u STATUSLINE_CONFIG_PATH -u STATUSLINE_CONFIG \
    HOME="$CANARY/home" XDG_DATA_HOME="$CANARY/data" \
    XDG_CONFIG_HOME="$CANARY/config" XDG_CACHE_HOME="$CANARY/cache" \
    target/debug/statusline >/dev/null
# Fallback location (XDG unset -> dirs::data_dir() under canary HOME)
printf '%s' "$SEED" | env -u STATUSLINE_CONFIG_PATH -u STATUSLINE_CONFIG \
    -u XDG_DATA_HOME -u XDG_CONFIG_HOME -u XDG_CACHE_HOME \
    HOME="$CANARY/home" \
    target/debug/statusline >/dev/null

for db in "$XDG_DB" "$FALLBACK_DB"; do
    if [ ! -f "$db" ]; then
        echo "ERROR: seeding did not create $db" >&2
        exit 1
    fi
    sqlite3 "$db" "UPDATE sessions SET max_tokens_observed = 12345;"
done

mkdir -p "$CACHE_STATE_DIR"
printf '{}' > "$CACHE_STATE_DIR/state-canary.json"

snapshot() {
    local files
    files="$(find "$CANARY" \( -path "$CANARY/home/.cargo" -o -path "$CANARY/home/.rustup" \) -prune -o -type f -print | LC_ALL=C sort)"
    [ -z "$files" ] && return 0
    while IFS= read -r f; do
        if [ "$OS" = "Darwin" ]; then
            printf '%s  %s\n' "$(shasum "$f" | cut -d' ' -f1)" "$(stat -f '%m %N' "$f")"
        else
            printf '%s  %s\n' "$(shasum "$f" | cut -d' ' -f1)" "$(stat -c '%Y %n' "$f")"
        fi
    done <<< "$files"
}

snapshot > "$SNAPDIR/before"
echo "==> Baseline: $(wc -l < "$SNAPDIR/before" | tr -d ' ') canary files"

run_tests() {
    # "$@" = extra env args (e.g. XDG vars or -u flags)
    env "$@" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_HOME="$REAL_RUSTUP_HOME" \
        HOME="$CANARY/home" cargo test --quiet --test hook_integration_tests
    env "$@" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_HOME="$REAL_RUSTUP_HOME" \
        HOME="$CANARY/home" cargo test --quiet --lib hook_handler
    env "$@" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_HOME="$REAL_RUSTUP_HOME" \
        HOME="$CANARY/home" cargo test --quiet --bin statusline hook_handler
}

check_canary() {
    local pass="$1"
    snapshot > "$SNAPDIR/after-$pass"
    local leak=0
    if ! diff -u "$SNAPDIR/before" "$SNAPDIR/after-$pass"; then
        leak=1
    fi
    for db in "$XDG_DB" "$FALLBACK_DB"; do
        local n
        n="$(sqlite3 "$db" "SELECT COUNT(*) FROM sessions WHERE max_tokens_observed = 12345;" 2>/dev/null || echo 0)"
        if [ "${n:-0}" -lt 1 ]; then
            echo "12345 stamp lost in $db" >&2
            leak=1
        fi
        if [ -e "$db-wal" ] || [ -e "$db-shm" ]; then
            echo "WAL/SHM sidecar appeared next to $db" >&2
            leak=1
        fi
    done
    if [ "$leak" -ne 0 ]; then
        echo "LEAK: hook tests touched the canary data/cache dir (pass $pass)" >&2
        exit 1
    fi
    echo "    pass $pass: canary untouched"
}

echo "==> Pass A: XDG_* set to canary"
if ! run_tests XDG_DATA_HOME="$CANARY/data" XDG_CONFIG_HOME="$CANARY/config" XDG_CACHE_HOME="$CANARY/cache"; then
    echo "FAIL: hook tests failed in pass A" >&2
    exit 1
fi
check_canary A

echo "==> Pass B: XDG_* unset (dirs fallback under canary HOME)"
if ! run_tests -u XDG_DATA_HOME -u XDG_CONFIG_HOME -u XDG_CACHE_HOME; then
    echo "FAIL: hook tests failed in pass B" >&2
    exit 1
fi
check_canary B

echo "✓ hook tests isolated (XDG set and unset)"
exit 0
