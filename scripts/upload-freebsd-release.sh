#!/bin/sh
# Upload FreeBSD binary to GitHub release
# Usage: ./scripts/upload-freebsd-release.sh v2.21.1
#
# This script uploads a locally-compiled FreeBSD binary to an existing GitHub release.
# Run this AFTER the CI creates the draft release with Linux binaries.
#
# Prerequisites:
#   - GitHub CLI (gh) installed and authenticated: pkg install gh
#   - Binary built with: cargo build --release
#   - Release must already exist (created by CI)

set -e

if [ -z "$1" ]; then
    echo "Usage: $0 <tag>"
    echo "Example: $0 v2.21.1"
    exit 1
fi

TAG="$1"
REPO="hagan/claudia-statusline"
BINARY="target/release/statusline"
ARTIFACT_NAME="statusline-freebsd-amd64"

# Check if binary exists
if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found at $BINARY"
    echo "Build first with: cargo build --release"
    exit 1
fi

# Verify it's a FreeBSD binary
if ! file "$BINARY" | grep -q "FreeBSD"; then
    echo "Warning: $BINARY may not be a FreeBSD binary"
    file "$BINARY"
    printf "Continue anyway? (y/N) "
    read -r REPLY
    if [ "$REPLY" != "y" ] && [ "$REPLY" != "Y" ]; then
        echo "Aborted"
        exit 1
    fi
fi

# Check if gh CLI is installed
if ! command -v gh >/dev/null 2>&1; then
    echo "Error: GitHub CLI (gh) not installed"
    echo "Install with: pkg install gh"
    exit 1
fi

# Check authentication
if ! gh auth status >/dev/null 2>&1; then
    echo "Error: Not authenticated with GitHub"
    echo "Run: gh auth login"
    exit 1
fi

# Check if release exists
if ! gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
    echo "Error: Release $TAG does not exist"
    echo "Create the release first (usually by pushing the tag)"
    exit 1
fi

# Create tar.gz archive
echo "Creating archive..."
ARCHIVE="${ARTIFACT_NAME}.tar.gz"
rm -f "$ARCHIVE" "${ARCHIVE}.sha256"

tar czf "$ARCHIVE" -C "$(dirname "$BINARY")" "$(basename "$BINARY")"

# Get binary info
SIZE=$(ls -lh "$ARCHIVE" | awk '{print $5}')

# FreeBSD uses sha256 command, fallback to shasum
if command -v sha256 >/dev/null 2>&1; then
    SHA256=$(sha256 -q "$ARCHIVE")
else
    SHA256=$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)
fi

# Create checksum file
echo "$SHA256  $ARCHIVE" > "${ARCHIVE}.sha256"

echo "Archive created:"
echo "  File: $ARCHIVE"
echo "  Size: $SIZE"
echo "  SHA256: $SHA256"
echo ""

# Upload to release
echo "Uploading to release $TAG..."
gh release upload "$TAG" "$ARCHIVE" "${ARCHIVE}.sha256" --repo "$REPO" --clobber

echo ""
echo "Upload complete!"
echo ""
echo "Add this to release notes:"
echo "| FreeBSD x86_64 | \`$ARTIFACT_NAME.tar.gz\` | \`$SHA256\` |"
