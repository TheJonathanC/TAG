#!/usr/bin/env bash
# ==============================================================================
# TAG Installer & Updater Script
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash -s -- --beta
# ==============================================================================

set -euo pipefail

REPO="TheJonathanC/TAG"
CHANNEL="${CHANNEL:-stable}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# Parse optional command line arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --beta)
      CHANNEL="beta"
      shift
      ;;
    --stable)
      CHANNEL="stable"
      shift
      ;;
    --version)
      TAG_VERSION="$2"
      shift 2
      ;;
    --dir)
      INSTALL_DIR="$2"
      shift 2
      ;;
    -h|--help)
      echo "TAG Installer"
      echo "Usage: install.sh [options]"
      echo "  --stable       Install latest stable release (default)"
      echo "  --beta         Install latest beta/pre-release"
      echo "  --version <v>  Install specific version tag (e.g. v0.1.0)"
      echo "  --dir <path>   Installation directory (default: ~/.local/bin)"
      exit 0
      ;;
    *)
      shift
      ;;
  esac
done

echo "==> Detecting operating system and architecture..."
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  linux)
    case "$ARCH" in
      x86_64|amd64)
        TARGET="x86_64-unknown-linux-musl"
        FALLBACK_TARGET="x86_64-unknown-linux-gnu"
        ;;
      aarch64|arm64)
        TARGET="aarch64-unknown-linux-gnu"
        FALLBACK_TARGET=""
        ;;
      *)
        echo "Error: Unsupported architecture for Linux: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  darwin)
    case "$ARCH" in
      arm64|aarch64)
        TARGET="aarch64-apple-darwin"
        FALLBACK_TARGET=""
        ;;
      x86_64|amd64)
        echo "Error: macOS Intel (x86_64) is not supported. TAG supports Apple Silicon (M1/M2/M3/M4, aarch64-apple-darwin)." >&2
        exit 1
        ;;
      *)
        echo "Error: Unsupported architecture for macOS: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Error: Unsupported operating system: $OS. TAG requires Linux or macOS." >&2
    exit 1
    ;;
esac

echo "    Platform: $OS ($ARCH) -> Target: $TARGET"
echo "    Channel:  $CHANNEL"

# Determine target version tag
if [ -n "${TAG_VERSION:-}" ]; then
  VERSION="$TAG_VERSION"
  echo "==> Targeted release version: $VERSION"
elif [ "$CHANNEL" = "beta" ]; then
  echo "==> Fetching latest release (including beta)..."
  VERSION="$(curl -sSL -H "Accept: application/vnd.github.v3+json" "https://api.github.com/repos/${REPO}/releases" | grep '"tag_name":' | head -n 1 | sed -E 's/.*"([^"]+)".*/\1/' || true)"
  if [ -z "$VERSION" ]; then
    echo "Warning: Could not query GitHub releases API, falling back to latest stable."
    VERSION="$(curl -sSL -H "Accept: application/vnd.github.v3+json" "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | head -n 1 | sed -E 's/.*"([^"]+)".*/\1/' || true)"
  fi
else
  echo "==> Fetching latest stable release..."
  VERSION="$(curl -sSL -H "Accept: application/vnd.github.v3+json" "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | head -n 1 | sed -E 's/.*"([^"]+)".*/\1/' || true)"
fi

if [ -z "$VERSION" ]; then
  echo "Error: Unable to resolve release version from GitHub." >&2
  echo "Make sure https://github.com/${REPO}/releases has published releases." >&2
  exit 1
fi

echo "    Selected version: $VERSION"

ARCHIVE_NAME="tag-${VERSION}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}"
CHECKSUM_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}.sha256"

TMP_DIR="$(mktemp -d -t tag-install-XXXXXX)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "==> Downloading ${ARCHIVE_NAME}..."
if ! curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ARCHIVE_NAME"; then
  if [ -n "${FALLBACK_TARGET:-}" ]; then
    echo "    $TARGET archive not found, trying fallback target $FALLBACK_TARGET..."
    TARGET="$FALLBACK_TARGET"
    ARCHIVE_NAME="tag-${VERSION}-${TARGET}.tar.gz"
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}"
    CHECKSUM_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}.sha256"
    curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ARCHIVE_NAME"
  else
    echo "Error: Failed to download release archive from $DOWNLOAD_URL" >&2
    exit 1
  fi
fi

# Verify checksum if sha256 file exists
if curl -fsSL "$CHECKSUM_URL" -o "$TMP_DIR/${ARCHIVE_NAME}.sha256" 2>/dev/null; then
  echo "==> Verifying SHA-256 checksum..."
  EXPECTED_HASH="$(awk '{print $1}' "$TMP_DIR/${ARCHIVE_NAME}.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL_HASH="$(sha256sum "$TMP_DIR/$ARCHIVE_NAME" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    ACTUAL_HASH="$(shasum -a 256 "$TMP_DIR/$ARCHIVE_NAME" | awk '{print $1}')"
  else
    ACTUAL_HASH=""
  fi

  if [ -n "$ACTUAL_HASH" ]; then
    if [ "$EXPECTED_HASH" != "$ACTUAL_HASH" ]; then
      echo "Error: SHA-256 checksum mismatch!" >&2
      echo "  Expected: $EXPECTED_HASH" >&2
      echo "  Actual:   $ACTUAL_HASH" >&2
      exit 1
    fi
    echo "    Checksum OK."
  fi
fi

echo "==> Extracting binaries..."
mkdir -p "$TMP_DIR/extracted"
tar -xzf "$TMP_DIR/$ARCHIVE_NAME" -C "$TMP_DIR/extracted"

mkdir -p "$INSTALL_DIR"

for bin in tag cockpit-notify tag-notify; do
  if [ -f "$TMP_DIR/extracted/$bin" ]; then
    echo "    Installing $bin -> $INSTALL_DIR/$bin"
    cp -f "$TMP_DIR/extracted/$bin" "$INSTALL_DIR/$bin"
    chmod +x "$INSTALL_DIR/$bin"
  fi
done

echo ""
echo "=================================================="
echo "  TAG ($VERSION) successfully installed to:"
echo "  $INSTALL_DIR/tag"
echo "=================================================="
echo ""

# Check if INSTALL_DIR is in PATH
case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    ;;
  *)
    echo "Note: $INSTALL_DIR is not in your PATH."
    echo "To run TAG from anywhere, add this to your shell config (~/.bashrc or ~/.zshrc):"
    echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
    echo ""
    ;;
esac

echo "Run 'tag --help' to get started!"
