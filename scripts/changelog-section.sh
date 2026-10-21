#!/usr/bin/env bash
#
# changelog-section.sh — Print the CHANGELOG.md section for one version
# (Phase 19 D-14).
#
# The release workflow (.github/workflows/release.yml) builds each GitHub
# Release body from the tagged version's CHANGELOG section, so contributor
# credit and release notes live in one place.
#
# WHAT THIS DOES:
#   1. Finds the heading `## [<version>]` (optionally followed by a space and
#      a date, provisional or final). The version is compared as a literal
#      string, never as a regex, so `.` is not a wildcard.
#   2. Collects every line after that heading up to the next `## [` heading
#      (or end of file).
#   3. Trims leading and trailing blank lines and prints the rest. The heading
#      itself is not printed; callers add their own title.
#
# USAGE:
#   scripts/changelog-section.sh <version> [changelog-path]
#
#   <version>         e.g. 3.4.0 (no leading v) or Unreleased
#   [changelog-path]  defaults to CHANGELOG.md at the repo root; a relative
#                     path resolves against the caller's working directory
#
# EXIT CODES:
#   0  section printed
#   1  version not found, or its section is empty
#   2  usage error or unreadable changelog file
#
set -euo pipefail

usage() {
    echo "changelog-section: $1" >&2
    echo "usage: scripts/changelog-section.sh <version> [changelog-path]" >&2
    exit 2
}

if [[ $# -lt 1 || $# -gt 2 ]]; then
    usage "expected 1 or 2 arguments, got $#"
fi

version="$1"
changelog="${2:-$(dirname "$0")/../CHANGELOG.md}"

if [[ ! "$version" =~ ^[0-9A-Za-z.+-]+$ ]]; then
    usage "invalid version: $version"
fi

if [[ ! -f "$changelog" || ! -r "$changelog" ]]; then
    usage "cannot read changelog: $changelog"
fi

rc=0
awk -v ver="$version" '
BEGIN {
    want = "## [" ver "]"
    wlen = length(want)
    found = 0
    in_section = 0
    n = 0
}
in_section {
    if (substr($0, 1, 4) == "## [") {
        in_section = 0
        exit
    }
    lines[++n] = $0
    next
}
!found && substr($0, 1, wlen) == want {
    next_char = substr($0, wlen + 1, 1)
    if (next_char == "" || next_char == " ") {
        found = 1
        in_section = 1
    }
}
END {
    if (!found) exit 3
    first = 1
    while (first <= n && lines[first] ~ /^[ \t]*$/) first++
    last = n
    while (last >= first && lines[last] ~ /^[ \t]*$/) last--
    if (first > last) exit 4
    for (i = first; i <= last; i++) print lines[i]
}
' "$changelog" || rc=$?

case "$rc" in
    0) exit 0 ;;
    3)
        echo "changelog-section: no section for $version in $changelog" >&2
        exit 1
        ;;
    4)
        echo "changelog-section: section for $version is empty" >&2
        exit 1
        ;;
    *)
        echo "changelog-section: awk failed with status $rc" >&2
        exit 2
        ;;
esac
