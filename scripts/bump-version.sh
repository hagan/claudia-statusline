#!/bin/bash

# Version bump script for Claudia Statusline
# Usage: ./scripts/bump-version.sh [major|minor|patch]

set -euo pipefail

# Check if bump type was provided
if [ $# -eq 0 ]; then
    echo "Usage: $0 [major|minor|patch]"
    echo "  major: Bumps X.0.0 (breaking changes)"
    echo "  minor: Bumps 0.X.0 (new features)"
    echo "  patch: Bumps 0.0.X (bug fixes)"
    exit 1
fi

BUMP_TYPE=$1

# Validate bump type
if [[ "$BUMP_TYPE" != "major" && "$BUMP_TYPE" != "minor" && "$BUMP_TYPE" != "patch" ]]; then
    echo "Error: Invalid bump type. Must be 'major', 'minor', or 'patch'"
    exit 1
fi

# Get current version from VERSION file
if [ ! -f VERSION ]; then
    echo "Error: VERSION file not found"
    exit 1
fi

CURRENT_VERSION=$(cat VERSION)
echo "Current version: $CURRENT_VERSION"

# Parse version components
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# Bump the appropriate component
case "$BUMP_TYPE" in
    major)
        NEW_MAJOR=$((MAJOR + 1))
        NEW_MINOR=0
        NEW_PATCH=0
        ;;
    minor)
        NEW_MAJOR=$MAJOR
        NEW_MINOR=$((MINOR + 1))
        NEW_PATCH=0
        ;;
    patch)
        NEW_MAJOR=$MAJOR
        NEW_MINOR=$MINOR
        NEW_PATCH=$((PATCH + 1))
        ;;
esac

NEW_VERSION="$NEW_MAJOR.$NEW_MINOR.$NEW_PATCH"
echo "New version: $NEW_VERSION"

# Function to update version in a file
update_version() {
    local file=$1
    local pattern=$2
    local replacement=$3

    if [ -f "$file" ]; then
        # Use platform-specific sed
        if [[ "$OSTYPE" == "darwin"* ]]; then
            sed -i '' "$pattern" "$file"
        else
            sed -i "$pattern" "$file"
        fi
        echo "  ✓ Updated $file"
    else
        echo "  ⚠ Warning: $file not found"
    fi
}

echo "Updating version in files..."

# Update VERSION file
echo "$NEW_VERSION" > VERSION
echo "  ✓ Updated VERSION"

# Update Cargo.toml (only the package version, not dependencies)
if [ -f "Cargo.toml" ]; then
    # Portable first-match replacement using awk (works on GNU/BSD)
    awk -v new_ver="$NEW_VERSION" '
        BEGIN { done=0 }
        {
          if (!done && $0 ~ /^version = ".*"$/) {
            print "version = \"" new_ver "\"";
            done=1;
          } else {
            print $0;
          }
        }
    ' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
    echo "  ✓ Updated Cargo.toml"
else
    echo "  ⚠ Warning: Cargo.toml not found"
fi

# Update integration test
update_version "tests/integration_tests.rs" \
    "s/assert!(stdout.contains(\"$CURRENT_VERSION\"))/assert!(stdout.contains(\"$NEW_VERSION\"))/" \
    "assert!(stdout.contains(\"$NEW_VERSION\"))"

# Update CLAUDE.md
update_version "CLAUDE.md" \
    "s/\*\*Current Version\*\*: $CURRENT_VERSION/**Current Version**: $NEW_VERSION/" \
    "**Current Version**: $NEW_VERSION"

# Update README.md version badge (if present)
update_version "README.md" \
    "s/version-$CURRENT_VERSION/version-$NEW_VERSION/g" \
    "version-$NEW_VERSION"

# Update README.md latest version references
update_version "README.md" \
    "s/Latest: v$CURRENT_VERSION/Latest: v$NEW_VERSION/g" \
    "Latest: v$NEW_VERSION"

# Note: embedding_example.rs now reads version from VERSION file directly via include_str!

# Run cargo build to update Cargo.lock
echo ""
echo "Updating Cargo.lock..."
cargo build --quiet 2>/dev/null || true
echo "  ✓ Updated Cargo.lock"

echo ""
echo "Version bumped from $CURRENT_VERSION to $NEW_VERSION"
echo ""
echo "Next steps:"
echo "1. Update CHANGELOG.md with release notes for v$NEW_VERSION"
echo "2. Run 'make test' to ensure all tests pass"
echo "3. Commit changes: git commit -am \"Bump version to v$NEW_VERSION\""
echo "4. Tag release: git tag -a v$NEW_VERSION -m \"Release v$NEW_VERSION\""
echo "5. Push changes: git push && git push --tags"
