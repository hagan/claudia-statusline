#!/bin/bash

# Claudia Statusline Uninstall Script
# This script safely removes the custom statusline for Claude Code without losing user settings

set -e

# Default values
INSTALL_DIR="$HOME/.local/bin"
CONFIG_DIR="$HOME/.claude"
CONFIG_FILE=""  # Will be determined later
DRY_RUN=false
FORCE=false
VERBOSE=false
TEST_MODE=false
SKIP_CONFIG=false
KEEP_LOGS=false
PREFIX=""

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

Uninstall Claudia Statusline with various options.

OPTIONS:
    -h, --help              Show this help message
    -d, --dry-run           Show what would be done without making changes
    -f, --force             Force uninstall without prompts
    -t, --test              Run in test mode (no colors, machine-readable output)
    -v, --verbose           Enable verbose output
    --prefix DIR            Uninstall from DIR instead of ~/.local/bin
    --config-dir DIR        Use DIR for config instead of ~/.claude
    --skip-config           Skip Claude Code configuration removal
    --keep-logs             Keep debug logs
    --no-color              Disable colored output

EXAMPLES:
    # Standard uninstallation
    $0

    # Dry run to see what would happen
    $0 --dry-run

    # Force uninstall without prompts
    $0 --force

    # Test mode for CI/CD
    $0 --test --prefix /tmp/test

    # Keep configuration intact
    $0 --skip-config

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
        -f|--force)
            FORCE=true
            shift
            ;;
        -t|--test)
            TEST_MODE=true
            FORCE=true  # Test mode implies force
            shift
            ;;
        -v|--verbose)
            VERBOSE=true
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
        --skip-config)
            SKIP_CONFIG=true
            shift
            ;;
        --keep-logs)
            KEEP_LOGS=true
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

# Check if a command exists
command_exists() {
    command -v "$1" &> /dev/null
}

# Detect the correct Claude config file
detect_config_file() {
    # If config file was explicitly set via command line, use that
    if [ -n "$CONFIG_FILE" ] && [ "$CONFIG_FILE" != "$CONFIG_DIR/settings.json" ]; then
        log_verbose "Using specified config file: $CONFIG_FILE"
        return 0
    fi

    # Check for existing config files in priority order
    # Note: settings.local.json takes precedence over settings.json
    if [ -f "$CONFIG_DIR/settings.local.json" ]; then
        CONFIG_FILE="$CONFIG_DIR/settings.local.json"
        log_verbose "Found settings.local.json (takes precedence)"
    elif [ -f "$CONFIG_DIR/settings.json" ]; then
        CONFIG_FILE="$CONFIG_DIR/settings.json"
        log_verbose "Found settings.json"
    else
        log_verbose "No Claude config file found"
        CONFIG_FILE="$CONFIG_DIR/settings.json"  # Default for messages
    fi

    if [ -f "$CONFIG_FILE" ]; then
        log "Using config file: $CONFIG_FILE"
    fi
}

# Show current configuration
show_current_config() {
    if [ "$SKIP_CONFIG" = true ] || [ ! -f "$CONFIG_FILE" ]; then
        return 0
    fi

    if grep -q '"statusLine"' "$CONFIG_FILE" 2>/dev/null; then
        if [ "$TEST_MODE" = false ]; then
            log "${YELLOW}Current statusLine configuration:${NC}"
            jq '.statusLine' "$CONFIG_FILE" 2>/dev/null | sed 's/^/  /'
            # Check if debug wrapper is being used
            if grep -q 'statusline-wrapper-debug.sh' "$CONFIG_FILE" 2>/dev/null; then
                log "${YELLOW}Note: Debug wrapper is currently active${NC}"
            fi
            echo ""
        else
            echo "[CONFIG] Current statusLine found in $CONFIG_FILE"
        fi
    fi
}

# Confirm uninstallation
confirm_uninstall() {
    if [ "$FORCE" = true ] || [ "$DRY_RUN" = true ]; then
        return 0
    fi

    log "${YELLOW}This will remove:${NC}"
    log "  - Statusline configuration from Claude settings"
    log "  - Statusline binary and any debug wrapper if present"
    [ "$KEEP_LOGS" = false ] && log "  - Debug logs"
    echo ""
    log "${GREEN}This will preserve:${NC}"
    log "  - All other Claude Code settings"
    log "  - A backup of your current settings"
    echo ""

    read -p "Do you want to proceed with uninstallation? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        log "${BLUE}Uninstallation cancelled.${NC}"
        exit 0
    fi
    echo ""
}

# Remove configuration
remove_config() {
    if [ "$SKIP_CONFIG" = true ]; then
        log_verbose "Skipping configuration removal (--skip-config)"
        return 0
    fi

    if [ ! -f "$CONFIG_FILE" ]; then
        log_verbose "No configuration file found"
        return 0
    fi

    log "${BLUE}Updating Claude Code settings...${NC}"

    if grep -q '"statusLine"' "$CONFIG_FILE"; then
        # Create backup
        if [ "$DRY_RUN" = false ]; then
            backup_file="$CONFIG_FILE.uninstall-backup-$(date +%Y%m%d-%H%M%S)"
            cp "$CONFIG_FILE" "$backup_file"
            # Verify backup was created successfully
            if [ -f "$backup_file" ] && [ -s "$backup_file" ]; then
                log_verbose "Created backup: $backup_file"
            else
                log_error "Failed to create backup"
                return 1
            fi
        fi

        # Remove statusLine configuration
        execute "jq 'del(.statusLine)' '$CONFIG_FILE' > '$CONFIG_FILE.tmp' && mv '$CONFIG_FILE.tmp' '$CONFIG_FILE'" \
                "Removed statusLine configuration from settings"

        # Check if file is now empty
        if [ "$DRY_RUN" = false ] && [ "$(jq -r 'keys | length' "$CONFIG_FILE")" -eq 0 ]; then
            echo '{}' > "$CONFIG_FILE"
            log_verbose "Kept empty settings.json to preserve config directory"
        fi
    else
        log_verbose "No statusLine configuration found in settings"
    fi
}

