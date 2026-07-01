#!/usr/bin/env bash
# uninstall.sh — removes whatever ./install.sh installed.
set -euo pipefail

OS="$(uname -s)"

case "$OS" in
  Darwin)
    removed=0
    for DEST in "/Applications" "$HOME/Applications"; do
      if [ -d "$DEST/Grove.app" ]; then
        rm -rf "$DEST/Grove.app"
        echo "Removed $DEST/Grove.app"
        removed=1
      fi
    done
    [ "$removed" = 1 ] || echo "No Grove.app found in /Applications or ~/Applications."
    ;;

  Linux)
    if command -v dpkg >/dev/null 2>&1 && dpkg -s grove >/dev/null 2>&1; then
      echo "Removing grove package (sudo)..."
      sudo dpkg -r grove
    else
      rm -f "$HOME/.local/bin/grove"
      rm -f "$HOME/.local/share/applications/grove.desktop"
      rm -f "$HOME/.local/share/icons/hicolor/512x512/apps/grove.png"
      echo "Removed grove from ~/.local (binary, launcher, icon)."
    fi
    ;;

  *)
    echo "Unsupported OS: $OS" >&2
    exit 1
    ;;
esac

# Older installs used `cargo install --path .`; clean that up too if present.
if command -v cargo >/dev/null 2>&1 && cargo install --list 2>/dev/null | grep -q '^grove '; then
  cargo uninstall grove
  echo "Uninstalled legacy 'grove' cargo binary."
fi

echo
echo "Your project registrations and theme settings under ~/.config/grove were left in place."
