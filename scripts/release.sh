#!/bin/bash

# Claudia Statusline Release Script
# Automates the release process with version tagging and GitHub release

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Read current version
VERSION=$(cat VERSION 2>/dev/null || echo "0.0.0")

echo -e "${BLUE}Claudia Statusline Release Process${NC}"
echo -e "Current version: ${YELLOW}v${VERSION}${NC}"
echo ""

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    echo -e "${YELLOW}Warning: You have uncommitted changes${NC}"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${RED}Release cancelled${NC}"
        exit 1
    fi
fi

# Check if we're on main branch
BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$BRANCH" != "main" ] && [ "$BRANCH" != "master" ]; then
    echo -e "${YELLOW}Warning: You're on branch '${BRANCH}', not main/master${NC}"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${RED}Release cancelled${NC}"
        exit 1
    fi
fi

# Check if tag already exists
if git rev-parse "v${VERSION}" >/dev/null 2>&1; then
    echo -e "${RED}Error: Tag v${VERSION} already exists${NC}"
    echo "Options:"
    echo "  1. Delete the tag and recreate: git tag -d v${VERSION}"
    echo "  2. Bump version: make bump-patch (or bump-minor/bump-major)"
    exit 1
fi

# Build release
echo -e "${BLUE}Building release binary...${NC}"
make clean
make release-build

# Run tests
echo -e "${BLUE}Running tests...${NC}"
make test

# Get binary size
BINARY_SIZE=$(ls -lh target/release/statusline | awk '{print $5}')

# Create release notes
RELEASE_NOTES=$(mktemp)
cat > "$RELEASE_NOTES" << EOF
# Claudia Statusline v${VERSION}

## Release Information
- **Version**: v${VERSION}
- **Date**: $(date +"%Y-%m-%d")
- **Binary Size**: ${BINARY_SIZE}
- **Git Hash**: $(git rev-parse --short HEAD)

## What's New
[Add release notes here]

## Features
- Modular architecture with 6 focused modules
- Comprehensive test coverage (28 tests)
- XDG-compliant stats tracking
- Git integration with detailed status
- Context usage tracking with progress bars
- Cost tracking with burn rate display
- Theme support (dark/light)
- Version information (--version)

## Installation

### Quick Install
\`\`\`bash
git clone https://github.com/hagan/claudia-statusline
cd claudia-statusline
./scripts/install-statusline.sh
\`\`\`

### Manual Install
Download the binary from the release assets and place it in your PATH.

## Verification
\`\`\`bash
statusline --version
\`\`\`

## Changelog
See [README.md](https://github.com/hagan/claudia-statusline/blob/main/README.md#changelog) for full changelog.
EOF

echo -e "${GREEN}Release notes created at: ${RELEASE_NOTES}${NC}"
echo ""
echo -e "${BLUE}Review release notes:${NC}"
cat "$RELEASE_NOTES"
echo ""

read -p "Proceed with creating the release? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    rm "$RELEASE_NOTES"
    echo -e "${RED}Release cancelled${NC}"
    exit 1
fi

# Create git tag
echo -e "${BLUE}Creating git tag v${VERSION}...${NC}"
git tag -a "v${VERSION}" -m "Release v${VERSION}"
echo -e "${GREEN}✓ Tag created${NC}"

# Create tarball
echo -e "${BLUE}Creating release tarball...${NC}"
RELEASE_NAME="claudia-statusline-v${VERSION}-linux-x64"
RELEASE_DIR="/tmp/${RELEASE_NAME}"
mkdir -p "${RELEASE_DIR}"

cp target/release/statusline "${RELEASE_DIR}/"
cp README.md "${RELEASE_DIR}/"
cp LICENSE "${RELEASE_DIR}/"
cp NOTICE "${RELEASE_DIR}/"
cp scripts/install-statusline.sh "${RELEASE_DIR}/"

cd /tmp
tar czf "${RELEASE_NAME}.tar.gz" "${RELEASE_NAME}"
cd - > /dev/null

echo -e "${GREEN}✓ Release tarball created: /tmp/${RELEASE_NAME}.tar.gz${NC}"

# Push to GitHub
echo ""
echo -e "${YELLOW}Next steps:${NC}"
echo "1. Push the tag to GitHub:"
echo "   git push origin v${VERSION}"
echo ""
echo "2. Create GitHub release:"
echo "   - Go to: https://github.com/hagan/claudia-statusline/releases/new"
echo "   - Tag: v${VERSION}"
echo "   - Title: Claudia Statusline v${VERSION}"
echo "   - Upload binary: target/release/statusline"
echo "   - Upload tarball: /tmp/${RELEASE_NAME}.tar.gz"
echo "   - Paste release notes from: ${RELEASE_NOTES}"
echo ""
echo "3. Or use GitHub CLI:"
echo "   gh release create v${VERSION} \\"
echo "     --title \"Claudia Statusline v${VERSION}\" \\"
echo "     --notes-file \"${RELEASE_NOTES}\" \\"
echo "     target/release/statusline \\"
echo "     /tmp/${RELEASE_NAME}.tar.gz"
echo ""
echo -e "${GREEN}Release preparation complete!${NC}"