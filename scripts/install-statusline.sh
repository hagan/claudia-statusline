#!/bin/bash

# Claudia Statusline Installation Script
# Supports both interactive installation and automated testing

set -e

# Default values
INSTALL_DIR="$HOME/.local/bin"
CONFIG_DIR="$HOME/.claude"
CONFIG_FILE=""  # Will be determined later
DRY_RUN=false
SKIP_BUILD=false
SKIP_CONFIG=false
SKIP_WRAPPER=false
VERBOSE=false
TEST_MODE=false
FORCE=false
PREFIX=""
WITH_DEBUG_LOGGING=false
WITH_STATS=false

# Colors for output (disabled in test mode)
setup_colors() {
    if [ "$TEST_MODE" = true ] || [ -n "$NO_COLOR" ]; then
        RED=''
        GREEN=''
        YELLOW=''
        BLUE=''
        NC=''
    else
        RED='\033[0;31m'
        GREEN='\033[0;32m'
        YELLOW='\033[1;33m'
        BLUE='\033[0;34m'
        NC='\033[0m' # No Color
    fi
}

# Usage function
usage() {
    cat << EOF
Usage: $0 [OPTIONS]

Install Claudia Statusline with various configuration options.

OPTIONS:
    -h, --help              Show this help message
    -d, --dry-run           Show what would be done without making changes
    -t, --test              Run in test mode (no colors, machine-readable output)
    -v, --verbose           Enable verbose output
    -f, --force             Force installation even if files exist
    --with-debug-logging    Install with debug logging enabled (logs all input/output)
    --with-stats             Enable persistent stats tracking across sessions
    --prefix DIR            Install to DIR instead of ~/.local/bin
    --config-dir DIR        Use DIR for config instead of ~/.claude
    --skip-build            Skip building the binary
    --skip-config           Skip Claude Code configuration
    --skip-wrapper          Skip wrapper script creation
    --no-color              Disable colored output

EXAMPLES:
    # Standard installation
    $0

    # Install with debug logging enabled for troubleshooting
    $0 --with-debug-logging

    # Dry run to see what would happen
    $0 --dry-run

    # Test mode for CI/CD
    $0 --test --prefix /tmp/test

    # Skip configuration (binary only)
    $0 --skip-config --skip-wrapper

    # Custom installation directory
    $0 --prefix /usr/local/bin

EOF
    exit 0
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            usage
            ;;
        -d|--dry-run)
            DRY_RUN=true
            shift
            ;;
        -t|--test)
            TEST_MODE=true
            shift
            ;;
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        -f|--force)
            FORCE=true
            shift
            ;;
        --with-debug-logging)
            WITH_DEBUG_LOGGING=true
            shift
            ;;
        --with-stats)
            WITH_STATS=true
            shift
            ;;
        --prefix)
            PREFIX="$2"
            INSTALL_DIR="$2"
            shift 2
            ;;
        --config-dir)
            CONFIG_DIR="$2"
            CONFIG_FILE=""  # Will be detected later
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --skip-config)
            SKIP_CONFIG=true
            shift
            ;;
        --skip-wrapper)
            SKIP_WRAPPER=true
            shift
            ;;
        --no-color)
            NO_COLOR=1
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Setup colors based on mode
setup_colors

# Logging functions
log() {
    if [ "$TEST_MODE" = true ]; then
        echo "[INFO] $1"
    else
        echo -e "$1"
    fi
}

log_verbose() {
    if [ "$VERBOSE" = true ]; then
        if [ "$TEST_MODE" = true ]; then
            echo "[DEBUG] $1"
        else
            echo -e "${BLUE}[DEBUG]${NC} $1"
        fi
    fi
}

log_success() {
    if [ "$TEST_MODE" = true ]; then
        echo "[SUCCESS] $1"
    else
        echo -e "${GREEN}✓${NC} $1"
    fi
}

log_error() {
    if [ "$TEST_MODE" = true ]; then
        echo "[ERROR] $1" >&2
    else
        echo -e "${RED}Error:${NC} $1" >&2
    fi
}

log_warning() {
    if [ "$TEST_MODE" = true ]; then
        echo "[WARNING] $1"
    else
        echo -e "${YELLOW}Warning:${NC} $1"
    fi
}

