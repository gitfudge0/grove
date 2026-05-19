#!/usr/bin/env bash
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

cargo install --path . --force

BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
echo
echo "Installed 'grove' to $BIN_DIR/grove"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "Note: $BIN_DIR is not on your PATH. Add it to your shell profile:"
     echo "  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
