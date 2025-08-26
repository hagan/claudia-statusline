#!/bin/bash

# Claudia Statusline Debug Toggle Script
# Switches between direct binary execution and debug wrapper without reinstalling

set -e

# Default values
CONFIG_DIR="$HOME/.claude"
CONFIG_FILE=""  # Will be determined later
INSTALL_DIR="$HOME/.local/bin"
FORCE=false
VERBOSE=false
STATUS_ONLY=false

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Usage function
usage() {
    cat << EOF
Usage: $0 [OPTIONS]

Toggle between direct binary execution and debug mode for Claudia Statusline.

OPTIONS:
    -h, --help              Show this help message
    -s, --status            Show current mode only (don't toggle)
    -f, --force             Force toggle without prompts
    -v, --verbose           Enable verbose output
    --config-dir DIR        Use DIR for config instead of ~/.claude
    --install-dir DIR       Use DIR for binaries instead of ~/.local/bin

EXAMPLES:
    # Toggle debug mode
    $0

    # Check current status
    $0 --status

    # Force toggle without prompts
    $0 --force

EOF
    exit 0
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            usage
            ;;
        -s|--status)
            STATUS_ONLY=true
            shift
            ;;
        -f|--force)
            FORCE=true
            shift
            ;;
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        --config-dir)
            CONFIG_DIR="$2"
            shift 2
            ;;
        --install-dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Logging functions
log() {
    echo -e "$1"
}

log_verbose() {
    if [ "$VERBOSE" = true ]; then
        echo -e "${BLUE}[DEBUG]${NC} $1"
    fi
}

log_success() {
    echo -e "${GREEN}✓${NC} $1"
}

log_error() {
    echo -e "${RED}Error:${NC} $1" >&2
}

log_warning() {
    echo -e "${YELLOW}Warning:${NC} $1"
}

# Detect the correct Claude config file
detect_config_file() {
    if [ -f "$CONFIG_DIR/settings.local.json" ]; then
        CONFIG_FILE="$CONFIG_DIR/settings.local.json"
        log_verbose "Found settings.local.json (takes precedence)"
    elif [ -f "$CONFIG_DIR/settings.json" ]; then
        CONFIG_FILE="$CONFIG_DIR/settings.json"
        log_verbose "Found settings.json"
    else
        log_error "No Claude configuration file found"
        log "Please run the installer first: ./install-statusline.sh"
        exit 1
    fi

    log_verbose "Using config file: $CONFIG_FILE"
}

# Check current mode
check_current_mode() {
    if [ ! -f "$CONFIG_FILE" ]; then
        echo "not-installed"
        return
    fi

    if grep -q 'statusline-wrapper-debug.sh' "$CONFIG_FILE" 2>/dev/null; then
        echo "debug"
    elif grep -q '"statusline"' "$CONFIG_FILE" 2>/dev/null; then
        echo "normal"
    else
        echo "not-configured"
    fi
}

# Check if jq is available
check_prerequisites() {
    if ! command -v jq &> /dev/null; then
        log_error "jq is required for configuration updates"
        log "Install jq:"
        log "  Ubuntu/Debian: sudo apt-get install jq"
        log "  Mac: brew install jq"
        exit 1
    fi

    # Check if binary exists
    if [ ! -f "$INSTALL_DIR/statusline" ]; then
        log_error "Statusline binary not found at $INSTALL_DIR/statusline"
        log "Please run the installer first: ./install-statusline.sh"
        exit 1
    fi

    # Debug wrapper is optional, create it if needed
    if [ ! -f "$INSTALL_DIR/statusline-wrapper-debug.sh" ] && [ "$current_mode" != "debug" ]; then
        log "Creating debug wrapper..."
        ./install-statusline.sh --with-debug-logging --skip-config
    fi
}

# Toggle the mode
toggle_mode() {
    local current_mode="$1"
    local new_mode
    local new_wrapper

    if [ "$current_mode" = "debug" ]; then
        new_mode="normal"
        new_command="statusline"
    else
        new_mode="debug"
        new_command="statusline-wrapper-debug.sh"
    fi

    log "Switching from ${YELLOW}$current_mode${NC} mode to ${GREEN}$new_mode${NC} mode..."

    # Create backup
    backup_file="$CONFIG_FILE.toggle-backup-$(date +%Y%m%d-%H%M%S)"
    cp "$CONFIG_FILE" "$backup_file"
    log_verbose "Created backup: $backup_file"

    # Update configuration
    if jq '.statusLine.command = "'$INSTALL_DIR'/'$new_command'"' "$CONFIG_FILE" > "$CONFIG_FILE.tmp"; then
        mv "$CONFIG_FILE.tmp" "$CONFIG_FILE"
        log_success "Configuration updated"
    else
        log_error "Failed to update configuration"
        log "Restoring from backup..."
        cp "$backup_file" "$CONFIG_FILE"
        exit 1
    fi

    return 0
}

# Show status
show_status() {
    local mode="$1"

    case "$mode" in
        debug)
            log "${YELLOW}Current mode: DEBUG${NC}"
            log "  Debug logs are being written to: ~/.cache/statusline-debug.log"
            log "  To view logs: tail -f ~/.cache/statusline-debug.log"
            ;;
        normal)
            log "${GREEN}Current mode: NORMAL${NC}"
            log "  No debug logging is active"
            ;;
        not-configured)
            log_warning "Statusline is not configured in Claude settings"
            log "Run the installer: ./install-statusline.sh"
            ;;
        not-installed)
            log_error "Claude configuration not found"
            log "Run the installer first: ./install-statusline.sh"
            ;;
        *)
            log_error "Unknown mode: $mode"
            ;;
    esac
}

# Main flow
main() {
    log "${BLUE}Claudia Statusline Debug Toggle${NC}"
    log "=============================="
    echo ""

    # Detect config file
    detect_config_file

    # Check current mode
    current_mode=$(check_current_mode)

    # If status only, show and exit
    if [ "$STATUS_ONLY" = true ]; then
        show_status "$current_mode"
        exit 0
    fi

    # Check if we can proceed
    if [ "$current_mode" = "not-installed" ] || [ "$current_mode" = "not-configured" ]; then
        show_status "$current_mode"
        exit 1
    fi

    # Check prerequisites
    check_prerequisites

    # Show current status
    show_status "$current_mode"
    echo ""

    # Confirm toggle
    if [ "$FORCE" = false ]; then
        if [ "$current_mode" = "debug" ]; then
            log "This will ${GREEN}disable${NC} debug logging"
        else
            log "This will ${YELLOW}enable${NC} debug logging (logs written to ~/.cache/statusline-debug.log)"
        fi
        echo ""
        read -p "Do you want to proceed? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            log "Toggle cancelled"
            exit 0
        fi
        echo ""
    fi

    # Toggle the mode
    toggle_mode "$current_mode"

    # Show new status
    echo ""
    new_mode=$(check_current_mode)
    show_status "$new_mode"

    echo ""
    log "${YELLOW}Note:${NC} Restart Claude Code for changes to take effect"

    if [ "$new_mode" = "debug" ]; then
        echo ""
        log "${BLUE}Debug tips:${NC}"
        log "  • Watch logs in real-time: tail -f ~/.cache/statusline-debug.log"
        log "  • Clear old logs: > ~/.cache/statusline-debug.log"
        log "  • Check log size: ls -lh ~/.cache/statusline-debug.log"
    fi
}

# Run main
main