#!/bin/bash
# Upload macOS binary to GitHub release
# Usage: ./scripts/upload-macos-release.sh v2.21.1 [arch]
#
# arch can be: amd64 (Intel) or arm64 (Apple Silicon)
# Default: auto-detect based on current machine
#
# This script uploads a locally-compiled macOS binary to an existing GitHub release.
# Run this AFTER the CI creates the draft release with Linux binaries.
#
# Prerequisites:
#   - GitHub CLI (gh) installed and authenticated
#   - Binary built with: cargo build --release
#   - Release must already exist (created by CI)

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

if [ -z "$1" ]; then
    echo -e "${RED}Usage: $0 <tag> [arch]${NC}"
    echo "Example: $0 v2.21.1"
    echo "Example: $0 v2.21.1 arm64"
    echo ""
    echo "arch: amd64 (Intel) or arm64 (Apple Silicon)"
    echo "Default: auto-detect"
    exit 1
fi

TAG="$1"
REPO="hagan/claudia-statusline"
BINARY="target/release/statusline"

# Detect or use specified architecture
if [ -n "$2" ]; then
    ARCH="$2"
else
    # Auto-detect architecture
    MACHINE=$(uname -m)
    if [ "$MACHINE" = "x86_64" ]; then
        ARCH="amd64"
    elif [ "$MACHINE" = "arm64" ] || [ "$MACHINE" = "aarch64" ]; then
        ARCH="arm64"
    else
        echo -e "${RED}Error: Unknown architecture: $MACHINE${NC}"
        echo "Specify arch manually: $0 $TAG amd64|arm64"
        exit 1
    fi
fi

ARTIFACT_NAME="statusline-darwin-${ARCH}"

# Check if binary exists
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}Error: Binary not found at $BINARY${NC}"
    echo "Build first with: cargo build --release"
    exit 1
fi

# Verify it's a macOS binary
if ! file "$BINARY" | grep -q "Mach-O"; then
    echo -e "${RED}Error: $BINARY is not a macOS binary${NC}"
    file "$BINARY"
    exit 1
fi

# Check if gh CLI is installed
if ! command -v gh >/dev/null 2>&1; then
    echo -e "${RED}Error: GitHub CLI (gh) not installed${NC}"
    echo "Install with: brew install gh"
    exit 1
fi

# Check authentication
if ! gh auth status >/dev/null 2>&1; then
    echo -e "${RED}Error: Not authenticated with GitHub${NC}"
    echo "Run: gh auth login"
    exit 1
fi

# Check if release exists
if ! gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
    echo -e "${RED}Error: Release $TAG does not exist${NC}"
    echo "Create the release first (usually by pushing the tag)"
    exit 1
fi

# Create tar.gz archive
echo -e "${BLUE}Creating archive for darwin-${ARCH}...${NC}"
ARCHIVE="${ARTIFACT_NAME}.tar.gz"
rm -f "$ARCHIVE" "${ARCHIVE}.sha256"

# Create archive with just the binary
tar czf "$ARCHIVE" -C "$(dirname "$BINARY")" "$(basename "$BINARY")"

# Get binary info
SIZE=$(ls -lh "$ARCHIVE" | awk '{print $5}')
SHA256=$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)

# Create checksum file
echo "$SHA256  $ARCHIVE" > "${ARCHIVE}.sha256"

echo -e "${GREEN}Archive created:${NC}"
echo "  File: $ARCHIVE"
echo "  Size: $SIZE"
echo "  SHA256: $SHA256"
echo "  Architecture: darwin-${ARCH}"
echo ""

# Upload to release
echo -e "${BLUE}Uploading to release $TAG...${NC}"
gh release upload "$TAG" "$ARCHIVE" "${ARCHIVE}.sha256" --repo "$REPO" --clobber

echo ""
echo -e "${GREEN}Upload complete!${NC}"
echo ""
echo -e "${YELLOW}Add this to release notes:${NC}"
if [ "$ARCH" = "arm64" ]; then
    echo "| macOS Apple Silicon | \`$ARTIFACT_NAME.tar.gz\` | \`$SHA256\` |"
else
    echo "| macOS Intel | \`$ARTIFACT_NAME.tar.gz\` | \`$SHA256\` |"
fi
