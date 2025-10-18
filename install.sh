#!/bin/bash
# Installation script for daft - Git Extensions Toolkit
#
# This script installs the daft binary and creates symlinks for Git integration

set -e

# Configuration
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
INSTALL_DIR="${INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_PATH="$SCRIPT_DIR/target/release/daft"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Helper functions
info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
    exit 1
}

# Check if binary exists
if [ ! -f "$BINARY_PATH" ]; then
    error "Binary not found at $BINARY_PATH. Please run 'cargo build --release' first."
fi

# Create install directory if it doesn't exist
if [ ! -d "$INSTALL_DIR" ]; then
    info "Creating install directory: $INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
fi

# Install the main binary
info "Installing daft binary to $INSTALL_DIR/daft"
cp "$BINARY_PATH" "$INSTALL_DIR/daft"
chmod +x "$INSTALL_DIR/daft"

# Create symlinks for Git integration
info "Creating symlinks for Git integration..."

symlinks=(
    "git-worktree-clone"
    "git-worktree-init"
    "git-worktree-checkout"
    "git-worktree-checkout-branch"
    "git-worktree-checkout-branch-from-default"
    "git-worktree-prune"
    "git-daft"
)

for symlink in "${symlinks[@]}"; do
    target="$INSTALL_DIR/$symlink"
    if [ -L "$target" ] || [ -f "$target" ]; then
        warn "Removing existing $symlink"
        rm "$target"
    fi
    ln -s "$INSTALL_DIR/daft" "$target"
    info "Created symlink: $symlink -> daft"
done

# Check if install directory is in PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    warn ""
    warn "⚠️  $INSTALL_DIR is not in your PATH!"
    warn ""
    warn "Add this line to your ~/.bashrc, ~/.zshrc, or similar:"
    warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
    warn ""
fi

# Success message
info ""
info "✅ Installation complete!"
info ""
info "Installed:"
info "  - daft binary: $INSTALL_DIR/daft"
info "  - Git extensions: git worktree-clone, git worktree-checkout, etc."
info "  - Documentation: git daft"
info ""
info "Test the installation:"
info "  daft                         # Show documentation"
info "  git daft                     # Show documentation (via Git)"
info "  git worktree-clone --help    # Show clone command help"
info ""

# Binary size info
binary_size=$(stat -f "%z" "$BINARY_PATH" 2>/dev/null || stat -c "%s" "$BINARY_PATH" 2>/dev/null || echo "unknown")
if [ "$binary_size" != "unknown" ]; then
    binary_size_kb=$((binary_size / 1024))
    info "Binary size: ${binary_size_kb}KB"
fi
