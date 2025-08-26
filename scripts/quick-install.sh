#!/bin/bash
#
# Claudia Statusline Quick Installer
# Downloads and installs the latest pre-built binary
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
#   wget -qO- https://raw.githubusercontent.com/hagan/claudia-statusline/main/scripts/quick-install.sh | bash
#

set -e

# Configuration
REPO="hagan/claudia-statusline"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
CLAUDE_CONFIG_DIR="$HOME/.claude"

# Colors
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    NC=''
fi

# Helper functions
info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

error() {
    echo -e "${RED}[✗]${NC} $1" >&2
    exit 1
}

# Detect OS and architecture
detect_platform() {
    local os arch

    # Detect OS
    case "$(uname -s)" in
        Linux*)  os="linux" ;;
        Darwin*) os="darwin" ;;
        CYGWIN*|MINGW*|MSYS*) os="windows" ;;
        *)       error "Unsupported operating system: $(uname -s)" ;;
    esac

    # Detect architecture
    case "$(uname -m)" in
        x86_64|amd64)       arch="amd64" ;;
        aarch64|arm64)      arch="arm64" ;;
        armv7l|armv7|arm)   arch="arm" ;;
        *)                  error "Unsupported architecture: $(uname -m)" ;;
    esac

    # Windows uses different naming
    if [ "$os" = "windows" ]; then
        echo "windows-amd64"
    else
        echo "${os}-${arch}"
    fi
}

# Get latest release version
get_latest_version() {
    curl -s "https://api.github.com/repos/${REPO}/releases/latest" | \
        grep '"tag_name":' | \
        sed -E 's/.*"([^"]+)".*/\1/'
}

# Download and install binary
install_binary() {
    local platform="$1"
    local version="$2"
    local temp_dir

    temp_dir=$(mktemp -d)
    trap "rm -rf $temp_dir" EXIT

    info "Downloading Claudia Statusline ${version} for ${platform}..."

    local asset_name ext
    if [[ "$platform" == "windows-"* ]]; then
        asset_name="statusline-${platform}.zip"
        ext="zip"
    else
        asset_name="statusline-${platform}.tar.gz"
        ext="tar.gz"
    fi

    local download_url="https://github.com/${REPO}/releases/download/${version}/${asset_name}"

    # Download the archive
    if ! curl -fsSL "$download_url" -o "$temp_dir/statusline.${ext}"; then
        error "Failed to download binary from ${download_url}"
    fi

    # Extract the binary
    info "Extracting binary..."
    cd "$temp_dir"
    if [ "$ext" = "zip" ]; then
        unzip -q "statusline.${ext}"
    else
        tar -xzf "statusline.${ext}"
    fi

    # Create install directory if it doesn't exist
    mkdir -p "$INSTALL_DIR"

    # Install the binary
    info "Installing to ${INSTALL_DIR}/statusline..."
    if [ -f "$INSTALL_DIR/statusline" ] && [ ! -w "$INSTALL_DIR/statusline" ]; then
        warning "Existing binary is not writable. You may need sudo privileges."
        sudo mv statusline "$INSTALL_DIR/statusline"
        sudo chmod +x "$INSTALL_DIR/statusline"
    else
        mv statusline "$INSTALL_DIR/statusline"
        chmod +x "$INSTALL_DIR/statusline"
    fi

    success "Binary installed successfully!"
}

# Configure Claude Code
configure_claude() {
    info "Configuring Claude Code..."

    # Check if Claude config directory exists
    if [ ! -d "$CLAUDE_CONFIG_DIR" ]; then
        warning "Claude config directory not found at $CLAUDE_CONFIG_DIR"
        warning "Please configure Claude Code manually after it's installed"
        return
    fi

    # Determine which config file to use
    local config_file
    if [ -f "$CLAUDE_CONFIG_DIR/settings.local.json" ]; then
        config_file="$CLAUDE_CONFIG_DIR/settings.local.json"
        info "Using settings.local.json"
    elif [ -f "$CLAUDE_CONFIG_DIR/settings.json" ]; then
        config_file="$CLAUDE_CONFIG_DIR/settings.json"
        info "Using settings.json"
    else
        # Create new settings.json
        config_file="$CLAUDE_CONFIG_DIR/settings.json"
        echo '{}' > "$config_file"
        info "Created new settings.json"
    fi

    # Check if statusLine is already configured
    if grep -q '"statusLine"' "$config_file"; then
        warning "statusLine already configured in Claude Code"
        info "Current configuration:"
        grep -A3 '"statusLine"' "$config_file" | head -5
    else
        # Add statusLine configuration
        info "Adding statusLine configuration..."
        local temp_config
        temp_config=$(mktemp)

        jq '. + {
            "statusLine": {
                "type": "command",
                "command": "'"$INSTALL_DIR/statusline"'",
                "padding": 0
            }
        }' "$config_file" > "$temp_config" && mv "$temp_config" "$config_file"

        success "Claude Code configured successfully!"
    fi
}

# Check if binary is in PATH
check_path() {
    if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
        warning "$INSTALL_DIR is not in your PATH"
        info "Add this to your shell configuration file:"
        echo "    export PATH=\"\$PATH:$INSTALL_DIR\""
    fi
}

# Main installation flow
main() {
    echo -e "${BLUE}═══════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}     Claudia Statusline Quick Installer${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════${NC}"
    echo

    # Detect platform
    info "Detecting platform..."
    PLATFORM=$(detect_platform)
    success "Platform: ${PLATFORM}"

    # Get latest version
    info "Getting latest version..."
    VERSION=$(get_latest_version)
    if [ -z "$VERSION" ]; then
        error "Failed to get latest version"
    fi
    success "Latest version: ${VERSION}"

    # Install binary
    install_binary "$PLATFORM" "$VERSION"

    # Configure Claude Code
    configure_claude

    # Check PATH
    check_path

    # Test the installation
    info "Testing installation..."
    if "$INSTALL_DIR/statusline" --version >/dev/null 2>&1; then
        success "Installation test passed!"
        echo
        echo -e "${GREEN}✨ Claudia Statusline ${VERSION} installed successfully!${NC}"
        echo
        echo "To test manually:"
        echo "  echo '{\"workspace\":{\"current_dir\":\"'$HOME'\"}}' | $INSTALL_DIR/statusline"
        echo
        if [ ! -d "$CLAUDE_CONFIG_DIR" ]; then
            echo "Note: Claude Code configuration was skipped (Claude not found)."
            echo "You can configure it manually later."
        fi
    else
        error "Installation test failed. Please check the installation."
    fi
}

# Handle errors
trap 'error "Installation failed. Please try again or install manually."' ERR

# Run main installation
main "$@"