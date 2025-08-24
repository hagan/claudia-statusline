#!/bin/bash

# Claude Statusline Uninstall Script
# This script safely removes the custom statusline for Claude Code without losing user settings

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}Claude Statusline Uninstaller${NC}"
echo "=============================="
echo ""

# Claude Code configuration location
CLAUDE_CONFIG_DIR="$HOME/.claude"
CLAUDE_CONFIG_FILE="$CLAUDE_CONFIG_DIR/settings.json"

# Check current settings and show what will be changed
if [ -f "$CLAUDE_CONFIG_FILE" ] && grep -q '"statusLine"' "$CLAUDE_CONFIG_FILE"; then
    echo -e "${YELLOW}Current statusLine configuration in settings.json:${NC}"
    jq '.statusLine' "$CLAUDE_CONFIG_FILE" 2>/dev/null | sed 's/^/  /'
    echo ""
fi

# Show what will be removed
echo -e "${YELLOW}This will remove:${NC}"
echo "  - Statusline configuration from Claude settings"
echo "  - Statusline binary and wrapper scripts"
echo "  - Optionally: debug logs"
echo ""
echo -e "${GREEN}This will preserve:${NC}"
echo "  - All other Claude Code settings"
echo "  - A backup of your current settings"
echo ""

# Ask for confirmation
read -p "Do you want to proceed with uninstallation? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo -e "${BLUE}Uninstallation cancelled.${NC}"
    exit 0
fi
echo ""

# Ask about settings.json modification
modify_settings=true
if [ -f "$CLAUDE_CONFIG_FILE" ] && grep -q '"statusLine"' "$CLAUDE_CONFIG_FILE"; then
    echo -e "${YELLOW}How would you like to handle settings.json?${NC}"
    echo "  1) Remove statusLine configuration automatically (recommended)"
    echo "  2) Skip - I'll edit settings.json manually"
    echo "  3) Cancel uninstallation"
    read -p "Choice [1-3]: " -n 1 -r
    echo
    case $REPLY in
        2)
            modify_settings=false
            echo -e "${BLUE}Skipping settings.json modification${NC}"
            echo -e "${YELLOW}Remember to manually remove the statusLine section from:${NC}"
            echo "  $CLAUDE_CONFIG_FILE"
            echo ""
            ;;
        3)
            echo -e "${BLUE}Uninstallation cancelled.${NC}"
            exit 0
            ;;
        *)
            modify_settings=true
            ;;
    esac
fi

# Function to check if a command exists
command_exists() {
    command -v "$1" &> /dev/null
}

# Step 1: Check if jq is available (required for safe JSON manipulation)
if ! command_exists jq; then
    echo -e "${RED}Error: jq is required for safe uninstallation.${NC}"
    echo -e "${BLUE}  Ubuntu/Debian: sudo apt-get install jq${NC}"
    echo -e "${BLUE}  Mac: brew install jq${NC}"
    echo -e "${BLUE}  Other: https://stedolan.github.io/jq/download/${NC}"
    echo ""
    echo -e "${YELLOW}Manual uninstall instructions:${NC}"
    echo "1. Edit $CLAUDE_CONFIG_FILE"
    echo "2. Remove the 'statusLine' section"
    echo "3. Delete ~/.local/bin/statusline*"
    exit 1
fi

