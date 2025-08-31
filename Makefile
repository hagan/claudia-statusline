# Claudia Statusline Makefile
# Build configuration for the Claudia Statusline

# Configuration
BINARY_NAME = statusline
SOURCES = src/main.rs src/models.rs src/git.rs src/stats.rs src/display.rs src/utils.rs src/version.rs
TARGET_DIR = target
INSTALL_DIR = $(HOME)/.local/bin
CARGO_TARGET = release

# Version information
VERSION := $(shell cat VERSION 2>/dev/null || echo "0.0.0")
GIT_HASH := $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")
GIT_TAG := $(shell git describe --tags --always 2>/dev/null || echo "v$(VERSION)")
GIT_DIRTY := $(shell git status --porcelain 2>/dev/null | grep -q . && echo "-dirty" || echo "")

# Build tool settings
CARGO = cargo
RUSTC = rustc
RUSTFLAGS = -C opt-level=3 -C target-cpu=native -C lto=fat -C codegen-units=1

# Colors for output
RED = \033[0;31m
GREEN = \033[0;32m
YELLOW = \033[1;33m
BLUE = \033[0;34m
NC = \033[0m # No Color

# Default target
.PHONY: all
all: build

# Help target
.PHONY: help
help:
	@echo "$(BLUE)Claudia Statusline Build System$(NC)"
	@echo ""
	@echo "$(GREEN)Available targets:$(NC)"
	@echo "  $(YELLOW)make$(NC)              - Build the release binary"
	@echo "  $(YELLOW)make build$(NC)        - Build the release binary"
	@echo "  $(YELLOW)make debug$(NC)        - Build debug binary with symbols"
	@echo "  $(YELLOW)make release$(NC)      - Build optimized release binary"
	@echo "  $(YELLOW)make install$(NC)      - Build and install to ~/.local/bin"
	@echo "  $(YELLOW)make uninstall$(NC)    - Remove installed binary"
	@echo "  $(YELLOW)make clean$(NC)        - Remove build artifacts"
	@echo "  $(YELLOW)make clean-whitespace$(NC) - Remove trailing whitespace from files"
	@echo "  $(YELLOW)make test$(NC)         - Run unit and integration tests"
	@echo "  $(YELLOW)make test-sqlite$(NC)  - Run SQLite integration tests"
	@echo "  $(YELLOW)make test-install$(NC) - Run installation verification"
	@echo "  $(YELLOW)make test-all$(NC)     - Run all tests"
	@echo "  $(YELLOW)make check$(NC)        - Check build environment"
	@echo "  $(YELLOW)make check-code$(NC)   - Run rustfmt and clippy"
	@echo "  $(YELLOW)make dev$(NC)          - Build and run with test input"
	@echo "  $(YELLOW)make bench$(NC)        - Run performance benchmark"
	@echo "  $(YELLOW)make version$(NC)      - Show version information"
	@echo "  $(YELLOW)make bump-major$(NC)   - Bump major version (X.0.0)"
	@echo "  $(YELLOW)make bump-minor$(NC)   - Bump minor version (0.X.0)"
	@echo "  $(YELLOW)make bump-patch$(NC)   - Bump patch version (0.0.X)"
	@echo "  $(YELLOW)make tag$(NC)          - Create git tag for release"
	@echo "  $(YELLOW)make release-build$(NC) - Build release with version tag"
	@echo ""
	@echo "$(GREEN)Installation paths:$(NC)"
	@echo "  Binary: $(INSTALL_DIR)/$(BINARY_NAME)"
	@echo ""
	@echo "$(GREEN)Build modes:$(NC)"
	@echo "  Release: Optimized for performance"
	@echo "  Debug:   Includes debugging symbols"

# Check build environment
.PHONY: check
check:
	@echo "$(BLUE)Checking build environment...$(NC)"
	@command -v rustc >/dev/null 2>&1 || { echo "$(RED)Error: rustc not found. Please install Rust.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) Rust compiler found: $$(rustc --version)"
	@command -v cargo >/dev/null 2>&1 || { echo "$(RED)Error: cargo not found. Please install Rust with Cargo.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) Cargo found: $$(cargo --version)"
	@echo "$(GREEN)✓$(NC) Build environment ready"

# Create target directory
$(TARGET_DIR):
	@mkdir -p $(TARGET_DIR)

# Build release binary (default)
.PHONY: build
build: release

# Build optimized release binary
.PHONY: release
release: check
	@echo "$(BLUE)Building release binary v$(VERSION) ($(GIT_HASH)$(GIT_DIRTY))...$(NC)"
	@$(CARGO) build --release
	@echo "$(GREEN)✓$(NC) Release binary built: $(TARGET_DIR)/release/$(BINARY_NAME)"
	@ls -lh $(TARGET_DIR)/release/$(BINARY_NAME) | awk '{print "  Size: " $$5}'
	@echo "  Version: v$(VERSION) ($(GIT_HASH)$(GIT_DIRTY))"

# Build debug binary
.PHONY: debug
debug: check
	@echo "$(BLUE)Building debug binary...$(NC)"
	@$(CARGO) build
	@echo "$(GREEN)✓$(NC) Debug binary built: $(TARGET_DIR)/debug/$(BINARY_NAME)"
	@ls -lh $(TARGET_DIR)/debug/$(BINARY_NAME) | awk '{print "  Size: " $$5}'

# Install binary
.PHONY: install
install: release
	@echo "$(BLUE)Installing $(BINARY_NAME)...$(NC)"
	@mkdir -p $(INSTALL_DIR)
	@cp $(TARGET_DIR)/release/$(BINARY_NAME) $(INSTALL_DIR)/
	@chmod 755 $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "$(GREEN)✓$(NC) Installed to $(INSTALL_DIR)/$(BINARY_NAME)"
	@echo ""
	@echo "$(YELLOW)Make sure $(INSTALL_DIR) is in your PATH:$(NC)"
	@echo '  export PATH="$$HOME/.local/bin:$$PATH"'

# Uninstall binary
.PHONY: uninstall
uninstall:
	@echo "$(BLUE)Uninstalling $(BINARY_NAME)...$(NC)"
	@rm -f $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "$(GREEN)✓$(NC) Removed $(INSTALL_DIR)/$(BINARY_NAME)"

# Clean build artifacts
.PHONY: clean
clean:
	@echo "$(BLUE)Cleaning build artifacts...$(NC)"
	@$(CARGO) clean 2>/dev/null || true
	@rm -rf $(TARGET_DIR)
	@echo "$(GREEN)✓$(NC) Build artifacts removed"

# Clean trailing whitespace from all project files
.PHONY: clean-whitespace
clean-whitespace:
	@echo "$(BLUE)Removing trailing whitespace from project files...$(NC)"
	@find . -type f \( -name "*.md" -o -name "*.sh" -o -name "*.rs" -o -name "*.toml" -o -name "*.yml" -o -name "*.yaml" -o -name "Makefile" \) \
		-not -path "./target/*" -not -path "./.git/*" \
		-exec $(SED_INPLACE) 's/[[:space:]]*$$//' {} \; 2>/dev/null || true
	@echo "$(GREEN)✓$(NC) Trailing whitespace removed"

# Development build and test
.PHONY: dev
dev: debug
	@echo "$(BLUE)Running with test input...$(NC)"
	@echo '{"workspace":{"current_dir":"'$$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | $(TARGET_DIR)/debug/$(BINARY_NAME)
	@echo ""

# Run tests
.PHONY: test
test: debug
	@echo "$(BLUE)Running tests...$(NC)"
	@$(CARGO) test

# Run SQLite integration tests
.PHONY: test-sqlite
test-sqlite: debug
	@echo "$(BLUE)Running SQLite integration tests...$(NC)"
	@$(CARGO) test sqlite_integration

# Run installation test
.PHONY: test-install
test-install: install
	@echo "$(BLUE)Running installation tests...$(NC)"
	@./scripts/test-installation.sh

# Run all tests
.PHONY: test-all
test-all: test test-sqlite test-install
	@echo "$(GREEN)✓$(NC) All tests completed!"
	@echo ""
	@echo "Test 1: Empty input"
	@echo '{}' | $(TARGET_DIR)/debug/$(BINARY_NAME)
	@echo ""
	@echo "Test 2: Current directory"
	@echo '{"workspace":{"current_dir":"'$$(pwd)'"}}' | $(TARGET_DIR)/debug/$(BINARY_NAME)
	@echo ""
	@echo "Test 3: With model info"
	@echo '{"workspace":{"current_dir":"'$$(pwd)'"},"model":{"display_name":"Claude Opus 3"}}' | $(TARGET_DIR)/debug/$(BINARY_NAME)
	@echo ""
	@echo "Test 4: With cost tracking"
	@echo '{"session_id":"test-123","workspace":{"current_dir":"'$$(pwd)'"},"cost":{"total_cost_usd":5.50}}' | $(TARGET_DIR)/debug/$(BINARY_NAME)
	@echo ""
	@echo "$(GREEN)✓$(NC) Tests completed"

# Run benchmark
.PHONY: bench
bench: release
	@echo "$(BLUE)Running performance benchmark...$(NC)"
	@echo "Timing 1000 invocations..."
	@time for i in $$(seq 1 1000); do \
		echo '{"workspace":{"current_dir":"'$$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | $(TARGET_DIR)/release/$(BINARY_NAME) > /dev/null; \
	done
	@echo "$(GREEN)✓$(NC) Benchmark completed"

# Format code
.PHONY: fmt
fmt:
	@command -v rustfmt >/dev/null 2>&1 || { echo "$(RED)Error: rustfmt not found$(NC)" >&2; exit 1; }
	@echo "$(BLUE)Formatting code...$(NC)"
	@cargo fmt
	@echo "$(GREEN)✓$(NC) Code formatted"

# Lint check
.PHONY: lint
lint:
	@echo "$(BLUE)Running clippy linter...$(NC)"
	@cargo clippy --all-targets --all-features -- -D warnings
	@echo "$(GREEN)✓$(NC) Linting completed"

# Format + Lint preflight
.PHONY: check-code
check-code: fmt lint
	@echo "$(GREEN)✓$(NC) Code formatting and lint checks passed"

# Show binary size
.PHONY: size
size: debug release
	@echo "$(BLUE)Binary size comparison:$(NC)"
	@echo "  Debug:   $$(ls -lh $(TARGET_DIR)/debug/$(BINARY_NAME) 2>/dev/null | awk '{print $$5}')"
	@echo "  Release: $$(ls -lh $(TARGET_DIR)/release/$(BINARY_NAME) 2>/dev/null | awk '{print $$5}')"

# Show version information
.PHONY: version
version:
	@echo "$(BLUE)Version Information:$(NC)"
	@echo "  Version:    v$(VERSION)"
	@echo "  Git Hash:   $(GIT_HASH)$(GIT_DIRTY)"
	@echo "  Git Tag:    $(GIT_TAG)"
	@if [ -f $(TARGET_DIR)/release/$(BINARY_NAME) ]; then \
		echo ""; \
		echo "$(BLUE)Binary version:$(NC)"; \
		$(TARGET_DIR)/release/$(BINARY_NAME) --version 2>/dev/null || echo "  Binary not found or error"; \
	fi

# Bump major version (X.0.0)
.PHONY: bump-major
bump-major:
	@echo "$(BLUE)Bumping major version...$(NC)"
	@./scripts/bump-version.sh major
	@echo "$(GREEN)✓$(NC) Major version bumped"

# Bump minor version (0.X.0)
.PHONY: bump-minor
bump-minor:
	@echo "$(BLUE)Bumping minor version...$(NC)"
	@./scripts/bump-version.sh minor
	@echo "$(GREEN)✓$(NC) Minor version bumped"

# Bump patch version (0.0.X)
.PHONY: bump-patch
bump-patch:
	@echo "$(BLUE)Bumping patch version...$(NC)"
	@./scripts/bump-version.sh patch
	@echo "$(GREEN)✓$(NC) Patch version bumped"

# Create a git tag for release
.PHONY: tag
tag:
	@echo "$(BLUE)Creating git tag for v$(VERSION)...$(NC)"
	@if git rev-parse "v$(VERSION)" >/dev/null 2>&1; then \
		echo "$(RED)Error: Tag v$(VERSION) already exists$(NC)"; \
		echo "To retag, first delete with: git tag -d v$(VERSION)"; \
		exit 1; \
	fi
	@git tag -a "v$(VERSION)" -m "Release v$(VERSION)"
	@echo "$(GREEN)✓$(NC) Created tag v$(VERSION)"
	@echo "  To push tags: git push origin v$(VERSION)"
	@echo "  To push all tags: git push --tags"

# Build a release with proper version tagging
.PHONY: release-build
release-build: clean
	@echo "$(BLUE)Building release v$(VERSION)...$(NC)"
	@if [ -n "$(GIT_DIRTY)" ]; then \
		echo "$(YELLOW)Warning: Working directory has uncommitted changes$(NC)"; \
		echo "  Consider committing changes before release build"; \
		echo ""; \
	fi
	@$(CARGO) build --release
	@echo "$(GREEN)✓$(NC) Release build complete"
	@echo ""
	@$(TARGET_DIR)/release/$(BINARY_NAME) --version
	@echo ""
	@echo "$(GREEN)Release binary:$(NC) $(TARGET_DIR)/release/$(BINARY_NAME)"
	@ls -lh $(TARGET_DIR)/release/$(BINARY_NAME) | awk '{print "  Size: " $$5}'
	@echo ""
	@echo "$(YELLOW)Next steps:$(NC)"
	@echo "  1. Test the binary: make test"
	@echo "  2. Create git tag: make tag"
	@echo "  3. Push to GitHub: git push --tags"
	@echo "  4. Create GitHub release with binary"

# Platform-specific sed command
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
    SED_INPLACE := sed -i ''
else
    SED_INPLACE := sed -i
endif


.DEFAULT_GOAL := help
