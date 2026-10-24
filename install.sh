#!/usr/bin/env bash
# install.sh — install a checkout build or a published Grove desktop app.
#
#   macOS: produces Grove.app (via cargo-bundle) and copies it to /Applications.
#   Linux: produces a .deb (via cargo-bundle) and installs it, or falls back to
#          a binary + .desktop launcher under ~/.local when dpkg is unavailable.
set -euo pipefail

REPO_URL="${GROVE_REPO_URL:-https://github.com/gitfudge0/grove.git}"
REPO_REF="${GROVE_REPO_REF:-main}"

# A checkout builds locally; curl installs published binaries unless --source is set.
SOURCE=0
PROFILE=local
for arg in "$@"; do
  case "$arg" in
    --source) SOURCE=1 ;;
    --release) PROFILE=release ;;
    --help|-h)
      echo "Usage: install.sh [--source] [--release]"
      echo "Checkout: fast local build; curl: latest prebuilt release."
      echo "--source builds a remote checkout; --release optimizes source builds."
      exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 1 ;;
  esac
done
OS="$(uname -s)"
ARCH="$(uname -m)"
if [ "$OS" = Darwin ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
  ARCH=arm64
fi
TMP_DIR=""
MOUNT_DIR=""
cleanup() {
  [ -z "$MOUNT_DIR" ] || hdiutil detach "$MOUNT_DIR" >/dev/null 2>&1 || true
  [ -z "$TMP_DIR" ] || rm -rf "$TMP_DIR"
}
trap cleanup EXIT
script_src="${BASH_SOURCE[0]:-}"
PREBUILT=0
if [ -n "$script_src" ] && [ -f "$(dirname "$script_src")/Cargo.toml" ]; then
  cd "$(dirname "$script_src")"
else
  TMP_DIR="$(mktemp -d -t grove-install.XXXXXX)"
  if [ "$SOURCE" = 1 ]; then
    command -v git >/dev/null 2>&1 || { echo "Error: git is required for --source." >&2; exit 1; }
    echo "Cloning $REPO_URL ($REPO_REF)..."
    git clone --depth 1 --branch "$REPO_REF" "$REPO_URL" "$TMP_DIR/grove"
    cd "$TMP_DIR/grove"
  else
    PREBUILT=1
    command -v curl >/dev/null 2>&1 || { echo "Error: curl is required." >&2; exit 1; }
    RELEASE_URL="${REPO_URL%.git}/releases"
    LATEST="$(curl -fsSL --proto '=https' --proto-redir '=https' -o /dev/null -w '%{url_effective}' "$RELEASE_URL/latest")"
    TAG="${LATEST##*/}"
    VERSION="${TAG#v}"
    case "$VERSION" in
      ''|*[!0-9.]*) echo "Error: cannot resolve latest release. Use --source to build instead." >&2; exit 1 ;;
    esac
    case "$OS:$ARCH" in
      Darwin:arm64|Darwin:aarch64) ASSET="Grove-$VERSION-macos-arm64.dmg" ;;
      Darwin:x86_64) ASSET="Grove-$VERSION-macos-x86_64.dmg" ;;
      Linux:x86_64) ASSET="Grove-$VERSION-linux-amd64.deb" ;;
      *) echo "Error: no prebuilt release for $OS/$ARCH. Use --source." >&2; exit 1 ;;
    esac
    echo "Downloading $ASSET..."
    curl -fSL --proto '=https' --proto-redir '=https' "$RELEASE_URL/download/$TAG/$ASSET" -o "$TMP_DIR/$ASSET"
    if [ "$OS" = Darwin ]; then
      MOUNT_DIR="$TMP_DIR/mount"
      mkdir -p "$MOUNT_DIR"
      hdiutil attach -readonly -nobrowse -mountpoint "$MOUNT_DIR" "$TMP_DIR/$ASSET" >/dev/null
      BUNDLE_DIR="$TMP_DIR"
      # The common installation path expects bundle/osx-style app discovery.
      mkdir -p "$TMP_DIR/osx"
      cp -R "$MOUNT_DIR/Grove.app" "$TMP_DIR/osx/"
      hdiutil detach "$MOUNT_DIR" >/dev/null
      MOUNT_DIR=""
    else
      command -v dpkg >/dev/null 2>&1 || { echo "Error: dpkg is required for the prebuilt package. Use --source." >&2; exit 1; }
      DEB="$TMP_DIR/$ASSET"
    fi
  fi
fi

# Remove only the obsolete command link created by older Grove installers.
remove_cli_link() {
  local executable="$1"
  local cli_path="${CARGO_HOME:-$HOME/.cargo}/bin/grove"
  if [ -L "$cli_path" ] && [ "$(readlink "$cli_path")" = "$executable" ]; then
    rm "$cli_path"
    echo "Removed obsolete Grove CLI link: $cli_path"
  fi
}

