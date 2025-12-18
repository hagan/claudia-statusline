#!/bin/bash
# Upload Windows binary to GitHub release
# Usage: ./scripts/upload-windows-release.sh v2.21.1
#
# This script uploads a locally-compiled Windows binary to an existing GitHub release.
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
    echo -e "${RED}Usage: $0 <tag>${NC}"
    echo "Example: $0 v2.21.1"
    exit 1
fi

TAG="$1"
REPO="hagan/claudia-statusline"
BINARY="target/release/statusline.exe"
ARTIFACT_NAME="statusline-windows-amd64"

# Check if binary exists
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}Error: Binary not found at $BINARY${NC}"
    echo "Build first with: cargo build --release"
    exit 1
fi

# Check if gh CLI is installed
if ! command -v gh >/dev/null 2>&1; then
    echo -e "${RED}Error: GitHub CLI (gh) not installed${NC}"
    echo "Install from: https://cli.github.com/"
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

# Create zip archive
echo -e "${BLUE}Creating archive...${NC}"
ARCHIVE="${ARTIFACT_NAME}.zip"
rm -f "$ARCHIVE" "${ARCHIVE}.sha256"

# Use PowerShell on Windows for zip creation
if command -v powershell.exe >/dev/null 2>&1; then
    powershell.exe -Command "Compress-Archive -Path '$BINARY' -DestinationPath '$ARCHIVE' -Force"
else
    # Fallback to zip command
    zip -j "$ARCHIVE" "$BINARY"
fi

# Get binary info
SIZE=$(ls -lh "$ARCHIVE" | awk '{print $5}')
SHA256=$(sha256sum "$ARCHIVE" | cut -d' ' -f1)

# Create checksum file
echo "$SHA256  $ARCHIVE" > "${ARCHIVE}.sha256"

echo -e "${GREEN}Archive created:${NC}"
echo "  File: $ARCHIVE"
echo "  Size: $SIZE"
echo "  SHA256: $SHA256"
echo ""

# Upload to release
echo -e "${BLUE}Uploading to release $TAG...${NC}"
gh release upload "$TAG" "$ARCHIVE" "${ARCHIVE}.sha256" --repo "$REPO" --clobber

echo ""
echo -e "${GREEN}Upload complete!${NC}"
echo ""
echo -e "${YELLOW}Add this to release notes:${NC}"
echo "| Windows x86_64 | \`$ARTIFACT_NAME.zip\` | \`$SHA256\` |"
