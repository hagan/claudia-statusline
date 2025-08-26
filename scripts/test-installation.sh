#!/bin/bash

# Claudia Statusline Installation Test Script
# This script verifies that your statusline installation is working correctly

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test results
TESTS_PASSED=0
TESTS_FAILED=0

# Helper functions
log_test() {
    echo -e "${BLUE}[TEST]${NC} $1"
}

log_pass() {
    echo -e "${GREEN}[PASS]${NC} $1"
    ((TESTS_PASSED++))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $1"
    ((TESTS_FAILED++))
}

log_info() {
    echo -e "${YELLOW}[INFO]${NC} $1"
}

# Start testing
echo "========================================="
echo "Claudia Statusline Installation Test"
echo "========================================="
echo ""

# Test 1: Check if statusline binary exists and is executable
log_test "Checking if statusline binary is installed..."
if command -v statusline &> /dev/null; then
    STATUSLINE_PATH=$(which statusline)
    log_pass "statusline found at: $STATUSLINE_PATH"
else
    log_fail "statusline not found in PATH"
    log_info "Please ensure statusline is installed and in your PATH"
    exit 1
fi

# Test 2: Check version
log_test "Checking statusline version..."
if VERSION_OUTPUT=$(statusline --version 2>&1); then
    VERSION=$(echo "$VERSION_OUTPUT" | head -1)
    log_pass "Version: $VERSION"
else
    log_fail "Could not get version"
fi

# Test 3: Test help output
log_test "Testing help flag..."
if statusline --help &> /dev/null; then
    log_pass "Help flag works"
else
    log_fail "Help flag failed"
fi

# Test 4: Test basic functionality
log_test "Testing basic functionality..."
TEST_OUTPUT=$(echo '{"workspace":{"current_dir":"/tmp"}}' | statusline 2>&1)
if [ $? -eq 0 ]; then
    log_pass "Basic input processed successfully"
    log_info "Output: $TEST_OUTPUT"
else
    log_fail "Basic input processing failed"
fi

# Test 5: Test with model name
log_test "Testing with model name..."
TEST_OUTPUT=$(echo '{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude Sonnet"}}' | statusline 2>&1)
if [ $? -eq 0 ] && echo "$TEST_OUTPUT" | grep -q "Sonnet"; then
    log_pass "Model name displayed correctly"
else
    log_fail "Model name not displayed"
fi

# Test 6: Test with cost data
log_test "Testing with cost data..."
TEST_OUTPUT=$(echo '{"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":5.50}}' | statusline 2>&1)
if [ $? -eq 0 ] && echo "$TEST_OUTPUT" | grep -q "$"; then
    log_pass "Cost data processed"
else
    log_fail "Cost data not processed"
fi

# Test 7: Check stats directory
log_test "Checking stats directory..."
STATS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/claudia-statusline"
if [ -d "$STATS_DIR" ]; then
    log_pass "Stats directory exists: $STATS_DIR"

    # Check for stats files
    if [ -f "$STATS_DIR/stats.json" ]; then
        log_info "JSON stats file found"
    fi

    if [ -f "$STATS_DIR/stats.db" ]; then
        log_info "SQLite database found (v2.2.0+ feature)"
    fi
else
    log_info "Stats directory not yet created (will be created on first use with cost data)"
fi

# Test 8: Test SQLite functionality (if sqlite3 is available)
if command -v sqlite3 &> /dev/null && [ -f "$STATS_DIR/stats.db" ]; then
    log_test "Testing SQLite database..."

    # Check if we can query the database
    if SESSION_COUNT=$(sqlite3 "$STATS_DIR/stats.db" "SELECT COUNT(*) FROM sessions;" 2>/dev/null); then
        log_pass "SQLite database is valid, sessions: $SESSION_COUNT"
    else
        log_fail "Could not query SQLite database"
    fi
else
    log_info "SQLite test skipped (sqlite3 not installed or database not created yet)"
fi

# Test 9: Test with session ID to trigger stats update
log_test "Testing stats tracking..."
TEST_SESSION="test-$(date +%s)"
echo "{\"workspace\":{\"current_dir\":\"/tmp\"},\"session_id\":\"$TEST_SESSION\",\"cost\":{\"total_cost_usd\":0.01}}" | statusline &> /dev/null
if [ $? -eq 0 ]; then
    log_pass "Stats tracking works"

    # Verify the session was recorded
    if [ -f "$STATS_DIR/stats.json" ] && grep -q "$TEST_SESSION" "$STATS_DIR/stats.json" 2>/dev/null; then
        log_info "Session recorded in JSON stats"
    fi

    if [ -f "$STATS_DIR/stats.db" ] && command -v sqlite3 &> /dev/null; then
        if sqlite3 "$STATS_DIR/stats.db" "SELECT session_id FROM sessions WHERE session_id='$TEST_SESSION';" 2>/dev/null | grep -q "$TEST_SESSION"; then
            log_info "Session recorded in SQLite database"
        fi
    fi
else
    log_fail "Stats tracking failed"
fi

# Test 10: Check Claude Code configuration
log_test "Checking Claude Code configuration..."
CLAUDE_CONFIG="$HOME/.claude/settings.json"
CLAUDE_LOCAL_CONFIG="$HOME/.claude/settings.local.json"

if [ -f "$CLAUDE_LOCAL_CONFIG" ]; then
    if grep -q "statusline" "$CLAUDE_LOCAL_CONFIG" 2>/dev/null; then
        log_pass "Claude Code configured (settings.local.json)"
    else
        log_info "Claude Code settings.local.json exists but statusline not configured"
    fi
elif [ -f "$CLAUDE_CONFIG" ]; then
    if grep -q "statusline" "$CLAUDE_CONFIG" 2>/dev/null; then
        log_pass "Claude Code configured (settings.json)"
    else
        log_info "Claude Code settings.json exists but statusline not configured"
    fi
else
    log_info "Claude Code configuration not found (expected at ~/.claude/)"
fi

# Test 11: Performance test
log_test "Testing performance..."
START_TIME=$(date +%s%N)
echo '{"workspace":{"current_dir":"/tmp"}}' | statusline > /dev/null 2>&1
END_TIME=$(date +%s%N)
DURATION=$((($END_TIME - $START_TIME) / 1000000))

if [ $DURATION -lt 100 ]; then
    log_pass "Performance excellent: ${DURATION}ms"
elif [ $DURATION -lt 500 ]; then
    log_pass "Performance good: ${DURATION}ms"
else
    log_fail "Performance slow: ${DURATION}ms (expected <100ms)"
fi

# Summary
echo ""
echo "========================================="
echo "Test Summary"
echo "========================================="
echo -e "${GREEN}Passed:${NC} $TESTS_PASSED"
echo -e "${RED}Failed:${NC} $TESTS_FAILED"

if [ $TESTS_FAILED -eq 0 ]; then
    echo ""
    echo -e "${GREEN}✓ All tests passed! Your statusline installation is working correctly.${NC}"
    exit 0
else
    echo ""
    echo -e "${YELLOW}⚠ Some tests failed. Please check the output above for details.${NC}"
    exit 1
fi