if [ "$PREBUILT" = 0 ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: cargo is required. Install Rust from https://rustup.rs" >&2
    exit 1
  fi

  BUNDLE_VERSION="0.12.0"
  if [ "$(cargo bundle --version 2>/dev/null || true)" != "cargo-bundle v$BUNDLE_VERSION" ]; then
    echo "Installing cargo-bundle $BUNDLE_VERSION..."
    cargo install cargo-bundle --version "$BUNDLE_VERSION" --locked
  fi

  # Pin the bundle format per-OS. Left unset, `cargo bundle` on macOS also builds
  # a .dmg, whose hdiutil unmount step intermittently fails with "Resource busy"
  # (Spotlight/fseventsd grab the freshly-mounted staging volume). We only ever
  # install the .app below, so build just that and skip the flaky DMG path.
  case "$OS" in
    Darwin) BUNDLE_FORMAT=osx ;;
    Linux)  BUNDLE_FORMAT=deb ;;
    *)      BUNDLE_FORMAT="" ;;
  esac

  TARGET="${CARGO_BUILD_TARGET:-}"
  BUILD_DIR="${CARGO_TARGET_DIR:-target}"
  if [ "$OS" = Darwin ]; then
    case "$ARCH" in
      arm64|aarch64) NATIVE_TARGET=aarch64-apple-darwin ;;
      x86_64) NATIVE_TARGET=x86_64-apple-darwin ;;
      *) echo "Error: unsupported macOS architecture: $ARCH" >&2; exit 1 ;;
    esac
    HOST="$(rustc -vV | sed -n 's/^host: //p')"
    # Native builds share ordinary cargo artifacts. Explicit targeting remains
    # necessary when the toolchain is running under Rosetta.
    if [ "$HOST" != "$NATIVE_TARGET" ] || [ -n "$TARGET" ]; then
      TARGET="$NATIVE_TARGET"
      rustup target add "$TARGET"
    fi
  fi
  PROFILE_DIR="$BUILD_DIR"
  [ -z "$TARGET" ] || PROFILE_DIR="$PROFILE_DIR/$TARGET"
  PROFILE_DIR="$PROFILE_DIR/$PROFILE"
  BUNDLE_DIR="$PROFILE_DIR/bundle"

  # An interrupted link can leave a non-executable stub that cargo still treats
  # as fresh; the bundled app then fails with "not supported on this Mac".
  # Drop grove's artifacts so cargo relinks instead of reusing the stub.
  if [ "$OS" = Darwin ] && [ -e "$PROFILE_DIR/grove" ] \
    && ! file -b "$PROFILE_DIR/grove" | grep -q '^Mach-O'; then
    echo "Discarding corrupt $PROFILE_DIR/grove; relinking..."
    CLEAN_ARGS=(-p grove --profile "$PROFILE")
    [ -n "$TARGET" ] && CLEAN_ARGS+=(--target "$TARGET")
    cargo clean "${CLEAN_ARGS[@]}"
  fi

  echo "Building $PROFILE bundle..."
  CARGO_BUNDLE_ARGS=(-p grove --profile "$PROFILE")
  [ -n "$BUNDLE_FORMAT" ] && CARGO_BUNDLE_ARGS+=(--format "$BUNDLE_FORMAT")
  [ -n "$TARGET" ] && CARGO_BUNDLE_ARGS+=(--target "$TARGET")
  cargo bundle "${CARGO_BUNDLE_ARGS[@]}"
fi

case "$OS" in
  Darwin)
    APP="$(find "$BUNDLE_DIR/osx" -maxdepth 1 -name '*.app' | head -n1)"
    [ -n "$APP" ] || { echo "Error: no .app produced." >&2; exit 1; }
    file -b "$APP/Contents/MacOS/grove" | grep -q '^Mach-O' \
      || { echo "Error: $APP has no valid executable; rerun install.sh." >&2; exit 1; }
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
    SIGN_ID="${GROVE_SIGN_IDENTITY:-gitfudge}"
    if security find-identity -v -p codesigning 2>/dev/null | grep -q "$SIGN_ID"; then
      echo "Signing with identity '$SIGN_ID'..."
      codesign --force --deep --sign "$SIGN_ID" "$DEST/$(basename "$APP")"
    else
      echo "No '$SIGN_ID' codesigning identity found; leaving ad-hoc signature." >&2
      echo "(TCC permission prompts will repeat after each rebuild until one exists.)" >&2
    fi
    echo
    echo "Installed $(basename "$APP") to $DEST"
    remove_cli_link "$DEST/$(basename "$APP")/Contents/MacOS/grove"
    echo "Launch it from Spotlight or Launchpad."
    ;;

  Linux)
    if [ "$PREBUILT" = 0 ]; then
      DEB="$(find "$BUNDLE_DIR/deb" -maxdepth 1 -name '*.deb' 2>/dev/null | head -n1 || true)"
    fi
    if [ -n "$DEB" ] && command -v dpkg >/dev/null 2>&1; then
      echo "Installing $DEB (sudo)..."
      sudo dpkg -i "$DEB" || sudo apt-get -f install -y
      remove_cli_link "/usr/bin/grove"
      echo "Installed. Launch 'Grove' from your application menu."
    else
      # Fallback: binary + .desktop + icon under ~/.local (no root needed).
      echo "dpkg/.deb unavailable — installing under ~/.local ..."
      install -Dm755 "$PROFILE_DIR/grove" "$HOME/.local/bin/grove"
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
      remove_cli_link "$HOME/.local/bin/grove"
      echo "Installed grove to ~/.local/bin and a launcher to $DESKTOP"
      echo "Ensure ~/.local/bin is on your PATH; launch 'Grove' from your menu."
    fi
    ;;

  *)
    echo "Unsupported OS: $OS" >&2
    exit 1
    ;;
esac

# Installation has succeeded; retain artifacts on every earlier failure for
# diagnosis. Clean the whole selected profile, including dependencies and bundles.
if [ "$PREBUILT" = 0 ]; then
  echo "Removing $PROFILE build artifacts..."
  CLEAN_ARGS=(--profile "$PROFILE")
  [ -z "$TARGET" ] || CLEAN_ARGS+=(--target "$TARGET")
  cargo clean "${CLEAN_ARGS[@]}"
fi
