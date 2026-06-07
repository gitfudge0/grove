#!/usr/bin/env bash
set -euo pipefail

cargo uninstall grove

BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
echo
echo "Uninstalled 'grove' from $BIN_DIR/grove"
echo "If you installed a desktop bundle with ./package.sh, remove that app/package separately."
