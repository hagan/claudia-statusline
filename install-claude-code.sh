#!/bin/bash

# Claude Code Statusline Installation Script
# This script sets up the custom statusline for Claude Code

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}Claude Code Statusline Installer${NC}"
echo "=================================="
echo ""

# Claude Code configuration location
# Note: Claude Code does NOT respect CLAUDE_HOME or XDG environment variables
# It always uses ~/.claude/settings.json
CLAUDE_CONFIG_DIR="$HOME/.claude"
CLAUDE_CONFIG_FILE="$CLAUDE_CONFIG_DIR/settings.json"
echo -e "${BLUE}Claude Code config location: $CLAUDE_CONFIG_DIR${NC}"

# Check if we're in the right directory
if [ ! -f "Makefile" ] || [ ! -f "statusline.patch" ]; then
    echo -e "${RED}Error: Required files not found. Please run this from the project directory.${NC}"
    exit 1
fi

# Step 1: Build the statusline binary
echo -e "${YELLOW}Step 1: Building statusline binary...${NC}"
if command -v cargo &> /dev/null; then
    export TMPDIR=/tmp
    make build
    echo -e "${GREEN}✓ Binary built successfully${NC}"
else
    echo -e "${RED}Error: Cargo not found. Please install Rust first.${NC}"
    exit 1
fi

# Step 2: Install binary to ~/.local/bin
echo -e "${YELLOW}Step 2: Installing binary...${NC}"
mkdir -p ~/.local/bin
cp target/statusline ~/.local/bin/
chmod 755 ~/.local/bin/statusline
echo -e "${GREEN}✓ Binary installed to ~/.local/bin/statusline${NC}"

# Step 3: Create config directory if it doesn't exist
echo -e "${YELLOW}Step 3: Setting up Claude configuration directory...${NC}"
mkdir -p "$CLAUDE_CONFIG_DIR"

# Step 4: Create wrapper script in ~/.local/bin
echo -e "${YELLOW}Step 4: Creating wrapper script...${NC}"
cat > ~/.local/bin/statusline-wrapper.sh << 'EOF'
#!/bin/bash
# Claude Code Statusline Wrapper
# This script passes Claude Code's JSON to the statusline binary

# Read JSON from stdin
json_input=$(cat)