# Validate directory path for security
validate_path() {
    local path="$1"
    # Resolve to absolute path
    path=$(realpath "$path" 2>/dev/null) || {
        log_error "Invalid path: $1"
        return 1
    }
    # Check for suspicious patterns
    if [[ "$path" =~ \.\. ]] || [[ "$path" =~ ^/proc/ ]] || [[ "$path" =~ ^/sys/ ]]; then
        log_error "Suspicious path detected: $path"
        return 1
    fi
    return 0
}

# Execute command (respects dry-run)
execute() {
    local cmd="$1"
    local description="$2"

    if [ "$DRY_RUN" = true ]; then
        log "[DRY-RUN] Would execute: $cmd"
        return 0
    fi

    log_verbose "Executing: $cmd"

    if /bin/bash -c "$cmd"; then
        [ -n "$description" ] && log_success "$description"
        return 0
    else
        [ -n "$description" ] && log_error "Failed: $description"
        return 1
    fi
}

# Detect the correct Claude config file
detect_config_file() {
    # If config file was explicitly set via command line, use that
    if [ -n "$CONFIG_FILE" ] && [ "$CONFIG_FILE" != "$CONFIG_DIR/settings.json" ]; then
        log_verbose "Using specified config file: $CONFIG_FILE"
        return 0
    fi

    # Check for existing config files in priority order
    if [ -f "$CONFIG_DIR/settings.local.json" ]; then
        CONFIG_FILE="$CONFIG_DIR/settings.local.json"
        log_verbose "Found settings.local.json (takes precedence)"
    elif [ -f "$CONFIG_DIR/settings.json" ]; then
        CONFIG_FILE="$CONFIG_DIR/settings.json"
        log_verbose "Found settings.json"
    else
        # Default to settings.local.json for new installations
        CONFIG_FILE="$CONFIG_DIR/settings.local.json"
        log_verbose "No existing config found, will create settings.local.json"
    fi

    log "Using config file: $CONFIG_FILE"
}

# Check prerequisites
check_prerequisites() {
    log_verbose "Checking prerequisites..."

    # Detect config file first
    detect_config_file

    # Check if we're in the right directory
    if [ ! -f "Makefile" ] || [ ! -f "Cargo.toml" ] || [ ! -d "src" ]; then
        log_error "Required files not found. Please run this from the project directory."
        exit 1
    fi

    # Check for required tools
    if [ "$SKIP_BUILD" = false ]; then
        if ! command -v cargo &> /dev/null; then
            log_error "Cargo not found. Please install Rust first."
            log "Visit: https://www.rust-lang.org/tools/install"
            exit 1
        fi
    fi

    if [ "$SKIP_CONFIG" = false ] && ! command -v jq &> /dev/null; then
        log_warning "jq not found. Configuration update will be skipped."
        log "Install jq for automatic configuration:"
        log "  Ubuntu/Debian: sudo apt-get install jq"
        log "  Mac: brew install jq"
        SKIP_CONFIG=true
    fi

    log_verbose "Prerequisites check complete"
}

# Build the binary
build_binary() {
    if [ "$SKIP_BUILD" = true ]; then
        log_verbose "Skipping build (--skip-build)"
        return 0
    fi

    log "${BLUE}Building statusline binary...${NC}"

    if [ "$DRY_RUN" = true ]; then
        log "[DRY-RUN] Would build binary with: cargo build --release"
    else
        if [ "$VERBOSE" = true ]; then
            cargo build --release
        else
            cargo build --release > /dev/null 2>&1
        fi
        log_success "Binary built successfully"
    fi
}

# Install binary
install_binary() {
    log "${BLUE}Installing binary to $INSTALL_DIR...${NC}"

    execute "mkdir -p '$INSTALL_DIR'" ""

    if [ -f "$INSTALL_DIR/statusline" ] && [ "$FORCE" = false ] && [ "$DRY_RUN" = false ]; then
        log_warning "Binary already exists at $INSTALL_DIR/statusline"
        read -p "Overwrite? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            log "Skipping binary installation"
            return 0
        fi
    fi

    execute "cp target/release/statusline '$INSTALL_DIR/'" "Binary installed to $INSTALL_DIR/statusline"
    execute "chmod 755 '$INSTALL_DIR/statusline'" ""

    # Install cost tracking tools if requested
    if [ "$WITH_STATS" = true ]; then
        log "${BLUE}Installing cost tracking tools...${NC}"
        # Stats tracking is now integrated in the Rust binary
        execute "cp costs '$INSTALL_DIR/'" "Cost CLI installed"
        execute "chmod 755 '$INSTALL_DIR/costs'" ""
        # Stats tracking is integrated in the binary, no wrapper needed
    fi
}

