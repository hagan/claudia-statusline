# Claude Statusline Makefile
# Build and installation configuration for the statusline utility

# Configuration
BINARY_NAME = statusline
SOURCE = statusline.rs
TARGET_DIR = target
INSTALL_DIR = $(HOME)/.local/bin
CARGO_TARGET = release

# Original source URL and validation
GIST_URL = https://gist.githubusercontent.com/steipete/8396e512171d31e934f0013e5651691e/raw/214162cb78163db044c522e3c1cc630e6753edb3/statusline.rs
PATCH_FILE = statusline.patch
EXPECTED_HASH = 5f7851061abbd896c2d4956323fa85848df79242448019bbea7799111d3cebda

# Build tool settings
CARGO = cargo
RUSTC = rustc
RUSTFLAGS = -C opt-level=3 -C target-cpu=native -C lto=fat -C codegen-units=1
DEBUG_FLAGS = -g
RELEASE_FLAGS = -O

# Colors for output
RED = \033[0;31m
GREEN = \033[0;32m
YELLOW = \033[1;33m
BLUE = \033[0;34m
NC = \033[0m # No Color

# Default target
.PHONY: all
all: $(SOURCE) build

# Help target
.PHONY: help
help:
	@echo "$(BLUE)Claude Statusline Build System$(NC)"
	@echo ""
	@echo "$(GREEN)Available targets:$(NC)"
	@echo "  $(YELLOW)make$(NC)              - Fetch source, apply patches, and build"
	@echo "  $(YELLOW)make build$(NC)        - Build the release binary"
	@echo "  $(YELLOW)make fetch-source$(NC) - Download and patch source file"
	@echo "  $(YELLOW)make debug$(NC)        - Build debug binary with symbols"
	@echo "  $(YELLOW)make release$(NC)      - Build optimized release binary"
	@echo "  $(YELLOW)make install$(NC)      - Build and install to ~/.local/bin"
	@echo "  $(YELLOW)make uninstall$(NC)    - Remove installed binary"
	@echo "  $(YELLOW)make clean$(NC)        - Remove build artifacts and source"
	@echo "  $(YELLOW)make test$(NC)         - Run basic functionality tests"
	@echo "  $(YELLOW)make check$(NC)        - Check build tools and dependencies"
	@echo "  $(YELLOW)make dev$(NC)          - Build and run with test input"
	@echo "  $(YELLOW)make bench$(NC)        - Run performance benchmark"
	@echo "  $(YELLOW)make clean-whitespace$(NC) - Remove trailing whitespace from all project files"
	@echo "  $(YELLOW)make update-patch$(NC) - Generate new patch from current source"
	@echo ""
	@echo "$(GREEN)Installation paths:$(NC)"
	@echo "  Binary: $(INSTALL_DIR)/$(BINARY_NAME)"
	@echo ""
	@echo "$(GREEN)Build modes:$(NC)"
	@echo "  Release: Optimized for performance"
	@echo "  Debug:   Includes debugging symbols"

# Download and patch the original source
$(SOURCE): $(PATCH_FILE)
	@echo "$(BLUE)Fetching original statusline.rs from gist...$(NC)"
	@curl -s $(GIST_URL) -o $(SOURCE).tmp
	@echo "$(BLUE)Validating source integrity...$(NC)"
	@ACTUAL_HASH=$$(sha256sum $(SOURCE).tmp | cut -d' ' -f1); \
	if [ "$$ACTUAL_HASH" != "$(EXPECTED_HASH)" ]; then \
		echo "$(RED)Error: Hash mismatch!$(NC)"; \
		echo "$(RED)Expected: $(EXPECTED_HASH)$(NC)"; \
		echo "$(RED)Got:      $$ACTUAL_HASH$(NC)"; \
		echo "$(YELLOW)The original gist may have been updated. Please review and update the patch.$(NC)"; \
		rm -f $(SOURCE).tmp; \
		exit 1; \
	fi
	@echo "$(GREEN)✓$(NC) Source integrity verified"
	@mv $(SOURCE).tmp $(SOURCE)
	@echo "$(BLUE)Applying patches...$(NC)"
	@patch $(SOURCE) < $(PATCH_FILE)
	@echo "$(GREEN)✓$(NC) Source file ready: $(SOURCE)"

# Force download of fresh source
.PHONY: fetch-source
fetch-source:
	@rm -f $(SOURCE)
	@$(MAKE) $(SOURCE)

# Verify source integrity without building
.PHONY: verify-source
verify-source:
	@echo "$(BLUE)Fetching and verifying original source...$(NC)"
	@curl -s $(GIST_URL) -o .verify.tmp
	@ACTUAL_HASH=$$(sha256sum .verify.tmp | cut -d' ' -f1); \
	if [ "$$ACTUAL_HASH" = "$(EXPECTED_HASH)" ]; then \
		echo "$(GREEN)✓$(NC) Source integrity verified - hash matches"; \
		echo "  Expected: $(EXPECTED_HASH)"; \
		echo "  Got:      $$ACTUAL_HASH"; \
	else \
		echo "$(RED)✗$(NC) Source integrity check failed - hash mismatch!"; \
		echo "  Expected: $(EXPECTED_HASH)"; \
		echo "  Got:      $$ACTUAL_HASH"; \
		echo "$(YELLOW)The original gist may have been updated.$(NC)"; \
	fi
	@rm -f .verify.tmp


# Clean trailing whitespace from all project files
.PHONY: clean-whitespace
clean-whitespace:
	@echo "$(BLUE)Cleaning trailing whitespace from project files...$(NC)"
	@files_cleaned=0; \
	for file in *.md *.sh *.patch Makefile Cargo.toml .claude/context/*.md; do \
		if [ -f "$$file" ]; then \
			if grep -q '[[:space:]]$$' "$$file"; then \
				sed -i 's/[[:space:]]*$$//' "$$file"; \
				echo "$(GREEN)✓$(NC) Cleaned: $$file"; \
				files_cleaned=$$((files_cleaned + 1)); \
			fi \
		fi \
	done; \
	if [ "$$files_cleaned" -eq 0 ]; then \
		echo "$(GREEN)✓$(NC) All files are clean (no trailing whitespace)"; \
	else \
		echo "$(GREEN)✓$(NC) Cleaned $$files_cleaned file(s)"; \
	fi

# Alias for backward compatibility
.PHONY: clean-patch
clean-patch: clean-whitespace

# Generate new patch from current source
.PHONY: update-patch
update-patch: $(SOURCE)
	@echo "$(BLUE)Generating new patch from current source...$(NC)"
	@if [ ! -f statusline.rs.orig ]; then \
		echo "$(YELLOW)Fetching original for comparison...$(NC)"; \
		curl -s $(GIST_URL) -o statusline.rs.orig; \
	fi
	@diff -u statusline.rs.orig $(SOURCE) > statusline.patch.tmp || true
	@mv statusline.patch.tmp statusline.patch
	@$(MAKE) -s clean-patch
	@echo "$(GREEN)✓$(NC) Patch updated and cleaned"

# Check build environment
.PHONY: check
check:
	@echo "$(BLUE)Checking build environment...$(NC)"
	@command -v rustc >/dev/null 2>&1 || { echo "$(RED)Error: rustc not found. Please install Rust.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) Rust compiler found: $$(rustc --version)"
	@command -v cargo >/dev/null 2>&1 || { echo "$(RED)Error: cargo not found. Please install Rust with Cargo.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) Cargo found: $$(cargo --version)"
	@command -v curl >/dev/null 2>&1 || { echo "$(RED)Error: curl not found. Please install curl.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) curl found: $$(curl --version | head -1)"
	@command -v patch >/dev/null 2>&1 || { echo "$(RED)Error: patch not found. Please install patch.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) patch found"
	@command -v sha256sum >/dev/null 2>&1 || { echo "$(RED)Error: sha256sum not found. Please install coreutils.$(NC)" >&2; exit 1; }
	@echo "$(GREEN)✓$(NC) sha256sum found"
	@echo "$(GREEN)✓$(NC) Build environment ready"

# Create target directory
$(TARGET_DIR):
	@mkdir -p $(TARGET_DIR)

# Build release binary (default)
.PHONY: build
build: release

# Build optimized release binary
.PHONY: release
release: check $(SOURCE)
	@echo "$(BLUE)Building release binary...$(NC)"
	@$(CARGO) build --release
	@cp $(TARGET_DIR)/release/$(BINARY_NAME) $(TARGET_DIR)/$(BINARY_NAME)
	@echo "$(GREEN)✓$(NC) Release binary built: $(TARGET_DIR)/$(BINARY_NAME)"
	@ls -lh $(TARGET_DIR)/$(BINARY_NAME) | awk '{print "  Size: " $$5}'

# Build debug binary
.PHONY: debug
debug: check $(SOURCE)
	@echo "$(BLUE)Building debug binary...$(NC)"
	@$(CARGO) build
	@cp $(TARGET_DIR)/debug/$(BINARY_NAME) $(TARGET_DIR)/$(BINARY_NAME)_debug
	@echo "$(GREEN)✓$(NC) Debug binary built: $(TARGET_DIR)/$(BINARY_NAME)_debug"
	@ls -lh $(TARGET_DIR)/$(BINARY_NAME)_debug | awk '{print "  Size: " $$5}'

# Build with maximum optimizations
.PHONY: release-optimized
release-optimized: check $(SOURCE)
	@echo "$(BLUE)Building highly optimized release binary...$(NC)"
	@RUSTFLAGS="$(RUSTFLAGS)" $(CARGO) build --release
	@cp $(TARGET_DIR)/release/$(BINARY_NAME) $(TARGET_DIR)/$(BINARY_NAME)_opt
	@strip $(TARGET_DIR)/$(BINARY_NAME)_opt 2>/dev/null || true
	@echo "$(GREEN)✓$(NC) Optimized binary built: $(TARGET_DIR)/$(BINARY_NAME)_opt"
	@ls -lh $(TARGET_DIR)/$(BINARY_NAME)_opt | awk '{print "  Size: " $$5}'

# Install binary
.PHONY: install
install: release
	@echo "$(BLUE)Installing $(BINARY_NAME)...$(NC)"
	@mkdir -p $(INSTALL_DIR)
	@cp $(TARGET_DIR)/$(BINARY_NAME) $(INSTALL_DIR)/
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
	@rm -f $(BINARY_NAME)
	@rm -f $(SOURCE)
	@rm -f Cargo.lock
	@echo "$(GREEN)✓$(NC) Build artifacts removed"

# Development build and test
.PHONY: dev
dev: debug
	@echo "$(BLUE)Running with test input...$(NC)"
	@echo '{"workspace":{"current_dir":"'$$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | $(TARGET_DIR)/$(BINARY_NAME)_debug
	@echo ""

# Run basic tests
.PHONY: test
test: release
	@echo "$(BLUE)Running basic tests...$(NC)"
	@echo ""
	@echo "Test 1: Empty input"
	@echo '{}' | $(TARGET_DIR)/$(BINARY_NAME)
	@echo ""
	@echo "Test 2: Current directory"
	@echo '{"workspace":{"current_dir":"'$$(pwd)'"}}' | $(TARGET_DIR)/$(BINARY_NAME)
	@echo ""
	@echo "Test 3: With model info"
	@echo '{"workspace":{"current_dir":"'$$(pwd)'"},"model":{"display_name":"Claude Opus 3"}}' | $(TARGET_DIR)/$(BINARY_NAME)
	@echo ""
	@echo "$(GREEN)✓$(NC) Basic tests completed"

# Run benchmark
.PHONY: bench
bench: release
	@echo "$(BLUE)Running performance benchmark...$(NC)"
	@echo "Timing 1000 invocations..."
	@time for i in $$(seq 1 1000); do \
		echo '{"workspace":{"current_dir":"'$$(pwd)'"},"model":{"display_name":"Claude Sonnet"}}' | $(TARGET_DIR)/$(BINARY_NAME) > /dev/null; \
	done
	@echo "$(GREEN)✓$(NC) Benchmark completed"

# Create test transcript for context usage testing
.PHONY: test-transcript
test-transcript:
	@echo "$(BLUE)Creating test transcript...$(NC)"
	@mkdir -p test_data
	@echo '{"message":{"role":"user"},"timestamp":1700000000}' > test_data/transcript.jsonl
	@echo '{"message":{"role":"assistant","usage":{"input_tokens":50000,"output_tokens":10000,"cache_read_input_tokens":5000,"cache_creation_input_tokens":2000}},"timestamp":1700000100}' >> test_data/transcript.jsonl
	@echo "$(GREEN)✓$(NC) Test transcript created: test_data/transcript.jsonl"
	@echo "Test with: echo '{\"transcript_path\":\"test_data/transcript.jsonl\",\"model\":{\"display_name\":\"Claude Sonnet\"}}' | $(TARGET_DIR)/$(BINARY_NAME)"

# Watch for changes and rebuild (requires entr)
.PHONY: watch
watch:
	@command -v entr >/dev/null 2>&1 || { echo "$(RED)Error: entr not found. Install with: apt-get install entr$(NC)" >&2; exit 1; }
	@echo "$(BLUE)Watching for changes...$(NC)"
	@ls $(SOURCE) | entr -c make dev

# Format check (requires rustfmt)
.PHONY: fmt-check
fmt-check:
	@command -v rustfmt >/dev/null 2>&1 || { echo "$(YELLOW)Warning: rustfmt not found$(NC)" >&2; exit 0; }
	@echo "$(BLUE)Checking code formatting...$(NC)"
	@rustfmt --check $(SOURCE) || { echo "$(YELLOW)Code needs formatting. Run: make fmt$(NC)"; exit 1; }
	@echo "$(GREEN)✓$(NC) Code formatting is correct"

# Format code
.PHONY: fmt
fmt:
	@command -v rustfmt >/dev/null 2>&1 || { echo "$(RED)Error: rustfmt not found$(NC)" >&2; exit 1; }
	@echo "$(BLUE)Formatting code...$(NC)"
	@rustfmt $(SOURCE)
	@echo "$(GREEN)✓$(NC) Code formatted"

# Lint check (requires clippy)
.PHONY: lint
lint:
	@command -v cargo >/dev/null 2>&1 || { echo "$(YELLOW)Warning: cargo not found, skipping clippy$(NC)" >&2; exit 0; }
	@echo "$(BLUE)Running clippy linter...$(NC)"
	@cargo clippy --all-targets --all-features -- -D warnings 2>/dev/null || echo "$(YELLOW)Note: Project not set up with Cargo$(NC)"

# Show binary size comparison
.PHONY: size
size: debug release release-optimized
	@echo "$(BLUE)Binary size comparison:$(NC)"
	@echo "  Debug:      $$(ls -lh $(TARGET_DIR)/$(BINARY_NAME)_debug | awk '{print $$5}')"
	@echo "  Release:    $$(ls -lh $(TARGET_DIR)/$(BINARY_NAME) | awk '{print $$5}')"
	@echo "  Optimized:  $$(ls -lh $(TARGET_DIR)/$(BINARY_NAME)_opt | awk '{print $$5}')"

# Package for distribution
.PHONY: dist
dist: release-optimized
	@echo "$(BLUE)Creating distribution package...$(NC)"
	@mkdir -p dist
	@cp $(TARGET_DIR)/$(BINARY_NAME)_opt dist/$(BINARY_NAME)
	@cp README.md dist/ 2>/dev/null || echo "No README.md found"
	@# Copy documentation if needed
	@tar czf $(BINARY_NAME)-$$(date +%Y%m%d).tar.gz -C dist .
	@echo "$(GREEN)✓$(NC) Distribution package created: $(BINARY_NAME)-$$(date +%Y%m%d).tar.gz"
	@rm -rf dist

# Default fallback
.DEFAULT_GOAL := help