# Remove binaries and scripts
remove_binaries() {
    log "${BLUE}Removing installed files...${NC}"

    local removed_files=()

    # Remove statusline binary
    if [ -f "$INSTALL_DIR/statusline" ]; then
        execute "rm -f '$INSTALL_DIR/statusline'" "Removed statusline binary"
        removed_files+=("statusline binary")
    fi

    # Remove wrapper scripts
    if [ -f "$INSTALL_DIR/statusline-wrapper.sh" ]; then
        execute "rm -f '$INSTALL_DIR/statusline-wrapper.sh'" "Removed wrapper script"
        removed_files+=("wrapper script")
    fi

    if [ -f "$INSTALL_DIR/statusline-wrapper-debug.sh" ]; then
        execute "rm -f '$INSTALL_DIR/statusline-wrapper-debug.sh'" "Removed debug wrapper"
        removed_files+=("debug wrapper")
    fi

    # Remove test script if exists
    if [ -f "$INSTALL_DIR/statusline-test.sh" ]; then
        execute "rm -f '$INSTALL_DIR/statusline-test.sh'" ""
        removed_files+=("test script")
    fi

    if [ ${#removed_files[@]} -eq 0 ]; then
        log_verbose "No files found to remove"
    fi
}

# Clean up logs
cleanup_logs() {
    if [ "$KEEP_LOGS" = true ]; then
        log_verbose "Keeping debug logs (--keep-logs)"
        return 0
    fi

    log "${BLUE}Cleaning up debug logs...${NC}"

    if [ -f "$HOME/.cache/statusline-debug.log" ]; then
        if [ "$FORCE" = true ] || [ "$DRY_RUN" = true ]; then
            execute "rm -f '$HOME/.cache/statusline-debug.log'" "Removed debug log"
        else
            read -p "Do you want to remove the debug log? (y/N): " -n 1 -r
            echo
            if [[ $REPLY =~ ^[Yy]$ ]]; then
                execute "rm -f '$HOME/.cache/statusline-debug.log'" "Removed debug log"
            else
                log_verbose "Kept debug log at: ~/.cache/statusline-debug.log"
            fi
        fi
    else
        log_verbose "No debug logs found"
    fi
}

# Print summary
print_summary() {
    echo ""

    if [ "$DRY_RUN" = true ]; then
        log "${GREEN}Dry run complete!${NC}"
        log "No changes were made. Remove --dry-run to perform actual uninstallation."
    elif [ "$TEST_MODE" = true ]; then
        echo "[COMPLETE] Uninstallation finished successfully"
        echo "[REMOVED] $INSTALL_DIR/statusline"
        [ "$SKIP_CONFIG" = false ] && echo "[CONFIG] Removed statusLine from $CONFIG_FILE"
    else
        log "${GREEN}Uninstallation complete!${NC}"
        echo ""

        log "${BLUE}Preserved:${NC}"
        log "  - Your Claude settings (minus statusLine configuration)"
        [ -n "$backup_file" ] && log "  - Backup of original settings at: $backup_file"
        echo ""

        log "${YELLOW}Note:${NC}"
        log "  Restart Claude Code for changes to take effect"

        # Check if debug log exists and offer advice
        if [ -f "$HOME/.cache/statusline-debug.log" ] && [ "$KEEP_LOGS" = true ]; then
            echo ""
            log "${BLUE}Debug log preserved at:${NC}"
            log "  ~/.cache/statusline-debug.log"
        fi
        echo ""

        if [ -f "Makefile" ] && [ -f "statusline.patch" ]; then
            log "${BLUE}You can also remove this project directory if no longer needed:${NC}"
            log "  rm -rf $(pwd)"
            echo ""
        fi

        log "${GREEN}Thank you for trying Claudia Statusline!${NC}"
    fi
}

# Main uninstall flow
main() {
    if [ "$TEST_MODE" = false ]; then
        log "${BLUE}Claudia Statusline Uninstaller${NC}"
        log "=============================="
        echo ""
    fi

    # Validate paths if custom directories are specified
    if [ -n "$PREFIX" ]; then
        validate_path "$INSTALL_DIR" || exit 1
    fi
    if [ "$CONFIG_DIR" != "$HOME/.claude" ]; then
        validate_path "$CONFIG_DIR" || exit 1
    fi

    # Detect config file first
    detect_config_file

    # Check for jq if we need to modify config
    if [ "$SKIP_CONFIG" = false ] && ! command_exists jq; then
        log_error "jq is required for safe uninstallation."
        log "Install jq:"
        log "  Ubuntu/Debian: sudo apt-get install jq"
        log "  Mac: brew install jq"
        echo ""
        log "${YELLOW}Manual uninstall instructions:${NC}"
        log "1. Edit $CONFIG_FILE"
        log "2. Remove the 'statusLine' section"
        log "3. Delete $INSTALL_DIR/statusline*"
        exit 1
    fi

    show_current_config
    confirm_uninstall
    remove_config
    remove_binaries
    cleanup_logs
    print_summary
}

# Run main
main