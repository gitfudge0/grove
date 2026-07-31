#!/usr/bin/env bash
# idle-power.sh — CPU cost of an idle Grove window, measured the way spike S1
# measured it.
#
# METHOD (do not substitute another one — a different method makes the numbers
# incomparable to the spike's, which is the entire point of this script):
#   read utime (field 14) + stime (field 15) from /proc/<pid>/stat at t=0 and
#   again at t=WINDOW seconds, divide the delta by `getconf CLK_TCK`, and
#   report  %CPU = 100 * ticks_delta / (CLK_TCK * WINDOW).
# `pidstat` is not installed on the reference box; the /proc delta is the same
# measurement without the dependency. `top`, `powertop` and RAPL are NOT
# equivalent and must not be swapped in.
#
# PLATFORM: Linux only (it reads /proc). The macOS equivalent is
#   ps -o time= -p <pid>
# sampled exactly the same way — take the cumulative CPU time at t=0 and at
# t=WINDOW, difference the two, and divide by the window. The user runs the
# macOS half by hand at the Phase B gate.
#
# The window must be OPEN and UNFOCUSED for the whole sample. The script
# refuses to continue if the pid dies or if the process's window count changes
# mid-sample (a new window is a different workload, not a longer sample).
#
# USAGE
#   scripts/idle-power.sh --pid 12345 [--windows 3] [--window-secs 60] [--label A-gpui]
#   scripts/idle-power.sh --label A-gpui --cmd 'target/release/grove-gpui'
#
set -euo pipefail

WINDOWS=3
WINDOW_SECS=60
PID=""
LABEL=""
CMD=""

usage() {
    sed -n '2,30p' "$0"
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --pid) PID="$2"; shift 2 ;;
        --windows) WINDOWS="$2"; shift 2 ;;
        --window-secs) WINDOW_SECS="$2"; shift 2 ;;
        --label) LABEL="$2"; shift 2 ;;
        --cmd) CMD="$2"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown argument: $1" >&2; usage ;;
    esac
done

[ "$(uname -s)" = "Linux" ] || {
    echo "idle-power.sh is Linux-only; on macOS sample 'ps -o time= -p <pid>' the same way" >&2
    exit 1
}

CLK_TCK="$(getconf CLK_TCK)"

started_here=0
if [ -z "$PID" ]; then
    [ -n "$CMD" ] || { echo "need --pid or --cmd" >&2; usage; }
    # shellcheck disable=SC2086
    $CMD >/dev/null 2>&1 &
    PID=$!
    started_here=1
    echo "launched: $CMD (pid $PID) — give it a window, then leave it UNFOCUSED"
    sleep 10
fi

[ -d "/proc/$PID" ] || { echo "pid $PID is not running" >&2; exit 1; }

# utime+stime, in clock ticks, for $PID. Field 2 (comm) may contain spaces and
# parentheses, so split on the LAST ')' before counting fields.
cpu_ticks() {
    local stat rest
    stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
    rest="${stat#*) }"
    # rest now starts at field 3 (state); utime is field 14 => rest field 12,
    # stime is field 15 => rest field 13.
    awk '{print $12 + $13}' <<<"$rest"
}

# How many toplevel windows this pid owns. Hyprland-specific; on another
# compositor this returns "?" and the guard degrades to a pid-alive check.
window_count() {
    if command -v hyprctl >/dev/null 2>&1; then
        hyprctl -j clients 2>/dev/null | jq -r --argjson p "$1" '[.[]|select(.pid==$p)]|length' 2>/dev/null || echo "?"
    else
        echo "?"
    fi
}

cmdline="$(tr '\0' ' ' < "/proc/$PID/cmdline" 2>/dev/null || echo '?')"
echo "label:        ${LABEL:-<unnamed>}"
echo "pid:          $PID"
echo "cmdline:      $cmdline"
echo "CLK_TCK:      $CLK_TCK"
echo "windows:      $WINDOWS x ${WINDOW_SECS}s"
echo "toplevels:    $(window_count "$PID")"
echo

total_delta=0
for w in $(seq 1 "$WINDOWS"); do
    t0="$(cpu_ticks "$PID")" || { echo "pid $PID vanished before window $w" >&2; exit 1; }
    n0="$(window_count "$PID")"
    sleep "$WINDOW_SECS"
    [ -d "/proc/$PID" ] || { echo "pid $PID vanished during window $w" >&2; exit 1; }
    t1="$(cpu_ticks "$PID")" || { echo "pid $PID vanished during window $w" >&2; exit 1; }
    n1="$(window_count "$PID")"
    if [ "$n0" != "$n1" ]; then
        echo "window count changed mid-sample ($n0 -> $n1); the sample is void" >&2
        exit 1
    fi
    delta=$((t1 - t0))
    total_delta=$((total_delta + delta))
    pct="$(awk -v d="$delta" -v c="$CLK_TCK" -v s="$WINDOW_SECS" 'BEGIN{printf "%.2f", 100*d/(c*s)}')"
    # The raw ticks are the evidence; the percentage is the summary.
    printf 'window %d: ticks %s -> %s  delta=%d  %%CPU=%s\n' "$w" "$t0" "$t1" "$delta" "$pct"
done

mean="$(awk -v d="$total_delta" -v c="$CLK_TCK" -v s="$WINDOW_SECS" -v n="$WINDOWS" \
    'BEGIN{printf "%.2f", 100*d/(c*s*n)}')"
echo
echo "total tick delta: $total_delta over $((WINDOWS * WINDOW_SECS))s"
echo "mean %CPU:        $mean"

if [ "$started_here" = "1" ]; then
    kill "$PID" 2>/dev/null || true
fi
