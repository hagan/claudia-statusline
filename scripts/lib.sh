# Claudia Statusline shared shell library
#
# This file is *sourced*, not executed (no shebang). It defines helper
# functions shared by install-statusline.sh and uninstall-statusline.sh.
#
# It relies on caller-set variables read at call time:
#   $TEST_MODE, $NO_COLOR, $DRY_RUN, $VERBOSE
# and on the color variables that setup_colors() itself defines:
#   $RED $GREEN $YELLOW $BLUE $NC
#
# Source this near the top of the caller (after `set -e` and option-default
# variable initialization), then call setup_colors() where the caller does.

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
