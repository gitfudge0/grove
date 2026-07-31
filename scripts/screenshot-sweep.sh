#!/usr/bin/env bash
# screenshot-sweep.sh — the Plan 10 Task 3 parity capture sweep.
#
# CAPTURE MECHANISM (probed on the reference box, 2026-07-31):
#   session:  Wayland / Hyprland  (XDG_SESSION_TYPE=wayland, XDG_CURRENT_DESKTOP=Hyprland)
#   present:  grim, slurp, hyprctl, jq, magick, import, ydotool
#   absent:   xwd, spectacle, gnome-screenshot
#   chosen:   `hyprctl -j clients` gives the target window's exact geometry;
#             `grim -g "X,Y WxH"` captures precisely that rectangle. No manual
#             region selection, so a pair is pixel-aligned by construction.
#             X11 fallback (`import -window`) is wired but UNTESTED here.
#
# WINDOW GEOMETRY — a pair is only comparable if both builds are the same size.
# The script pins the target window to 1280x800 via
#   hyprctl dispatch resizewindowpixel exact 1280 800,pid:<pid>
# before the first capture and re-checks before every capture, refusing to
# shoot if the geometry has drifted. On a non-Hyprland compositor, resize the
# window by hand once and pass --no-pin.
#
# This script drives the OPERATOR, not the app. gpui at ZED_REV exposes no
# screenshot API and neither build has a scripted UI driver, so for each slug
# it prints how to reach that state, waits for Enter, and captures.
#
# USAGE
#   ./scripts/screenshot-sweep.sh gpui            # runs target/release/grove-gpui
#   ./scripts/screenshot-sweep.sh iced            # runs ~/.local/bin/grove
#   ./scripts/screenshot-sweep.sh gpui --only modal-launcher   # resume a prefix
#   ./scripts/screenshot-sweep.sh gpui --force                 # re-shoot existing
#   ./scripts/screenshot-sweep.sh --list                       # print the slug table
#
# Output: target/parity-shots/<build>/<slug>.png  (gitignored, never committed)
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_BASE="$ROOT/target/parity-shots"
WIN_W=1280
WIN_H=800

BUILD=""
ONLY=""
FORCE=0
PIN=1
LIST=0

# ─────────────────────────────────────────────────────────────────────────────
# The capture list. `slug<TAB>how to reach it`.
#
# BASE CONFIGURATION for every row in SCREENS and MODALS:
#   1280x800, zoom 1.0, TokyoNight dark, chrome visible.
#
# Sources: spec §8.3 (every screen/modal x 3 zooms x 4 themes + follow-system
# flip x grid n in {1,2,3,5} x panel open/zen) and Appendix A. Taken literally
# that is a combinatorial explosion nobody will review, so the sweep is
# STRATIFIED: the full screen/modal list once at the base configuration, plus a
# zoom/theme cross-section over a representative subset.
# ─────────────────────────────────────────────────────────────────────────────
read -r -d '' SCREENS <<'EOF' || true
workspace-empty-no-projects	Remove/archive every project so the sidebar has none. Empty workspace copy names the "add a project" route.
workspace-empty-has-projects	At least one active project, no session running. Empty copy names "click a worktree's start button".
workspace-single-session	One session running and focused. THE baseline shot.
sidebar-collapsed	Tree-expand toggle -> Collapsed.
sidebar-sessions-only	Tree-expand toggle -> SessionsOnly.
sidebar-all	Tree-expand toggle -> All.
sidebar-hover-actions	Hover a worktree row so its action strip appears. Do not click.
sidebar-agent-menu	Click a worktree's agent chevron; the anchored overlay is open.
sidebar-git-suffix	A worktree with dirty + ahead/behind state, suffix visible in its row.
sidebar-archived-empty-state	Open the archived list with nothing archived.
sidebar-terminals-expanded	TERMINALS section expanded inside the scroll area.
sidebar-terminals-docked	TERMINALS collapsed, so the header docks outside the scroll area.
grid-n1	Grid view, 1 session.
grid-n2	Grid view, 2 sessions.
grid-n3	Grid view, 3 sessions (the ragged column).
grid-n5	Grid view, 5 sessions.
grid-drag-target	Grid view, mid-drag: source tile dimmed, target tile with the cyan inset.
grid-tile-waiting	Grid view, one tile in WaitingForInput: amber border + scrim + respond chip.
zen-single	Single session, chrome hidden.
zen-attention-pill	Zen with something waiting, so the floating pill shows.
terminal-tab	The home-terminal tab, one shell.
terminal-tab-multiple	Home-terminal tab with more than one shell open.
panel-20	Slide-over worktree panel at the 20% minimum.
panel-40	Panel at the 40% default.
panel-75	Panel at the 75% maximum.
panel-tabs-multi-shell	Panel with more than one shell tab.
appbar-attention-pill	Chrome visible, something waiting, pill in the appbar.
attention-dropdown	Attention pill clicked, dropdown open.
statusbar-default	Statusbar with no toast.
statusbar-toast-info	An info toast visible in the statusbar.
statusbar-toast-error	An error toast visible in the statusbar.
session-header-working	Session actively working: spinner + 3-dot + "in progress".
EOF