# Create wrapper scripts
create_wrappers() {
    # Only create debug wrapper if debug logging is enabled
    if [ "$WITH_DEBUG_LOGGING" != true ]; then
        log_verbose "No wrapper needed - using binary directly"
        return 0
    fi

    log "${BLUE}Creating debug wrapper script...${NC}"

    if [ "$DRY_RUN" = true ]; then
        log "[DRY-RUN] Would create debug wrapper at: $INSTALL_DIR/statusline-wrapper-debug.sh"
    else
        create_debug_wrapper
    fi
}

# Create debug wrapper
create_debug_wrapper() {
    local debug_wrapper="$INSTALL_DIR/statusline-wrapper-debug.sh"

    cat > "$debug_wrapper" << 'EOF'
#!/bin/bash
# Claude Code Statusline Debug Wrapper
# Simple logging wrapper for debugging - passes JSON directly to binary

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

# Execute the statusline binary directly (it handles all JSON parsing)
output=$(echo "$json_input" | STATUSLINE_THEME="${STATUSLINE_THEME:-dark}" INSTALL_DIR_PLACEHOLDER/statusline 2>> "$LOG_FILE")

# Log the output
echo "[$(date)] Output:" >> "$LOG_FILE"
echo "$output" >> "$LOG_FILE"
echo "---" >> "$LOG_FILE"

# Output the result
echo "$output"

exit 0
EOF

    ESCAPED_DIR=$(printf '%s' "$INSTALL_DIR" | sed 's/[[\/.*^$()+?{|]/\\&/g')
    sed -i "s|INSTALL_DIR_PLACEHOLDER|$ESCAPED_DIR|g" "$debug_wrapper" || {
        log_error "Failed to update debug wrapper with install directory"
        return 1
    }
    chmod 755 "$debug_wrapper"
    log_verbose "Debug wrapper created at $debug_wrapper"
}

# Configure Claude Code
configure_claude() {
    if [ "$SKIP_CONFIG" = true ]; then
        log_verbose "Skipping Claude configuration (--skip-config)"
        return 0
    fi

    log "${BLUE}Configuring Claude Code settings...${NC}"

    execute "mkdir -p '$CONFIG_DIR'" ""

    if [ "$DRY_RUN" = true ]; then
        log "[DRY-RUN] Would update config at: $CONFIG_FILE"
        return 0
    fi

    if [ -f "$CONFIG_FILE" ]; then
        # Backup existing config
        cp "$CONFIG_FILE" "$CONFIG_FILE.backup"
        log_verbose "Backed up existing config to $CONFIG_FILE.backup"

        # Check if statusLine is already configured
        if grep -q '"statusLine"' "$CONFIG_FILE"; then
            log_warning "statusLine already configured in $CONFIG_FILE"
            if [ "$FORCE" = false ]; then
                read -p "Update configuration? (y/N): " -n 1 -r
                echo
                if [[ ! $REPLY =~ ^[Yy]$ ]]; then
                    log "Skipping configuration update"
                    return 0
                fi
            fi
            # Update existing
            # Use debug wrapper if debug logging is enabled, otherwise use binary directly
            if [ "$WITH_DEBUG_LOGGING" = true ]; then
                jq '.statusLine = {"type": "command", "command": "'$INSTALL_DIR'/statusline-wrapper-debug.sh", "padding": 0}' "$CONFIG_FILE" > "$CONFIG_FILE.tmp"
            else
                jq '.statusLine = {"type": "command", "command": "'$INSTALL_DIR'/statusline", "padding": 0}' "$CONFIG_FILE" > "$CONFIG_FILE.tmp"
            fi
        else
            # Add new
            # Use debug wrapper if debug logging is enabled, otherwise use binary directly
            if [ "$WITH_DEBUG_LOGGING" = true ]; then
                jq '. + {"statusLine": {"type": "command", "command": "'$INSTALL_DIR'/statusline-wrapper-debug.sh", "padding": 0}}' "$CONFIG_FILE" > "$CONFIG_FILE.tmp"
            else
                jq '. + {"statusLine": {"type": "command", "command": "'$INSTALL_DIR'/statusline", "padding": 0}}' "$CONFIG_FILE" > "$CONFIG_FILE.tmp"
            fi
        fi
        mv "$CONFIG_FILE.tmp" "$CONFIG_FILE"
    else
        # Create new config
        # No wrapper needed unless debug logging is enabled
        cat > "$CONFIG_FILE" << EOF
{
  "statusLine": {
    "type": "command",
    "command": "$INSTALL_DIR/$wrapper_script",
    "padding": 0
  }
}
EOF
    fi

    log_success "Configuration updated"
}

