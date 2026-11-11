#!/bin/bash
# Finder-configured drag-to-Applications installer. Requires macOS and a GUI login
# (including the standard GitHub-hosted macOS runner's Finder session).
set -euo pipefail
if [ "$#" -ne 2 ] || [ "$(uname -s)" != Darwin ]; then
  echo "Usage (macOS): $0 /path/to/Grove.app /path/to/Grove.dmg" >&2
  exit 1
fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
[ -d "$APP/Contents" ] || { echo "Not an app bundle: $APP" >&2; exit 1; }
# This pipeline deliberately ad-hoc signs its staged copy, as release packaging
# already did. Never replace a Developer ID signature with an ad-hoc signature.
# Future Developer ID delivery must inject installer art before its signing step.
if SIGNATURE="$(codesign -dv "$APP" 2>&1)"; then
  if ! printf '%s\n' "$SIGNATURE" | grep -q '^Signature=adhoc$'; then
    echo "Refusing to replace a non-ad-hoc input signature; add installer art before signing." >&2
    exit 1
  fi
fi
mkdir -p "$(dirname "$2")"
OUTPUT="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/grove-dmg.XXXXXX")"
MOUNT="$WORK/volume"
ATTACHED=0
cleanup() {
  if [ "$ATTACHED" = 1 ]; then
    if ! hdiutil detach "$MOUNT" >/dev/null 2>&1; then
      echo "Could not detach staging volume; preserving $WORK" >&2
      return
    fi
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/staging" "$MOUNT"
ditto "$APP" "$WORK/staging/Grove.app"
mkdir -p "$WORK/staging/Grove.app/Contents/Resources"
swift "$ROOT/scripts/dmg-background.swift" "$ROOT/assets/dmg/canopy.png" \
  "$WORK/staging/Grove.app/Contents/Resources/installer-background.png"
# Store the picture inside the staged app so hidden-file mode still exposes
# only Grove and Applications at the image root. The input app stays untouched.
codesign --force --deep -s - "$WORK/staging/Grove.app"
codesign --verify --deep --strict "$WORK/staging/Grove.app"
ln -s /Applications "$WORK/staging/Applications"
APP_KIB="$(du -A -sk "$WORK/staging" | awk '{print $1}')"
IMAGE_MIB=$(( (APP_KIB + 1023) / 1024 * 2 + 32 ))
for attempt in 1 2 3 4 5; do
  if hdiutil create -volname "Grove-Setup-${WORK##*.}" -layout NONE -size "${IMAGE_MIB}m" -fs HFS+ -ov "$WORK/installer.dmg"; then break; fi
  [ "$attempt" -lt 5 ] || exit 1
  sleep 3
done
hdiutil attach -readwrite -noverify -nobrowse -mountpoint "$MOUNT" "$WORK/installer.dmg"
ATTACHED=1
ditto "$WORK/staging" "$MOUNT"
# Set positions on the mounted volume so Finder stores portable .DS_Store data.
osascript - "$MOUNT" <<'APPLESCRIPT'
on run argv
  set diskFolder to POSIX file (item 1 of argv) as alias
  tell application "Finder"
    open diskFolder
    set installerWindow to front Finder window
    set current view of installerWindow to icon view
    set toolbar visible of installerWindow to false
    set statusbar visible of installerWindow to false
    set bounds of installerWindow to {180, 120, 900, 588}
    set options to icon view options of installerWindow
    set arrangement of options to not arranged
    set icon size of options to 112
    set text size of options to 14
    set background picture of options to file "Grove.app:Contents:Resources:installer-background.png" of diskFolder
    set label position of options to bottom
    set position of item "Grove.app" of diskFolder to {190, 215}
    set position of item "Applications" of diskFolder to {530, 215}
    update diskFolder without registering applications
    delay 3
    close installerWindow
    delay 1
    open diskFolder
    set installerWindow to front Finder window
    set options to icon view options of installerWindow
    if arrangement of options is not not arranged then error "Installer arrangement did not persist"
    if position of item "Grove.app" of diskFolder is not {190, 215} then error "Grove icon position did not persist"
    if position of item "Applications" of diskFolder is not {530, 215} then error "Applications icon position did not persist"
    delay 1
    close installerWindow
  end tell
end run
APPLESCRIPT
[ -s "$MOUNT/.DS_Store" ] || { echo "Finder did not save the installer layout" >&2; exit 1; }
# Give the final image its public name only after configuring the uniquely named staging volume.
diskutil rename "$MOUNT" Grove
# Finder stores the initial window and background reference in .DS_Store.
sync
for attempt in 1 2 3 4 5; do
  if hdiutil detach "$MOUNT"; then break; fi
  [ "$attempt" -lt 5 ] || exit 1
  sleep 3
done
ATTACHED=0
python3 "$ROOT/scripts/dmg-auto-open.py" "$WORK/installer.dmg"
hdiutil convert "$WORK/installer.dmg" -format UDZO -imagekey zlib-level=9 -ov -o "$OUTPUT"
echo "Built $OUTPUT"