read -r -d '' MODALS <<'EOF' || true
modal-input	Any text-input modal (e.g. rename).
modal-confirm	Any confirm modal.
modal-confirm-quit	Cmd/Ctrl+Q with a live session -> the quit confirm.
modal-addproject-step1	Add project, step 1 (path).
modal-addproject-autocomplete	Add project step 1 with the path autocomplete list open.
modal-addproject-step2-git-init	Add project step 2 offering git init on a non-repo path.
modal-removeproject	Remove-project confirm.
modal-removeproject-progress	Remove-project mid-progress.
modal-archiveproject	Archive-project confirm.
modal-archived-list	The archived-projects list with at least one entry.
modal-message	A plain message modal.
modal-tmuxchoice	The tmux attach/new choice modal.
modal-agentpicker	The agent picker.
modal-launcher-recents	Session launcher, recents view (no query).
modal-launcher-filtered	Session launcher with a query that filters the list.
modal-launcher-row-actions	Launcher with a row's actions revealed.
modal-launcher-switch-drill	Launcher drilled into the switch view.
modal-launcher-settings-drill	Launcher drilled into settings.
modal-launcher-theme-preview	Launcher previewing a theme live.
modal-themepicker-dark	Theme picker, a dark theme selected.
modal-themepicker-light	Theme picker, a light theme selected.
modal-themepicker-project-scope	Theme picker scoped to a project.
modal-thememanager	The theme manager.
modal-theme-editor	The theme editor.
modal-settings-general	Settings, General tab.
modal-settings-tools	Settings, Tools tab.
modal-shortcutoverlay-workspace	Shortcut overlay from the workspace.
modal-shortcutoverlay-grid	Shortcut overlay from grid view.
modal-teardown-running	Teardown modal while it is running.
modal-teardown-done	Teardown modal once complete.
modal-scriptseditor	The scripts editor.
modal-updating	The updating modal.
modal-changelog	The changelog modal.
onboarding-step1	Onboarding, step 1.
onboarding-step2	Onboarding, step 2.
EOF

# Cross-section. Zooms {0.6, 2.0} (the 1.0 column is already in SCREENS/MODALS)
# over 5 representative states = 10 slugs per build.
read -r -d '' CROSS_ZOOM <<'EOF' || true
zoom060-workspace-single-session	Zoom to 0.6, single session.
zoom060-grid-n3	Zoom to 0.6, grid n=3.
zoom060-panel-40	Zoom to 0.6, panel at 40%.
zoom060-modal-launcher-recents	Zoom to 0.6, launcher recents.
zoom060-modal-settings-general	Zoom to 0.6, settings General.
zoom200-workspace-single-session	Zoom to 2.0, single session.
zoom200-grid-n3	Zoom to 2.0, grid n=3.
zoom200-panel-40	Zoom to 2.0, panel at 40%.
zoom200-modal-launcher-recents	Zoom to 2.0, launcher recents.
zoom200-modal-settings-general	Zoom to 2.0, settings General.
EOF

# Themes: four representative, NAMED here and in the index doc.
#   dark          = tokyonight        (the default dark)
#   light         = tokyonight-day    (its light counterpart)
#   high-contrast = <pick the highest-contrast built-in; name it in the index>
#   custom        = <a theme from the user's themes.json; name it in the index>
read -r -d '' CROSS_THEME <<'EOF' || true
theme-dark-workspace-single-session	Theme tokyonight, single session.
theme-dark-grid-n3	Theme tokyonight, grid n=3.
theme-dark-modal-themepicker-dark	Theme tokyonight, theme picker open.
theme-light-workspace-single-session	Theme tokyonight-day, single session.
theme-light-grid-n3	Theme tokyonight-day, grid n=3.
theme-light-modal-themepicker-dark	Theme tokyonight-day, theme picker open.
theme-contrast-workspace-single-session	High-contrast theme, single session.
theme-contrast-grid-n3	High-contrast theme, grid n=3.
theme-contrast-modal-themepicker-dark	High-contrast theme, theme picker open.
theme-custom-workspace-single-session	A themes.json custom theme, single session.
theme-custom-grid-n3	A themes.json custom theme, grid n=3.
theme-custom-modal-themepicker-dark	A themes.json custom theme, theme picker open.
EOF

# Follow-system: the FIRST FRAME after launch is the named Appendix A behavior
# ("not a flash of the wrong one"), so shoot it before touching anything.
read -r -d '' CROSS_SYSTEM <<'EOF' || true
follow-system-first-frame-dark	Follow-system ON, OS in dark. Relaunch and capture the FIRST frame.
follow-system-first-frame-light	Follow-system ON, OS flipped to light. Relaunch and capture the FIRST frame.
EOF

# grid + panel: Plan 07 recorded that the panel is SUPPRESSED in grid view,
# with no exception. `grid-n3-panel-open` is therefore N/A, not a miss; it is
# listed so the index can say so rather than leave a silent hole.
read -r -d '' CROSS_GRIDPANEL <<'EOF' || true
grid-n3-panel-open	N/A — Plan 07: the panel is suppressed in grid view, with no exception. Press s to skip; the index records it as N/A.
EOF

