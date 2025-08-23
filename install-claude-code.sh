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

# Detect Claude Code configuration location
# Claude Code uses XDG_CONFIG_HOME or ~/.config/claude on Linux/Mac
if [ -d "${XDG_CONFIG_HOME:-$HOME/.config}/claude" ]; then
    CLAUDE_CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/claude"
    CLAUDE_CONFIG_FILE="$CLAUDE_CONFIG_DIR/claude.json"
    echo -e "${BLUE}Detected XDG config location: $CLAUDE_CONFIG_DIR${NC}"
else
    # Fallback to legacy location
    CLAUDE_CONFIG_DIR="$HOME/.claude"
    CLAUDE_CONFIG_FILE="$CLAUDE_CONFIG_DIR/settings.json"
    echo -e "${BLUE}Using legacy config location: $CLAUDE_CONFIG_DIR${NC}"
fi

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
# This script adapts Claude Code's camelCase JSON to snake_case for the statusline binary

# Read JSON from stdin
json_input=$(cat)

# Convert camelCase to snake_case for the statusline binary
# Claude Code sends: currentDir, displayName
# Statusline expects: current_dir, display_name
converted_json=$(echo "$json_input" | jq '{
  workspace: {
    current_dir: .workspace.currentDir
  },
  model: {
    display_name: .model.displayName
  },
  session_id: .sessionId,
  transcript_path: .transcriptPath
}')

# Execute the statusline binary with the converted input
echo "$converted_json" | ~/.local/bin/statusline

# Exit with success
exit 0
EOF

chmod 755 ~/.local/bin/statusline-wrapper.sh
echo -e "${GREEN}✓ Wrapper script created at ~/.local/bin/statusline-wrapper.sh${NC}"

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
echo "1. Restart Claude Code or reload settings"
echo "2. Your custom statusline should now be active"
echo ""
echo -e "${BLUE}Configuration location:${NC} $CLAUDE_CONFIG_FILE"
echo ""
echo -e "${BLUE}To test manually:${NC}"
echo 'echo '"'"'{"workspace":{"currentDir":"/path/to/dir"},"model":{"displayName":"Claude Sonnet"}}'"'"' | ~/.local/bin/statusline-wrapper.sh'
echo ""
echo -e "${BLUE}To uninstall:${NC}"
echo "1. Remove statusLine from $CLAUDE_CONFIG_FILE"
echo "2. Delete ~/.local/bin/statusline-wrapper.sh"
echo "3. Delete ~/.local/bin/statusline"