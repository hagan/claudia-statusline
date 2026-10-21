#!/usr/bin/env bash
#
# check-ci-test-steps.sh — Local proof that a failing `cargo test` fails the CI
# test steps in .github/workflows/test.yml (Phase 16 D-12, cross-AI review).
#
# Both test steps pipe cargo through `tee`. Without pipefail the pipeline takes
# tee's exit status (always 0), so a cargo failure would be reported as a pass.
#
# WHAT THIS DOES:
#   For each step (`test` in job test-matrix, `isolated_test` in job
#   test-isolated):
#   1. Asserts the step declares `shell: bash` (GitHub then runs it with
#      `bash --noprofile --norc -eo pipefail {0}`).
#   2. Extracts the step's `run` script with yq and strips every `${{ ... }}`
#      expression.
#   3. Prefixes the script with a shell function named `cargo` that either
#      fails (prints a FAILED test result, returns 101) or passes.
#   4. Runs both variants under `bash -e` (GitHub's default when `shell` is
#      unset) and `bash -eo pipefail` (`shell: bash`), and requires exit 1 for
#      the failing shim and exit 0 for the passing shim in every combination.
#
# This is a local check; it is not wired into CI.
#
# USAGE:
#   scripts/check-ci-test-steps.sh
#
# EXIT CODES:
#   0  every case behaves correctly
#   1  at least one case failed (a cargo failure could be masked)
#   2  yq is not installed
#
set -euo pipefail

cd "$(dirname "$0")/.."

WORKFLOW=".github/workflows/test.yml"

if ! command -v yq >/dev/null 2>&1; then
    echo "check-ci-test-steps: yq is required (https://github.com/mikefarah/yq)" >&2
    exit 2
fi

WORK="$(mktemp -d)"
trap '/bin/rm -rf "$WORK"' EXIT

FAILING_SHIM='cargo() { echo "test result: FAILED. 3 passed; 1 failed"; return 101; }'
PASSING_SHIM='cargo() { echo "test result: ok. 4 passed; 0 failed"; return 0; }'

failures=0

# job:step pairs. The step id `test` also exists in the test-build job (which
# runs `make test` without tee), so each step is selected within its own job.
for pair in test-matrix:test test-isolated:isolated_test; do
    job="${pair%%:*}"
    step="${pair#*:}"
    selector=".jobs.\"$job\".steps[] | select(.id == \"$step\")"
    shell_key="$(yq "$selector | .shell" "$WORKFLOW")"
    if [ "$shell_key" = "bash" ]; then
        echo "PASS  $step: shell is bash"
    else
        echo "FAIL  $step: shell is '$shell_key', expected 'bash'"
        failures=$((failures + 1))
    fi

    body="$WORK/$step.body.sh"
    yq "$selector | .run" "$WORKFLOW" \
        | sed -E 's/\$\{\{[^}]*\}\}//g' > "$body"
    if [ ! -s "$body" ]; then
        echo "FAIL  $step: could not extract run script"
        failures=$((failures + 1))
        continue
    fi

    { echo "$FAILING_SHIM"; cat "$body"; } > "$WORK/$step.failing.sh"
    { echo "$PASSING_SHIM"; cat "$body"; } > "$WORK/$step.passing.sh"

    for mode in "-e" "-eo pipefail"; do
        for shim in failing passing; do
            if [ "$shim" = "failing" ]; then expected=1; else expected=0; fi
            rundir="$WORK/run-$step-$shim-${mode// /_}"
            mkdir -p "$rundir"
            set +e
            # shellcheck disable=SC2086 # $mode is intentionally word-split
            (cd "$rundir" && GITHUB_OUTPUT="$rundir/output" GITHUB_STEP_SUMMARY="$rundir/summary" \
                bash --noprofile --norc $mode "$WORK/$step.$shim.sh" >/dev/null 2>&1)
            actual=$?
            set -e
            if [ "$actual" -eq "$expected" ]; then
                echo "PASS  $step [bash $mode] $shim cargo -> exit $actual"
            else
                echo "FAIL  $step [bash $mode] $shim cargo -> exit $actual (expected $expected)"
                failures=$((failures + 1))
            fi
        done
    done
done

if [ "$failures" -ne 0 ]; then
    echo "check-ci-test-steps: $failures case(s) failed" >&2
    exit 1
fi
echo "check-ci-test-steps: all cases passed"
