#!/bin/bash
# Database maintenance wrapper for Claudia Statusline
# Can be run from cron or manually
# Exit codes: 0 = success, 1 = integrity check failed, 2 = other error

set -euo pipefail

# Configuration
STATUSLINE_BIN="${STATUSLINE_BIN:-statusline}"
LOG_FILE="${LOG_FILE:-}"
QUIET="${QUIET:-false}"

# Parse arguments
FORCE_VACUUM=false
NO_PRUNE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --force-vacuum)
            FORCE_VACUUM=true
            shift
            ;;
        --no-prune)
            NO_PRUNE=true
            shift
            ;;
        --quiet|-q)
            QUIET=true
            shift
            ;;
        --log)
            LOG_FILE="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Database maintenance for Claudia Statusline"
            echo ""
            echo "Options:"
            echo "  --force-vacuum    Force VACUUM even if not needed"
            echo "  --no-prune        Skip data retention pruning"
            echo "  --quiet, -q       Run in quiet mode (errors only)"
            echo "  --log FILE        Log output to FILE"
            echo "  --help, -h        Show this help message"
            echo ""
            echo "Exit codes:"
            echo "  0 - Success"
            echo "  1 - Integrity check failed"
            echo "  2 - Other error"
            echo ""
            echo "Environment variables:"
            echo "  STATUSLINE_BIN - Path to statusline binary (default: statusline)"
            echo "  LOG_FILE       - Log file path (can also use --log)"
            echo "  QUIET          - Run in quiet mode (can also use --quiet)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            echo "Use --help for usage information" >&2
            exit 2
            ;;
    esac
done

# Build command
CMD="$STATUSLINE_BIN db-maintain"

if [ "$FORCE_VACUUM" = true ]; then
    CMD="$CMD --force-vacuum"
fi

if [ "$NO_PRUNE" = true ]; then
    CMD="$CMD --no-prune"
fi

if [ "$QUIET" = true ]; then
    CMD="$CMD --quiet"
fi

# Function to run maintenance
run_maintenance() {
    if [ -n "$LOG_FILE" ]; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting database maintenance" >> "$LOG_FILE"
        if $CMD >> "$LOG_FILE" 2>&1; then
            echo "[$(date '+%Y-%m-%d %H:%M:%S')] Maintenance completed successfully" >> "$LOG_FILE"
            return 0
        else
            local exit_code=$?
            echo "[$(date '+%Y-%m-%d %H:%M:%S')] Maintenance failed with exit code $exit_code" >> "$LOG_FILE"
            return $exit_code
        fi
    else
        $CMD
    fi
}

# Run maintenance
if run_maintenance; then
    exit 0
else
    exit_code=$?
    if [ "$exit_code" -eq 1 ]; then
        # Integrity check failed
        if [ "$QUIET" != true ]; then
            echo "ERROR: Database integrity check failed!" >&2
            echo "Consider rebuilding from JSON backup." >&2
        fi
    fi
    exit $exit_code
fi