# Step 2: Remove statusLine configuration from settings.json
if [ "$modify_settings" = true ]; then
    echo -e "${YELLOW}Step 1: Updating Claude Code settings...${NC}"

    if [ -f "$CLAUDE_CONFIG_FILE" ]; then
        # Check if statusLine is configured
        if grep -q '"statusLine"' "$CLAUDE_CONFIG_FILE"; then
            # Create backup before modifying
            backup_file="$CLAUDE_CONFIG_FILE.uninstall-backup-$(date +%Y%m%d-%H%M%S)"
            cp "$CLAUDE_CONFIG_FILE" "$backup_file"
            echo -e "${BLUE}  Created backup: $backup_file${NC}"

            # Remove statusLine configuration while preserving all other settings
            jq 'del(.statusLine)' "$CLAUDE_CONFIG_FILE" > "$CLAUDE_CONFIG_FILE.tmp"

            # Check if the resulting file has any content besides empty braces
            if [ "$(jq -r 'keys | length' "$CLAUDE_CONFIG_FILE.tmp")" -eq 0 ]; then
                echo -e "${BLUE}  Settings file would be empty after removing statusLine${NC}"
                echo -e "${YELLOW}  Keeping empty settings.json to preserve Claude's config directory${NC}"
                echo '{}' > "$CLAUDE_CONFIG_FILE.tmp"
            fi

            # Replace the original file
            mv "$CLAUDE_CONFIG_FILE.tmp" "$CLAUDE_CONFIG_FILE"
            echo -e "${GREEN}✓ Removed statusLine configuration from settings${NC}"
            echo -e "${BLUE}  Your other Claude settings have been preserved${NC}"
        else
            echo -e "${BLUE}  No statusLine configuration found in settings${NC}"
        fi
    else
        echo -e "${BLUE}  No Claude settings file found${NC}"
    fi
else
    echo -e "${YELLOW}Step 1: Skipping settings.json modification (manual mode)${NC}"
fi

# Step 3: Remove installed files
echo -e "${YELLOW}Step 2: Removing installed files...${NC}"

# Track what was removed
removed_files=()

# Remove statusline binary
if [ -f "$HOME/.local/bin/statusline" ]; then
    rm -f "$HOME/.local/bin/statusline"
    removed_files+=("statusline binary")
    echo -e "${GREEN}✓ Removed statusline binary${NC}"
fi

# Remove wrapper scripts
if [ -f "$HOME/.local/bin/statusline-wrapper.sh" ]; then
    rm -f "$HOME/.local/bin/statusline-wrapper.sh"
    removed_files+=("wrapper script")
    echo -e "${GREEN}✓ Removed wrapper script${NC}"
fi

if [ -f "$HOME/.local/bin/statusline-wrapper-debug.sh" ]; then
    rm -f "$HOME/.local/bin/statusline-wrapper-debug.sh"
    removed_files+=("debug wrapper")
    echo -e "${GREEN}✓ Removed debug wrapper${NC}"
fi

# Step 4: Clean up debug logs if they exist
echo -e "${YELLOW}Step 3: Cleaning up debug logs...${NC}"

if [ -f "$HOME/.cache/statusline-debug.log" ]; then
    echo -e "${BLUE}  Found debug log file${NC}"
    read -p "  Do you want to remove the debug log? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        rm -f "$HOME/.cache/statusline-debug.log"
        echo -e "${GREEN}✓ Removed debug log${NC}"
    else
        echo -e "${BLUE}  Kept debug log at: ~/.cache/statusline-debug.log${NC}"
    fi
else
    echo -e "${BLUE}  No debug logs found${NC}"
fi

# Step 5: Summary
echo ""
echo -e "${GREEN}Uninstallation complete!${NC}"
echo ""

if [ ${#removed_files[@]} -gt 0 ]; then
    echo -e "${BLUE}Removed components:${NC}"
    for item in "${removed_files[@]}"; do
        echo "  - $item"
    done
    echo ""
fi

echo -e "${BLUE}Preserved:${NC}"
echo "  - Your Claude settings (minus statusLine configuration)"
echo "  - Backup of original settings at: ${backup_file:-No backup needed}"
echo ""

echo -e "${YELLOW}Note:${NC}"
echo "  Restart Claude Code for changes to take effect"
echo ""

# Optional: Ask about removing the project directory
if [ -f "Makefile" ] && [ -f "statusline.patch" ]; then
    echo -e "${BLUE}You can also remove this project directory if no longer needed:${NC}"
    echo "  rm -rf $(pwd)"
    echo ""
fi

echo -e "${GREEN}Thank you for trying Claude Statusline!${NC}"