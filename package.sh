#!/usr/bin/env bash
# package.sh — build & install Grove as a native desktop app.
#
#   macOS: produces Grove.app (via cargo-bundle) and copies it to /Applications.
#   Linux: produces a .deb (via cargo-bundle) and installs it, or falls back to
#          a binary + .desktop launcher under ~/.local when dpkg is unavailable.
#
# The plain binary install (`./install.sh` / `cargo install --path .`) is
# unaffected — this adds desktop launcher/package integration.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

if ! command -v cargo >/dev/null 2>&1; then
  echo "Error: cargo is required. Install Rust from https://rustup.rs" >&2
  exit 1
fi

if ! cargo bundle --help >/dev/null 2>&1; then
  echo "Installing cargo-bundle..."
  cargo install cargo-bundle
fi

echo "Building release bundle..."
cargo bundle --release

OS="$(uname -s)"
case "$OS" in
  Darwin)
    APP="$(find target/release/bundle/osx -maxdepth 1 -name '*.app' | head -n1)"
    [ -n "$APP" ] || { echo "Error: no .app produced." >&2; exit 1; }
    DEST="/Applications"
    [ -w "$DEST" ] || DEST="$HOME/Applications"
    mkdir -p "$DEST"
    rm -rf "$DEST/$(basename "$APP")"
    cp -R "$APP" "$DEST/"
    # Strip the quarantine flag so it opens without a Gatekeeper prompt.
    xattr -dr com.apple.quarantine "$DEST/$(basename "$APP")" 2>/dev/null || true
    echo
    echo "Installed $(basename "$APP") to $DEST"
    echo "Launch it from Spotlight or Launchpad."
    ;;

  Linux)
    DEB="$(find target/release/bundle/deb -maxdepth 1 -name '*.deb' 2>/dev/null | head -n1 || true)"
    if [ -n "$DEB" ] && command -v dpkg >/dev/null 2>&1; then
      echo "Installing $DEB (sudo)..."
      sudo dpkg -i "$DEB" || sudo apt-get -f install -y
      echo "Installed. Launch 'Grove' from your application menu."
    else
      # Fallback: binary + .desktop + icon under ~/.local (no root needed).
      echo "dpkg/.deb unavailable — installing under ~/.local ..."
      install -Dm755 target/release/grove "$HOME/.local/bin/grove"
      install -Dm644 assets/icon/512x512.png \
        "$HOME/.local/share/icons/hicolor/512x512/apps/grove.png"
      DESKTOP="$HOME/.local/share/applications/grove.desktop"
      mkdir -p "$(dirname "$DESKTOP")"
      cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=Grove
Comment=A worktree launchpad for AI coding agents
Exec=$HOME/.local/bin/grove
Icon=grove
Terminal=false
Categories=Development;
EOF
      command -v update-desktop-database >/dev/null 2>&1 \
        && update-desktop-database "$HOME/.local/share/applications" || true
      echo
      echo "Installed grove to ~/.local/bin and a launcher to $DESKTOP"
      echo "Ensure ~/.local/bin is on your PATH; launch 'Grove' from your menu."
    fi
    ;;

  *)
    echo "Unsupported OS: $OS" >&2
    exit 1
    ;;
esac