# Check PATH
check_path() {
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        log_warning "$INSTALL_DIR is not in your PATH"
        log "Add this to your shell configuration:"
        log "  export PATH=\"$INSTALL_DIR:\$PATH\""
    else
        log_verbose "$INSTALL_DIR is in PATH"
    fi
}

# Print summary
print_summary() {
    echo ""
    if [ "$DRY_RUN" = true ]; then
        log "${GREEN}Dry run complete!${NC}"
        log "No changes were made. Remove --dry-run to perform actual installation."
    elif [ "$TEST_MODE" = true ]; then
        echo "[COMPLETE] Installation finished successfully"
        echo "[INSTALL_DIR] $INSTALL_DIR"
        echo "[CONFIG_DIR] $CONFIG_DIR"
        echo "[BINARY] $INSTALL_DIR/statusline"
        echo "[BINARY] $INSTALL_DIR/statusline"
    else
        log "${GREEN}Installation complete!${NC}"
        echo ""
        log "${BLUE}Installed files:${NC}"
        log "  Binary: $INSTALL_DIR/statusline"
        if [ "$SKIP_WRAPPER" = false ]; then
            if [ "$WITH_DEBUG_LOGGING" = true ]; then
                log "  Debug wrapper: $INSTALL_DIR/statusline-wrapper-debug.sh (active)"
            else
                log "  Binary: $INSTALL_DIR/statusline (direct execution)"
            fi
        fi
        [ "$SKIP_CONFIG" = false ] && log "  Config: $CONFIG_FILE"
        echo ""
        log "${BLUE}Next steps:${NC}"
        log "1. ${RED}Restart Claude Code${NC} for changes to take effect"
        log "2. Test manually with:"
        log "   echo '{\"workspace\":{\"current_dir\":\"/tmp\"},\"model\":{\"display_name\":\"Claude Sonnet\"}}' | $INSTALL_DIR/statusline"
        if [ "$WITH_DEBUG_LOGGING" = true ]; then
            echo ""
            log "${YELLOW}Debug logging enabled:${NC}"
            log "  Logs will be written to: ~/.cache/statusline-debug.log"
            log "  To switch to normal mode, run: $0 --skip-build"
        else
            echo ""
            log "${BLUE}To enable debug logging:${NC}"
            log "  $0 --skip-build --with-debug-logging"
        fi

        if [ "$WITH_STATS" = true ]; then
            echo ""
            log "${GREEN}Cost tracking enabled:${NC}"
            log "  View costs: $INSTALL_DIR/costs"
            log "  Reset daily: $INSTALL_DIR/costs reset today"
            log "  Watch live: $INSTALL_DIR/costs watch"
        else
            echo ""
            log "${BLUE}To enable cost tracking:${NC}"
            log "  $0 --skip-build --with-stats"
        fi
        echo ""
        log "${BLUE}To uninstall:${NC}"
        log "  ./uninstall-statusline.sh"
    fi
}

# Main installation flow
main() {
    if [ "$TEST_MODE" = false ]; then
        log "${BLUE}Claudia Statusline Installer${NC}"
        log "============================"
        echo ""
    fi

    check_prerequisites
    build_binary
    install_binary
    create_wrappers
    configure_claude
    check_path
    print_summary
}

# Run main
main