#!/usr/bin/env bash
# install.sh — build & install Grove as a native desktop app.
#
#   macOS: produces Grove.app (via cargo-bundle) and copies it to /Applications.
#   Linux: produces a .deb (via cargo-bundle) and installs it, or falls back to
#          a binary + .desktop launcher under ~/.local when dpkg is unavailable.
set -euo pipefail

REPO_URL="${GROVE_REPO_URL:-https://github.com/gitfudge0/grove.git}"
REPO_REF="${GROVE_REPO_REF:-main}"

# Detect whether we're running from a local checkout or being piped (curl | bash).
# When piped, BASH_SOURCE is unset/empty or points at a non-file, and there's no
# Cargo.toml next to the script.
script_src="${BASH_SOURCE[0]:-}"
if [ -n "$script_src" ] && [ -f "$(dirname "$script_src")/Cargo.toml" ]; then
  cd "$(dirname "$script_src")"
else
  if ! command -v git >/dev/null 2>&1; then
    echo "Error: git is required to install grove from a remote URL." >&2
    exit 1
  fi
  TMP_DIR="$(mktemp -d -t grove-install.XXXXXX)"
  trap 'rm -rf "$TMP_DIR"' EXIT
  echo "Cloning $REPO_URL ($REPO_REF)..."
  git clone --depth 1 --branch "$REPO_REF" "$REPO_URL" "$TMP_DIR/grove"
  cd "$TMP_DIR/grove"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Error: cargo is required. Install Rust from https://rustup.rs" >&2
  exit 1
fi

if ! cargo bundle --help >/dev/null 2>&1; then
  echo "Installing cargo-bundle..."
  cargo install cargo-bundle
fi

OS="$(uname -s)"

# Pin the bundle format per-OS. Left unset, `cargo bundle` on macOS also builds
# a .dmg, whose hdiutil unmount step intermittently fails with "Resource busy"
# (Spotlight/fseventsd grab the freshly-mounted staging volume). We only ever
# install the .app below, so build just that and skip the flaky DMG path.
case "$OS" in
  Darwin) BUNDLE_FORMAT=osx ;;
  Linux)  BUNDLE_FORMAT=deb ;;
  *)      BUNDLE_FORMAT="" ;;
esac

TARGET=""
BUNDLE_DIR="target/release/bundle"
if [ "$OS" = "Darwin" ]; then
  # `uname -m` reflects Rosetta when the shell itself runs translated, so it
  # can report x86_64 on Apple Silicon hardware. Ask sysctl for the real CPU
  # to pick the native target and avoid an accidental Rosetta-emulated build.
  if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
    ARCH=arm64
  else
    ARCH="$(uname -m)"
  fi
  case "$ARCH" in
    arm64|aarch64) TARGET=aarch64-apple-darwin ;;
    x86_64)        TARGET=x86_64-apple-darwin ;;
    *) echo "Warning: unrecognized macOS arch '$ARCH', letting cargo pick the default target." >&2 ;;
  esac
  if [ -n "$TARGET" ]; then
    rustup target add "$TARGET" >/dev/null 2>&1 || true
    BUNDLE_DIR="target/$TARGET/release/bundle"
  fi
fi

echo "Building release bundle..."
CARGO_BUNDLE_ARGS=(--release)
[ -n "$BUNDLE_FORMAT" ] && CARGO_BUNDLE_ARGS+=(--format "$BUNDLE_FORMAT")
[ -n "$TARGET" ] && CARGO_BUNDLE_ARGS+=(--target "$TARGET")
cargo bundle "${CARGO_BUNDLE_ARGS[@]}"

case "$OS" in
  Darwin)
    APP="$(find "$BUNDLE_DIR/osx" -maxdepth 1 -name '*.app' | head -n1)"
    [ -n "$APP" ] || { echo "Error: no .app produced." >&2; exit 1; }
    DEST="/Applications"
    [ -w "$DEST" ] || DEST="$HOME/Applications"
    mkdir -p "$DEST"
    rm -rf "$DEST/$(basename "$APP")"
    cp -R "$APP" "$DEST/"
    # Strip the quarantine flag so it opens without a Gatekeeper prompt.
    xattr -dr com.apple.quarantine "$DEST/$(basename "$APP")" 2>/dev/null || true
    # Sign with a stable identity when one exists so macOS TCC permission
    # grants survive rebuilds (ad-hoc signatures change every build, which
    # makes macOS re-prompt for folder/data access after each install).
    SIGN_ID="${GROVE_SIGN_IDENTITY:-Grove Dev}"
    if security find-identity -v -p codesigning 2>/dev/null | grep -q "$SIGN_ID"; then
      echo "Signing with identity '$SIGN_ID'..."
      codesign --force --deep --sign "$SIGN_ID" "$DEST/$(basename "$APP")"
    else
      echo "No '$SIGN_ID' codesigning identity found; leaving ad-hoc signature." >&2
      echo "(TCC permission prompts will repeat after each rebuild until one exists.)" >&2
    fi
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
