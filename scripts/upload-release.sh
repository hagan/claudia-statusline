#!/bin/bash
# Unified release upload script
# Usage: ./scripts/upload-release.sh v2.21.1 [--turso]
#
# Automatically detects platform and architecture, builds, and uploads.
# Use --turso flag to build and upload turso-sync variant.
#
# This script:
#   1. Detects current platform (Windows/macOS/FreeBSD/Linux)
#   2. Builds release binary (with optional turso-sync feature)
#   3. Creates archive with checksum
#   4. Uploads to existing GitHub release
#
# Prerequisites:
#   - GitHub CLI (gh) installed and authenticated
#   - Rust toolchain installed
#   - Release must already exist (created by CI with Linux binaries)

set -e

# Colors (if terminal supports it)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    NC=''
fi

TURSO=false
TAG=""

# Parse arguments
for arg in "$@"; do
    case $arg in
        --turso)
            TURSO=true
            ;;
        v*)
            TAG="$arg"
            ;;
        *)
            echo -e "${RED}Unknown argument: $arg${NC}"
            exit 1
            ;;
    esac
done

if [ -z "$TAG" ]; then
    echo -e "${RED}Usage: $0 <tag> [--turso]${NC}"
    echo "Example: $0 v2.21.1"
    echo "Example: $0 v2.21.1 --turso"
    exit 1
fi

REPO="hagan/claudia-statusline"

# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
    darwin)
        PLATFORM="darwin"
        ;;
    freebsd)
        PLATFORM="freebsd"
        ;;
    linux)
        PLATFORM="linux"
        ;;
    mingw*|msys*|cygwin*)
        PLATFORM="windows"
        ;;
    *)
        echo -e "${RED}Error: Unsupported platform: $OS${NC}"
        exit 1
        ;;
esac

case "$ARCH" in
    x86_64|amd64)
        ARCH_NAME="amd64"
        ;;
    arm64|aarch64)
        ARCH_NAME="arm64"
        ;;
    *)
        echo -e "${RED}Error: Unsupported architecture: $ARCH${NC}"
        exit 1
        ;;
esac

# Build artifact name
if [ "$TURSO" = true ]; then
    ARTIFACT_NAME="statusline-turso-${PLATFORM}-${ARCH_NAME}"
    BUILD_FLAGS="--features turso-sync"
else
    ARTIFACT_NAME="statusline-${PLATFORM}-${ARCH_NAME}"
    BUILD_FLAGS=""
fi

echo -e "${BLUE}Platform: ${PLATFORM}-${ARCH_NAME}${NC}"
echo -e "${BLUE}Artifact: ${ARTIFACT_NAME}${NC}"
if [ "$TURSO" = true ]; then
    echo -e "${BLUE}Building with: turso-sync feature${NC}"
fi
echo ""

# Check if gh CLI is installed
if ! command -v gh >/dev/null 2>&1; then
    echo -e "${RED}Error: GitHub CLI (gh) not installed${NC}"
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

# Build
echo -e "${BLUE}Building release binary...${NC}"
cargo build --release $BUILD_FLAGS

# Determine binary path
if [ "$PLATFORM" = "windows" ]; then
    BINARY="target/release/statusline.exe"
    ARCHIVE="${ARTIFACT_NAME}.zip"
else
    BINARY="target/release/statusline"
    ARCHIVE="${ARTIFACT_NAME}.tar.gz"
fi

if [ ! -f "$BINARY" ]; then
    echo -e "${RED}Error: Binary not found at $BINARY${NC}"
    exit 1
fi

# Strip binary (except on Windows and ARM)
if [ "$PLATFORM" != "windows" ] && [ "$ARCH_NAME" != "arm64" ]; then
    echo -e "${BLUE}Stripping binary...${NC}"
    strip "$BINARY" 2>/dev/null || true
fi

# Create archive
echo -e "${BLUE}Creating archive...${NC}"
rm -f "$ARCHIVE" "${ARCHIVE}.sha256"

if [ "$PLATFORM" = "windows" ]; then
    if command -v powershell.exe >/dev/null 2>&1; then
        powershell.exe -Command "Compress-Archive -Path '$BINARY' -DestinationPath '$ARCHIVE' -Force"
    else
        zip -j "$ARCHIVE" "$BINARY"
    fi
else
    tar czf "$ARCHIVE" -C "$(dirname "$BINARY")" "$(basename "$BINARY")"
fi

# Calculate checksum
SIZE=$(ls -lh "$ARCHIVE" | awk '{print $5}')
if command -v sha256sum >/dev/null 2>&1; then
    SHA256=$(sha256sum "$ARCHIVE" | cut -d' ' -f1)
elif command -v shasum >/dev/null 2>&1; then
    SHA256=$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)
elif command -v sha256 >/dev/null 2>&1; then
    SHA256=$(sha256 -q "$ARCHIVE")
else
    echo -e "${RED}Error: No sha256 tool found${NC}"
    exit 1
fi

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
echo "| ${PLATFORM} ${ARCH_NAME} | \`${ARCHIVE}\` | \`$SHA256\` |"
