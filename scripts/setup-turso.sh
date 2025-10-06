#!/bin/bash
# Turso Database Setup Script for Claudia Statusline

set -e

# Colors
BLUE='\033[0;34m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${BLUE}Turso Database Setup for Claudia Statusline${NC}"
echo

# Check if turso CLI is installed
if ! command -v turso &> /dev/null; then
    echo -e "${RED}Error: turso CLI not found${NC}"
    echo "Please install it from: https://docs.turso.tech/cli/installation"
    echo
    echo "Quick install:"
    echo "  curl -sSfL https://get.tur.so/install.sh | bash"
    exit 1
fi

# Check for database URL argument
if [ -z "$1" ]; then
    echo -e "${RED}Error: Database URL required${NC}"
    echo
    echo "Usage: $0 <database-url>"
    echo "Example: $0 libsql://claude-statusline-hagan.aws-us-west-2.turso.io"
    exit 1
fi

DB_URL="$1"
DB_NAME=$(echo "$DB_URL" | sed 's|.*://||' | cut -d'.' -f1)

echo -e "${BLUE}Database:${NC} $DB_NAME"
echo -e "${BLUE}URL:${NC} $DB_URL"
echo

# Apply schema
echo -e "${BLUE}Applying database schema...${NC}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if turso db shell "$DB_NAME" < "$SCRIPT_DIR/setup-turso-schema.sql"; then
    echo -e "${GREEN}✓ Schema applied successfully${NC}"
else
    echo -e "${RED}✗ Failed to apply schema${NC}"
    exit 1
fi

echo

# Get auth token
echo -e "${BLUE}Getting auth token...${NC}"
AUTH_TOKEN=$(turso db tokens create "$DB_NAME")

if [ -z "$AUTH_TOKEN" ]; then
    echo -e "${RED}✗ Failed to create auth token${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Auth token created${NC}"
echo

# Show setup instructions
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}Setup Complete!${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo
echo -e "${YELLOW}Add this to your shell RC file (~/.bashrc, ~/.zshrc, etc.):${NC}"
echo
echo "export TURSO_AUTH_TOKEN=\"$AUTH_TOKEN\""
echo
echo -e "${YELLOW}Or add to your current session:${NC}"
echo
echo "export TURSO_AUTH_TOKEN=\"$AUTH_TOKEN\""
echo "source ~/.bashrc  # or ~/.zshrc"
echo
echo -e "${YELLOW}Configuration file location:${NC}"
echo "~/.config/claudia-statusline/config.toml"
echo
echo -e "${YELLOW}Test the connection:${NC}"
echo "statusline sync --status"
echo
echo -e "${YELLOW}Push your local stats:${NC}"
echo "statusline sync --push --dry-run  # Preview first"
echo "statusline sync --push             # Actually push"
echo
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