# Claude Code already sends the correct format (snake_case), just ensure fields are present
# and handle optional cost data
converted_json=$(echo "$json_input" | jq '{
  session_id: .session_id,
  transcript_path: .transcript_path,
  workspace: {
    current_dir: (.workspace.current_dir // .cwd // null)
  },
  model: {
    display_name: .model.display_name
  },
  cost: (if .cost then {
    total_cost_usd: .cost.total_cost_usd,
    total_lines_added: .cost.total_lines_added,
    total_lines_removed: .cost.total_lines_removed
  } else null end)
}')

# Execute the statusline binary with the converted input
echo "$converted_json" | ~/.local/bin/statusline

# Exit with success
exit 0
EOF

chmod 755 ~/.local/bin/statusline-wrapper.sh
echo -e "${GREEN}✓ Wrapper script created at ~/.local/bin/statusline-wrapper.sh${NC}"

# Create debug wrapper for troubleshooting
cat > ~/.local/bin/statusline-wrapper-debug.sh << 'EOF'
#!/bin/bash
# Claude Code Statusline Debug Wrapper
# This script logs input and output for debugging

# Log file (user-specific and secure)
LOG_FILE="$HOME/.cache/statusline-debug.log"

# Ensure log directory exists and is secure
mkdir -p "$(dirname "$LOG_FILE")"
touch "$LOG_FILE"
chmod 600 "$LOG_FILE"  # Only user can read/write

# Read JSON from stdin
json_input=$(cat)

# Log the raw input
echo "[$(date)] Raw input:" >> "$LOG_FILE"
echo "$json_input" >> "$LOG_FILE"

# Convert to format expected by statusline binary
# Claude Code already sends correct format, just ensure fields are present
converted_json=$(echo "$json_input" | jq '{
  session_id: .session_id,
  transcript_path: .transcript_path,
  workspace: {
    current_dir: (.workspace.current_dir // .cwd // null)
  },
  model: {
    display_name: .model.display_name
  },
  cost: (if .cost then {
    total_cost_usd: .cost.total_cost_usd,
    total_lines_added: .cost.total_lines_added,
    total_lines_removed: .cost.total_lines_removed
  } else null end)
}' 2>> "$LOG_FILE")

# Log the converted JSON
echo "[$(date)] Converted JSON:" >> "$LOG_FILE"
echo "$converted_json" >> "$LOG_FILE"

# Detect theme - you can set CLAUDE_THEME=light to override
# Default to dark mode for better visibility
THEME="${CLAUDE_THEME:-dark}"

# Execute the statusline binary and capture output with theme
output=$(echo "$converted_json" | STATUSLINE_THEME="$THEME" ~/.local/bin/statusline 2>> "$LOG_FILE")

# Log the output
echo "[$(date)] Output:" >> "$LOG_FILE"
echo "$output" >> "$LOG_FILE"
echo "---" >> "$LOG_FILE"

# Output the result
echo "$output"

exit 0
EOF

chmod 755 ~/.local/bin/statusline-wrapper-debug.sh
echo -e "${BLUE}  Debug wrapper created at ~/.local/bin/statusline-wrapper-debug.sh${NC}"

# Step 5: Configure Claude Code settings
echo -e "${YELLOW}Step 5: Configuring Claude Code settings...${NC}"

# Check if jq is available (required for JSON manipulation)
if ! command -v jq &> /dev/null; then
    echo -e "${RED}Error: jq is required for JSON configuration. Please install jq first.${NC}"
    echo -e "${BLUE}  Ubuntu/Debian: sudo apt-get install jq${NC}"
    echo -e "${BLUE}  Mac: brew install jq${NC}"
    echo -e "${BLUE}  Other: https://stedolan.github.io/jq/download/${NC}"
    exit 1
fi

if [ -f "$CLAUDE_CONFIG_FILE" ]; then
    # Backup existing config
    cp "$CLAUDE_CONFIG_FILE" "$CLAUDE_CONFIG_FILE.backup"
    echo -e "${BLUE}  Backed up existing config to $CLAUDE_CONFIG_FILE.backup${NC}"

    # Check if statusLine is already configured
    if grep -q '"statusLine"' "$CLAUDE_CONFIG_FILE"; then
        echo -e "${YELLOW}  Warning: statusLine already configured in $CLAUDE_CONFIG_FILE${NC}"
        echo -e "${YELLOW}  Updating configuration...${NC}"
        # Update existing statusLine configuration
        jq '.statusLine = {"type": "command", "command": "~/.local/bin/statusline-wrapper.sh", "padding": 0}' "$CLAUDE_CONFIG_FILE" > "$CLAUDE_CONFIG_FILE.tmp"
    else
        # Add statusLine configuration
        echo -e "${BLUE}  Adding statusLine configuration...${NC}"
        jq '. + {"statusLine": {"type": "command", "command": "~/.local/bin/statusline-wrapper.sh", "padding": 0}}' "$CLAUDE_CONFIG_FILE" > "$CLAUDE_CONFIG_FILE.tmp"
    fi
    mv "$CLAUDE_CONFIG_FILE.tmp" "$CLAUDE_CONFIG_FILE"
    echo -e "${GREEN}✓ Configuration updated with statusLine settings${NC}"
else
    # Create new config file based on location
    if [ "$CLAUDE_CONFIG_DIR" = "${XDG_CONFIG_HOME:-$HOME/.config}/claude" ]; then
        # For XDG location, create minimal claude.json with just statusLine
        cat > "$CLAUDE_CONFIG_FILE" << 'EOF'
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/statusline-wrapper.sh",
    "padding": 0
  }
}
EOF
    else
        # For legacy location, create settings.json
        cat > "$CLAUDE_CONFIG_FILE" << 'EOF'
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/statusline-wrapper.sh",
    "padding": 0
  }
}
EOF
    fi
    echo -e "${GREEN}✓ Created new config with statusLine configuration${NC}"
fi

# Step 6: Add to PATH if needed
echo -e "${YELLOW}Step 6: Checking PATH...${NC}"
if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
    echo -e "${YELLOW}  ~/.local/bin is not in your PATH${NC}"
    echo -e "${BLUE}  Add this to your shell configuration:${NC}"
    echo '  export PATH="$HOME/.local/bin:$PATH"'
else
    echo -e "${GREEN}✓ ~/.local/bin is in PATH${NC}"
fi

echo ""
echo -e "${GREEN}Installation complete!${NC}"
echo ""
echo -e "${BLUE}Next steps:${NC}"
echo "1. ${RED}IMPORTANT: Restart Claude Code${NC} (the statusline won't appear until you restart)"
echo "2. Your custom statusline should now be active"
echo ""
echo -e "${BLUE}Configuration location:${NC} $CLAUDE_CONFIG_FILE"
echo ""
echo -e "${YELLOW}Troubleshooting:${NC}"
echo "If you only see '~' in the statusline:"
echo "1. Switch to debug mode to see what Claude is sending:"
echo "   jq '.statusLine.command = \"~/.local/bin/statusline-wrapper-debug.sh\"' $CLAUDE_CONFIG_FILE > /tmp/config.tmp && mv /tmp/config.tmp $CLAUDE_CONFIG_FILE"
echo "2. Restart Claude Code"
echo "3. Check the debug log: cat ~/.cache/statusline-debug.log"
echo "4. Report issues at: https://github.com/hagan/claude-statusline/issues"
echo ""
echo -e "${BLUE}To test manually:${NC}"
echo 'echo '"'"'{"workspace":{"current_dir":"/path/to/dir"},"model":{"display_name":"Claude Sonnet"}}'"'"' | ~/.local/bin/statusline-wrapper.sh'
echo ""
echo -e "${BLUE}To uninstall:${NC}"
echo "./uninstall-statusline.sh"