#!/bin/sh

# pinner installation script
# Supported OS: Linux, macOS
# Supported Arch: x86_64, aarch64

set -e

REPO="ffalcinelli/pinner"
GITHUB_URL="https://github.com/$REPO"

# Detect OS
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$OS_NAME" in
    linux*)  OS="linux" ;;
    darwin*) OS="macos" ;;
    *)
        echo "Error: Unsupported OS '$OS_NAME'"
        exit 1
        ;;
esac

# Detect Architecture
ARCH_NAME=$(uname -m)
case "$ARCH_NAME" in
    x86_64|amd64) ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)
        echo "Error: Unsupported architecture '$ARCH_NAME'"
        exit 1
        ;;
esac

ASSET_NAME="pinner-$OS-$ARCH"
EXTENSION="tar.gz"

# Determine install directory
if [ -d "$HOME/.cargo/bin" ]; then
    INSTALL_DIR="$HOME/.cargo/bin"
elif [ -d "$HOME/.local/bin" ]; then
    INSTALL_DIR="$HOME/.local/bin"
else
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
fi

echo "Installing pinner to $INSTALL_DIR..."

# Get latest release tag
LATEST_RELEASE=$(curl -sSf "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST_RELEASE" ]; then
    echo "Error: Could not determine latest release version."
    exit 1
fi

DOWNLOAD_URL="$GITHUB_URL/releases/download/$LATEST_RELEASE/$ASSET_NAME.$EXTENSION"

# Create a temporary directory
TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

echo "Downloading $DOWNLOAD_URL..."
curl -LsSf "$DOWNLOAD_URL" -o "$TEMP_DIR/pinner.$EXTENSION"

# Extract
tar -xzf "$TEMP_DIR/pinner.$EXTENSION" -C "$TEMP_DIR"

# Move to install directory
mv "$TEMP_DIR/pinner" "$INSTALL_DIR/pinner"
chmod +x "$INSTALL_DIR/pinner"

echo "pinner $LATEST_RELEASE installed successfully to $INSTALL_DIR"

# Check if INSTALL_DIR is in PATH
if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
    echo ""
    echo "Warning: $INSTALL_DIR is not in your PATH."
    case "$SHELL" in
        */zsh)  PROFILE="$HOME/.zshrc" ;;
        */bash) PROFILE="$HOME/.bashrc" ;;
        *)      PROFILE="$HOME/.profile" ;;
    esac
    echo "You can add it by running:"
    echo "  echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> $PROFILE"
    echo "  source $PROFILE"
fi