all_rows() {
    printf '%s\n%s\n%s\n%s\n%s\n%s\n' \
        "$SCREENS" "$MODALS" "$CROSS_ZOOM" "$CROSS_THEME" "$CROSS_SYSTEM" "$CROSS_GRIDPANEL"
}

while [ $# -gt 0 ]; do
    case "$1" in
        gpui|iced) BUILD="$1"; shift ;;
        --only) ONLY="$2"; shift 2 ;;
        --force) FORCE=1; shift ;;
        --no-pin) PIN=0; shift ;;
        --list) LIST=1; shift ;;
        -h|--help) sed -n '2,32p' "$0"; exit 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ "$LIST" = "1" ]; then
    all_rows | grep -v '^$' | nl -ba
    exit 0
fi

[ -n "$BUILD" ] || { echo "usage: $0 {gpui|iced} [--only PREFIX] [--force] [--no-pin]" >&2; exit 2; }

case "$BUILD" in
    gpui) BIN="$ROOT/target/release/grove-gpui" ;;
    iced) BIN="$HOME/.local/bin/grove" ;;
esac
[ -x "$BIN" ] || { echo "missing binary: $BIN" >&2; exit 1; }

command -v grim >/dev/null || { echo "grim is required for Wayland capture" >&2; exit 1; }
command -v hyprctl >/dev/null || { echo "hyprctl is required to resolve window geometry" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

OUT="$OUT_BASE/$BUILD"
mkdir -p "$OUT"

echo "build:  $BUILD  ($BIN)"
echo "output: $OUT"
echo
echo "Launch the build yourself in another terminal, then come back:"
echo "    $BIN"
echo "Base configuration: ${WIN_W}x${WIN_H}, zoom 1.0, TokyoNight dark, chrome visible."
echo
read -r -p "pid of the running $BUILD window: " APP_PID
[ -d "/proc/$APP_PID" ] || { echo "pid $APP_PID is not running" >&2; exit 1; }

geometry() {
    hyprctl -j clients | jq -r --argjson p "$APP_PID" \
        '.[] | select(.pid==$p) | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' | head -1
}

if [ "$PIN" = "1" ]; then
    hyprctl dispatch focuswindow "pid:$APP_PID" >/dev/null 2>&1 || true
    hyprctl dispatch setfloating "pid:$APP_PID" >/dev/null 2>&1 || true
    hyprctl dispatch resizewindowpixel "exact $WIN_W $WIN_H,pid:$APP_PID" >/dev/null 2>&1 || true
    sleep 1
fi

GEO="$(geometry)"
[ -n "$GEO" ] || { echo "no window found for pid $APP_PID" >&2; exit 1; }
echo "window geometry: $GEO"
case "$GEO" in
    *" ${WIN_W}x${WIN_H}") : ;;
    *) echo "WARNING: window is not ${WIN_W}x${WIN_H}; pairs will not be comparable." >&2
       read -r -p "continue anyway? [y/N] " ok
       [ "$ok" = "y" ] || exit 1 ;;
esac
PINNED_GEO="$GEO"

captured=0; skipped=0; missing=0
while IFS=$'\t' read -r slug how; do
    [ -n "$slug" ] || continue
    case "$slug" in \#*) continue ;; esac
    if [ -n "$ONLY" ]; then
        case "$slug" in "$ONLY"*) : ;; *) continue ;; esac
    fi
    png="$OUT/$slug.png"
    if [ -f "$png" ] && [ "$FORCE" = "0" ]; then
        skipped=$((skipped + 1))
        continue
    fi

    echo
    echo "── $slug"
    echo "   $how"
    read -r -p "   [Enter]=capture  s=skip (record MISSING)  q=quit > " key
    case "$key" in
        q) break ;;
        s) echo "   -> recorded MISSING/N-A in $BUILD"; missing=$((missing + 1)); continue ;;
    esac

    [ -d "/proc/$APP_PID" ] || { echo "the app exited; relaunch and resume with --only $slug" >&2; exit 1; }
    GEO="$(geometry)"
    if [ "$GEO" != "$PINNED_GEO" ]; then
        echo "   geometry drifted ($PINNED_GEO -> $GEO); re-pinning" >&2
        hyprctl dispatch resizewindowpixel "exact $WIN_W $WIN_H,pid:$APP_PID" >/dev/null 2>&1 || true
        sleep 1
        GEO="$(geometry)"
        [ "$GEO" = "$PINNED_GEO" ] || { echo "   could not restore geometry; fix it and retry" >&2; continue; }
    fi
    grim -g "$GEO" "$png"
    echo "   -> $png"
    captured=$((captured + 1))
done < <(all_rows)

echo
echo "captured $captured, skipped-existing $skipped, recorded-missing $missing"
echo "index doc: docs/superpowers/plans/2026-07-31-gpui-rewrite-10-screenshot-sweep.md"
